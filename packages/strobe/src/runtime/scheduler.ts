// The look-ahead scheduler ("two clocks"): each ~25 ms tick queries every active track's
// pattern over [now, now+lookahead], turns each onset Hap<ControlMap> into engine records
// (NoteOn/NoteOff + ParamSet/ParamRamp/TrackFx), frame-stamps and batch-encodes them into
// the fixed 32-byte protocol, and pushes once. The Phase 0 skeleton is unchanged; only the
// content generation is richer (the Hap→events compiler).

import type { Engine } from "../backend/engine";
import type { Clock } from "../clock/clock";
import { activeTracks, promotePending } from "./tracks";
import { queryArc, hasOnset, type Hap, type Pattern } from "../pattern/pattern";
import { noteToFreq, packFx, PARAM_REGISTRY, PARAM_RAMP_KEYS, type ControlMap } from "../pattern/control";
import { isContinuousPattern } from "../pattern/signals";
import { PRESETS } from "../instruments/presets";

const REC = 32;
const MAX_EVENTS_PER_TICK = 4096;
const TICK_MS = 25;
const LOOKAHEAD_S = 0.1;

const VERSION = 1;
const OP_NOTE_ON = 0x01;
const OP_NOTE_OFF = 0x02;
const OP_PARAM_SET = 0x10;
const OP_PARAM_RAMP = 0x11;
const OP_TRIG_SAMPLE = 0x20;
const OP_TRACK_FX = 0x30;
const OP_TEMPO = 0x40;

export interface SchedulerOpts {
  tickMs?: number;
  lookaheadS?: number;
}

// ── 32-byte record encoders (mirror event.rs / event-protocol.md) ─────────────

export function writeNoteOn(
  view: DataView,
  i: number,
  track: number,
  frame: number,
  voice: number,
  freq: number,
  vel: number,
  durFrames: number,
  instrument: number,
  seq: number,
): void {
  const o = i * REC;
  view.setUint8(o + 0, VERSION);
  view.setUint8(o + 1, OP_NOTE_ON);
  view.setUint16(o + 2, track, true);
  view.setUint32(o + 4, frame >>> 0, true);
  view.setUint32(o + 8, voice >>> 0, true);
  view.setFloat32(o + 12, freq, true);
  view.setFloat32(o + 16, vel, true);
  view.setFloat32(o + 20, durFrames, true);
  view.setFloat32(o + 24, instrument, true);
  view.setUint32(o + 28, seq >>> 0, true);
}

function writeSimple(
  view: DataView,
  i: number,
  opcode: number,
  track: number,
  frame: number,
  vp: number,
  arg0: number,
  arg2: number,
  seq: number,
): void {
  const o = i * REC;
  view.setUint8(o + 0, VERSION);
  view.setUint8(o + 1, opcode);
  view.setUint16(o + 2, track, true);
  view.setUint32(o + 4, frame >>> 0, true);
  view.setUint32(o + 8, vp >>> 0, true);
  view.setFloat32(o + 12, arg0, true);
  view.setFloat32(o + 16, 0, true);
  view.setFloat32(o + 20, arg2, true);
  view.setFloat32(o + 24, 0, true);
  view.setUint32(o + 28, seq >>> 0, true);
}

export function writeTempo(view: DataView, i: number, frame: number, cps: number, seq: number): void {
  writeSimple(view, i, OP_TEMPO, 0, frame, 0, cps, 0, seq);
}

/** One compiled record: its frame (for ordering) and an encoder. */
interface RawEvent {
  frame: number;
  encode: (view: DataView, i: number, seq: number) => void;
}

const noteOn = (
  track: number,
  frame: number,
  voice: number,
  freq: number,
  vel: number,
  dur: number,
  instrument: number,
): RawEvent => ({
  frame,
  encode: (v, i, seq) => writeNoteOn(v, i, track, frame, voice, freq, vel, dur, instrument, seq),
});
const noteOff = (track: number, frame: number, voice: number): RawEvent => ({
  frame,
  encode: (v, i, seq) => writeSimple(v, i, OP_NOTE_OFF, track, frame, voice, 0, 0, seq),
});
const paramSet = (track: number, frame: number, param: number, value: number): RawEvent => ({
  frame,
  encode: (v, i, seq) => writeSimple(v, i, OP_PARAM_SET, track, frame, param, value, 0, seq),
});
const paramRamp = (
  track: number,
  frame: number,
  param: number,
  target: number,
  rampFrames: number,
): RawEvent => ({
  frame,
  encode: (v, i, seq) => writeSimple(v, i, OP_PARAM_RAMP, track, frame, param, target, rampFrames, seq),
});
const trackFx = (track: number, frame: number, param: number, value: number): RawEvent => ({
  frame,
  encode: (v, i, seq) => writeSimple(v, i, OP_TRACK_FX, track, frame, param, value, 0, seq),
});

/** TrigSample (0x20): vp = region length (frames, 0 = whole), arg0=speed, arg1=gain,
 *  arg2=start(frames), arg3=sample id, flags_seq = reverse/loop/interp flags. */
function writeTrigSample(
  view: DataView,
  i: number,
  track: number,
  frame: number,
  sampleId: number,
  speed: number,
  gain: number,
  startFrames: number,
  regionLen: number,
  flags: number,
): void {
  const o = i * REC;
  view.setUint8(o + 0, VERSION);
  view.setUint8(o + 1, OP_TRIG_SAMPLE);
  view.setUint16(o + 2, track, true);
  view.setUint32(o + 4, frame >>> 0, true);
  view.setUint32(o + 8, regionLen >>> 0, true);
  view.setFloat32(o + 12, speed, true);
  view.setFloat32(o + 16, gain, true);
  view.setFloat32(o + 20, startFrames, true);
  view.setFloat32(o + 24, sampleId, true);
  view.setUint32(o + 28, flags >>> 0, true);
}

const trigSample = (
  track: number,
  frame: number,
  sampleId: number,
  speed: number,
  gain: number,
  startFrames: number,
  regionLen: number,
  flags: number,
): RawEvent => ({
  frame,
  encode: (v, i) => writeTrigSample(v, i, track, frame, sampleId, speed, gain, startFrames, regionLen, flags),
});

// ── The Hap → events compiler ────────────────────────────────────────────────

const HANDLED = new Set(["s", "note", "freq", "gain", "vel", "instrument"]);
const DRUM_NAMES = new Set(["bd", "sn", "cp", "hh", "oh"]);

/** Resolve a control value to a number, sampling a `Pat<number>` at the onset. */
function num(value: unknown, t: number): number {
  if (typeof value === "number") return value;
  if (value && typeof value === "object") {
    return (queryArc(value as Pattern<number>, t, t)[0]?.value as number) ?? 0;
  }
  return 0;
}

function resolveInstrument(cm: ControlMap): { id: number; drum: boolean } {
  if (typeof cm.instrument === "number") {
    return { id: cm.instrument, drum: cm.instrument <= 4 };
  }
  const name = typeof cm.s === "string" ? cm.s : "saw";
  const id = (PRESETS as Record<string, number>)[name] ?? PRESETS.saw;
  return { id, drum: DRUM_NAMES.has(name) || id <= 4 };
}

export class Scheduler {
  private buf = new ArrayBuffer(MAX_EVENTS_PER_TICK * REC);
  private view = new DataView(this.buf);
  private timer: ReturnType<typeof setInterval> | null = null;
  private scheduledUntilCycle = 0;
  private seq = 0;
  private voiceCtr = 1;
  /** Last value sent per (track, packed-fx-id), so an unchanged track-fx param isn't re-sent
   *  on every note (persistent chain state — re-sending floods the tick and the ring). */
  private lastFx = new Map<number, number>();
  private readonly opts: Required<SchedulerOpts>;

  constructor(
    private engine: Engine,
    private clock: Clock,
    opts: SchedulerOpts = {},
  ) {
    this.opts = { tickMs: opts.tickMs ?? TICK_MS, lookaheadS: opts.lookaheadS ?? LOOKAHEAD_S };
  }

  start(): void {
    this.scheduledUntilCycle = this.clock.nowCycle();
    this.timer = setInterval(() => this.tick(), this.opts.tickMs);
  }
  stop(): void {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }

  private allocVoice(): number {
    this.voiceCtr = (this.voiceCtr % 0xffffffff) + 1;
    return this.voiceCtr;
  }

  private compileHap(track: number, hap: Hap<ControlMap>, out: RawEvent[]): void {
    const cm = hap.value;
    const onsetFrame = this.clock.cycleToFrame(hap.part.begin);
    const durFrames = hap.whole ? this.clock.cyclesToFrames(hap.whole.end - hap.whole.begin) : 0;

    if (cm.sampleId != null) {
      // A loaded sample → TrigSample (a one-shot voice, like a drum). begin is a 0..1 fraction.
      const speed = num(cm.speed ?? 1, hap.part.begin);
      const gain = Math.min(4, Math.max(0, num(cm.gain ?? 1, hap.part.begin)));
      const startFrames = Math.round(num(cm.begin ?? 0, hap.part.begin) * (cm._frames ?? 0));
      out.push(
        trigSample(track, onsetFrame, cm.sampleId, speed, gain, startFrames, cm._regionLen ?? 0, cm._flags ?? 0),
      );
    } else {
      const { id: instrument, drum } = resolveInstrument(cm);
      const velSrc = cm.gain ?? (cm as Record<string, unknown>).vel ?? 0.9;
      const vel = Math.min(1, Math.max(0, num(velSrc, hap.part.begin))); // velocity is 0..1
      const freq =
        cm.freq != null ? num(cm.freq, hap.part.begin) : cm.note != null ? noteToFreq(cm.note) : 0;
      if (drum) {
        out.push(noteOn(track, onsetFrame, 0, freq, vel, durFrames, instrument)); // one-shot
      } else {
        const voice = this.allocVoice();
        out.push(noteOn(track, onsetFrame, voice, freq, vel, 0, instrument)); // held
        if (hap.whole) out.push(noteOff(track, this.clock.cycleToFrame(hap.whole.end), voice));
      }
    }

    for (const [key, value] of Object.entries(cm)) {
      if (HANDLED.has(key) || value == null) continue;
      const route = PARAM_REGISTRY[key];
      if (!route || route.kind === "noteField" || route.kind === "trigField") continue;
      if (typeof value === "string") {
        // sidechain("p1") → duck source = track number 1 (the only string-valued control).
        if (key === "sidechain" && route.kind === "trackFx") {
          const m = /^p(\d+)$/.exec(value);
          if (m) {
            this.pushFx(track, onsetFrame, packFx(route.slot, route.param), parseInt(m[1], 10), out);
          }
        }
        continue;
      }

      const id = route.kind === "trackFx" ? packFx(route.slot, route.param) : route.param;
      if (
        typeof value !== "number" &&
        isContinuousPattern(value) &&
        PARAM_RAMP_KEYS.has(key) &&
        hap.whole &&
        route.kind === "trackFx" // only effect params ramp on the audio thread; per-voice params sample per onset
      ) {
        const target = num(value, hap.whole.end);
        out.push(paramRamp(track, onsetFrame, id, target, durFrames));
      } else {
        const v = num(value, hap.part.begin);
        if (route.kind === "paramSet") out.push(paramSet(track, onsetFrame, id, v));
        else this.pushFx(track, onsetFrame, id, v, out);
      }
    }
  }

  /** Emit a track-fx event only when its value changed since last time (chain state persists,
   *  so a constant `.room`/`.dist`/… needn't re-send on every note). */
  private pushFx(track: number, frame: number, id: number, v: number, out: RawEvent[]): void {
    const key = track * 0x10000 + id;
    if (this.lastFx.get(key) !== v) {
      this.lastFx.set(key, v);
      out.push(trackFx(track, frame, id, v));
    }
  }

  /** One scheduler wake — public so it can be driven deterministically in tests. */
  tick(): void {
    const clock = this.clock;
    const nowCycle = clock.nowCycle();
    const windowEnd = nowCycle + this.opts.lookaheadS * clock.cps;
    const begin = Math.max(this.scheduledUntilCycle, nowCycle);
    if (windowEnd <= begin) return;

    promotePending(begin); // swap any staged patterns due by this cycle, before querying

    const events: RawEvent[] = [];
    for (const { id, pattern } of activeTracks()) {
      const haps = queryArc(pattern as Pattern<ControlMap>, begin, windowEnd);
      for (const h of haps) {
        if (!hasOnset(h)) continue;
        const onset = h.whole!.begin;
        if (onset < begin || onset >= windowEnd) continue;
        this.compileHap(id, h, events);
      }
    }
    this.scheduledUntilCycle = windowEnd;
    if (events.length) this.encodeAndPush(events);
  }

  private encodeAndPush(events: RawEvent[]): void {
    events.sort((a, b) => a.frame - b.frame);
    const n = Math.min(events.length, MAX_EVENTS_PER_TICK);
    for (let i = 0; i < n; i++) events[i].encode(this.view, i, this.seq++);
    this.engine.pushEvents(this.buf.slice(0, n * REC));
  }
}
