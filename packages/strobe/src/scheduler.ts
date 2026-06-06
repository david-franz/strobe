// The TypeScript side of the "two clocks" technique (architecture.md). A coarse JS
// timer wakes the scheduler every ~25 ms; on each wake it queries the active pattern
// over [now, now+lookahead], converts each event's cycle-time to an exact engine
// frame, encodes it into a fixed 32-byte record, and hands the batch to the engine in
// one pushEvents call. Jitter here is invisible because the scheduler always works
// ~100 ms into the future and the sample count decides the music.

import type { Engine } from "./engine";
import type { Clock } from "./clock";
import { getTrack } from "./tracks";
import { queryArc, type Hap } from "./pattern";

const REC = 32; // bytes per record (event-protocol.md)
const MAX_EVENTS_PER_TICK = 1024;

const TICK_MS = 25; // coarse JS timer
const LOOKAHEAD_S = 0.1; // 100 ms window (configurable)

const VERSION = 1;
const OP_NOTE_ON = 0x01;
const OP_TEMPO = 0x40;

export interface SchedulerOpts {
  tickMs?: number;
  lookaheadS?: number;
}

/** Encode a NoteOn into `view` at record index `i`, little-endian, no allocation.
 *  Exact mirror of the Rust `EventRecord` (event.rs / event-protocol.md). */
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
  view.setUint32(o + 4, frame >>> 0, true); // frame is engine-relative u32
  view.setUint32(o + 8, voice >>> 0, true);
  view.setFloat32(o + 12, freq, true); // arg0
  view.setFloat32(o + 16, vel, true); // arg1
  view.setFloat32(o + 20, durFrames, true); // arg2
  view.setFloat32(o + 24, instrument, true); // arg3
  view.setUint32(o + 28, seq >>> 0, true); // flags/seq
}

/** Encode a Tempo (global track) into `view` at record index `i`. */
export function writeTempo(view: DataView, i: number, frame: number, cps: number, seq: number): void {
  const o = i * REC;
  view.setUint8(o + 0, VERSION);
  view.setUint8(o + 1, OP_TEMPO);
  view.setUint16(o + 2, 0, true); // track 0 = global
  view.setUint32(o + 4, frame >>> 0, true);
  view.setUint32(o + 8, 0, true);
  view.setFloat32(o + 12, cps, true); // arg0 = cps
  view.setFloat32(o + 16, 0, true);
  view.setFloat32(o + 20, 0, true);
  view.setFloat32(o + 24, 0, true);
  view.setUint32(o + 28, seq >>> 0, true);
}

export class Scheduler {
  private buf = new ArrayBuffer(MAX_EVENTS_PER_TICK * REC);
  private view = new DataView(this.buf);
  private timer: ReturnType<typeof setInterval> | null = null;
  private scheduledUntilCycle = 0; // frontier in cycle-time
  private seq = 0;
  private voiceCtr = 1;
  private readonly opts: Required<SchedulerOpts>;

  constructor(
    private engine: Engine,
    private clock: Clock,
    opts: SchedulerOpts = {},
  ) {
    this.opts = {
      tickMs: opts.tickMs ?? TICK_MS,
      lookaheadS: opts.lookaheadS ?? LOOKAHEAD_S,
    };
  }

  start(): void {
    // Initialize the frontier from the current cycle so we don't backfill the past.
    this.scheduledUntilCycle = this.clock.nowCycle();
    this.timer = setInterval(() => this.tick(), this.opts.tickMs);
  }

  stop(): void {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }

  /** One scheduler wake: query the new slice of cycle-time, encode, push once.
   *  Public so it can be driven deterministically in tests. */
  tick(): void {
    const clock = this.clock;
    const nowCycle = clock.nowCycle();
    const lookaheadCycles = this.opts.lookaheadS * clock.cps; // seconds → cycles
    const windowEnd = nowCycle + lookaheadCycles;

    // Overlapping windows: only schedule the *new* slice [scheduledUntilCycle, windowEnd).
    const begin = Math.max(this.scheduledUntilCycle, nowCycle); // never backfill the past
    if (windowEnd <= begin) return;

    const pattern = getTrack(1); // Phase 0: single slot p1
    let n = 0;
    if (pattern) {
      const haps: Hap<unknown>[] = queryArc(pattern, begin, windowEnd);
      for (const h of haps) {
        if (n >= MAX_EVENTS_PER_TICK) break;
        // The event fires at the onset of its whole; skip haps whose onset isn't in
        // this slice (e.g. a hap clipped at the window edge).
        const onsetCycle = h.whole ? h.whole.begin : h.part.begin;
        if (onsetCycle < begin || onsetCycle >= windowEnd) continue;

        const frame = clock.cycleToFrame(onsetCycle); // exact engine frame
        const durFrames = h.whole ? clock.cyclesToFrames(h.whole.end - h.whole.begin) : 0;
        // Phase 0 value is { s: "bd" } → the kick (instrument 0), body default freq.
        writeNoteOn(this.view, n, 1, frame, this.voiceCtr++, 55, 0.9, durFrames, 0, this.seq++);
        n++;
      }
    }

    this.scheduledUntilCycle = windowEnd;

    if (n > 0) {
      // One pushEvents per tick: a copy of exactly the bytes we wrote.
      this.engine.pushEvents(this.buf.slice(0, n * REC));
    }
  }
}
