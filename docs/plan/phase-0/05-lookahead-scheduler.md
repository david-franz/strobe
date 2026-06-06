# 05 — The look-ahead scheduler

## Purpose

This is the TypeScript side of the "two clocks" technique ([architecture.md](../../architecture.md#the-look-ahead-scheduler-two-clocks)) — the mechanism that makes a jittery, garbage-collected control plane produce *sample-accurate* music. A coarse JS timer wakes the scheduler every ~25 ms; on each wake it asks the active pattern what events fall in the window `[now, now + lookahead]` (default ~100 ms), converts each event's cycle-time to an exact engine **frame**, encodes it into a fixed 32-byte record, and hands the whole batch to the engine via one `pushEvents` call ([04-napi-bridge.md](04-napi-bridge.md)). The audio thread — which holds the only accurate clock, the sample count — then plays each event precisely when its frame arrives.

The reason this works, and the reason **jitter here is invisible**, is that the scheduler always works ~100 ms *into the future*. If a GC pause or an OS hiccup delays a tick by 5 ms, it doesn't matter: the events for that window were already enqueued on the previous (overlapping) tick, stamped with exact frames, and the audio thread plays them on time regardless of when the JS timer actually fired. The JS clock only has to be *roughly* on time; the sample count decides the music. This file implements the tick loop, the cycle→seconds→frames math, and the allocation-free `DataView` encoder.

## Location

```
packages/strobe/
  src/
    scheduler.ts     # NEW — this feature: tick loop + frame math + encoder
    engine.ts        # (04) the Engine interface it pushes to
    clock.ts         # (06) cps clock + cycle↔frame mapping helpers
    pattern.ts       # (06) Pattern/Hap/TimeSpan + queryArc
    tracks.ts        # (06) the p1 track slot the scheduler reads
```

This document owns `packages/strobe/src/scheduler.ts`. It consumes the clock and track from [06-clock-and-tracks.md](06-clock-and-tracks.md), the `Engine` from [04-napi-bridge.md](04-napi-bridge.md), and produces records mirroring [event-protocol.md](../../event-protocol.md#encoding-in-typescript).

## Design

### Constants & the reusable buffer

The encoder writes into **one** `ArrayBuffer` allocated at construction, viewed through one `DataView`, reused every tick — **no per-event allocation** ([event-protocol.md](../../event-protocol.md#encoding-in-typescript)). We size it for far more events than a 100 ms window can hold.

```ts
// packages/strobe/src/scheduler.ts
const REC = 32;                 // bytes per record (event-protocol.md)
const MAX_EVENTS_PER_TICK = 1024;

const TICK_MS = 25;             // coarse JS timer
const LOOKAHEAD_S = 0.1;        // 100 ms window (configurable)

export interface SchedulerOpts {
  tickMs?: number;
  lookaheadS?: number;
}
```

### The 32-byte `NoteOn` encoder (mirror of the Rust record)

This is the exact mirror of `EventRecord` in [02-event-ring-buffer.md](02-event-ring-buffer.md) and the snippet in [event-protocol.md](../../event-protocol.md#encoding-in-typescript). Every field is written little-endian by offset; no object is allocated.

```ts
const VERSION = 1;
const OP_NOTE_ON = 0x01;
const OP_TEMPO   = 0x40;

function writeNoteOn(
  view: DataView, i: number,
  track: number, frame: number, voice: number,
  freq: number, vel: number, durFrames: number, instrument: number, seq: number,
) {
  const o = i * REC;
  view.setUint8  (o + 0,  VERSION);
  view.setUint8  (o + 1,  OP_NOTE_ON);
  view.setUint16 (o + 2,  track,      true);
  view.setUint32 (o + 4,  frame >>> 0, true);  // frame is engine-relative u32
  view.setUint32 (o + 8,  voice >>> 0, true);
  view.setFloat32(o + 12, freq,       true);   // arg0
  view.setFloat32(o + 16, vel,        true);   // arg1
  view.setFloat32(o + 20, durFrames,  true);   // arg2
  view.setFloat32(o + 24, instrument, true);   // arg3
  view.setUint32 (o + 28, seq >>> 0,  true);   // flags/seq
}

function writeTempo(view: DataView, i: number, frame: number, cps: number, seq: number) {
  const o = i * REC;
  view.setUint8  (o + 0,  VERSION);
  view.setUint8  (o + 1,  OP_TEMPO);
  view.setUint16 (o + 2,  0, true);            // track 0 = global
  view.setUint32 (o + 4,  frame >>> 0, true);
  view.setUint32 (o + 8,  0, true);
  view.setFloat32(o + 12, cps, true);          // arg0 = cps
  view.setFloat32(o + 16, 0, true);
  view.setFloat32(o + 20, 0, true);
  view.setFloat32(o + 24, 0, true);
  view.setUint32 (o + 28, seq >>> 0, true);
}
```

### The Scheduler class & tick loop

The scheduler tracks how far into cycle-time it has already scheduled (`scheduledUntilCycle`) so successive overlapping windows never double-emit an event. On each tick it advances that frontier to `nowCycle + lookahead`, queries the track's pattern over `[scheduledUntilCycle, frontier)`, encodes the haps, and pushes once.

```ts
import type { Engine } from "./engine";
import { Clock } from "./clock";          // (06)
import { getTrack } from "./tracks";      // (06): returns p1's current pattern
import { queryArc, type Hap } from "./pattern"; // (06)

export class Scheduler {
  private buf = new ArrayBuffer(MAX_EVENTS_PER_TICK * REC);
  private view = new DataView(this.buf);
  private timer: ReturnType<typeof setInterval> | null = null;
  private scheduledUntilCycle = 0;        // frontier in cycle-time
  private seq = 0;
  private voiceCtr = 1;

  constructor(
    private engine: Engine,
    private clock: Clock,
    private opts: Required<SchedulerOpts> = { tickMs: TICK_MS, lookaheadS: LOOKAHEAD_S },
  ) {}

  start() {
    // Initialize the frontier from the current cycle so we don't backfill the past.
    this.scheduledUntilCycle = this.clock.nowCycle();
    this.timer = setInterval(() => this.tick(), this.opts.tickMs);
  }
  stop() { if (this.timer) { clearInterval(this.timer); this.timer = null; } }

  private tick() {
    const clock = this.clock;
    const nowCycle = clock.nowCycle();
    const lookaheadCycles = this.opts.lookaheadS * clock.cps;   // seconds → cycles
    const windowEnd = nowCycle + lookaheadCycles;

    // Overlapping windows: only schedule the *new* slice [scheduledUntilCycle, windowEnd).
    const begin = Math.max(this.scheduledUntilCycle, nowCycle); // never backfill the past
    if (windowEnd <= begin) return;

    const pattern = getTrack(1);            // Phase 0: single slot p1
    let n = 0;
    if (pattern) {
      const haps: Hap<any>[] = queryArc(pattern, begin, windowEnd);
      for (const h of haps) {
        if (n >= MAX_EVENTS_PER_TICK) break;
        // The event fires at the START of its whole (its onset). Skip haps whose
        // onset isn't inside this slice (e.g. a hap clipped at the window edge).
        const onsetCycle = h.whole ? h.whole.begin : h.part.begin;
        if (onsetCycle < begin || onsetCycle >= windowEnd) continue;

        const frame = clock.cycleToFrame(onsetCycle);   // (06): exact engine frame
        const durFrames = h.whole
          ? clock.cyclesToFrames(h.whole.end - h.whole.begin)
          : 0;
        // Phase 0 value is { s: "bd" } → one instrument (the kick). freq = body default.
        writeNoteOn(this.view, n,
          /*track*/ 1, frame, /*voice*/ this.voiceCtr++,
          /*freq*/ 55, /*vel*/ 0.9, durFrames, /*instrument*/ 0, this.seq++);
        n++;
      }
    }

    this.scheduledUntilCycle = windowEnd;

    if (n > 0) {
      // One pushEvents per tick: a view onto exactly the bytes we wrote.
      this.engine.pushEvents(this.buf.slice(0, n * REC));
    }
  }
}
```

> Implementation detail: `this.buf.slice(0, n*REC)` allocates a small copy. To stay strictly per-event-allocation-free *and* avoid even that copy, the engine's `pushEvents` can instead accept `(buffer, byteLength)` so we pass the persistent buffer plus a count; Phase 0 may use either. The **records themselves** are always written in place into the reusable buffer — the property [event-protocol.md](../../event-protocol.md#encoding-in-typescript) requires.

### Cycle → seconds → frames

The actual conversion lives in the `Clock` ([06-clock-and-tracks.md](06-clock-and-tracks.md)) so the scheduler stays transport-agnostic, but the chain is:

```
cycle  --(÷ cps)-->  seconds-since-epoch  --(× sampleRate)-->  engine frame
```

`clock.cycleToFrame(cycle)` returns the absolute engine frame, anchored so that the clock's notion of "now in cycles" lines up with the engine's `stats().currentFrame`. The scheduler periodically re-reads `engine.stats().currentFrame` through the clock to correct any slow drift between the JS wall clock and the audio sample clock (the sample count is the source of truth — [architecture.md](../../architecture.md#the-clock-and-musical-time)).

### Why jitter is invisible (restated concretely)

If `setInterval` fires late, `nowCycle` is simply larger; the window `[scheduledUntilCycle, nowCycle + lookahead)` still covers all the cycle-time we hadn't yet scheduled, so **no event is missed** and **no event is emitted twice** (the frontier guarantees coverage exactly once). As long as a tick lands within `lookahead - tickMs` of the previous one (here 100 − 25 = 75 ms of slack), every event is enqueued with its exact frame *before* its play time. Late events (frame already passed when the engine drains them) only occur if the JS thread stalls longer than that slack, and they are counted by the engine ([02-event-ring-buffer.md](02-event-ring-buffer.md)) — which is exactly the `stats().lateEvents` we assert is zero.

## Interfaces

- **To [04-napi-bridge.md](04-napi-bridge.md):** calls `engine.pushEvents(buffer)` once per tick; reads `engine.stats().currentFrame` (via the clock) to anchor frame math.
- **To [06-clock-and-tracks.md](06-clock-and-tracks.md):** uses `clock.nowCycle()`, `clock.cps`, `clock.cycleToFrame(cycle)`, `clock.cyclesToFrames(dCycles)`, and `getTrack(1)` / `queryArc(pattern, begin, end)`.
- **Event protocol:** writes 32-byte records whose layout exactly mirrors `EventRecord` ([02](02-event-ring-buffer.md)) and [event-protocol.md](../../event-protocol.md#event-record-layout); Phase 0 emits only `NoteOn` (and optionally a `Tempo` on `setcps`).
- **To [07-hot-reload-demo.md](07-hot-reload-demo.md):** the runtime owns one `Scheduler`, calls `start()/stop()`, and relies on the frontier logic to make a mid-stream track swap take effect at the next cycle boundary without double-firing.

## Edge cases & risks

- **Double-emit / gaps across overlapping windows.** Solved by the `scheduledUntilCycle` frontier: each cycle-time slice is queried exactly once. *Test below.*
- **Tick stall > slack.** If a tick is delayed beyond ~75 ms, some events may be enqueued past their frame → the engine applies them immediately and counts `lateEvents`. Mitigation: lookahead is configurable; Phase 0's defaults give comfortable slack on macOS.
- **Frame as u32.** Encoder masks with `>>> 0`; the clock anchors frames to a session epoch so the value fits u32 for Phase 0 sessions ([02](02-event-ring-buffer.md) notes the wrap window).
- **Hot-swap mid-window.** A track reassigned between ticks is picked up on the *next* tick's query; events already pushed for the committed window are not rewritten — exactly the seamless-swap semantics of [architecture.md](../../architecture.md#tracks-and-hot-swapping). [06](06-clock-and-tracks.md)/[07](07-hot-reload-demo.md) align the swap to a cycle boundary.
- **Per-event allocation.** Records are written in place into the reusable `ArrayBuffer`/`DataView`; no object per hap. The only optional allocation is the small `buf.slice` per tick, removable via the `(buffer, length)` push variant.
- **Clock drift.** Re-anchoring to `stats().currentFrame` keeps JS wall-clock estimates honest; without it, accumulated `setInterval` error would slowly detune the groove.

## Acceptance criteria

- [ ] **Encoder byte-exactness.** *Unit test:* `writeNoteOn` a known event into a fresh `DataView`, read the 32 bytes back, and assert they equal the expected little-endian layout from [event-protocol.md](../../event-protocol.md#event-record-layout). Cross-check the same bytes decode correctly in the Rust `decode` test ([02](02-event-ring-buffer.md)).
- [ ] **Each cycle-time slice scheduled exactly once.** *Unit test:* with a fake clock and a `bd*4` pattern, run 10 overlapping ticks; collect all encoded frames; assert no duplicate onset frame and no missing quarter-cycle onset across the covered span.
- [ ] **Frame math.** *Unit test:* at `cps = 130/60/4`, `sampleRate = 48000`, assert `clock.cycleToFrame(1.0)` equals the frame one cycle later (≈ `48000 / cps` frames after cycle 0's frame), within ±1.
- [ ] **One pushEvents per tick.** *Unit test:* spy on `engine.pushEvents`; assert at most one call per tick and that its `byteLength` is `n*32`.
- [ ] **Jitter tolerance (integration).** *How to test:* run the example ([07](07-hot-reload-demo.md)) for 60 s while inducing GC/CPU load; assert `stats().lateEvents === 0` and the kick stays in time (audible check).
