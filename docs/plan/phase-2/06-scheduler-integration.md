# Phase 2.06 — Scheduler integration (`scheduler.ts`)

## Purpose

Phase 0 shipped a look-ahead scheduler that queried one trivial pattern on one track and emitted `NoteOn`s ([phase-0/05-lookahead-scheduler.md](../phase-0/05-lookahead-scheduler.md)). Phase 2 upgrades it to the real thing: each tick it **queries every track's arbitrary `Pattern<ControlMap>`** over the lookahead window, turns each onset `Hap` into the right engine events (`NoteOn`/`TrigSample` + `ParamSet`/`ParamRamp`/`TrackFx`), allocates voice handles, computes frame timestamps, and **batch-encodes** them into the fixed 32-byte protocol ([event-protocol.md](../../event-protocol.md)) for one `pushEvents` call.

The scheduler is where the pure pattern world meets the engine bridge. Crucially, **none of Phase 0's invariants change**: ~25 ms tick, query `[now, now+lookahead]`, cycle→seconds→frames, reusable `ArrayBuffer` written via `DataView`, one non-blocking `pushEvents` per tick ([architecture.md](../../architecture.md#the-look-ahead-scheduler-two-clocks)). Only the *content* generation is richer.

## Location

```
packages/strobe/src/
  scheduler.ts     # the tick loop, the Hap→events compiler, the encoder
  voices.ts        # voice-handle allocation (TS side; engine maps handle→physical voice)
  scheduler.test.ts
```

`scheduler.ts` imports `queryArc`/`filterOnsets` ([01](01-pattern-core.md)), `PARAM_REGISTRY`/`noteToFreq` ([04](04-sound-note-api.md)), the continuous-vs-ramp logic ([05](05-continuous-signals.md)), and the track registry ([07-tracks-hot-swap.md](07-tracks-hot-swap.md)). It targets the Phase 0 `Engine` interface ([event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)).

## Design

### The tick (unchanged skeleton, richer body)

```ts
function tick() {
  const win = nextWindow();                  // { beginCycle, endCycle } from the cps clock + lookahead
  const events: RawEvent[] = [];
  for (const track of registry.activeTracks()) {        // [07]
    const haps = queryArc(track.pattern, win.beginCycle, win.endCycle);
    for (const hap of haps) {
      if (!hasOnset(hap)) continue;          // [01] skip cycle-split fragments — only true note starts trigger
      compileHap(track.id, hap, events);     // ← the new heart of Phase 2
    }
  }
  encodeAndPush(events);                      // reusable ArrayBuffer + one pushEvents (Phase 0 mechanic)
}
```

`hasOnset` ([01](01-pattern-core.md)) is the gate: a hap whose `part.begin > whole.begin` is a fragment of a note that already started in an earlier window, so it must **not** re-trigger. Continuous-signal haps (no `whole`) are likewise never triggered (they only ever appear as *parameter values* inside a `ControlMap`, sampled below).

### Cycle → frame

Reused from Phase 0 ([event-protocol.md](../../event-protocol.md#the-frame-timestamp)):

```ts
const secondsOf = (cycle: number) => (cycle - engineStartCycle) / cps;
const frameOf   = (cycle: number) => Math.round(secondsOf(cycle) * sampleRate) + engineStartFrame;
const framesOf  = (cycleSpan: number) => Math.round((cycleSpan / cps) * sampleRate);
```

`frameOf(hap.part.begin)` is the onset frame; `framesOf(whole.end - whole.begin)` is the note duration in frames.

### Voice-handle allocation (`voices.ts`)

For gated (sustained) synth notes, TS supplies a `voice` handle in the `NoteOn` and later emits a `NoteOff` for it ([event-protocol.md](../../event-protocol.md#voice-ids)). One-shots (drums, samples) pass `duration` in `arg2` and need no `NoteOff`.

```ts
let nextHandle = 1;
const allocVoice = () => (nextHandle = (nextHandle % 0xffffffff) + 1);

function isGated(cm: ControlMap): boolean {
  // sustained if it has a sustain/release envelope and no explicit one-shot flag.
  return cm.s !== "bd" && cm.s !== "sn" && cm.s !== "hh" && cm.s !== "cp" && cm.release !== undefined;
}
```

The engine's `voice_alloc` maps the handle to a physical voice and remembers it across stealing, so `NoteOff` finds the right one ([event-protocol.md](../../event-protocol.md#voice-ids)).

### The `Hap` → events compiler

This is the new core. It splits a `ControlMap` into one trigger event plus any number of param/fx events, using `PARAM_REGISTRY` ([04](04-sound-note-api.md)) to decide packed-vs-separate, and [05](05-continuous-signals.md)'s rule to decide per-note-sample vs `ParamRamp`.

```ts
function compileHap(track: number, hap: Hap<ControlMap>, out: RawEvent[]) {
  const cm = hap.value;
  const onsetFrame = frameOf(hap.part.begin);
  const durFrames  = hap.whole ? framesOf(hap.whole.end - hap.whole.begin) : 0;

  // 1) Resolve every control value: a plain number stays; a Pat<number>/signal is sampled now.
  const resolved = resolveValues(cm, hap);     // { freq?, gain?, lpf?, pan?, … } all numbers + routing hints

  // 2) Decide the trigger opcode from `s`.
  const isSample = isSampleName(resolved.s);
  const voice = isGated(cm) ? allocVoice() : 0;

  if (isSample) {
    out.push(trigSample(track, onsetFrame, {
      speed: resolved.speed ?? 1, gain: resolved.gain ?? 1,
      start: 0, sample: sampleId(resolved.s),
    }));
  } else {
    out.push(noteOn(track, onsetFrame, voice, {
      freq: resolved.freq ?? noteToFreq(resolved.note ?? 60),
      vel:  resolved.gain ?? 1,
      durFrames,                       // one-shot drums carry duration here; no NoteOff
      instrument: instrumentId(resolved.s ?? "saw"),
    }));
    if (voice !== 0 && hap.whole) {
      out.push(noteOff(track, frameOf(hap.whole.end), voice));   // gated note release
    }
  }

  // 3) Separate per-voice / per-track param events for keys not packed above.
  for (const [key, val] of Object.entries(resolved.extras)) {
    const route = PARAM_REGISTRY[key];
    if (!route) { warnUnknownParam(key); continue; }
    switch (route.kind) {
      case "paramSet": out.push(paramSet(track, onsetFrame, route.param, val)); break;          // 0x10
      case "trackFx":  out.push(trackFx(track, onsetFrame, route.param, val)); break;           // 0x30
      case "ramp":     out.push(paramRamp(track, onsetFrame, route.param, val.target, val.frames)); break; // 0x11
      // noteField/trigField were already packed in step 2.
    }
  }
}
```

`resolveValues` is where [05](05-continuous-signals.md)'s decision rule runs per key:

```ts
function resolveValues(cm: ControlMap, hap: Hap<ControlMap>) {
  const packed: any = {}; const extras: Record<string, any> = {};
  for (const [key, value] of Object.entries(cm)) {
    if (key === "s") { packed.s = value; continue; }
    if (typeof value === "string") { packed[key] = value; continue; }   // e.g. sidechain target
    const route = PARAM_REGISTRY[key];
    if (typeof value === "number") {
      route?.kind === "noteField" || route?.kind === "trigField"
        ? (packed[key] = value) : (extras[key] = value);
      continue;
    }
    // value is a Pat<number>: sample (or ramp) per [05].
    if (isContinuousPattern(value) && PARAM_RAMP_KEYS.has(key) && hap.whole) {
      const v0 = queryArc(value, hap.whole.begin, hap.whole.begin)[0].value;
      const v1 = queryArc(value, hap.whole.end,   hap.whole.end)[0].value;
      extras[key] = { kind: "ramp", target: v1, from: v0, frames: framesOf(hap.whole.end - hap.whole.begin) };
    } else {
      const v = queryArc(value, hap.part.begin, hap.part.begin)[0]?.value ?? 0;   // zero-width onset sample
      const route = PARAM_REGISTRY[key];
      route?.kind === "noteField" || route?.kind === "trigField"
        ? (packed[key] = v) : (extras[key] = v);
    }
  }
  return { ...packed, extras };
}
```

### Worked example — `p1(sound("bd*4").gain(0.9))`

Query cycle 0 (`bd*4` → 4 onsets at `0, .25, .5, .75`, value `{s:"bd", gain:0.9}`):

```
for each onset:
  resolved = { s:"bd", gain:0.9, extras:{} }
  isSample("bd") → false (synth drum) ; isGated → false (drum one-shot)
  → NoteOn(track=1, frame=frameOf(t_i), voice=0,
           freq=<bd base>, vel=0.9, durFrames=framesOf(0.25), instrument=id("bd"))
  no extras → no ParamSet/TrackFx
→ 4 NoteOn records, evenly frame-spaced, vel 0.9
```

### Worked example — the README bass

`p3(note("c2 c2 eb2 g2").saw().lpf(sine.range(300,1200).slow(8)).room(0.3).gain(0.85))`, one onset:

```
cm = { note:36, s:"saw", lpf:<signal>, room:0.3, gain:0.85 }
resolveValues:
  note 36 → packed.note=36 (→ freq via noteToFreq)        [noteField]
  gain 0.85 → packed.gain=0.85 (→ velocity)               [noteField]
  s "saw" → packed.s
  lpf <continuous signal>, ramp key, has whole →
      extras.lpf = ramp{ from: sine@onset, target: sine@noteEnd, frames }   [05 → ParamRamp]
  room 0.3 → extras.room = 0.3                              [trackFx]
emit:
  NoteOn(track=3, voice=alloc, freq=noteToFreq(36), vel=0.85, instrument=id("saw"))
  NoteOff(track=3, frame=frameOf(whole.end), voice)        (gated: saw has release)
  ParamRamp(track=3, param=0x01/*lpf*/, target, frames)    (0x11)
  TrackFx(track=3, param=0x02/*room*/, 0.3)                (0x30)
```

This realizes the README's "8-cycle filter sweep" as `ParamRamp`s that the engine interpolates on the audio thread.

### Batch encoding (Phase 0 mechanic, more opcodes)

`encodeAndPush` writes each `RawEvent` into a reusable `ArrayBuffer` via `DataView` using the writers from [event-protocol.md](../../event-protocol.md#encoding-in-typescript) (`writeNoteOn`, and siblings for `NoteOff`/`ParamSet`/`ParamRamp`/`TrigSample`/`TrackFx`), then one `engine.pushEvents(buffer)`:

```ts
function encodeAndPush(events: RawEvent[]) {
  events.sort((a, b) => a.frame - b.frame);            // frame-ordered for the consumer
  ensureCapacity(events.length);                       // grow the reusable buffer if needed
  const view = new DataView(scratch);
  events.forEach((e, i) => writeRecord(view, i, e));   // dispatch on opcode → 32-byte writer
  engine.pushEvents(scratch.slice(0, events.length * 32));
}
```

No per-event allocation beyond the `RawEvent` objects (control plane); the encoded bytes reuse one buffer. The audio thread drains `frame ≤ currentFrame + bufferLen` and applies each at its in-buffer offset ([event-protocol.md](../../event-protocol.md#the-frame-timestamp)).

## Interfaces

- **[01-pattern-core.md](01-pattern-core.md):** `queryArc` per track per tick; `hasOnset` gates triggers; zero-width `queryArc` samples param patterns.
- **[04-sound-note-api.md](04-sound-note-api.md):** `PARAM_REGISTRY` (packed-vs-separate routing), `noteToFreq`, `s`→instrument/sample id. The registry is the contract.
- **[05-continuous-signals.md](05-continuous-signals.md):** `isContinuousPattern`, `PARAM_RAMP_KEYS`, and the per-note-vs-ramp decision live there; the scheduler executes them in `resolveValues`.
- **[07-tracks-hot-swap.md](07-tracks-hot-swap.md):** `registry.activeTracks()` yields each track's current pattern and `track.id` (the protocol `track` field, p1=1…). Mute/solo filter which tracks `activeTracks()` returns. Hot-swap timing (next cycle boundary) is the registry's job; the scheduler just reads whatever pattern is current at query time.
- **Phase 0 `Engine`:** `pushEvents` (hot path), `stats()` (underruns/late/ring fullness surfaced for telemetry). The 32-byte layout and `writeNoteOn` are reused verbatim.

## Edge cases & risks

- **Purity:** the scheduler must not mutate patterns; it only queries them. Because patterns are pure, re-querying an overlapping window (overlapping lookahead windows are expected, [architecture.md](../../architecture.md#the-look-ahead-scheduler-two-clocks)) yields consistent haps. **De-duplication across overlapping windows** is handled by tracking the last-scheduled frame per track and only emitting onsets with `frame >= lastPushedFrame`, so the same onset is never sent twice.
- **Fractional time precision:** `frameOf` rounds cycle→frame once at the boundary; using `sameTime` for onset comparisons ([01](01-pattern-core.md)) prevents an onset landing at `0.9999999` from being missed at a window edge. Window edges align to the tick, not to onsets, so an onset exactly on the boundary is claimed by exactly one window (half-open `[begin, end)`).
- **Continuous/infinite patterns:** a continuous signal as a *top-level* track pattern yields no onsets → silence; the scheduler warns once. Param signals are only sampled, never triggered. A pattern with unbounded haps in a finite window cannot occur because `spanCycles` bounds work to the window.
- **Late events / backpressure:** an onset whose `frame < currentFrame` (tick ran late) is still pushed and applied immediately, counted as a late-event warning, never dropped musically ([event-protocol.md](../../event-protocol.md#the-frame-timestamp)). If `pushEvents` finds the ring full it drops and bumps a counter surfaced via `stats()` ([event-protocol.md](../../event-protocol.md#the-event-ring-buffer)).
- **Voice-handle exhaustion / leaks:** handles wrap at u32; a gated note must always emit its `NoteOff` (we emit it in the same compile pass from `whole.end`), so no stuck voices. One-shots never allocate a handle.
- **Performance of querying:** per tick = (tracks × haps-in-window) `queryArc`s plus per-onset param sampling. Lookahead spans a small constant number of cycles, tracks are few, so this is well within the ~25 ms budget on the control plane; the audio thread never runs any of it.

## Acceptance criteria

- `p1(sound("bd*4"))` over one cycle produces exactly 4 `NoteOn` records at evenly spaced frames, no `NoteOff` (one-shot), no param events.
- `p1(sound("bd*4").gain(0.9))` sets each `NoteOn.vel === 0.9` (gain packed as velocity per `PARAM_REGISTRY`).
- A gated synth note (`note("c2").saw().release(0.2)`) emits a `NoteOn` with an allocated `voice` **and** a matching `NoteOff` at the note's end frame.
- `.lpf(800)` (static) emits one `ParamSet` (0x10, param 0x01) at the note frame; `.room(0.3)` emits one `TrackFx` (0x30) on the track channel.
- `.lpf(sine.range(300,1200).slow(8))` emits a `ParamRamp` (0x11) per note whose `from`/`target` match the signal sampled at the note's `whole.begin`/`whole.end`.
- A cycle-split fragment (`hasOnset === false`) produces **no** events.
- Overlapping lookahead windows do not double-emit an onset (dedup by last-pushed frame).
- All emitted records are exactly 32 bytes, little-endian, frame-sorted, pushed in one `pushEvents` call; `stats()` reports zero underruns/late events under the README song's load.
