# Phase 2.05 — Continuous signals (`signals.ts`)

## Purpose

Continuous signals are how you get *movement*: `note("c2*8").lpf(sine.range(300,1200).slow(8))` sweeps the filter over 8 cycles ([pattern-model.md](../../pattern-model.md#parameters-can-themselves-be-patterns)). This file implements the built-in signals from the model — `sine`, `cosine`, `saw`, `isaw`, `tri`, `square`, `rand`, `perlin` — as **continuous patterns** (`whole: undefined`, output `0..1`), plus `.range(lo, hi)` to rescale and `.slow`/`.fast` to set the period.

It also specifies the crucial decision the model implies but does not pin down: **how a signal is consumed when used as a parameter value** — sampled per note in TS, or handed to the engine as a `ParamRamp` for continuous audio-rate modulation. That decision lives here and is executed by the scheduler ([06-scheduler-integration.md](06-scheduler-integration.md)).

## Location

```
packages/strobe/src/
  signals.ts       # sine, cosine, saw, isaw, tri, square, rand, perlin, .range
  signals.test.ts
```

`signals.ts` builds on `signal` from [01-pattern-core.md](01-pattern-core.md) and shares the `hashFloat` core from `rng.ts` ([03-combinators.md](03-combinators.md)) for `rand`/`perlin`. `.range` is a `Pat<number>` method mixed onto the prototype.

## Design

Per the model, signals "output `0..1` by default; `.range(lo, hi)` rescales" ([pattern-model.md](../../pattern-model.md#parameters-can-themselves-be-patterns)). Each is a `signal(t => …)` ([01](01-pattern-core.md)) over cycle-time `t`, where one period = one cycle (so `.slow(8)` makes the period 8 cycles).

```ts
import { signal } from "./pattern";
import { hashFloat } from "./rng";

const frac = (t: number) => t - Math.floor(t);   // 0..1 within the cycle

export const saw    = signal((t: number) => frac(t));                       // 0→1 ramp
export const isaw   = signal((t: number) => 1 - frac(t));                   // 1→0 ramp
export const sine   = signal((t: number) => 0.5 + 0.5 * Math.sin(2 * Math.PI * t));
export const cosine = signal((t: number) => 0.5 + 0.5 * Math.cos(2 * Math.PI * t));
export const tri    = signal((t: number) => { const x = frac(t); return x < 0.5 ? 2 * x : 2 - 2 * x; });
export const square = signal((t: number) => (frac(t) < 0.5 ? 0 : 1));
export const rand   = signal((t: number) => hashFloat(Math.floor(t * 1e6)));   // white noise, deterministic by time
export const perlin = signal((t: number) => smoothNoise(t));                    // smoothed value noise (1D)
```

`smoothNoise` interpolates `hashFloat` at integer lattice points with a smoothstep, so `perlin` is continuous and wanders smoothly, while `rand` is per-sample white noise. Both are **pure functions of time** — same `t` ⇒ same value — so the whole system stays reproducible (a hot-swap re-querying the same arc reads the same modulation).

All eight are `Pat<number>` with `whole: undefined`; querying any of them over a positive-width span returns exactly one continuous hap (midpoint sample; see `signal` in [01](01-pattern-core.md)).

### `.range(lo, hi)` — rescale

```ts
declare module "./pattern" { interface Pat<number> { range(lo: number, hi: number): Pat<number>; } }
Pat.prototype.range = function (this: Pat<number>, lo: number, hi: number): Pat<number> {
  return this.fmap(v => lo + v * (hi - lo));     // fmap from [04]; value-only, preserves continuity
};
```

`range` is `fmap`, so it preserves `whole: undefined`. `sine.range(300, 1200)` maps `0..1` → `300..1200`.

### `.slow` / `.fast` on signals

These are the same combinators from [03-combinators.md](03-combinators.md) — `withTime` scales the *time* the signal is sampled at:

```
sine.slow(8)  ≡  fast(1/8, sine)  ≡  withTime(t => t/8, t => t*8, sine)
```

So sampling `sine.slow(8)` at cycle `t` reads `sine` at `t/8` — one full sine cycle spread over 8 musical cycles. Continuity is preserved because `fast`/`slow` only remap time and a signal has no `whole` to break.

### Worked example — sampling a signal as a parameter

`note("c2*8").lpf(sine.range(300,1200).slow(8))`:

1. The bass pattern has 8 onsets per cycle (`c2*8`). Each onset is at some cycle-time `t_i` (e.g. `0, 1/8, 2/8, …`).
2. The `lpf` control value is the continuous `Pat<number>` `sine.range(300,1200).slow(8)`.
3. At each note onset, the scheduler does a **zero-width query** at `t_i` (see [06](06-scheduler-integration.md)):

```ts
const lpfHz = queryArc(lpfSignal, t_i, t_i)[0].value;
// e.g. at t=0:    sine(0/8)=0.5 → 0.5*(1200-300)+300 = 750 Hz
//      at t=2:    sine(2/8)=sin(π/2 mapped)…  → climbing toward 1200 over 8 cycles
```

Because `sine.slow(8)` has period 8, consecutive cycles read progressively further along the sine, producing the audible 8-cycle sweep.

```
queryArc(sine.range(300,1200), 0, 0)   → [{ whole: undefined, part:{0,0}, value: 750 }]
queryArc(sine.range(300,1200), 0.25,0.25) → value 1200 (sine peak at quarter cycle)
```

### Per-note sampling vs. audio-rate `ParamRamp`

The model shows signals used both for *per-note* values (`.gain("0.6 0.9")`, alternating per note) and for *smooth sweeps* (`.lpf(sine…slow(8))`). The engine and protocol give us two ways to deliver a parameter:

- **Per-note sampled value → packed field or `ParamSet`.** Sample the signal once at the note onset (zero-width query) and send that scalar with the note. Cheap; the value is constant for the note's duration. This is the **default**.
- **Audio-rate modulation → `ParamRamp`.** For a parameter that must glide *within/across* notes smoothly (a slow filter sweep that should not "stair-step" at note boundaries), the scheduler can instead sample the signal at the **start and end of the note's whole** and emit a `ParamRamp` (`0x11`: target value + ramp-frames; [event-protocol.md](../../event-protocol.md#opcodes)) so the engine interpolates linearly between samples on the audio thread.

#### The decision rule

```ts
// In [06], per (control key, value) pair:
function deliverParam(key: string, value: number | Pat<number>, hap: Hap<ControlMap>) {
  if (typeof value === "number") return packedOrSet(key, value);          // static scalar
  // value is a Pat<number> (patterned or continuous)
  const isContinuous = isContinuousPattern(value);                        // whole === undefined on query
  if (isContinuous && PARAM_RAMP_KEYS.has(key) && hap.whole) {
    const t0 = hap.whole.begin, t1 = hap.whole.end;
    const v0 = queryArc(value, t0, t0)[0].value;
    const v1 = queryArc(value, t1, t1)[0].value;
    return emitRamp(key, v0, v1, framesOf(t1 - t0));                      // 0x11 ParamRamp
  }
  const v = queryArc(value, hap.part.begin, hap.part.begin)[0].value;     // zero-width sample at onset
  return packedOrSet(key, v);                                             // static for this note
}
```

- `isContinuousPattern` detects the `whole: undefined` signature (a continuous signal vs. a discrete value-pattern like `"0.6 0.9"`).
- `PARAM_RAMP_KEYS` (e.g. `lpf`, `hpf`, `pan`, `room`, `gain`) is the set the engine supports ramping; others fall back to per-note sampling.
- A **discrete** value-pattern (`.gain("0.6 0.9")`) is always per-note: the scheduler samples it at each onset and gets the alternating values — no ramp needed.

This keeps the common case cheap (one scalar) while letting genuinely continuous controls glide on the audio thread instead of stair-stepping per note. The choice is a scheduler concern; signals themselves are just pure `0..1` functions of time.

## Interfaces

- **[01-pattern-core.md](01-pattern-core.md):** every signal is `signal(t => …)`; zero-width `queryArc` returns `sample(t)` exactly, which the scheduler relies on for onset sampling.
- **[03-combinators.md](03-combinators.md):** `.fast`/`.slow` (and `.range` via `fmap`) work on signals through `withTime`; `rand`/`perlin` share `rng.ts`'s `hashFloat`.
- **[04-sound-note-api.md](04-sound-note-api.md):** signals are stored as control values inside a `ControlMap` (`{ lpf: sine.range(...) }`). The modifier methods accept `number | Pat<number>` precisely so a signal fits.
- **[06-scheduler-integration.md](06-scheduler-integration.md):** owns the per-note-vs-`ParamRamp` decision above, samples signals at onset (or onset+end for ramps), and emits `ParamSet`/`ParamRamp`/`TrackFx` accordingly.

## Edge cases & risks

- **Purity / reproducibility:** `rand`/`perlin` are pure functions of `Math.floor(t·k)` / a lattice — no global state, no `Math.random()`. Hot-swapping and re-querying the same arc reads identical modulation, so a swapped pattern's filter doesn't jump.
- **Fractional precision:** signals are sampled at the *exact* onset time produced by [01]/[02]; `frac(t)` and `2πt` are stable for the small cycle numbers in a lookahead window. For very large cycle counts (`t` in the thousands) `Math.sin(2πt)` keeps adequate precision; `rand`'s `Math.floor(t·1e6)` is the resolution knob (deterministic but quantized) and is documented.
- **Continuous patterns must stay continuous:** `.range`, `.slow`, `.fast` all preserve `whole: undefined`. A test asserts that after `sine.range(300,1200).slow(8)`, a positive-width query still returns a single hap with `whole === undefined` (so it is never mistaken for a triggerable note in [06]).
- **Never triggered, only sampled:** a continuous signal used as a *track's* top-level pattern produces no onsets ⇒ silence; [06] warns. Signals are legal only as parameter values.
- **Ramp endpoint sampling:** emitting a `ParamRamp` requires the hap's `whole` (a note duration); for a hap with `whole: undefined` (shouldn't happen for a note) the scheduler falls back to per-note sampling. Linear ramp approximates a curve over one note; long notes get a closer fit, which is acceptable and matches the protocol's linear `ParamRamp`.
- **Performance:** sampling a signal is one zero-width query (O(1)); even with several patterned params per note across many notes per tick, cost is small and on the control plane.

## Acceptance criteria

- `queryArc(sine, 0, 0)[0].value === 0.5`; `queryArc(sine, 0.25, 0.25)[0].value ≈ 1.0`; all values lie in `0..1`.
- `queryArc(saw, 0.25, 0.25)[0].value ≈ 0.25`; `queryArc(isaw, 0.25, 0.25)[0].value ≈ 0.75`.
- `queryArc(square, 0.1, 0.1)[0].value === 0` and `queryArc(square, 0.6, 0.6)[0].value === 1`.
- Every signal query returns a hap with `whole === undefined` (continuity invariant), including after `.range`/`.slow`/`.fast`.
- `queryArc(sine.range(300, 1200), 0.25, 0.25)[0].value ≈ 1200`.
- `sine.slow(8)` sampled at cycles `0` and `2` gives different values consistent with reading `sine` at `0` and `0.25` (period stretched ×8).
- `rand` and `perlin` are deterministic: two `queryArc` calls at the same `t` return the identical value.
- A scheduler-level test: `.lpf(sine.range(300,1200).slow(8))` over 8 cycles produces a monotone-then-reversing sequence of per-note `lpf` values approximating one sine period (the audible sweep), and the equivalent `PARAM_RAMP_KEYS` path emits `ParamRamp` records with matching endpoints.
