# 07 — Parameter automation (any effect param from a signal)

## Purpose

The roadmap requires that **any effect parameter is automatable by a control pattern/signal** and
that "sweeping an effect param via a signal is smooth and click-free"
([roadmap](../../roadmap.md#phase-3--effects-rack)). The mechanism already exists — Phase 2 built the
continuous-signal sampling and the `ParamSet`/`ParamRamp` decision
([phase-2/05-continuous-signals.md](../phase-2/05-continuous-signals.md),
[phase-2/06-scheduler-integration.md](../phase-2/06-scheduler-integration.md)). This file pins down
**how that machinery reaches effect parameters specifically**: the end-to-end path from
`.lpf(sine.range(300,1200).slow(8))` (or `.crush(rand.range(4,12))`, `.delay("1/8", { feedback:
saw.range(0,0.8).slow(16) })`) in TypeScript to a **moving effect** on the audio thread, and the
**audio-rate smoothing** every effect param shares so none of it zippers.

It is mostly **integration glue**: Phase 2 produces the events, [01](01-effects-framework.md) decodes
the `(fx-slot, fx-param)` id, and the shared `Smoothed<f32>` turns discrete updates into a glide.

## Location

```
engine/src/effects/smoothing.rs   # Smoothed<f32>: per-param one-pole + linear ParamRamp (new; shared)
engine/src/scheduler.rs           # apply ParamSet/ParamRamp/TrackFx to the right slot/param (extended)
packages/strobe/src/instrument.ts  # PARAM_REGISTRY entries for effect keys (routing + ramp set) (extended)
```

Builds on [phase-2/05](../phase-2/05-continuous-signals.md) (signal sampling, `PARAM_RAMP_KEYS`),
[phase-2/06](../phase-2/06-scheduler-integration.md) (the `Hap`→events compiler), and
[01-effects-framework.md](01-effects-framework.md#trackfx-param-id-encoding) (the packed id).

## Design

### The two delivery modes (recap, applied to effects)

Phase 2 already decides, per `(key, value)` pair, between two ways to deliver a parameter
([phase-2/05](../phase-2/05-continuous-signals.md#per-note-sampling-vs-audio-rate-paramramp)). For
**effect** params:

- **Note-rate `ParamSet` (`0x10`) / `TrackFx` (`0x30`) — the default.** A static number
  (`.lpf(800)`) or a **discrete** value-pattern (`.crush("8 4 12")`) is sampled at the note onset
  (zero-width query) and sent once. The value is constant until the next update. Cheap; correct for
  stepped automation. For an *effect*, the routing target is a `TrackFx` with the packed
  `(fx-slot, fx-param)` id ([01](01-effects-framework.md#trackfx-param-id-encoding)); a per-voice
  param (the synth filter) stays a `ParamSet`.
- **Audio-rate `ParamRamp` (`0x11`) — for smooth sweeps.** A **continuous** signal
  (`sine.range(...).slow(8)`) on a **ramp-eligible** key emits a `ParamRamp`: the scheduler samples
  the signal at the note's `whole.begin` and `whole.end` and sends `(target, ramp_frames)`; the
  engine interpolates **linearly on the audio thread** between updates, so the value glides within
  and across notes instead of stair-stepping at note boundaries.

`PARAM_RAMP_KEYS` ([phase-2/05](../phase-2/05-continuous-signals.md)) is extended to include the
ramp-worthy effect keys (`lpf`/`hpf`/`bpf`, `cutoff`, `room`, `delayfb`, `drive`, `crush`, `rate`,
`depth`, `scAmount`, …); keys not in the set fall back to per-note sampling. The choice is purely a
scheduler/registry concern — **signals themselves stay pure `0..1` functions of time**.

### Audio-rate smoothing — the shared `Smoothed<f32>`

Every automatable effect parameter is held in a **`Smoothed<f32>`**, not a bare `f32`. It is the
single mechanism that prevents zipper noise across the whole rack (the same idea Phase 1 used for the
filter cutoff via fundsp's `follow()`, [phase-1/02](../phase-1/02-filters.md#cutoff-smoothing-no-zipper-noise)),
and it natively understands both a `ParamSet` step and a `ParamRamp` line:

```rust
/// Per-parameter audio-rate smoother. Handles instant steps (ParamSet) and linear ramps
/// (ParamRamp) and de-zippers both. One f32 of state; no allocation; called per sample.
pub struct Smoothed<f32> {
    current: f32,
    target:  f32,
    // one-pole de-zipper for ParamSet steps:
    coef:    f32,        // from smooth_ms (e.g. 5 ms)
    // linear-ramp state for ParamRamp:
    ramp_step: f32,      // (target - current) / ramp_frames; 0 when not ramping
    ramp_left: u32,
}

impl Smoothed<f32> {
    /// ParamSet / TrackFx: jump the target; the one-pole glides current → target.
    pub fn set_target(&mut self, v: f32) { self.target = v; self.ramp_left = 0; }

    /// ParamRamp: linear glide to target over ramp_frames (the scheduler's note-length ramp).
    pub fn ramp_to(&mut self, target: f32, ramp_frames: u32) {
        self.target = target;
        self.ramp_step = if ramp_frames > 0 { (target - self.current) / ramp_frames as f32 } else { 0.0 };
        self.ramp_left = ramp_frames;
    }

    #[inline] pub fn next(&mut self) -> f32 {
        if self.ramp_left > 0 {                          // linear ParamRamp segment
            self.current += self.ramp_step;
            self.ramp_left -= 1;
        } else {                                         // one-pole de-zipper toward target
            self.current += (1.0 - self.coef) * (self.target - self.current);
        }
        self.current
    }
    #[inline] pub fn cur(&self) -> f32 { self.current }
}
```

- **`ParamSet`/`TrackFx`** → `set_target`: the one-pole glides over `smooth_ms` (default ~5 ms), so a
  stepped value-pattern (`.crush("8 4 12")`) or a live tweak never clicks.
- **`ParamRamp`** → `ramp_to`: an exact linear segment over the note's frame length, matching what
  the scheduler computed from the signal endpoints — the audio thread reproduces the sweep
  sample-accurately with no per-sample query.
- Every effect's params (cutoff, drive, feedback, sidechain amount, LFO rate, …) are `Smoothed<f32>`,
  so **the smoothing is uniform and automatic** — an effect author gets click-free automation for
  free just by storing params this way.

### End-to-end path — the canonical example

`p3( note("c2*8").lpf( sine.range(300,1200).slow(8) ).room(0.3) )` where `.lpf` here targets the
**track performance filter** ([02-filters-eq.md](02-filters-eq.md)):

```
TypeScript (control plane)                            Rust engine (audio plane)
──────────────────────────                           ─────────────────────────
1. .lpf(sine.range(300,1200).slow(8))
   stores { lpf: <continuous Pat<number>> } in the
   ControlMap ([phase-2/04]).

2. scheduler tick queries the track; for each note
   onset, resolveValues sees lpf is CONTINUOUS and a
   ramp key, and the hap has a whole ([phase-2/05/06]):
     v0 = signal @ whole.begin
     v1 = signal @ whole.end
   → emit ParamRamp(0x11,
        param = pack_fx(filterSlot, cutoff=0x01),       ([01] id encoding)
        target = v1, ramp_frames = note length)
   .room(0.3) (static) → TrackFx(0x30, chain send level).

3. encodeAndPush: 32-byte records, frame-sorted,
   one pushEvents ([phase-2/06]).
                                       ─ ring buffer ─►
                                                     4. scheduler.rs drains the ParamRamp at its
                                                        frame; decodes (fx_slot, fx_param) ([01]);
                                                        finds slot=filter, param=cutoff;
                                                        calls fx.ramp_param(0x01, target, frames).
                                                     5. PerfFilter's cutoff Smoothed<f32>.ramp_to(...)
                                                        glides the SVF cutoff input each sample.
                                                     6. next note's ParamRamp continues from the
                                                        current value → one seamless 8-cycle sweep.
```

The result is the README's "8-cycle filter sweep" realized as a chain of per-note `ParamRamp`s the
engine interpolates on the audio thread — **smooth and click-free**, the Phase 3 done-when. The exact
same path drives **any** effect param: `.crush(rand.range(4,12))` (per-note `TrackFx`, smoothed),
`.delay("1/8", { feedback: saw.range(0,0.8).slow(16) })` (ramped feedback), `.sidechain("p1", {
amount: tri.range(0.3,0.9).slow(4) })` (ramped duck depth).

### Why the value joins continuously across notes

Each `ParamRamp` starts from the smoother's **current** value (not from `v0`), so even though the
scheduler samples endpoints per note, consecutive ramps **chain** without a discontinuity at note
boundaries — the filter doesn't jump back at each `c2`. Within a note, the linear segment matches the
signal closely; across an 8-cycle slow sine, the concatenated segments approximate the curve to well
under audible error (longer notes fit tighter; the linear `ParamRamp` is the protocol's contract).

### Registry wiring (TS side)

`PARAM_REGISTRY` ([phase-2/04](../phase-2/04-sound-note-api.md)) gains an entry per effect key that
declares its **routing** (`paramSet` for per-voice, `trackFx` for chain inserts/sends with the
`(slot, param)` packer) and whether it is **ramp-eligible**:

```ts
// excerpt — effect keys added in Phase 3
lpf:       { kind: "trackFx", pack: (s) => packFx(s, 0x01), ramp: true,  slotKind: "Filter" },
delayfb:   { kind: "trackFx", pack: (s) => packFx(s, 0x03), ramp: true,  slotKind: "Delay"  },
crush:     { kind: "trackFx", pack: (s) => packFx(s, 0x01), ramp: true,  slotKind: "Bitcrush" },
room:      { kind: "trackFx", pack: ()  => CHAIN_SEND(0),   ramp: true,  slotKind: "send"   },
scAmount:  { kind: "trackFx", pack: (s) => packFx(s, 0x01), ramp: true,  slotKind: "Sidechain" },
// non-ramp keys (mode/shape enums) → ramp:false → always per-note ParamSet/TrackFx
```

The scheduler reads this registry exactly as it already does for Phase 2 keys; **no new scheduler
code path** — effects just register more keys.

## Interfaces

- **Event protocol** ([event-protocol.md](../../event-protocol.md)): `ParamSet (0x10)`,
  `ParamRamp (0x11)`, `TrackFx (0x30)` — all unchanged; effect automation reuses them with the packed
  `(fx-slot, fx-param)` id ([01](01-effects-framework.md#trackfx-param-id-encoding)).
- **[phase-2/05-continuous-signals.md](../phase-2/05-continuous-signals.md):** the
  per-note-vs-`ParamRamp` decision, `isContinuousPattern`, `PARAM_RAMP_KEYS` (now incl. effect keys).
- **[phase-2/06-scheduler-integration.md](../phase-2/06-scheduler-integration.md):** the
  `Hap`→events compiler and `resolveValues` already emit `ParamSet`/`ParamRamp`/`TrackFx`; Phase 3
  only adds registry entries so effect keys flow through.
- **[01-effects-framework.md](01-effects-framework.md):** decodes the id, dispatches to the slot,
  calls `set_param`/`ramp_param`, which update the param's `Smoothed<f32>`.
- **[02](02-filters-eq.md)–[06](06-dynamics-sidechain.md):** each effect stores every automatable
  param as a `Smoothed<f32>`, inheriting click-free automation automatically.

## Edge cases & risks

- **Zipper noise:** the entire reason every param is `Smoothed<f32>`. A stepped value-pattern glides
  via the one-pole; a continuous signal glides via the linear ramp. Acceptance asserts no step
  transient.
- **Cross-note continuity:** ramps start from the smoother's current value, so concatenated per-note
  ramps don't jump at note boundaries. A test asserts a continuous sweep across many notes.
- **Ramp without a `whole`:** a continuous signal needs the hap's note length to emit a ramp; if
  `whole` is missing the scheduler falls back to per-note sampling
  ([phase-2/05](../phase-2/05-continuous-signals.md#edge-cases--risks)).
- **Non-ramp params:** enum/mode params (filter mode, distortion shape) are never ramped — they're
  structural and set once via `TrackFx`/config; ramping them is meaningless and the registry marks
  them `ramp:false`.
- **Reproducibility:** signals are pure functions of time ([phase-2/05]); a hot-swap re-querying the
  same arc reads identical modulation, so a swapped pattern's effect param doesn't jump — the same
  guarantee Phase 2 gives, extended to effects.
- **Real-time safety:** smoothing is one f32 of state and a few ops per sample; no allocation, no
  locks; `set_param`/`ramp_param` only update the smoother target. The control-plane sampling is O(1)
  per onset.
- **Smoothing time vs responsiveness:** default `smooth_ms ≈ 5` kills zipper noise without feeling
  laggy; slow pads can raise it. Per-effect overridable.

## Acceptance criteria

- [ ] `.lpf(sine.range(300,1200).slow(8))` on a track produces a **smooth, click-free** 8-cycle
      filter sweep (the Phase 3 done-when). **Test:** render 8 cycles, assert the cutoff control
      traces one sine period with no step transient above −60 dB, continuous across note boundaries.
- [ ] A **discrete** value-pattern on an effect (`.crush("8 4 12")`) emits per-note `TrackFx` and the
      smoother glides between the stepped values (no click at each step). **Test:** assert smoothed
      transitions between 8/4/12 bits.
- [ ] A `ParamRamp` to an effect param interpolates **linearly on the audio thread** matching the
      scheduler endpoints. **Test:** drive a known ramp, sample the engine param per block, assert the
      line.
- [ ] Concatenated per-note ramps are **continuous** (no jump back at note onsets). **Test:**
      `note("c2*8").lpf(slow sweep)`, assert monotone-then-reversing continuous trace.
- [ ] Automating params on **every** effect family works through the same path (filter cutoff, delay
      feedback, distortion drive, bitcrush bits, LFO rate, sidechain amount). **Test:** one ramp per
      family reaches the right `(slot, param)` and glides. (Decoding covered by
      [01](01-effects-framework.md)'s id test.)
- [ ] **No allocation / locks** on the audio thread for any automation. **Test:** alloc-counter stress
      check while sweeping several effect params.
- [ ] **No underruns** while automating multiple effect params under the example load. **Test:**
      `stats()` clean during the techno-loop example.
