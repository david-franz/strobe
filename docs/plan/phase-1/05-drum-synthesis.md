# 05 — Synthesized drum kit

## Purpose

This is the "base drum click" the project is named around, grown into a usable kit. It
provides **synthesized** (not sampled) drums — **kick (bd), snare/clap (sn/cp), and closed/
open hats (hh/oh)** — each a small `fundsp` graph built from the same primitives as the synth
voice (oscillator 01, filter 02, envelopes 03). Every drum is a **one-shot** voice (no
`NoteOff`; length from `NoteOn.arg2`) and **all of its parameters are live-settable** so a
beat can be reshaped while it plays. These map to `sound("bd sn hh cp")`.

## Location

```
engine/src/voices/drum.rs   # DrumKind, per-drum graphs, DrumVoice (new)
engine/src/graph.rs         # build_drum_voice(DrumConfig) (extended)
engine/src/voices/mod.rs    # Voice trait impl for DrumVoice (new)
packages/tekno/src/drums.ts # sound() name → drum instrument mapping + params (new)
```

Pooled/triggered by [06-voice-allocation.md](06-voice-allocation.md); summed in
[07-master-bus-limiter.md](07-master-bus-limiter.md).

## Design

Each drum is a one-shot instrument registered via `defineInstrument()` and referenced by
`NoteOn.arg3`. They reuse the `Voice` trait from [04](04-synth-voice.md) (`trigger` opens the
one-shot envelopes; `release` is a no-op; `is_finished` frees on tail end).

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DrumKind { Kick, Snare, Clap, ClosedHat, OpenHat }
```

### Kick (bd) — pitched sine + fast pitch & amp envelope

A kick is a sine whose **pitch drops fast** (a "pitch envelope" from a click-y high start to
a low body) while its **amplitude decays fast**. The pitch drop is the thump; the high start
is the beater click.

```rust
use fundsp::hacker::*;

// pitch_env: starts at start_hz, drops to body_hz over pitch_decay_s (exp)
// amp_env:   one-shot, fast decay over amp_decay_s
fn build_kick(c: &KickConfig) -> Box<dyn AudioUnit> {
    let pitch = An(EnvNode::pitch_drop(c.start_hz, c.body_hz, c.pitch_decay_s)); // 0-in/1-out Hz
    let amp   = An(EnvNode::one_shot(c.amp_env));                                // 0-in/1-out
    let tone  = pitch >> sine();                  // sine whose freq follows the pitch env
    let body  = tone * amp;
    // optional click/drive: soft-clip for harmonics on hard kicks
    Box::new((body >> shape(Shape::Tanh(c.drive))) * constant(c.gain))
}
```

| Param | Range | Default | Notes |
| --- | --- | --- | --- |
| `start_hz` | 50 .. 400 | 150 | beater-click pitch |
| `body_hz` | 30 .. 120 | 50 | sustained body pitch |
| `pitch_decay_s` | 0.005 .. 0.15 | 0.04 | how fast pitch drops |
| `amp_decay_s` | 0.05 .. 1.0 | 0.25 | body length |
| `drive` | 0 .. 5 | 0.5 | tanh saturation for punch |
| `gain` | 0 .. 1 | 0.9 | |

### Snare (sn) — noise + tuned bandpass + tone

A snare = a short **noise burst** (the "snares" rattle) through a **tuned bandpass**, plus a
**tonal body** (a couple of sine/triangle partials for the drum shell), each with its own fast
decay.

```rust
fn build_snare(c: &SnareConfig) -> Box<dyn AudioUnit> {
    let noise_env = An(EnvNode::one_shot(c.noise_env));
    let body_env  = An(EnvNode::one_shot(c.body_env));
    // noise → bandpass(tuned) → env
    let rattle = noise() >> (pass() | constant(c.bp_hz) | constant(c.bp_q)) >> bandpass();
    let rattle = rattle * noise_env;
    // tonal body: two partials
    let body = (constant(c.tone_hz) >> sine()
              + constant(c.tone_hz * 1.6) >> sine() * constant(0.5)) * body_env;
    Box::new((rattle * constant(c.noise_gain) + body * constant(c.body_gain)) * constant(c.gain))
}
```

| Param | Range | Default | Notes |
| --- | --- | --- | --- |
| `bp_hz` | 800 .. 5000 | 1800 | noise bandpass center |
| `bp_q` | 0.5 .. 8 | 1.5 | rattle brightness/width |
| `tone_hz` | 100 .. 400 | 180 | shell body pitch |
| `noise_decay_s` | 0.02 .. 0.4 | 0.12 | rattle length |
| `body_decay_s` | 0.02 .. 0.3 | 0.08 | body length |
| `noise_gain` / `body_gain` | 0 .. 1 | 0.7 / 0.5 | mix |

### Clap (cp) — noise bursts + bandpass, multi-tap

A clap is like a snare's noise side but **several very short noise bursts in quick
succession** (the hand-claps) followed by a longer "room" tail, through a bandpass. We model
the bursts as a fast multi-trigger amp envelope (3–4 spikes) plus a decaying tail.

```rust
fn build_clap(c: &ClapConfig) -> Box<dyn AudioUnit> {
    // burst_env: 3-4 closely spaced spikes then an exp tail (custom one-shot env curve)
    let burst = An(EnvNode::clap_bursts(c.spread_s, c.taps, c.tail_s));
    let bp = noise() >> (pass() | constant(c.bp_hz) | constant(c.bp_q)) >> bandpass();
    Box::new((bp * burst) * constant(c.gain))
}
```

| Param | Range | Default | Notes |
| --- | --- | --- | --- |
| `bp_hz` | 800 .. 4000 | 1200 | clap color |
| `bp_q` | 0.5 .. 4 | 1.2 | |
| `taps` | 2 .. 5 | 3 | number of bursts |
| `spread_s` | 0.005 .. 0.03 | 0.012 | spacing between bursts |
| `tail_s` | 0.05 .. 0.4 | 0.2 | room tail length |

### Closed / open hats (hh / oh) — filtered noise, short/long decay

Hats are **filtered noise** (usually high-passed, optionally band/high-pass shimmer) with a
**very short** decay for closed and a **long** decay for open — the only essential difference.
A metallic flavor can be added by mixing a few high square partials, but filtered noise alone
is usable.

```rust
fn build_hat(c: &HatConfig) -> Box<dyn AudioUnit> {
    let env = An(EnvNode::one_shot(c.amp_env));   // short (closed) or long (open) decay
    // high-pass the noise so it sits up top; optional bandpass shimmer
    let src = noise() >> (pass() | constant(c.hp_hz) | constant(c.hp_q)) >> highpass();
    Box::new((src * env) * constant(c.gain))
}
```

| Param | Range | Default (closed) | Default (open) | Notes |
| --- | --- | --- | --- | --- |
| `hp_hz` | 3000 .. 12000 | 7000 | 6000 | brightness |
| `hp_q` | 0.5 .. 4 | 0.7 | 0.7 | |
| `decay_s` | 0.01 .. 0.08 (closed) / 0.1 .. 0.8 (open) | 0.04 | 0.3 | the closed/open distinction |
| `gain` | 0 .. 1 | 0.6 | 0.6 | |

> **Choke group:** an open hat should be cut by a following closed hat (real hi-hats can't ring
> while closed). Modeled as a choke group in the allocator: triggering `hh` releases any
> sounding `oh` on the same group. See [06-voice-allocation.md](06-voice-allocation.md).

### Live-settable parameters

Every field above is reachable via `ParamSet` (`0x10`) with drum param ids continuing the
namespace from [04](04-synth-voice.md). This satisfies the roadmap's "**all parameters
live-settable**" requirement — e.g. `sound("bd*4").decay(0.4)` retunes the kick body while it
plays.

### `DrumConfig` (the `defineInstrument()` payload)

```ts
// packages/tekno/src/drums.ts
export interface DrumConfig {
  type: "drum";
  kind: "bd"|"sn"|"cp"|"hh"|"oh";
  // kind-specific params (kick: startHz/bodyHz/pitchDecay/ampDecay/drive; …) all optional,
  // sensible defaults baked in; chainable setters override them.
  [param: string]: number | string;
}
```

## Interfaces

- **Event protocol** ([event-protocol.md](../../event-protocol.md)):
  - `NoteOn` (`0x01`): `arg3` = drum instrument id; `arg1` = velocity (accent); `arg2` =
    one-shot duration in frames (the drum self-times its tail — no `NoteOff`). `arg0` (freq)
    may optionally **tune** a drum (e.g. `sound("bd").note("c1")` to pitch the kick), else the
    drum uses its configured base pitch.
  - `ParamSet` (`0x10`): live param edits via drum `ParamId`s.
- **Song API:** `sound("bd sn hh cp")` resolves each token to a drum instrument id (bd→kick,
  sn→snare, hh→closed hat, oh→open hat, cp→clap) registered via `defineInstrument()`. Chainable
  setters (`.gain()`, `.pan()`, plus drum-specific `.decay()`, `.tone()`, …) override
  `DrumConfig` defaults or emit `ParamSet`s.
- **Voice allocation:** drums are one-shot voices in the same pool; hats use a **choke group**.
  See [06](06-voice-allocation.md).
- **Shared primitives:** oscillator [01](01-oscillators.md), filter [02](02-filters.md) (the
  bandpass/highpass), envelopes [03](03-envelopes.md) (one-shot + the pitch-drop / clap-burst
  curves).

## Edge cases & risks

- **Clicks on short decays:** very fast envelopes still start at 0 and end at epsilon (03), so
  even a 10 ms hat is click-free. Min-ramp clamp protects against a `decay 0`.
- **Real-time safety:** graphs pre-built in the pool; `trigger` only sets vars/envelopes. The
  clap's multi-burst and the kick's pitch-drop are custom env curves — still allocation-free
  state machines.
- **Denormals:** noise never denormals; the filtered/enveloped tails snap to zero via the env
  epsilon + global FTZ ([02](02-filters.md)).
- **Kick DC / sub overload:** a low sine kick carries strong sub energy that can dominate the
  master; the limiter ([07](07-master-bus-limiter.md)) and a high-pass-free, centered mix keep
  it in check. Consider a gentle 25 Hz high-pass on the master to avoid inaudible sub eating
  headroom.
- **CPU:** drums are cheaper than synth voices (no continuous gate); they count toward the
  16-voice budget but are light.

## Acceptance criteria

- [ ] **bd** has a pitch drop (audible thump) and a fast amp decay; spectrum shows energy
      sliding from `start_hz` to `body_hz`. **Test:** render, spectrogram, assert the
      descending fundamental.
- [ ] **sn** combines a noise rattle (bandpass-shaped) with a tonal body; both decay fast.
      **Test:** FFT shows a bandpass-centered noise hump plus a tonal partial at `tone_hz`.
- [ ] **cp** produces multiple closely spaced bursts then a tail. **Test:** envelope follower on
      the render shows `taps` distinct spikes within `spread_s`.
- [ ] **hh** (closed) is short and **oh** (open) is long, both high-passed bright noise.
      **Test:** measure decay times; assert closed ≪ open and high-frequency emphasis.
- [ ] A closed hat **chokes** a ringing open hat. **Test:** trigger oh then hh; assert oh's tail
      is cut at the hh onset.
- [ ] All drum params are live-settable via `ParamSet` mid-beat with no glitch. **Test:**
      retune `bd` body 50→80 Hz on a running `bd*4`; assert smooth change, no underrun.
- [ ] The full kit `sound("bd sn hh cp")` "sounds usable for a real beat" (the roadmap bar).
      **Test:** the Phase 1 example plays a 1-bar groove; subjective sign-off + `stats()` clean.
