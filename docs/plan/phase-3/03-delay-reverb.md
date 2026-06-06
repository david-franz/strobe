# 03 — Delay & reverb

## Purpose

The two **time/space** effects that give a mix depth, and the canonical **send-bus** effects: many
tracks feed one shared reverb (a Phase 3 "done when") or one shared delay rather than paying for an
instance per track.

1. **Delay** — mono, stereo, and **ping-pong** echo with adjustable **time** (free in ms **or
   tempo-synced** to `cps`, e.g. a dotted-eighth), **feedback**, and a **filter in the feedback
   path** (the classic darkening/“dub” echo that loses highs each repeat).
2. **Reverb** — an **algorithmic** reverb (FDN / Freeverb-style, or fundsp's `reverb`) with
   **size**, **decay**, **damping**, and **mix**, for everything from a tight room to a long hall.

Both implement the [`Effect` trait](01-effects-framework.md#the-effect-trait) and are the primary
inhabitants of the [send buses](01-effects-framework.md#global-send-buses), though delay can also be
a per-track insert.

## Location

```
engine/src/effects/delay.rs    # Delay: mono/stereo/ping-pong, tempo-sync, filtered feedback (new)
engine/src/effects/reverb.rs   # Reverb: FDN/Freeverb-style algorithmic reverb (new)
engine/src/effects/sends.rs    # both are the default send-bus effects ([01]) (extended)
engine/src/graph.rs            # builds both fundsp graphs from EffectConfig (extended)
```

`EffectKind::Delay` / `EffectKind::Reverb`
([01-effects-framework.md](01-effects-framework.md#effect-kinds)).

## Design

### Delay

A delay line per channel, pre-allocated to the **maximum delay time** (e.g. 2 s × `SR`), read at a
**smoothed** delay-time tap, with feedback summed back through a one-pole/SVF damping filter.

```rust
#[derive(Clone, Copy, PartialEq)]
pub enum DelayMode { Mono, Stereo, PingPong }

pub struct Delay {
    mode: DelayMode,
    time_l: Smoothed<f32>,   // seconds (smoothed so retuning time pitch-glides cleanly)
    time_r: Smoothed<f32>,
    feedback: Smoothed<f32>, // 0 .. ~0.95 (clamped < 1 for stability)
    damp_hz:  Smoothed<f32>, // feedback-path low-pass cutoff (darkening per repeat)
    buf_l: Vec<f32>,         // pre-allocated to MAX_DELAY_SECS * SR (built off-thread)
    buf_r: Vec<f32>,
    write: usize,
    fb_lp_l: OnePole, fb_lp_r: OnePole,  // damping filters in the feedback path
}
```

Per-block processing (sketch — written directly rather than via a fundsp `feedback()` node so the
ping-pong cross-routing and feedback-path filter are explicit and the buffers stay pre-allocated):

```rust
for i in 0..n {
    let read_l = read_interp(&self.buf_l, self.write, self.time_l.next() * sr); // fractional read
    let read_r = read_interp(&self.buf_r, self.write, self.time_r.next() * sr);
    let fb = self.feedback.next();
    let (fl, fr) = match self.mode {
        DelayMode::Mono     => (read_l, read_l),
        DelayMode::Stereo   => (read_l, read_r),
        DelayMode::PingPong => (read_r, read_l),   // cross-feed: echoes bounce L↔R
    };
    let dl = self.fb_lp_l.lp(fl, self.damp_hz.cur()); // damp the feedback (lose highs each repeat)
    let dr = self.fb_lp_r.lp(fr, self.damp_hz.cur());
    self.buf_l[self.write] = l[i] + fb * dl;          // input + filtered feedback
    self.buf_r[self.write] = r[i] + fb * dr;
    l[i] = dl; r[i] = dr;                              // wet out (chain/send applies wet/dry)
    self.write = (self.write + 1) % self.buf_l.len();
}
```

- **Fractional read (`read_interp`)** is linear/all-pass interpolation so that *changing* the delay
  time glides (the tape-style pitch slide) instead of clicking. `time_l/time_r` are smoothed for the
  same reason.
- **Ping-pong** cross-feeds the two read taps so a centered input bounces L→R→L. Mono and stereo are
  the degenerate routings.
- **Filtered feedback** (`fb_lp`) is a one-pole low-pass (optionally the [SVF](02-filters-eq.md) for
  resonant dub) inside the loop, so each repeat is darker — the defining dub-delay character. A
  high-pass option keeps low end from building up.

#### Tempo sync

Delay time can be specified as a **note division** synced to `cps` instead of absolute ms. One cycle
= `1/cps` seconds; with the convention of 4 beats/cycle (from
[architecture.md](../../architecture.md#the-clock-and-musical-time)), a beat = `1/(cps*4)` s. A
sync value like `"1/8"` or `"1/8."` (dotted) maps to seconds:

```rust
// division in cycles (e.g. 1/8 note = 1/8 cycle when 8 divisions/cycle), dotted/triplet factor f
fn synced_time_secs(division_cycles: f32, cps: f32) -> f32 { division_cycles / cps }
```

The engine learns `cps` from the `Tempo (0x40)` event and from `FxCtx::cps`
([01](01-effects-framework.md#the-effect-trait)); when tempo changes, the synced target time updates
and the **smoother glides** the read tap to it (no click, a brief pitch slide). TS passes either a
number (ms) or a sync string; the registry sets a `synced` flag in the param so the engine recomputes
time on tempo change.

#### Feedback stability

`feedback` is **clamped < 1.0** (default max ~0.95). The feedback-path damping filter additionally
removes energy each pass, so even near-unity feedback decays rather than runs away. A runaway is
further bounded by the master limiter, but the clamp is the real guard.

#### Delay params

| Param | `fx_param` | Range | Unit | Default | Notes |
| --- | --- | --- | --- | --- | --- |
| `mode` | build | mono/stereo/ping | enum | stereo | |
| `time` (or L) | `0x01` | 0 .. 2000 | ms **or** sync | 375 ms (≈⅛.) | smoothed; `synced` flag |
| `time_r` | `0x02` | 0 .. 2000 | ms / sync | = L | independent R for stereo |
| `feedback` | `0x03` | 0 .. 0.95 | — | 0.4 | clamped < 1 |
| `damp` | `0x04` | 200 .. 20000 | Hz | 6000 | feedback-path LP cutoff |
| `mix` | `0x00` | 0 .. 1 | — | per use | wet/dry (insert); send level if on a bus |

### Reverb

An **algorithmic** reverb. Two acceptable implementations, behind the same `Effect`/param surface:

- **fundsp `reverb`** (e.g. `reverb_stereo(room_size, time, damping)` / `reverb2_stereo`) wrapped
  with smoothed params — the simplest path and the default.
- A hand-built **FDN (feedback delay network) / Freeverb-style** topology if we want finer control:
  N parallel comb filters (with damped feedback) into a few series all-pass diffusers, per channel,
  with a stereo spread.

```rust
pub struct Reverb {
    size:    Smoothed<f32>,  // 0..1 → comb delay lengths / room scale
    decay:   Smoothed<f32>,  // 0..1 → comb feedback (RT60); clamped < 1
    damping: Smoothed<f32>,  // 0..1 → HF absorption in the comb feedback (one-pole LP)
    mix:     Smoothed<f32>,  // 0..1 (when used as an insert; sends use the bus return)
    graph: Box<dyn AudioUnit>,  // pre-allocated; longest comb sized for max size
}
```

FDN/Freeverb graph sketch (per channel; fundsp operators):

```rust
// N damped comb filters in parallel (& = bus/sum), then series all-pass diffusion (>>):
let combs = comb(d1, fb, damp) & comb(d2, fb, damp) & comb(d3, fb, damp) & comb(d4, fb, damp);
let diffuse = allpass(a1) >> allpass(a2) >> allpass(a3);
let one_channel = pass() >> combs >> diffuse;
// stereo: slightly different delay lengths L vs R give width; cross-feed for a wider tail.
```

- **size** scales the comb delay lengths (small = tight room, large = hall). Changing size live is
  smoothed; because it changes delay lengths, we crossfade or use fractional taps to avoid clicks.
- **decay** sets comb feedback → RT60; **clamped < 1** for stability.
- **damping** is a one-pole low-pass *inside* each comb's feedback so the tail loses highs over time
  (natural air absorption) — directly analogous to the delay's feedback filter.
- A small **pre-delay** and a high-pass on the input keep the reverb from muddying the low end (the
  kick stays punchy even with a big tail).

#### Reverb params

| Param | `fx_param` | Range | Unit | Default | Notes |
| --- | --- | --- | --- | --- | --- |
| `size` | `0x01` | 0 .. 1 | — | 0.5 | room scale |
| `decay` | `0x02` | 0 .. 1 | — | 0.6 | RT60; clamped < 1 |
| `damping` | `0x03` | 0 .. 1 | — | 0.4 | HF absorption in tail |
| `predelay` | `0x04` | 0 .. 200 | ms | 10 | onset gap |
| `mix` / send | `0x00` / send lvl | 0 .. 1 | — | per use | insert wet/dry **or** bus return |

### Send-bus usage (the headline)

Reverb (and delay) are configured **once** as a [send bus](01-effects-framework.md#global-send-buses)
(`engine.configureSend(bus=0, Reverb{…})`). Each track raises its **send level** to that bus via
`.room(x)`; the bus sums all sends, runs the reverb once, and returns into the master. This is the
exact mechanism the README's `.room(0.3)` uses, and the "multiple tracks into one reverb" done-when.
Per-track *insert* reverb is also possible (a `Reverb` slot in the chain with its own `mix`) but is
the exception; the send is the idiomatic, CPU-cheap path.

## Interfaces

- **Event protocol** ([event-protocol.md](../../event-protocol.md)):
  - `TrackFx (0x30)` with `param = pack_fx(slot, fx_param)` sets delay/reverb params on an **insert**
    slot; for **sends**, a chain-level `TrackFx` (`fx_slot = 0xFF`) sets this track's **send level**
    to the bus, and bus-effect params are set with a global (track 0) `TrackFx`/`ParamSet`.
  - `Tempo (0x40)` updates `cps`; the engine recomputes any `synced` delay time and glides to it.
  - `ParamRamp (0x11)` automates any of these params smoothly
    ([07](07-parameter-automation.md)).
- **TS chain API:**
  - `.delay(time, { feedback, damp, mode })` — `time` is ms **or** a sync string (`"1/8"`, `"1/8."`,
    `"1/4t"`). Insert by default; `.delaySend(level)` routes to the shared delay bus instead.
  - `.room(amount)` — raises this track's **reverb send level** (the README usage). `.reverb({ size,
    decay, damping, mix })` configures/【or inserts】 the reverb; `.size()`, `.decay()`, `.damping()`
    set individual params.
- **Per-track chain vs send:** reverb → **send bus** by default (shared); delay → **insert** by
  default, optional shared send. Both selectable.

## Edge cases & risks

- **Feedback stability:** `feedback`/`decay` clamped < 1; the feedback-path damping filter removes
  energy each pass; master limiter is the final net. A self-oscillating delay is never reachable from
  the API.
- **Click on time change:** delay time and reverb size are **smoothed** and read via **fractional
  interpolation**, so retuning glides (tape-style) rather than clicking. Large jumps can briefly
  pitch-slide — documented and musical.
- **Denormals:** long reverb/delay tails decaying to silence are a classic denormal source; the
  global FTZ flag ([phase-1/02-filters.md](../phase-1/02-filters.md)) plus an optional tiny dither
  guard cover it.
- **Low-end buildup / mud:** input high-pass + feedback damping keep repeats/tails from accumulating
  sub-bass that eats headroom (consistent with the master DC/sub high-pass,
  [phase-1/07](../phase-1/07-master-bus-limiter.md)).
- **Tail on reconfig:** when a delay/reverb slot or send is removed, its `tail_frames`
  ([01](01-effects-framework.md#the-effect-trait)) keep it alive until the tail decays before the
  retired chain is dropped off-thread — no chopped tails.
- **Real-time safety:** all delay buffers pre-allocated to max time off-thread; no allocation in
  `process`. Size/time changes are smoother updates, not reallocations.
- **CPU:** reverb is the heaviest effect; the send bus runs it **once for all tracks**, which is the
  whole point of the send architecture.

## Acceptance criteria

- [ ] A mono delay at 375 ms / feedback 0.4 produces decaying echoes at the right spacing. **Test:**
      impulse in, assert echo peaks at 375 ms intervals decaying by ~feedback each repeat.
- [ ] Ping-pong: a centered impulse bounces L→R→L. **Test:** impulse in, assert alternating L/R echo
      energy.
- [ ] Feedback-path damping audibly darkens each repeat. **Test:** impulse in, FFT successive echoes,
      assert HF rolls off per repeat; `damp` raises/lowers the rolloff.
- [ ] Tempo-synced delay (`"1/8"`) tracks `cps`: changing tempo changes the echo spacing to stay an
      eighth note, gliding (no click). **Test:** send `Tempo`, assert new spacing = `1/8` cycle and a
      smooth time transition.
- [ ] Feedback at 0.95 **decays** (does not run away). **Test:** sustained input then silence; assert
      the tail trends to silence and never exceeds the ceiling.
- [ ] Reverb size/decay/damping behave: bigger size = longer/denser tail, more damping = darker tail.
      **Test:** impulse in, measure RT60 and tail spectral tilt vs each param.
- [ ] **Two tracks into one reverb send** both contribute to a single shared tail; muting one changes
      it. **Test:** instrument the send bus, assert one reverb instance, additive input.
- [ ] No denormal CPU spike on a long tail decaying to silence. **Test:** silence-out for seconds,
      CPU flat (FTZ).
- [ ] No underruns with delay + reverb send active under the example load. **Test:** `stats()` clean.
