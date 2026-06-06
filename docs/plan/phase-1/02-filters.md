# 02 — State-variable filter (SVF)

## Purpose

The filter is what turns a raw oscillator into a *musical* timbre — and in techno it is the
single most important shaping tool (the filter sweep is the genre's signature gesture). This
feature provides a **state-variable filter (SVF)** with four selectable modes — **low-pass,
high-pass, band-pass, notch** — plus **resonance/Q**, with **cutoff smoothing** so that
sweeping the cutoff (whether by the filter envelope or by a control signal) produces a clean
glide and **no zipper noise**.

## Location

```
engine/src/filter.rs    # SvfMode, SvfConfig, cutoff smoother, build wiring (new)
engine/src/graph.rs     # inserts the SVF into the synth voice chain (extended)
```

Consumed by the synth voice ([04-synth-voice.md](04-synth-voice.md)) and the drum kit
([05-drum-synthesis.md](05-drum-synthesis.md), e.g. the band-pass for snare and the
low/high-pass for hats).

## Design

### Modes & config

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SvfMode { Lowpass, Highpass, Bandpass, Notch }

#[derive(Clone, Copy, Debug)]
pub struct SvfConfig {
    pub mode: SvfMode,
    pub cutoff_hz: f32,   // 20 .. 20000, clamped to Nyquist
    pub q: f32,           // resonance / Q; 0.5 .. ~20
    pub smooth_ms: f32,   // cutoff smoothing time constant; default 5 ms
}
```

### Using fundsp's filter components

fundsp ships state-variable filters as typed nodes. Each takes **audio in + cutoff +
Q as inputs**, which is exactly what we want for modulation: we feed cutoff from a smoothed
control signal rather than baking it in.

| Mode | fundsp node |
| --- | --- |
| Lowpass | `lowpass()` |
| Highpass | `highpass()` |
| Bandpass | `bandpass()` |
| Notch | `notch()` |

These are the SVF family (3-input form: `audio | cutoff | q`). Signature shape:

```rust
// audio | cutoff_hz | q  ->  filtered audio
let svf = (pass() | smoothed_cutoff | constant(q)) >> lowpass();
```

`pass()` carries the dry audio through to the filter's audio input; `smoothed_cutoff` is the
de-zippered cutoff signal (below); `constant(q)` is resonance.

### Cutoff smoothing (no zipper noise)

If cutoff jumps in discrete steps — which it does whenever a `ParamSet` updates it, or a
control pattern steps between values — the filter coefficient jumps with it and you hear
**zipper noise** (a gritty stepping). The fix is to run the *target* cutoff through a
one-pole smoother before it reaches the filter's cutoff input:

```rust
/// One-pole exponential smoother. `smooth_ms` sets the glide time.
/// `follow(t)` in fundsp builds exactly this: a leaky follower with time constant t seconds.
fn smoothed_cutoff(target: An<impl AudioNode>, smooth_ms: f32) -> An<impl AudioNode> {
    target >> follow(smooth_ms * 0.001)
}
```

`follow(t)` is fundsp's smoothing/lag node. The same smoother absorbs the discrete steps of
a stepped control pattern (e.g. `.lpf("400 800 1600")`) and the per-buffer updates of a
continuous signal, so both feel like a glide. Default `smooth_ms = 5` is fast enough to feel
responsive, slow enough to kill zipper noise; instruments can raise it for slow pad sweeps.

When cutoff is driven by the **filter envelope** ([03-envelopes.md](03-envelopes.md)) the
envelope itself is already smooth, so we can sum the envelope into the (post-smoother) cutoff
signal:

```rust
// base_cutoff (smoothed) + env_amount * filter_env  -> filter cutoff input
let cutoff_sig = smoothed_cutoff(constant(base_hz), smooth_ms)
               + filter_env * constant(env_amount_hz);
```

### Full SVF wiring sketch

```rust
use fundsp::hacker::*;

/// Build the SVF stage. `cutoff_sig` is the (already summed/smoothed) cutoff control.
pub fn build_svf(cfg: SvfConfig, cutoff_sig: An<impl AudioNode>) -> Box<dyn AudioUnit> {
    let q = cfg.q.clamp(0.5, 20.0);
    match cfg.mode {
        SvfMode::Lowpass  => Box::new((pass() | cutoff_sig | constant(q)) >> lowpass()),
        SvfMode::Highpass => Box::new((pass() | cutoff_sig | constant(q)) >> highpass()),
        SvfMode::Bandpass => Box::new((pass() | cutoff_sig | constant(q)) >> bandpass()),
        SvfMode::Notch    => Box::new((pass() | cutoff_sig | constant(q)) >> notch()),
    }
}
```

Within the voice this sits **between the oscillator and the amp**:
`osc (01) >> svf (this) >> amp (04)` — see the full chain in
[04-synth-voice.md](04-synth-voice.md).

### Self-oscillation

At high `q` (roughly `q >= 10`) the SVF approaches **self-oscillation**: it rings at the
cutoff frequency even with little input, effectively becoming a sine oscillator. This is a
desirable musical effect (acid-style resonant whistles), but it can also produce sudden loud
output. We:

- allow `q` up to ~20 so self-oscillation is reachable;
- **clamp** above that to avoid blow-up;
- rely on the master limiter ([07-master-bus-limiter.md](07-master-bus-limiter.md)) as the
  final safety net against a resonant peak clipping the output.

### Parameter list & ranges

| Param | Range | Default | Live-settable | Notes |
| --- | --- | --- | --- | --- |
| `mode` | enum | Lowpass | per-instrument | LP/HP/BP/notch |
| `cutoff_hz` | 20 .. ~Nyquist | 1000 | yes (`ParamSet`/signal) | clamped < SR/2 |
| `q` | 0.5 .. 20 | 0.7 | yes | ≥10 → self-oscillation |
| `smooth_ms` | 0 .. 200 | 5 | per-instrument | zipper-noise guard |
| `env_amount_hz` | −20000 .. 20000 | 0 | per-instrument | filter-envelope depth (see 03) |

## Interfaces

- **Event protocol** ([event-protocol.md](../../event-protocol.md#opcodes)):
  - `NoteOn` (`0x01`) does **not** carry cutoff/Q directly (its args are
    freq/vel/dur/instrument). The instrument's base cutoff/Q/mode come from the
    `InstrumentConfig` (`defineInstrument()` control call).
  - `ParamSet` (`0x10`) updates `cutoff_hz` or `q` on a live track/voice (param id encodes
    which). This is how `.lpf(800)` as a per-track control and `.lpf(sine.range(...))` as a
    modulating signal reach the filter — the scheduler emits `ParamSet` events that the
    smoother turns into a glide.
- **Song API** ([04-synth-voice.md](04-synth-voice.md)): `.lpf(hz)` / `.hpf(hz)` /
  `.bpf(hz)` / `.notch(hz)` select mode + base cutoff; `.resonance(q)` (alias `.res()`) sets
  Q; `.fenv(amount, attack, decay, sustain, release)` sets the filter envelope depth/shape.
- **Voice allocation:** the filter is part of the voice's persistent fundsp graph; the
  allocator's `reset()` on retrigger clears filter state so a stolen voice doesn't carry
  resonant ringing into the next note.

## Edge cases & risks

- **Zipper noise:** the entire reason for `follow()` on cutoff. Acceptance includes a test
  that a stepped cutoff change produces no broadband step transient.
- **Denormals:** an SVF fed silence with high resonance can settle into denormal numbers
  that cost huge CPU on some hardware. Guard by enabling **flush-denormals-to-zero** at the
  top of the audio callback (set the FPU FTZ/DAZ flags once when the cpal stream starts) and
  optionally adding a tiny DC/noise floor (`+ dc(1e-20)`) — pick the FTZ approach as primary
  since it's free and global.
- **Self-oscillation blow-up:** `q` clamp + master limiter; never allow `q` past the clamp.
- **Real-time safety:** the SVF is a fixed fundsp node; no allocation. `smooth_ms` change is
  a coefficient update, not a reallocation.
- **Cutoff at Nyquist:** clamp `cutoff_hz < SR/2 - margin` so a high LP cutoff or a sweeping
  signal can't push the coefficient unstable.
- **CPU:** an SVF is ~a handful of multiply-adds per sample; negligible next to band-limited
  oscillators. The smoother is one extra one-pole.

## Acceptance criteria

- [ ] Each mode has the correct magnitude response: LP attenuates above cutoff, HP below, BP
      peaks at cutoff, notch nulls at cutoff. **Test:** sweep a sine / feed white noise, FFT,
      assert the response shape and −3 dB point near `cutoff_hz`.
- [ ] Raising `q` narrows the BP / sharpens the LP resonant peak measurably. **Test:** white
      noise in, compare peak height at cutoff across `q` = 0.7 / 4 / 12.
- [ ] At `q ≈ 15` with cutoff at 200 Hz, the filter self-oscillates into an audible ~200 Hz
      tone with minimal input. **Test:** tiny impulse in, FFT shows a sustained peak at
      cutoff.
- [ ] A stepped cutoff change (e.g. 400→1600 Hz in one buffer) produces **no zipper/step
      transient** above −60 dB; with smoothing disabled it does. **Test:** A/B render,
      measure step transient energy.
- [ ] `.lpf(sine.range(300,1200).slow(8))`-style modulation audibly and smoothly sweeps with
      no stepping. **Test:** drive cutoff from a stepped signal, assert a continuous glide.
- [ ] No denormal CPU spike when the filter is fed silence at high resonance. **Test:** run
      silence-in for several seconds, assert CPU stays flat (FTZ enabled).
