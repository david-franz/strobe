# 03 — ADSR envelopes (amp & filter)

## Purpose

An envelope is what gives a note a *shape over time* — the difference between a click and a
pad. This feature provides **ADSR (attack / decay / sustain / release) envelopes** used in
two roles inside a voice: the **amp envelope** (multiplies the audio to fade it in and out)
and the **filter envelope** (modulates the SVF cutoff for that classic "filter opens on the
attack" motion). It supports two trigger styles: **gated** (held from `NoteOn` to `NoteOff`,
for sustained synth notes) and **one-shot** (a fixed length from `NoteOn.arg2`, for drums and
plucks that need no `NoteOff`). All transitions are **click-free**.

## Location

```
engine/src/envelope.rs   # AdsrConfig, the Adsr AudioUnit, gate handling (new)
engine/src/graph.rs      # wires amp env into amp, filter env into SVF cutoff (extended)
```

Used by the synth voice ([04-synth-voice.md](04-synth-voice.md)) for both amp and filter,
and by the drum kit ([05-drum-synthesis.md](05-drum-synthesis.md)) for the fast amp/pitch
envelopes that define percussive hits.

## Design

### Why a custom `AudioUnit`, not just `fundsp`'s `adsr_live`

fundsp provides an `adsr_live(a, d, s, r)` envelope driven by a control input (the gate).
That covers the **gated** case directly. But Phase 1 needs three things fundsp's stock
envelope doesn't give in one piece:

1. a **one-shot** mode that auto-releases after `NoteOn.arg2` frames (no `NoteOff`),
2. a **per-segment curve** choice (exponential vs linear),
3. a clean **retrigger** that re-attacks from the *current* level (not a hard jump to 0).

So we wrap fundsp's gate-driven envelope where it fits and implement a small custom
`AudioUnit` for the one-shot/curve/retrigger logic. The custom unit is allocation-free and
holds only a few floats of state.

### Config

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EnvMode {
    Gated,     // held from NoteOn until NoteOff (sustained synth notes)
    OneShot,   // fixed length = NoteOn.arg2 frames; ignores NoteOff (drums/plucks)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SegCurve { Linear, Exponential }

#[derive(Clone, Copy, Debug)]
pub struct AdsrConfig {
    pub attack_s:  f32,   // 0.0005 .. 10   (min ~0.5 ms to stay click-free)
    pub decay_s:   f32,   // 0.0    .. 10
    pub sustain:   f32,   // 0.0    .. 1.0  (level, not time)
    pub release_s: f32,   // 0.0005 .. 20
    pub mode:      EnvMode,
    pub attack_curve:  SegCurve,   // default Linear (clean click-free ramp from 0)
    pub decay_curve:   SegCurve,   // default Exponential (natural decay)
    pub release_curve: SegCurve,   // default Exponential
}
```

### Exponential vs linear segments

- **Attack: linear** by default. A linear ramp from exactly 0 to the peak is the most
  reliably click-free attack (it starts at true zero; an exponential "attack" that starts
  near zero can still click if it starts above the noise floor). Exponential attack is
  available for snappy percussive curves.
- **Decay & release: exponential** by default — this matches how natural sounds and analog
  envelopes decay (fast at first, then trailing off), and an exponential release reaching the
  noise floor is perceptually click-free because the level is tiny when it finally hits zero.

Exponential segments are computed as a per-sample coefficient toward a target (a one-pole),
which is cheap and never allocates:

```rust
// one-pole step toward `target` with time constant derived from segment seconds
level += coeff * (target - level);   // coeff = 1 - exp(-1 / (seconds * sample_rate))
```

### Click-free attack / release

- Attack always starts the multiplied audio from **level 0** (linear ramp), so there is no
  step at note start regardless of oscillator phase (this is why the oscillator's
  `phase_reset` in [01-oscillators.md](01-oscillators.md) can be true without clicking).
- Release ramps to 0 with an exponential tail; the voice is only marked **free** for the
  allocator ([06-voice-allocation.md](06-voice-allocation.md)) once the amp env's level is
  below a tiny epsilon (e.g. −80 dB), so a voice is never cut mid-tail.
- Minimum attack/release of ~0.5 ms is enforced (clamp) so a user asking for `0` still gets a
  sub-millisecond ramp instead of a hard edge.

### Retrigger

When a physical voice is retriggered (same voice gets a new `NoteOn`, e.g. a fast hi-hat
roll or a stolen voice):

- the amp env **re-attacks from its current level** rather than snapping to 0, so a fast roll
  doesn't click between hits;
- if `phase_reset` is set on the oscillator, the osc resets while the env's smooth re-attack
  hides any discontinuity.

### Gate from `NoteOn` / `NoteOff`; one-shot from `NoteOn.arg2`

This is the core link to the [event protocol](../../event-protocol.md#voice-ids):

- **Gated** (`EnvMode::Gated`): `NoteOn` (`0x01`) opens the gate → A→D→S; the env holds at
  `sustain` until the matching `NoteOff` (`0x02`) for that voice id closes the gate → R.
- **One-shot** (`EnvMode::OneShot`): `NoteOn` carries a **duration in frames in `arg2`**
  (per the protocol: "One-shots (drums, samples) pass a `duration` in `arg2` and need no
  `NoteOff`"). The env runs A→D→S and then **auto-fires its release at `frame_start +
  arg2`**, so the whole hit is self-timed. No `NoteOff` is ever sent for one-shots.

```rust
/// Custom click-free, retriggerable ADSR with gated + one-shot modes.
pub struct Adsr {
    cfg: AdsrConfig,
    sr: f32,
    level: f32,            // current output 0..1
    stage: Stage,          // Idle | Attack | Decay | Sustain | Release
    samples_since_on: u64, // for one-shot auto-release timing
    one_shot_len: u64,     // = NoteOn.arg2 frames; 0 for gated
}

enum Stage { Idle, Attack, Decay, Sustain, Release }

impl Adsr {
    /// Called by the voice on NoteOn. `dur_frames` is arg2 (0 ⇒ gated).
    pub fn trigger(&mut self, dur_frames: u64) {
        self.stage = Stage::Attack;
        self.samples_since_on = 0;
        self.one_shot_len = if self.cfg.mode == EnvMode::OneShot { dur_frames } else { 0 };
        // NOTE: level is NOT zeroed → click-free retrigger (re-attack from current level).
    }

    /// Called by the voice on NoteOff (gated only).
    pub fn gate_off(&mut self) {
        if self.cfg.mode == EnvMode::Gated { self.stage = Stage::Release; }
    }

    /// One sample of envelope. Advances stage machine; handles one-shot auto-release.
    pub fn tick(&mut self) -> f32 {
        if self.cfg.mode == EnvMode::OneShot
            && self.one_shot_len != 0
            && self.samples_since_on >= self.one_shot_len {
            self.stage = Stage::Release;
        }
        self.samples_since_on += 1;
        self.advance()   // moves level toward the stage target (lin/exp per SegCurve)
    }

    /// True once a released env has decayed below epsilon → voice can be freed.
    pub fn is_finished(&self) -> bool {
        matches!(self.stage, Stage::Release) && self.level < 1.0e-4
    }
}
```

To use it inside a fundsp graph we expose it as an `AudioUnit` (0 inputs, 1 output — a
control signal) so it composes with `*` (amplitude) and `+` (filter-cutoff sum):

```rust
// amp:    voice_audio * amp_env
// filter: base_cutoff + env_amount * filter_env   (see 02-filters.md)
let amp_env    = An(EnvNode::new(amp_cfg));     // wraps Adsr as a 0-in/1-out AudioUnit
let filter_env = An(EnvNode::new(filt_cfg));
```

### Parameter list & ranges

| Param | Range | Default (amp) | Default (filter) | Live-settable |
| --- | --- | --- | --- | --- |
| `attack_s` | 0.0005 .. 10 | 0.005 | 0.005 | yes (`ParamSet`) |
| `decay_s` | 0 .. 10 | 0.1 | 0.1 | yes |
| `sustain` | 0 .. 1 | 0.7 | 0.0 | yes |
| `release_s` | 0.0005 .. 20 | 0.2 | 0.2 | yes |
| `mode` | enum | Gated | Gated | per-instrument |
| `*_curve` | enum | A:lin D/R:exp | same | per-instrument |
| filter `env_amount_hz` | −20000 .. 20000 | — | 0 | per-instrument (see 02) |

## Interfaces

- **Event protocol** ([event-protocol.md](../../event-protocol.md)):
  - `NoteOn` (`0x01`): opens the gate; `arg2` (duration frames) drives one-shot auto-release;
    `arg1` (velocity) scales the amp env's peak level (velocity sensitivity).
  - `NoteOff` (`0x02`): closes the gate on the matching voice id (gated mode only).
  - `ParamSet` (`0x10`): live-edits A/D/S/R on a track's instrument (param id encodes which
    segment and which envelope, amp vs filter).
- **Voice allocation** ([06-voice-allocation.md](06-voice-allocation.md)): `is_finished()`
  tells the allocator a released voice's tail has ended and the slot is reclaimable. Stealing
  a voice calls `trigger()` which re-attacks click-free.
- **Song API** ([04-synth-voice.md](04-synth-voice.md)): `.adsr(a,d,s,r)` sets the amp
  envelope; `.fenv(amount, a,d,s,r)` sets the filter envelope; drums (05) set short fixed
  one-shot envelopes internally.

## Edge cases & risks

- **Clicks at note boundaries:** the whole design centers on this — linear zero-start attack,
  exponential-to-epsilon release, min-ramp clamp, and re-attack-from-current-level retrigger.
- **Real-time safety:** the env is a few floats of state; `tick()` allocates nothing. Stage
  transitions are branches, not allocations.
- **One-shot timing:** `arg2` is in **frames** (already converted from cycle-time by the TS
  scheduler), so auto-release is sample-accurate and independent of buffer size.
- **Denormals:** an exponential release asymptotes toward 0 and can produce denormals;
  `is_finished()`'s epsilon snaps level to 0 and frees the voice before that matters, and the
  global FTZ flag ([02-filters.md](02-filters.md)) covers the rest.
- **Voice never freed:** if `sustain > 0` and a gated note's `NoteOff` is dropped, the voice
  would hang. Mitigation: the allocator can steal a hung sustained voice; and the scheduler
  guarantees `NoteOff` delivery for every gated `NoteOn`.
- **CPU:** one-pole per segment per envelope; two envelopes per synth voice = trivial.

## Acceptance criteria

- [ ] Gated env: a `NoteOn`→(hold)→`NoteOff` produces A→D→S→R with the configured times
      (±1 ms). **Test:** render, detect segment slopes/durations from the level signal.
- [ ] One-shot env: a `NoteOn` with `arg2 = 0.2 s` worth of frames releases automatically at
      0.2 s with **no** `NoteOff`. **Test:** render with no NoteOff, assert release begins at
      the right frame.
- [ ] Attack is click-free from silence: the first samples ramp from exactly 0; no step
      transient. **Test:** render a note, assert sample[0] ≈ 0 and a smooth ramp.
- [ ] Release is click-free: the tail decays to below −80 dB before the voice frees; no abrupt
      cutoff. **Test:** render, assert monotonic exponential tail to epsilon.
- [ ] Retrigger mid-note re-attacks from the current level (no click, no snap to 0). **Test:**
      trigger, then re-trigger 50 ms later; assert continuity at the retrigger sample.
- [ ] Velocity (`arg1`) scales the peak amp level proportionally. **Test:** two notes at
      vel 0.5 and 1.0, assert ~6 dB difference at the sustain.
- [ ] Filter env modulates cutoff by `env_amount_hz` over the attack. **Test:** with
      `env_amount_hz>0`, assert the SVF's −3 dB point moves up during attack (FFT over time).
