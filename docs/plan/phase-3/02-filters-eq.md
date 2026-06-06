# 02 — Filters & EQ (track effects)

## Purpose

Two effects that shape **the whole track's signal** rather than a single voice:

1. A **performance filter** — a sweepable resonant **low-pass / high-pass / band-pass** that sits in
   a track's effect chain and is built to be *played* with a control signal (the genre-defining
   filter sweep across a whole loop). It is **distinct from the per-voice SVF**
   ([phase-1/02-filters.md](../phase-1/02-filters.md)): that one carves each note's timbre inside the
   voice; *this* one filters the summed output of a track (and is the natural target of
   `.lpf(sine.range(...).slow(8))` when you want the *mix*, not each note, to sweep).
2. A **multiband EQ** — low shelf + high shelf + one or more **peaking (bell) bands** — for
   corrective and tonal shaping of a track or send return.

Both are `fundsp`-based and implement the [`Effect` trait](01-effects-framework.md#the-effect-trait),
with **smoothed parameters** so sweeps and live tweaks are click-free.

## Location

```
engine/src/effects/filter_eq.rs   # PerfFilter + Eq (new)
engine/src/graph.rs               # builds both fundsp graphs from EffectConfig (extended)
```

Registered as `EffectKind::Filter` and `EffectKind::Eq`
([01-effects-framework.md](01-effects-framework.md#effect-kinds)).

## Design

### Performance filter

Same SVF family as the per-voice filter (fundsp `lowpass()`/`highpass()`/`bandpass()`, the 3-input
`audio | cutoff | q` form documented in
[phase-1/02-filters.md](../phase-1/02-filters.md#using-fundsps-filter-components)) — reused so the
sound and the cutoff-smoothing behavior are identical and familiar. The difference is **where it
runs** (track insert, stereo) and that it is **designed to be swept** by automation.

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PerfMode { Lowpass, Highpass, Bandpass }

pub struct PerfFilter {
    mode: PerfMode,
    cutoff: Smoothed<f32>,   // 20 .. ~Nyquist, smoothed (one-pole, ~5 ms) — kills zipper noise
    q:      Smoothed<f32>,   // 0.5 .. ~20
    // pre-allocated stereo fundsp SVF graph (cutoff + q fed as control inputs)
    graph_l: Box<dyn AudioUnit>,
    graph_r: Box<dyn AudioUnit>,
}
```

fundsp graph sketch (per channel; cutoff/q are *inputs* so the smoothed control drives the
coefficient continuously):

```rust
// audio | cutoff_hz | q  ->  filtered audio   (one channel)
let lp = (pass() | var(&cutoff_var) | var(&q_var)) >> lowpass();
// var(&x) reads a shared atomic-ish cell updated from the Smoothed each block (no realloc).
```

`process` per block: advance the smoothers, write their current values into the `cutoff_var`/`q_var`
cells, tick the graph over `l` and `r`. Because cutoff is a *smoothed input signal* (not a coefficient
recomputed on a step), a stepped automation value (`.lpf("400 1600")`) or a ramp
(`.lpf(sine…slow(8))`) both arrive as a clean glide — exactly the mechanism from
[phase-1/02-filters.md](../phase-1/02-filters.md#cutoff-smoothing-no-zipper-noise).

**Stereo link:** L and R share the same `cutoff_var`/`q_var` so the filter doesn't tilt the stereo
image; only the audio paths are independent.

**Self-oscillation** at high `q` (≳10) is allowed and musical (acid whistle), clamped at ~20 and
caught by the [master limiter](../phase-1/07-master-bus-limiter.md) — identical policy to the
per-voice SVF.

#### Performance-filter params

| Param | `fx_param` | Range | Unit | Default | Notes |
| --- | --- | --- | --- | --- | --- |
| `mode` | `0x00`+ | LP/HP/BP | enum | LP | set at build (per-instrument) |
| `cutoff` | `0x01` | 20 .. ~Nyquist | Hz | 1000 | **shared id with per-voice SVF**; smoothed |
| `q` / resonance | `0x02` | 0.5 .. 20 | Q | 0.7 | ≥10 → self-oscillation |
| `smooth_ms` | `0x03` | 0 .. 200 | ms | 5 | zipper-noise guard |
| `mix` | `0x00` (chain) | 0 .. 1 | — | 1.0 | wet/dry (from the chain, [01](01-effects-framework.md)) |

> Keeping `cutoff=0x01`, `q=0x02` identical to the per-voice ids means the **same** TS automation
> path drives either; the registry decides whether `.lpf()` packs a per-voice `ParamSet` or a track
> `TrackFx` slot based on whether it's used as a voice modifier or a chain insert
> ([04-sound-note-api.md](../phase-2/04-sound-note-api.md)).

### Multiband EQ

A serial stack of shelving and peaking biquads. fundsp provides the building blocks; we chain them:

| Band | fundsp node | Params |
| --- | --- | --- |
| Low shelf | `lowshelf()` | freq, gain(dB), Q/slope |
| Peaking ×N | `peak()` (or `bell()`) | freq, gain(dB), Q |
| High shelf | `highshelf()` | freq, gain(dB), Q/slope |

```rust
pub struct EqBand { freq: Smoothed<f32>, gain_db: Smoothed<f32>, q: Smoothed<f32>, kind: BandKind }

pub struct Eq {
    bands: [EqBand; MAX_EQ_BANDS],   // e.g. low shelf, 3 peaks, high shelf  (fixed, pre-allocated)
    n: usize,
    graph_l: Box<dyn AudioUnit>,
    graph_r: Box<dyn AudioUnit>,
}
```

fundsp graph sketch (per channel — serial biquads, each fed its own smoothed control inputs):

```rust
// in -> low-shelf -> peak1 -> peak2 -> peak3 -> high-shelf -> out
let eq = pass()
       >> (pass() | var(&ls_f) | var(&ls_q) | var(&ls_g)) >> lowshelf()
       >> (pass() | var(&p1_f) | var(&p1_q) | var(&p1_g)) >> peak()
       >> (pass() | var(&p2_f) | var(&p2_q) | var(&p2_g)) >> peak()
       >> (pass() | var(&p3_f) | var(&p3_q) | var(&p3_g)) >> peak()
       >> (pass() | var(&hs_f) | var(&hs_q) | var(&hs_g)) >> highshelf();
```

(Exact fundsp shelf/peak input arity is matched to the crate; the point is each band's freq/gain/Q
are **smoothed control inputs**, so any band parameter is automatable and click-free. A band with
`gain_db = 0` is transparent — `peak()` at 0 dB is unity — so unused bands cost a no-op biquad.)

#### EQ params (per band *b*)

| Param | `fx_param` | Range | Unit | Default | Notes |
| --- | --- | --- | --- | --- | --- |
| band `b` freq | `0x10 + b*4` | 20 .. 20000 | Hz | per band | log-mapped from the TS side |
| band `b` gain | `0x11 + b*4` | −24 .. +24 | dB | 0 | 0 = transparent |
| band `b` Q | `0x12 + b*4` | 0.3 .. 18 | Q | 0.7 | shelf slope for shelves |
| `mix` | `0x00` | 0 .. 1 | — | 1.0 | wet/dry (chain) |

## Interfaces

- **Event protocol** ([event-protocol.md](../../event-protocol.md)):
  - `TrackFx (0x30)` with `param` = `pack_fx(slot, fx_param)`
    ([01](01-effects-framework.md#trackfx-param-id-encoding)) sets any filter/EQ parameter on its
    slot.
  - `ParamRamp (0x11)` with the same packed id drives a smooth cutoff/gain sweep from a signal
    ([07-parameter-automation.md](07-parameter-automation.md)).
- **TS chain API:**
  - **Performance filter:** `.lpf(hz)` / `.hpf(hz)` / `.bpf(hz)` and `.resonance(q)` (alias
    `.res()`). When applied at *track* level (as a mix effect) these append a `Filter` slot; when
    used as a *voice* modifier they reach the per-voice SVF — the registry distinguishes by context.
    Example: `p3(note("…").saw().lpf( sine.range(300,1200).slow(8) ))` sweeps the **track** filter.
  - **EQ:** `.eq({ low: -3, mid: [1000, +2, 1.2], high: +1.5 })` or per-band
    `.eqPeak(freq, gainDb, q)`, `.lowShelf(freq, gainDb)`, `.highShelf(freq, gainDb)`. Each maps to a
    band's `TrackFx` params on the EQ slot.
- **Per-track chain vs send:** both are **chain inserts** (serial), never sends — they shape a
  track's own signal in place. (A send return can itself carry an EQ slot, e.g. to dull a reverb.)
- **Relation to per-voice SVF** ([phase-1/02-filters.md](../phase-1/02-filters.md)): same fundsp
  nodes and smoothing; different scope (track vs voice) and stereo. No code duplication beyond the
  thin `Effect` wrapper.

## Edge cases & risks

- **Zipper noise:** every cutoff/freq/gain/Q is a `Smoothed<f32>` feeding a fundsp *input* (not a
  recomputed coefficient on a step); default `smooth_ms = 5`. A stepped automation value glides.
- **Denormals:** an idle resonant filter fed silence can drift into denormals; covered by the global
  FTZ flag set at stream start ([phase-1/02-filters.md](../phase-1/02-filters.md#edge-cases--risks)).
- **Self-oscillation blow-up:** `q` clamped ≤ ~20; master limiter is the safety net.
- **Cutoff at Nyquist:** clamp `cutoff < SR/2 − margin` so a sweeping signal can't push the SVF
  unstable.
- **EQ gain stacking:** several boosted bands can raise level a lot; bands default to 0 dB and the
  limiter protects the master, but the EQ also exposes a small output trim if needed.
- **Real-time safety:** fixed fundsp graphs, no allocation in `process`; band count is fixed at
  build, parameter changes are smoother target updates only.
- **CPU:** a handful of biquads per channel — negligible next to the voice stack.

## Acceptance criteria

- [ ] The performance LP/HP/BP each have the correct magnitude response on a **track sum** (not per
      voice): LP attenuates above cutoff, HP below, BP peaks at cutoff. **Test:** white-noise track in,
      FFT, assert response shape and −3 dB point near `cutoff`.
- [ ] `.lpf(sine.range(300,1200).slow(8))` on a track audibly and **smoothly** sweeps the whole
      track's filter over 8 cycles with no stepping. **Test:** drive cutoff from a stepped signal,
      assert a continuous glide (no step transient above −60 dB).
- [ ] At `q ≈ 15`, cutoff 200 Hz, the performance filter self-oscillates audibly without blowing up
      past the limiter ceiling. **Test:** tiny input, FFT shows a sustained ~200 Hz peak; output ≤
      ceiling.
- [ ] EQ: a low shelf at −6 dB / a +4 dB peak at 1 kHz / a +2 dB high shelf produce the matching
      composite curve. **Test:** sweep/FFT, assert gains at band centers within ±0.5 dB.
- [ ] An EQ with all bands at 0 dB is **transparent** (null against dry within −80 dB). **Test:** null
      test.
- [ ] Automating an EQ band's gain via a signal is click-free. **Test:** ramp a peak gain, assert no
      step transient.
- [ ] No denormal CPU spike when either effect is fed silence at high resonance. **Test:** silence-in
      for several seconds, CPU flat (FTZ enabled).
