# 06 — The cps clock, track slot, and minimal pattern

## Purpose

The scheduler ([05-lookahead-scheduler.md](05-lookahead-scheduler.md)) needs three things to do its job: a way to know "what cycle-time is it now and how do cycles map to engine frames" (the **clock**), a place to read "what is the music" (the **track slot** `p1`), and a minimal notion of a **pattern** that, when queried over a span, yields the events to play. This feature provides all three at the smallest scale that proves the spine: the cps clock with its cycle↔wall-clock↔frame mapping, a single mutable track slot, and a trivial "play `bd` every 1/4 cycle" pattern.

Time in `strobe` is **cycle-based** following TidalCycles ([architecture.md](../../architecture.md#the-clock-and-musical-time)): the unit is the *cycle*, tempo is *cycles per second* (`cps`), and `setcps(x)` sets it directly with the BPM helper `bpm = cps * 4 * 60`. The pattern shape here — `Pattern`, `Hap`, `TimeSpan`, and `queryArc` — is the exact (tiny) subset of [pattern-model.md](../../pattern-model.md) that Phase 0 uses, so that when Phase 2 builds the full pattern engine and mini-notation parser, nothing above the bridge changes shape.

## Location

```
packages/strobe/
  src/
    clock.ts        # NEW — Clock: cps, setcps, bpm, cycle↔frame mapping
    pattern.ts      # NEW — Pattern/Hap/TimeSpan + queryArc (minimal)
    tracks.ts       # NEW — the p1 slot + getTrack
    index.ts        # NEW — public exports: setcps, p1, bd-pattern helper
    scheduler.ts    # (05) consumes Clock + getTrack + queryArc
```

This document owns `clock.ts`, `pattern.ts`, `tracks.ts`, and the Phase 0 exports in `index.ts`. The types here are consistent with [pattern-model.md](../../pattern-model.md) and are the precursor to the full engine in Phase 2.

## Design

### The minimal Pattern / Hap / TimeSpan (`pattern.ts`)

Verbatim shapes from [pattern-model.md](../../pattern-model.md#the-three-primitives) — a pattern is a pure function from a span of time to the haps in it:

```ts
// packages/strobe/src/pattern.ts

/** A half-open interval of cycle-time, [begin, end). */
export interface TimeSpan { begin: number; end: number; }

/** One thing that happens: a value, when it happens, the visible part. */
export interface Hap<T> {
  whole: TimeSpan | undefined;  // full extent (undefined = continuous/analog)
  part:  TimeSpan;              // portion intersecting the query
  value: T;                     // what plays (Phase 0: { s: "bd" })
}

/** A pattern is a pure function from a span to the haps in it. */
export type Pattern<T> = (span: TimeSpan) => Hap<T>[];

/** Convenience: query a pattern over [begin, end) in cycle-time. */
export function queryArc<T>(pat: Pattern<T>, begin: number, end: number): Hap<T>[] {
  return pat({ begin, end });
}
```

Because patterns are **pure functions** ([pattern-model.md](../../pattern-model.md#why-pure-functions-not-a-sequencer-object)), the scheduler can query any window at any time with no side effects — the property that makes seamless hot-swapping possible.

### The Phase 0 pattern: "play `bd` every 1/4 cycle"

Phase 0 has no mini-notation parser (that is Phase 2). Instead a tiny helper builds the `bd*n` pattern directly: `n` evenly-spaced kick onsets per cycle. It correctly handles being queried over *any* span (including multi-cycle and fractional spans), by emitting one hap per `1/n` slot whose onset lies inside the queried span — exactly what `queryArc(sound("bd*4"), 0, 1)` would return ([pattern-model.md](../../pattern-model.md#testing-patterns)).

```ts
// packages/strobe/src/pattern.ts (continued)

export interface SoundValue { s: string; }

/** Phase 0 stand-in for sound("bd*n"): n evenly spaced "bd" onsets per cycle. */
export function bdEvery(n: number): Pattern<SoundValue> {
  return ({ begin, end }) => {
    const haps: Hap<SoundValue>[] = [];
    const step = 1 / n;
    // First slot index at or after `begin`.
    let k = Math.ceil(begin / step - 1e-9);
    for (;;) {
      const onset = k * step;
      if (onset >= end) break;
      const wholeEnd = onset + step;
      haps.push({
        whole: { begin: onset, end: wholeEnd },
        part:  { begin: Math.max(onset, begin), end: Math.min(wholeEnd, end) },
        value: { s: "bd" },
      });
      k++;
    }
    return haps;
  };
}
```

> `bd*4` → `bdEvery(4)`; editing the song to `bd*8` → `bdEvery(8)`. A Euclidean `bd(3,8)` is a trivial variant for the demo (precompute the 3-of-8 onset mask and emit only those slots) — included as `bdEuclid(3, 8)` so the hot-reload test in [07-hot-reload-demo.md](07-hot-reload-demo.md) can exercise it. The full `*`, `(n,k)`, `[]`, `<>` mini-notation arrives in Phase 2.

### The cps clock (`clock.ts`)

The clock holds the tempo and maps cycle-time to engine frames. It is anchored to the engine's frame counter (the source of truth — [architecture.md](../../architecture.md#the-clock-and-musical-time)) so JS wall-clock jitter never detunes the groove: "now in cycles" is derived from `stats().currentFrame`, not from `Date.now()`.

```ts
// packages/strobe/src/clock.ts
import type { Engine } from "./engine";

export class Clock {
  cps = 0.5;                 // cycles per second (default ~120 BPM at 4 beats/cycle)
  private sampleRate = 48000;
  private epochCycle = 0;    // cycle-time at the engine-frame epoch below
  private epochFrame = 0;    // engine frame corresponding to epochCycle

  constructor(private engine: Engine) {}

  /** Capture the engine's frame/sample-rate as the clock epoch. Called at start(). */
  anchor(sampleRate: number) {
    this.sampleRate = sampleRate;
    this.epochFrame = this.engine.stats().currentFrame;
    this.epochCycle = 0;     // cycle 0 is "now" at start
  }

  /** Set tempo directly (TidalCycles `setcps`). */
  setcps(x: number) {
    // Re-anchor so the cycle position is continuous across a tempo change:
    const nowFrame = this.engine.stats().currentFrame;
    this.epochCycle = this.nowCycle();   // freeze current cycle position
    this.epochFrame = nowFrame;
    this.cps = x;
  }

  /** BPM helper: at 4 beats/cycle, bpm = cps*4*60. */
  get bpm() { return this.cps * 4 * 60; }
  setbpm(bpm: number) { this.setcps(bpm / 60 / 4); }

  /** Current cycle-time, derived from the engine's sample clock (source of truth). */
  nowCycle(): number {
    const frame = this.engine.stats().currentFrame;
    const secs = (frame - this.epochFrame) / this.sampleRate;
    return this.epochCycle + secs * this.cps;
  }

  /** cycle-time → absolute engine frame (used to stamp events). */
  cycleToFrame(cycle: number): number {
    const secs = (cycle - this.epochCycle) / this.cps;
    return Math.round(this.epochFrame + secs * this.sampleRate);
  }

  /** a duration in cycles → a duration in frames (for note length). */
  cyclesToFrames(dCycles: number): number {
    return Math.round((dCycles / this.cps) * this.sampleRate);
  }
}
```

The mapping chain, in both directions:

```
cycle  ─(− epochCycle, ÷ cps)→  seconds  ─(× sampleRate, + epochFrame)→  engine frame
frame  ─(− epochFrame, ÷ sampleRate)→ seconds ─(× cps, + epochCycle)→  cycle
```

`setcps` re-anchors `epochCycle`/`epochFrame` so changing tempo mid-session doesn't teleport the cycle position. When `setcps` is called live, the scheduler may also emit a `Tempo` (`0x40`) event so the engine's notion of cps stays in sync ([event-protocol.md](../../event-protocol.md#opcodes)); in Phase 0 the engine's cps only matters for echoing in `stats`, since all timing is already baked into the per-event `frame`.

### The single track slot (`tracks.ts`)

Phase 0 has one mutable slot, `p1`. Assigning it is a plain variable write — cheap and atomic from JS's perspective ([architecture.md](../../architecture.md#tracks-and-hot-swapping)). Re-running `p1(...)` swaps the pattern reference; the scheduler picks up the new pattern on its next query, and [07-hot-reload-demo.md](07-hot-reload-demo.md) aligns the visible effect to the next cycle boundary.

```ts
// packages/strobe/src/tracks.ts
import type { Pattern } from "./pattern";

const slots: (Pattern<any> | null)[] = [null, null]; // index 1 = p1

/** Assign p1's pattern (the live-coding entry point). */
export function p1(pattern: Pattern<any>): void { slots[1] = pattern; }

/** Scheduler reads the current pattern for a track id. */
export function getTrack(id: number): Pattern<any> | null { return slots[id] ?? null; }
```

### Public exports (`index.ts`)

```ts
// packages/strobe/src/index.ts (Phase 0 surface)
export { p1 } from "./tracks";
export { bdEvery, bdEuclid } from "./pattern";   // Phase 0 stand-ins for sound("bd*n")
export type { Pattern, Hap, TimeSpan } from "./pattern";

// The clock is created by the runtime ([07]) and exposes setcps:
//   setcps(130/60/4);  p1( bdEvery(4) );
// `sound("bd*4")` is intentionally NOT in Phase 0 — it lands with the parser in Phase 2.
```

## Interfaces

- **To [05-lookahead-scheduler.md](05-lookahead-scheduler.md):** provides `Clock` (`nowCycle`, `cps`, `cycleToFrame`, `cyclesToFrames`), `getTrack(1)`, and `queryArc`. The scheduler stamps every hap's onset via `cycleToFrame`.
- **To [04-napi-bridge.md](04-napi-bridge.md):** `Clock.anchor` and `nowCycle` read `engine.stats().currentFrame`; `setcps` (live) may push a `Tempo` event through the scheduler.
- **To [07-hot-reload-demo.md](07-hot-reload-demo.md):** the song file calls `setcps(...)` and `p1(...)`; the runtime owns the single `Clock` and the slot module's state persists across reloads so the swap is just a reassignment.
- **Event protocol:** the pattern's `value` (`{ s: "bd" }`) maps to a `NoteOn` for the kick instrument; `whole` length → `arg2` duration in frames.
- **Consistency:** `Pattern`/`Hap`/`TimeSpan`/`queryArc` are exactly the [pattern-model.md](../../pattern-model.md) shapes, so Phase 2 replaces `bdEvery` with the real `sound()` + parser without changing the scheduler or clock.

## Edge cases & risks

- **Tempo change continuity.** `setcps` re-anchors so the cycle position is continuous; without it a live tempo edit would jump the groove. *Test below.*
- **Span boundary correctness in `bdEvery`.** The `Math.ceil(begin/step − ε)` start index plus the half-open `onset < end` test ensure each onset is emitted exactly once across overlapping scheduler windows (no duplicates, no gaps) — this is what keeps the scheduler's frontier logic ([05](05-lookahead-scheduler.md)) honest.
- **Floating-point onset drift.** Onsets are computed as `k * step` (exact for power-of-two-ish `n`); the small `1e-9` epsilon avoids missing an onset that lands exactly on a window edge.
- **Clock drift vs. JS time.** `nowCycle` derives from `stats().currentFrame`, not `Date.now()`, so accumulated `setInterval` error cannot detune playback; the sample count is authoritative.
- **Slot state across reload.** `tracks.ts` module state must survive a song re-eval; the runtime re-imports the song but keeps the library module instance ([07](07-hot-reload-demo.md)). If the song re-imported a fresh copy of the library, the slot would reset — explicitly avoided by the runtime's module strategy.
- **Pre-`anchor` queries.** If `nowCycle` is called before `start()`/`anchor()`, it uses `currentFrame = 0` and returns 0 — harmless; the scheduler only runs after start.

## Acceptance criteria

- [ ] **`queryArc(bdEvery(4), 0, 1)` is correct.** *Unit test:* assert 4 haps at onsets `0, 0.25, 0.5, 0.75`, each `value:{s:"bd"}`, `whole.end = onset+0.25` — matching the [pattern-model.md](../../pattern-model.md#testing-patterns) style.
- [ ] **No duplicate/missing onsets across windows.** *Unit test:* query `bdEvery(4)` over `[0,0.3)`, `[0.3,0.6)`, `[0.6,1.0)`; concatenate; assert exactly the 4 onsets, each once.
- [ ] **Multi-cycle query.** *Unit test:* `queryArc(bdEvery(4), 3.0, 4.0)` returns the same shape shifted to cycle 3 (onsets `3.0, 3.25, 3.5, 3.75`).
- [ ] **BPM helper.** *Unit test:* `setbpm(130)` ⇒ `cps ≈ 130/60/4` and `bpm` getter returns ≈130.
- [ ] **cycle↔frame round-trip.** *Unit test:* with `sampleRate=48000`, `cps=130/60/4`, after `anchor`, assert `cycleToFrame(0) ≈ epochFrame` and `cycleToFrame(1) − cycleToFrame(0) ≈ 48000/cps` (±1); and that `nowCycle()` increases monotonically as `currentFrame` increases (fake engine).
- [ ] **setcps continuity.** *Unit test:* call `setcps` mid-session (fake clock at a known frame); assert `nowCycle()` is continuous across the change (no jump), and subsequent frames advance at the new rate.
- [ ] **Slot swap.** *Unit test:* `p1(bdEvery(4)); getTrack(1)` returns that pattern; `p1(bdEvery(8))` then returns the new one (reference replaced).
