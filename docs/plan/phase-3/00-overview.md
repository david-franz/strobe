# Phase 3 — Effects rack (overview)

> **Goal:** comprehensive, **production-grade effects**, usable both as **per-track effect
> chains** and as **global send buses**. Phase 0 proved the spine; Phase 1 gave the engine
> real voices and a master limiter; Phase 2 gave us the full pattern language and made *any*
> parameter automatable from a control pattern or continuous signal. Phase 3 widens the
> **effects** layer of that proven spine — the `effects.rs` box the
> [architecture diagram](../../architecture.md#the-rust-engine-internals) already drew between
> the voices and the master bus — into the rack a producer expects: filters/EQ, delay, reverb,
> distortion, bitcrush, modulation FX, dynamics, and the headline techno **sidechain pump**.

This document is the index for the Phase 3 plan. Each feature has its own file with the full
**Purpose / Location / Design / Interfaces / Edge cases & risks / Acceptance criteria**
breakdown, in the same style as [Phase 1](../phase-1/00-overview.md).

## Where this sits

Phase 3 is the fourth phase of the [roadmap](../../roadmap.md#phase-3--effects-rack). It builds
directly on the proven spine and **changes nothing above the bridge that earlier phases settled**:

- The [two-plane architecture](../../architecture.md#the-core-principle-two-planes) (TS control
  plane / Rust audio plane) is fixed. **No allocation, no locks, and no JS run on the audio
  thread** — every effect graph, delay line, and look-ahead buffer introduced here is
  **pre-allocated** inside the engine and reconfigured by an atomic double-buffered swap, never by
  allocating in the `cpal` callback (see [01-effects-framework.md](01-effects-framework.md)).
- The [event protocol](../../event-protocol.md) and its **32-byte fixed record** are frozen.
  Phase 3 implements the opcodes earlier phases stubbed for effects:
  - **`TrackFx` (`0x30`)** — set/route a track effect param; the `param` id **encodes
    `(fx-slot, fx-param)`** (the encoding is specified in
    [01-effects-framework.md](01-effects-framework.md#trackfx-param-id-encoding)).
  - **`ParamSet` (`0x10`)** / **`ParamRamp` (`0x11`)** — the same automation path Phase 2 already
    uses, now reaching effect parameters so a signal can sweep any of them
    ([07-parameter-automation.md](07-parameter-automation.md)).
  - Phase 3 invents **no new opcodes**.
- Everything DSP is a [`fundsp`](https://github.com/SamiPerttu/fundsp) graph composed with
  fundsp's operators `>>` (pipe), `|` (stack), `&` (bus), `^` (branch) — we do not hand-roll
  delay lines or biquads where fundsp gives a typed, zero-cost building block, exactly as Phase 1
  did for [filters](../phase-1/02-filters.md) and the
  [master limiter](../phase-1/07-master-bus-limiter.md).
- Time stays [cycle-based](../../architecture.md#the-clock-and-musical-time) with `cps` tempo.
  Tempo-synced effect times (delay time, LFO rate) are derived from `cps`
  ([03-delay-reverb.md](03-delay-reverb.md), [05-modulation-fx.md](05-modulation-fx.md)); the
  engine learns `cps` from the existing `Tempo` (`0x40`) event.

The effects stage slots **exactly where Phase 1 left room for it**: the
[master-bus doc](../phase-1/07-master-bus-limiter.md#where-it-sits-in-the-render-loop) explicitly
notes that per-track FX chains and send buses are Phase 3 and that the master bus was built "so the
per-track stage slots cleanly above it later (voices → [future per-track FX] → master bus →
limiter)." Phase 3 fills that gap.

## What Phase 3 delivers (the feature files)

| # | Feature | File | One-line summary |
| --- | --- | --- | --- |
| 01 | Effects framework | [01-effects-framework.md](01-effects-framework.md) | the `Effect` trait, ordered per-track slot chains, global send buses, `TrackFx` `(fx-slot, fx-param)` id encoding, wet/dry + bypass, RT-safe double-buffered chain reconfiguration |
| 02 | Filters & EQ | [02-filters-eq.md](02-filters-eq.md) | sweepable resonant LP/HP/BP **performance filter** (distinct from the per-voice SVF) and a multiband **EQ** (low/high shelf + peaking bands) as track effects |
| 03 | Delay & reverb | [03-delay-reverb.md](03-delay-reverb.md) | mono / stereo / ping-pong **delay** (tempo-synced, filtered feedback) and an algorithmic **reverb** (FDN/Freeverb-style) with size/decay/damping/mix; send-bus usage |
| 04 | Distortion & bitcrush | [04-distortion-bitcrush.md](04-distortion-bitcrush.md) | tanh / waveshaper / soft-clip **drive** and **bitcrush** (bit-depth + sample-rate decimation) with anti-aliasing — the core techno/hip-hop grit |
| 05 | Modulation FX | [05-modulation-fx.md](05-modulation-fx.md) | **chorus / flanger / phaser** as LFO-modulated delay / all-pass lines; shared LFO, rate (tempo-synced) / depth / feedback / stereo spread |
| 06 | Dynamics & sidechain | [06-dynamics-sidechain.md](06-dynamics-sidechain.md) | **compressor** (threshold/ratio/attack/release/makeup + envelope follower) and the headline **sidechain pump** — a track ducked by a control signal tapped from another track's output |
| 07 | Parameter automation | [07-parameter-automation.md](07-parameter-automation.md) | the end-to-end path from `.lpf(sine.range(...).slow(8))` in TS to a moving filter: `ParamSet` vs smoothed `ParamRamp`, audio-rate smoothing on every effect param |

### How they fit together

```
NoteOn/TrigSample ─► voices (P1) ─► per-track FX chain (01: ordered slots)
                                       slot0 ─ slot1 ─ … ─ slotN     ┐ each slot = one Effect
                                       (02 filter/EQ, 04 dist/crush,  │  (a fundsp graph,
                                        05 mod, 06 comp, …)           │   02–06)
                                            │  └─ send taps ──────────┼──► send buses (01)
                                            ▼                         │      ├─ reverb send (03)
                                       per-track sum (mixer)          │      └─ delay  send (03)
                                            │  ◄──────────────────────┘   (returns sum back in)
                                            ▼
                                  master bus + limiter (P1/07)
                                            ▼
                                       cpal output

sidechain (06): track A's pre-fader output ─► envelope follower ─► duck gain on track B
```

This is the [engine-internals diagram](../../architecture.md#the-rust-engine-internals) from the
architecture doc, filled in for the effects layer. The numbered boxes are the feature files above.

## Files touched (engine-wide map)

All paths under `engine/src/`. Phases 0–1 already created `audio_io.rs`, `scheduler.rs`, the ring
buffer, the voices, and `effects.rs` (which currently holds only the master limiter). Phase 3 grows
`effects.rs` into a module and adds the chain/send infrastructure.

```
engine/src/
  effects/
    mod.rs            # Effect trait, EffectKind, slot/chain dispatch, send-bus router (new; 01)
    chain.rs          # per-track ordered slot chain + double-buffered config swap (new; 01)
    sends.rs          # global send buses + per-track send levels + return summing (new; 01)
    filter_eq.rs      # performance filter + multiband EQ (new; 02)
    delay.rs          # mono/stereo/ping-pong delay, tempo-synced, filtered feedback (new; 03)
    reverb.rs         # FDN/Freeverb-style algorithmic reverb (new; 03)
    distortion.rs     # saturation / waveshaper / soft-clip (new; 04)
    bitcrush.rs       # bit-depth reduction + sample-rate decimation (new; 04)
    modulation.rs     # chorus / flanger / phaser + shared LFO (new; 05)
    dynamics.rs       # compressor + envelope follower (new; 06)
    sidechain.rs      # source tap + duck applied to a target track (new; 06)
    smoothing.rs      # shared per-param one-pole smoother (Smoothed<f32>) (new; 07)
  effects.rs          # re-exports effects/ ; master limiter stays here (Phase 1, extended)
  audio_io.rs         # render loop: voices → per-track chains → sends → master (extended)
  scheduler.rs        # dispatch TrackFx(0x30) + ParamSet/ParamRamp to chains/sends (extended)
  graph.rs            # build effect fundsp graphs from EffectConfig (extended)
```

TS side (`packages/tekno/`) gains the chainable effect methods (`.lpf()`/`.hpf()` as *track*
effects, `.delay()`, `.room()`, `.crush()`, `.dist()`, `.chorus()`, `.flanger()`, `.phaser()`,
`.comp()`, `.sidechain("p1")`, …), the per-track-chain-vs-send routing in `PARAM_REGISTRY`, and the
`TrackFx` param-id packing. The pattern/automation machinery is already present from
[Phase 2](../phase-2/06-scheduler-integration.md); Phase 3 only registers new effect keys into it.

## Phase 3 "done when" (restated from the [roadmap](../../roadmap.md#phase-3--effects-rack))

- A **techno loop with a sidechained bass audibly "pumps" against the kick**
  ([06-dynamics-sidechain.md](06-dynamics-sidechain.md)).
- **Sends work** — multiple tracks routed into one shared reverb
  ([01-effects-framework.md](01-effects-framework.md), [03-delay-reverb.md](03-delay-reverb.md)).
- **Sweeping an effect param via a signal is smooth and click-free**
  ([07-parameter-automation.md](07-parameter-automation.md)).

Plus the [cross-cutting definition of done](../../roadmap.md#cross-cutting-definition-of-done-every-phase):
a runnable `examples/` song exercises the rack (the README quickstart techno loop —
`.lpf(sine.range(300,1200).slow(8))`, `.room(0.3)`, `.sidechain("p1")` — is exactly this song),
`stats()` is clean under that load, pure-TS routing/registry logic has unit tests, every effect has
golden/null-test checks where practical, and it is validated on macOS first (Linux/Windows via CI).
