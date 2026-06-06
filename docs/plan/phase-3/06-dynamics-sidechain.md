# 06 — Dynamics & sidechain (the techno pump)

## Purpose

Two related dynamics effects, and the **headline feature of the whole phase**:

1. **Compressor** — a standard feed-forward compressor (**threshold / ratio / attack / release /
   makeup**) built on an **envelope follower**, for glue, punch, and level control.
2. **Sidechain** — the techno **pump**: a target track's level is **ducked by a control signal
   derived from another track** (classically the kick). When the kick hits, the bass/pads dip and
   "breathe" back in — the genre's signature rhythmic motion. The README's `.sidechain("p1")` is
   exactly this.

Both implement the [`Effect` trait](01-effects-framework.md#the-effect-trait). The compressor is a
**chain insert**; the sidechain is a chain insert whose *control input* is **tapped from another
track**, which is the one piece of cross-track routing in the rack and the main real-time-safety
design problem here.

## Location

```
engine/src/effects/dynamics.rs    # Compressor + EnvelopeFollower (new)
engine/src/effects/sidechain.rs   # SidechainBus (source taps) + SidechainDuck (target) (new)
engine/src/audio_io.rs            # taps each track's pre-fader sum into the SidechainBus (extended)
engine/src/graph.rs               # build the gain-reduction graph (extended)
```

`EffectKind::Comp` ([01-effects-framework.md](01-effects-framework.md#effect-kinds)); the sidechain
duck is a specialized slot that reads the `SidechainBus` from
[`FxCtx`](01-effects-framework.md#the-effect-trait).

## Design

### Envelope follower (shared)

Both effects need a smooth estimate of a signal's level. A peak/RMS follower with separate
attack/release time constants:

```rust
pub struct EnvelopeFollower {
    env: f32,                 // current level estimate (linear)
    atk_coef: f32,            // exp coefficient from attack_ms
    rel_coef: f32,            // exp coefficient from release_ms
}
impl EnvelopeFollower {
    #[inline] fn process(&mut self, x: f32) -> f32 {
        let rect = x.abs();                                  // (or x*x for RMS-ish)
        let coef = if rect > self.env { self.atk_coef } else { self.rel_coef };
        self.env = coef * self.env + (1.0 - coef) * rect;    // one-pole, asymmetric
        self.env
    }
    fn set_times(&mut self, atk_ms: f32, rel_ms: f32, sr: f32) {
        self.atk_coef = (-1.0 / (atk_ms * 0.001 * sr)).exp();
        self.rel_coef = (-1.0 / (rel_ms * 0.001 * sr)).exp();
    }
}
```

### Compressor

Feed-forward: follow the input level, compute gain reduction in dB above the threshold by the ratio,
smooth it (the follower's attack/release **are** the smoothing), apply makeup.

```rust
pub struct Compressor {
    threshold_db: Smoothed<f32>,  // -60 .. 0 dB
    ratio:        Smoothed<f32>,  // 1 .. 20  (∞ ≈ limiter)
    knee_db:      f32,            // soft knee width, e.g. 6 dB
    makeup_db:    Smoothed<f32>,  // 0 .. 24 dB
    follower:     EnvelopeFollower,
}
impl Effect for Compressor {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], ctx: &FxCtx) {
        for i in 0..l.len() {
            let det = 0.5 * (l[i].abs() + r[i].abs());       // stereo-linked detection
            let env_db = lin_to_db(self.follower.process(det));
            let over = env_db - self.threshold_db.cur();     // dB above threshold
            let gr_db = if over <= 0.0 { 0.0 }
                        else { -over * (1.0 - 1.0/self.ratio.cur()) };  // soft-knee elided
            let g = db_to_lin(gr_db + self.makeup_db.cur());
            l[i] *= g; r[i] *= g;                            // stereo-linked gain
        }
    }
}
```

- **Stereo-linked** gain (same reduction on L/R) keeps the image stable, like the
  [master limiter](../phase-1/07-master-bus-limiter.md#the-look-ahead-brick-wall-limiter).
- **Attack/release** live in the follower; a soft **knee** smooths the threshold.
- The master limiter is the brick-wall safety net; this compressor is the *creative* dynamics tool
  the master limiter doc deferred to Phase 3.

#### Compressor params

| Param | `fx_param` | Range | Unit | Default | Notes |
| --- | --- | --- | --- | --- | --- |
| `threshold` | `0x01` | −60 .. 0 | dB | −18 | |
| `ratio` | `0x02` | 1 .. 20 | :1 | 4 | |
| `attack` | `0x03` | 0.1 .. 100 | ms | 10 | follower atk |
| `release` | `0x04` | 10 .. 1000 | ms | 100 | follower rel |
| `makeup` | `0x05` | 0 .. 24 | dB | 0 | |
| `knee` | `0x06` | 0 .. 24 | dB | 6 | soft knee |
| `mix` | `0x00` | 0 .. 1 | — | 1.0 | wet/dry → parallel "NY" compression |

### Sidechain — the pump

The sidechain ducks a **target track** using a control signal **tapped from a source track**. Two
flavors, both supported:

- **Envelope-follow duck (true sidechain):** follow the *source track's* level and use it to compute
  gain reduction on the *target* — when the kick is loud, the target ducks. Musical and reacts to the
  source's actual dynamics.
- **Pattern/triggered duck:** a fixed ducking *envelope* re-triggered on the source's onsets (a
  shaped dip per kick). More controllable, classic "volume-shaper" pump. Selectable.

#### Tapping the source signal (real-time-safe)

The cross-track routing must not allocate, lock, or create back-references on the audio thread. We
use a **pre-allocated, single-writer-per-track `SidechainBus`** that the render loop fills *before*
any duck reads it:

```rust
/// One per engine. Indexed by track id. Filled by audio_io.rs as each source track renders,
/// read by any SidechainDuck slot. All fixed-size, no allocation, no locks.
pub struct SidechainBus {
    /// Per-track follower output for the current block (one f32 per track, plus per-sample
    /// detail in a small fixed scratch for sample-accurate ducking).
    env: [EnvelopeFollower; MAX_TRACKS],
    level: [[f32; MAX_BLOCK]; MAX_TRACKS],   // per-sample source level, this block
}
```

Render-loop integration (in `audio_io.rs`, [01](01-effects-framework.md#where-it-all-runs-in-the-render-loop)):

```
per block:
  for each track t:
      render voices into track_l/track_r
      sidechain_bus.tap(t, track_l, track_r)       // run follower → fill level[t][..]  (pre-fx or post, configurable)
      track.process(track_l/track_r, ctx{ sidechain: &sidechain_bus }, sends)
      master += track
```

Because every track's source level is computed and stored **before** the chains run (or at least
before any duck that references it — source tracks are ordered first; a one-block latency is the
fallback and is inaudible at this scale), a `SidechainDuck` slot on the target track just **reads**
`ctx.sidechain.level[source_id][i]` per sample — a plain array read, no synchronization.

> **Source ordering:** the kick (`p1`) is typically a lower track id and renders first, so its
> `level[]` is ready when a higher-id target reads it the same block. If a target references a
> later-id source, it reads the **previous block's** level — a ≤ one-buffer (~5 ms) latency, far
> below perceptual relevance for a pump and never a correctness problem. This keeps the tap strictly
> single-pass and lock-free.

#### The duck

```rust
pub struct SidechainDuck {
    source: u16,                  // track id tapped (from .sidechain("p1") → 1)
    amount: Smoothed<f32>,        // 0..1 depth of ducking
    follower: EnvelopeFollower,   // shapes the duck attack/release (the pump curve)
    mode: DuckMode,               // EnvFollow | Triggered
}
impl Effect for SidechainDuck {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], ctx: &FxCtx) {
        let src = &ctx.sidechain.level[self.source as usize];
        for i in 0..l.len() {
            let drive = self.follower.process(src[i]);        // smooth the source level → pump curve
            let duck = 1.0 - self.amount.next() * clamp01(drive); // 1 = open, →0 = ducked
            l[i] *= duck; r[i] *= duck;
        }
    }
}
```

The follower's **attack** (fast — duck quickly when the kick hits) and **release** (the "breathe
back" time, ~100–300 ms) are *the* pump-feel controls. `amount` sets how deep the duck goes.

#### Sidechain params

| Param | `fx_param` | Range | Unit | Default | Notes |
| --- | --- | --- | --- | --- | --- |
| `source` | build | track id | — | from `.sidechain("p1")` | which track taps |
| `amount` | `0x01` | 0 .. 1 | — | 0.7 | duck depth |
| `attack` | `0x02` | 0.1 .. 50 | ms | 2 | how fast it ducks |
| `release` | `0x03` | 20 .. 1000 | ms | 200 | the "breathe back" — the pump feel |
| `mode` | build | env/trig | enum | env | follow vs triggered envelope |

## Interfaces

- **Event protocol** ([event-protocol.md](../../event-protocol.md)):
  - `TrackFx (0x30)` with `param = pack_fx(slot, fx_param)` sets compressor/sidechain params on the
    slot. The **source track id** is sent in the chain config (control call), not per-event, since it
    is structural.
  - `ParamRamp (0x11)` automates threshold/amount/release from a signal
    ([07](07-parameter-automation.md)).
  - String control values: per the scheduler, a **string** param value (like `"p1"`) is passed
    through ([06-scheduler-integration.md](../phase-2/06-scheduler-integration.md) — "e.g. sidechain
    target") and resolved to a track id when the chain is configured.
- **TS chain API:**
  - `.comp({ threshold, ratio, attack, release, makeup })` or `.comp(ratio)` — appends a `Comp` slot.
  - **`.sidechain("p1")`** — appends a `SidechainDuck` slot whose source is track `p1`. Optional
    `.sidechain("p1", { amount, attack, release })`. This is the README headline:
    `p3( note("c2 c2 eb2 g2").saw().lpf(...).room(0.3).sidechain("p1") )` ducks the bass to the kick.
- **Per-track chain vs send:** both are **inserts** on the **target** track. The sidechain reads the
  **source** track's tapped level via the `SidechainBus` in `FxCtx` — the only cross-track read,
  done lock-free as above.

## Edge cases & risks

- **Real-time safety of the cross-track tap:** the `SidechainBus` is **fixed-size, single-writer per
  track, filled before duck reads** in the same single render pass — no locks, no allocation, no
  back-references. A later-id source incurs ≤ one-block latency, documented and inaudible.
- **No feedback loop:** ducking only *reads* a source level and *scales* the target; the source is
  unaffected, so there is no audio feedback path. Two tracks sidechaining each other is still safe
  (each reads the other's level; bounded one-block latency, no runaway).
- **Pump artifacts:** an over-fast release distorts (modulates the waveform like a tremolo into the
  audio band); release is clamped ≥ ~20 ms and the follower smooths the curve. Acceptance checks the
  duck is rhythmic, not buzzy.
- **Self/empty source:** `.sidechain` on a non-existent or silent source produces no ducking (level
  stays 0 → `duck = 1`); a warning is surfaced once (control plane).
- **Denormals:** follower env decaying to silence → FTZ flag covers it
  ([phase-1/02](../phase-1/02-filters.md)).
- **Stereo image:** both effects apply **stereo-linked** gain so ducking/compression don't shift the
  image.
- **Tap point (pre/post-fx):** source level is tapped **pre-fader** by default (so the source's own
  effects don't change the trigger), configurable. Documented so the pump tracks the kick's transient.

## Acceptance criteria

- [ ] Compressor: above threshold, output level rises less than input by the ratio; below, it's
      unity. **Test:** static-level sweep in, assert the gain-reduction curve matches threshold/ratio
      within tolerance; attack/release times measured from a step.
- [ ] Parallel comp (`mix < 1`) preserves transients while adding body. **Test:** transient + sustain
      in, assert peaks retained, sustain raised.
- [ ] **The pump:** a bass track with `.sidechain("p1")` against a 4-on-the-floor kick audibly ducks
      on each kick and breathes back over the release. **Test (audible + measured):** render kick +
      bass, assert the bass envelope dips synchronized to each kick onset by ~`amount`, recovering
      over ~`release`; this is the Phase 3 "done when".
- [ ] The duck **reads the source track's level** (changing the kick pattern changes the pump; muting
      the kick stops the pump). **Test:** alter `p1`, assert the duck timing follows.
- [ ] Cross-track tap is **lock-free and allocation-free**. **Test:** alloc-counter stress check
      across a sidechained mix; no audio-thread allocation/locks.
- [ ] Sweeping `amount`/`release` via a signal is click-free. **Test:** ramp the param, assert no step
      transient.
- [ ] No underruns with compressor + sidechain active under the techno-loop example load. **Test:**
      `stats()` clean during the example.
