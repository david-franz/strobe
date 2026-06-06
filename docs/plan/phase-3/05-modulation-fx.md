# 05 — Modulation FX (chorus / flanger / phaser)

## Purpose

The **movement / width** family: three effects that all work by **modulating a short delay or
all-pass line with an LFO**, differing only in delay range, feedback, and whether they sweep a delay
or a phase notch. They add stereo width, shimmer, and the swirling “jet plane” sweeps that texture
techno pads, hip-hop keys, and synth lines.

1. **Chorus** — several LFO-modulated short delays (~10–30 ms) mixed with the dry → thickening,
   ensemble width.
2. **Flanger** — a single very short LFO-modulated delay (~0.1–10 ms) **with feedback** → the deep
   comb-filter "whoosh".
3. **Phaser** — a cascade of LFO-modulated **all-pass** stages → sweeping phase notches, no pitch
   wobble.

All three share **one LFO design** and implement the
[`Effect` trait](01-effects-framework.md#the-effect-trait); they are **chain inserts**.

## Location

```
engine/src/effects/modulation.rs   # Chorus, Flanger, Phaser + shared Lfo (new)
engine/src/graph.rs                # build the modulated delay / all-pass graphs (extended)
```

`EffectKind::Chorus` / `EffectKind::Flanger` / `EffectKind::Phaser`
([01-effects-framework.md](01-effects-framework.md#effect-kinds)).

## Design

### Shared LFO

One small LFO type drives all three. It produces a `0..1` (or bipolar) value per sample, with a
**rate** that is free (Hz) **or tempo-synced** to `cps`, a **depth**, a **waveform**, and a **stereo
phase offset** so L and R sweep out of phase (width).

```rust
#[derive(Clone, Copy)]
pub enum LfoWave { Sine, Triangle, Saw }

pub struct Lfo {
    rate_hz: Smoothed<f32>,  // 0.01 .. 20 Hz, or derived from cps when synced
    phase:   f32,            // 0..1, advances by rate/SR each sample
    wave:    LfoWave,
    stereo_offset: f32,      // R phase = phase + offset (0..0.5) → width
    synced:  bool,           // if true, rate_hz recomputed from cps (see below)
}

impl Lfo {
    #[inline] fn next_pair(&mut self, sr: f32) -> (f32, f32) {     // (L, R) in -1..1
        let l = Self::shape(self.wave, self.phase);
        let r = Self::shape(self.wave, frac(self.phase + self.stereo_offset));
        self.phase = frac(self.phase + self.rate_hz.next() / sr);
        (l, r)
    }
}
```

**Tempo sync:** a sync rate like `"1/4"` means one LFO cycle per that note division; with one cycle =
`1/cps` seconds, `rate_hz = cps * divisions_per_cycle`. The LFO reads `cps` from
[`FxCtx`](01-effects-framework.md#the-effect-trait); on a `Tempo (0x40)` change the synced rate
updates and the smoother glides — no click. This is the same sync convention as the
[tempo-synced delay](03-delay-reverb.md#tempo-sync).

### Chorus (graph sketch)

Dry plus N (default 2–3) **modulated short delay** voices, each at a slightly different base delay and
LFO phase, summed with stereo spread. Delay lines are pre-allocated; read taps are **fractional**
(interpolated) so the LFO sweep glides without zipper steps.

```
            ┌─ delay(base1 + depth·lfo_a) ─┐
in (L/R) ───┼─ delay(base2 + depth·lfo_b) ─┼─► mix with dry ─► out (L/R, spread)
            └─ delay(base3 + depth·lfo_c) ─┘
base ≈ 10..30 ms,  depth ≈ 1..8 ms,  rate ≈ 0.1..2 Hz,  no/low feedback
```

```rust
// per voice, per channel: tap = base + depth * lfo;  y = read_interp(buf, write, tap*sr)
// fundsp-flavored: (pass() | (lfo_sig*depth + base)) >> tap_linear(max_delay) ;  sum voices & dry.
```

### Flanger (graph sketch)

A **single** very short modulated delay **with feedback** — the feedback deepens the comb notches
into the dramatic flange sweep.

```
in ─►(+)─► delay(base + depth·lfo,  base≈0.1..10 ms) ─┬─► mix with dry ─► out
      ▲                                               │
      └──────────────── feedback (−0.95..0.95) ───────┘
rate ≈ 0.05..2 Hz ;  feedback sign flips the notch pattern (through-zero-ish character)
```

```rust
pub struct Flanger {
    lfo: Lfo,
    base: Smoothed<f32>,     // ms
    depth: Smoothed<f32>,    // ms
    feedback: Smoothed<f32>, // -0.95 .. 0.95 (clamped |fb| < 1)
    buf_l: Vec<f32>, buf_r: Vec<f32>, write: usize,   // pre-allocated to max delay
}
```

### Phaser (graph sketch)

A cascade of **modulated all-pass** stages (default 4–8); the LFO sweeps each all-pass corner
frequency, moving the phase notches up and down. Optional feedback sharpens the notches. No delay
line → no pitch wobble (the chorus/flanger character), just spectral sweeping.

```
in ─►(+)─► allpass(f·lfo) >> allpass(f·lfo) >> … (N stages) ─┬─► mix with dry ─► out
      ▲                                                      │
      └──────────────────── feedback ────────────────────────┘
stages 2..12 ;  rate ≈ 0.05..2 Hz ;  center freq + range set the sweep band
```

```rust
// fundsp: each stage is a 1st-order allpass whose coefficient comes from the LFO-swept freq.
// chain N of them with >> ; the swept freq = center * 2^(range * lfo).
```

### Why one LFO type for all three

The only differences are: delay range (chorus long, flanger tiny, phaser none/all-pass), feedback
(chorus none, flanger/phaser yes), and modulation target (delay tap vs all-pass corner). Sharing the
`Lfo` keeps rate/depth/sync/stereo-spread behavior **identical and reusable**, and makes
tempo-synced, stereo-wide modulation a single well-tested code path.

### Parameter lists

Common (all three):

| Param | `fx_param` | Range | Unit | Default | Notes |
| --- | --- | --- | --- | --- | --- |
| `rate` | `0x01` | 0.01 .. 20 | Hz **or** sync | 0.5 | smoothed; `synced` flag, glides on `Tempo` |
| `depth` | `0x02` | 0 .. 1 | — | 0.5 | scales the modulation amount |
| `stereo` | `0x05` | 0 .. 0.5 | phase | 0.25 | L/R LFO offset → width |
| `wave` | build | sine/tri/saw | enum | sine | |
| `mix` | `0x00` | 0 .. 1 | — | 0.5 | wet/dry (chain) |

Effect-specific:

| Effect | Extra params | `fx_param` | Range | Default |
| --- | --- | --- | --- | --- |
| Chorus | `voices` | build | 1 .. 4 | 3 |
| Flanger | `feedback` | `0x03` | −0.95 .. 0.95 | 0.5 |
| Phaser | `stages` | build | 2 .. 12 | 6 |
| Phaser | `feedback` | `0x03` | −0.9 .. 0.9 | 0.3 |
| Phaser | `center`/`range` | `0x06`/`0x07` | 100..5000 Hz / 0..4 oct | 800 / 2 |

## Interfaces

- **Event protocol** ([event-protocol.md](../../event-protocol.md)):
  - `TrackFx (0x30)` with `param = pack_fx(slot, fx_param)` sets rate/depth/feedback/mix on the slot.
  - `Tempo (0x40)` updates `cps`; a synced `rate` recomputes and glides
    ([03](03-delay-reverb.md#tempo-sync)).
  - `ParamRamp (0x11)` automates depth/rate from a signal ([07](07-parameter-automation.md)).
- **TS chain API:**
  - `.chorus(depth?)` / `.chorus({ rate, depth, stereo })`
  - `.flanger({ rate, depth, feedback })`
  - `.phaser({ rate, depth, feedback, stages })`
  - `rate` accepts a number (Hz) or a sync string (`"1/4"`, `"1/8"`). Example:
    `p4(note("c3 eb3 g3").saw().phaser({ rate: "1/8", depth: 0.7 }))`.
- **Per-track chain vs send:** **inserts** (they need the dry signal in series to comb/notch it);
  not sends.

## Edge cases & risks

- **Feedback stability (flanger/phaser):** feedback **clamped `|fb| < 1`**; the modulation keeps the
  comb moving so energy doesn't park at one resonant frequency; master limiter is the net.
- **Zipper-free sweeps:** delay taps are **fractional/interpolated** and all params (incl. the LFO
  rate and all-pass corner) are smoothed, so even a stepped automation of depth/rate glides.
- **Stereo correlation:** the L/R LFO offset widens the image; at extreme settings it can hurt mono
  compatibility — a `stereo=0` mono-safe setting is available and documented.
- **Pitch wobble:** chorus/flanger modulate delay time → intentional micro-pitch wobble; phaser uses
  all-pass → no pitch change (chosen when wobble is unwanted).
- **Denormals:** feedback paths decaying to silence → FTZ flag covers it
  ([phase-1/02](../phase-1/02-filters.md)).
- **Real-time safety:** delay buffers and all-pass state pre-allocated off-thread; `process` does no
  allocation; voice/stage counts fixed at build.
- **CPU:** modest — a few short delay reads or N all-pass stages per sample.

## Acceptance criteria

- [ ] Chorus thickens a mono source into a wider, shimmering signal with no audible pitch jump.
      **Test:** assert added stereo width (decorrelation) and a comb pattern that moves with the LFO.
- [ ] Flanger produces a moving comb-filter sweep; raising `feedback` deepens the notches. **Test:**
      white noise in, spectrogram shows sweeping notches; notch depth increases with feedback.
- [ ] Phaser produces sweeping phase notches **without** pitch modulation. **Test:** spectrogram
      shows N moving notches; a pitch-tracking test shows no delay-style wobble.
- [ ] Tempo-synced `rate "1/4"` makes the LFO complete one cycle per quarter note and re-syncs on a
      `Tempo` change, gliding. **Test:** measure LFO period vs `cps`; assert smooth transition.
- [ ] Stereo offset widens the image; `stereo=0` is mono-safe (no comb cancellation when summed).
      **Test:** mono-sum correlation at `stereo=0` vs `>0`.
- [ ] Sweeping `depth`/`rate` via a signal is click-free. **Test:** ramp the param, assert no step
      transient above −60 dB.
- [ ] Flanger/phaser feedback at max **does not** self-oscillate uncontrollably. **Test:** sustained
      then silent input; assert decay and output ≤ ceiling.
- [ ] No underruns with all three on a track under the example load. **Test:** `stats()` clean.
