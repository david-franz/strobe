# 01 — Effects framework

## Purpose

This is the **foundation the other six effect files plug into**. It defines how effects exist and
run inside the engine: the **`Effect` trait** every effect implements, the **per-track ordered slot
chain** (voices → slot0 → slot1 → … → master), the **global send buses** (e.g. one shared reverb
that many tracks feed), how a `TrackFx` (`0x30`) `param` id **encodes `(fx-slot, fx-param)`**, how
**wet/dry and bypass** work uniformly, and — the hard real-time problem — how a chain is **built and
reconfigured from TypeScript without ever allocating, locking, or touching JS on the audio thread**,
via a **double-buffered config swap**.

Concrete effects ([02](02-filters-eq.md)–[06](06-dynamics-sidechain.md)) only have to implement the
trait; routing, wet/dry, bypass, automation smoothing, and reconfiguration are all provided here.

## Location

```
engine/src/effects/
  mod.rs            # Effect trait, EffectKind enum, render-loop dispatch (new)
  chain.rs          # TrackChain: ordered fixed slot array + double-buffered swap (new)
  sends.rs          # SendBus pool + per-track send levels + return summing (new)
  smoothing.rs      # Smoothed<f32> one-pole param smoother (new; shared, see 07)
engine/src/effects.rs   # re-exports effects/ ; master limiter stays here (Phase 1, extended)
engine/src/audio_io.rs  # render loop now drives chains + sends before the master bus (extended)
engine/src/scheduler.rs # routes TrackFx(0x30)/ParamSet/ParamRamp into chains & sends (extended)
engine/src/graph.rs     # builds each effect's fundsp graph from an EffectConfig (extended)
```

This sits in the [engine-internals diagram](../../architecture.md#the-rust-engine-internals)
between the voice sum and the [master bus + limiter](../phase-1/07-master-bus-limiter.md), exactly
where Phase 1 reserved space for it.

## Design

### The `Effect` trait

Every effect is a stereo, block-based, **real-time-safe** processor. It owns a pre-allocated fundsp
graph (built once, off the audio thread) plus any state, and is driven by the render loop in fixed
blocks (the `cpal` buffer length, e.g. 256 frames). Parameters arrive as `(fx_param_id, value)`
pairs and are applied through **smoothers** so no setter ever steps a coefficient (see
[07-parameter-automation.md](07-parameter-automation.md)).

```rust
/// One stereo audio effect. Implemented by every effect in 02–06.
/// All methods are called on the audio thread and MUST NOT allocate, lock, or block.
pub trait Effect: Send {
    /// Process one block IN PLACE. `l`/`r` are the slot's input on entry, output on exit.
    /// `ctx` carries sample rate, current cps, and the sidechain bus snapshot (06).
    fn process(&mut self, l: &mut [f32], r: &mut [f32], ctx: &FxCtx);

    /// Set one parameter (already decoded from a TrackFx/ParamSet). Pushes a new *target*
    /// into the relevant Smoothed<f32>; the value glides on the audio thread. Never reallocs.
    fn set_param(&mut self, fx_param: u8, value: f32);

    /// Begin a linear ParamRamp (0x11): set the smoother's target and ramp length in frames.
    fn ramp_param(&mut self, fx_param: u8, target: f32, ramp_frames: u32);

    /// Reset internal state (delay lines, filter memory, envelopes) to silence.
    /// Called when a slot is (re)assigned so stale tails don't bleed into a new effect.
    fn reset(&mut self);

    /// Tail length in frames (for delay/reverb) so a chain knows when a swapped-out
    /// effect has fully decayed and can be recycled. 0 for memoryless effects.
    fn tail_frames(&self) -> u32 { 0 }
}
```

```rust
/// Per-block context passed to every Effect (read-only on the audio thread).
pub struct FxCtx<'a> {
    pub sample_rate: f32,
    pub cps: f32,                 // current tempo, for tempo-synced delay/LFO (03, 05)
    pub block_start_frame: u64,   // engine frame at block start (for ParamRamp progress)
    pub sidechain: &'a SidechainBus, // per-track ducking signal snapshot (06)
}
```

The trait is deliberately **block-based, not per-sample-virtual**: dispatch cost is paid once per
block, and inside `process` the effect runs its fundsp graph tightly. fundsp's own
`AudioUnit::process` fills buffers, so an effect is usually a thin wrapper that copies `l`/`r` into
the graph's input buffers, ticks the graph, and copies back — with the wet/dry mix applied by the
chain (below), not by each effect.

### Effect kinds

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EffectKind {
    None = 0,
    Filter   = 1,   // 02
    Eq       = 2,   // 02
    Delay    = 3,   // 03
    Reverb   = 4,   // 03  (also usable as a send bus)
    Distort  = 5,   // 04
    Bitcrush = 6,   // 04
    Chorus   = 7,   // 05
    Flanger  = 8,   // 05
    Phaser   = 9,   // 05
    Comp     = 10,  // 06
    Gain     = 11,  // trivial gain/pan utility
    Pan      = 12,
}
```

`EffectKind` is the discriminant TS sends to build a slot; `graph.rs` maps it to a concrete
`Box<dyn Effect>` (built off-thread, see "reconfiguration").

### Per-track chains (ordered slots)

Each track owns a **fixed-capacity array of effect slots** (`MAX_SLOTS = 8`, plenty for a chain;
configurable at build). Slots are processed **in order**, output of slot *i* feeding slot *i+1*. An
empty slot is `None` and is skipped.

```rust
pub const MAX_SLOTS: usize = 8;

pub struct Slot {
    pub effect: Option<Box<dyn Effect>>,
    pub bypass: bool,            // smoothed wet→dry crossfade, not a hard cut (below)
    pub mix: Smoothed<f32>,      // wet/dry 0..1 (1 = fully wet)
    pub xfade: Smoothed<f32>,    // bypass crossfade gain, 0=bypassed 1=active
}

pub struct TrackChain {
    pub slots: [Slot; MAX_SLOTS],
    pub send: [Smoothed<f32>; MAX_SENDS], // per-send levels (e.g. reverb send), see sends.rs
    scratch_dry_l: [f32; MAX_BLOCK],      // pre-allocated dry copy for wet/dry & bypass
    scratch_dry_r: [f32; MAX_BLOCK],
}
```

Render loop per track, per block:

```rust
impl TrackChain {
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], ctx: &FxCtx, sends: &mut SendRouter) {
        for slot in self.slots.iter_mut() {
            let Some(fx) = slot.effect.as_mut() else { continue; };
            // 1) keep a dry copy for wet/dry + bypass crossfade
            self.scratch_dry_l[..l.len()].copy_from_slice(l);
            self.scratch_dry_r[..r.len()].copy_from_slice(r);
            // 2) run the effect in place (fully wet)
            fx.process(l, r, ctx);
            // 3) wet/dry mix and bypass crossfade, sample-by-sample on smoothed gains
            for i in 0..l.len() {
                let mix   = slot.mix.next();        // smoothed 0..1
                let xf    = slot.xfade.next();      // smoothed 0..1 (bypass = ramps to 0)
                let wetl  = self.scratch_dry_l[i] + mix * (l[i] - self.scratch_dry_l[i]);
                let wetr  = self.scratch_dry_r[i] + mix * (r[i] - self.scratch_dry_r[i]);
                l[i] = self.scratch_dry_l[i] + xf * (wetl - self.scratch_dry_l[i]);
                r[i] = self.scratch_dry_r[i] + xf * (wetr - self.scratch_dry_r[i]);
            }
        }
        // 4) post-chain: tap into the send buses at this track's smoothed send levels
        sends.add_from_track(l, r, &mut self.send);
    }
}
```

- **Wet/dry (`mix`)** is uniform across all effects, so individual effects need not implement it —
  they always render fully wet and the chain crossfades against the retained dry copy. (Effects that
  are naturally parallel — reverb, delay — typically run as **sends** instead; see below.)
- **Bypass** is a *smoothed crossfade* (`xfade` ramps 0↔1 over ~5 ms), never a hard switch, so
  toggling bypass live is click-free. A fully bypassed slot still calls `process` only while
  `xfade > 0` and during the effect's `tail_frames` so its tail decays naturally; once silent the
  chain skips it.

### `TrackFx` param id encoding

The protocol's `TrackFx` (`0x30`) carries a single `param` id that **encodes which slot and which
parameter** ([event-protocol.md](../../event-protocol.md#opcodes): "`param` id encodes (fx-slot,
fx-param)"). We pack it into the protocol's 32-bit `voice/param` field:

```
 31              16 15            8 7             0
┌──────────────────┬───────────────┬───────────────┐
│   reserved (0)    │   fx_slot     │   fx_param    │
└──────────────────┴───────────────┴───────────────┘
   16 bits            8 bits          8 bits

fx_slot  : 0..MAX_SLOTS-1   = which slot in the track's chain
           0xFF             = "chain-level"  (send levels, chain bypass)
fx_param : 0..N             = which parameter within that effect (each effect defines its own)
           well-known: 0x00 = mix (wet/dry), 0x01 = bypass, others effect-specific
```

```rust
#[inline] pub fn fx_slot(param_id: u32)  -> u8 { ((param_id >> 8) & 0xFF) as u8 }
#[inline] pub fn fx_param(param_id: u32) -> u8 {  (param_id       & 0xFF) as u8 }
#[inline] pub fn pack_fx(slot: u8, p: u8) -> u32 { ((slot as u32) << 8) | (p as u32) }
```

The scheduler ([06](../phase-2/06-scheduler-integration.md)) emits `TrackFx` with this packed id;
the engine decodes it and dispatches:

```rust
fn apply_trackfx(track: &mut TrackChain, param_id: u32, value: f32) {
    let (slot, p) = (fx_slot(param_id), fx_param(param_id));
    match slot {
        0xFF => apply_chain_param(track, p, value),         // send levels, chain bypass
        s    => {
            let slot = &mut track.slots[s as usize];
            match p {
                0x00 => slot.mix.set_target(value),         // well-known: wet/dry
                0x01 => slot.xfade.set_target(if value >= 0.5 {1.0} else {0.0}), // bypass
                _    => if let Some(fx) = slot.effect.as_mut() { fx.set_param(p, value); }
            }
        }
    }
}
```

> **`fx_param` reuses Phase 1's well-known param ids where they overlap.** For the
> [filter effect](02-filters-eq.md) we keep `0x01 = cutoff`, `0x02 = q` consistent with the per-voice
> SVF param ids so the same TS automation path (`.lpf(...)` → smoothed cutoff) feels identical.

`ParamSet (0x10)` / `ParamRamp (0x11)` events that target an effect param carry the **same packed id**
in their `param` field and are dispatched the same way — that is what lets Phase 2's automation
(`ParamRamp` for smooth sweeps) reach effect parameters unchanged
([07-parameter-automation.md](07-parameter-automation.md)).

### Global send buses

A **send bus** is a single shared effect (most importantly a [reverb](03-delay-reverb.md), also a
shared delay) that **many tracks feed in parallel**. Each track has a per-bus **send level**; the
bus sums all sends, processes once, and **returns** the result into the master bus. This is how
"multiple tracks into one reverb" (a Phase 3 "done when") works, and it is far cheaper than a reverb
per track.

```rust
pub const MAX_SENDS: usize = 4;   // e.g. 0 = reverb, 1 = delay, 2/3 = user

pub struct SendBus {
    pub effect: Box<dyn Effect>,   // a Reverb or Delay (03)
    pub return_gain: Smoothed<f32>,
    in_l: [f32; MAX_BLOCK],        // pre-allocated accumulators
    in_r: [f32; MAX_BLOCK],
}

pub struct SendRouter { pub buses: [Option<SendBus>; MAX_SENDS] }

impl SendRouter {
    /// Called by each TrackChain after its chain: accumulate (track * sendLevel) into each bus.
    pub fn add_from_track(&mut self, l: &[f32], r: &[f32], send: &mut [Smoothed<f32>; MAX_SENDS]) {
        for (bus, lvl) in self.buses.iter_mut().zip(send.iter_mut()) {
            let Some(bus) = bus.as_mut() else { continue; };
            for i in 0..l.len() {
                let g = lvl.next();
                bus.in_l[i] += g * l[i];
                bus.in_r[i] += g * r[i];
            }
        }
    }
    /// After all tracks: process each bus once and add its return into the master accumulator.
    pub fn process_returns(&mut self, master_l: &mut [f32], master_r: &mut [f32], ctx: &FxCtx) {
        for bus in self.buses.iter_mut().flatten() {
            bus.effect.process(&mut bus.in_l, &mut bus.in_r, ctx);
            for i in 0..master_l.len() {
                let g = bus.return_gain.next();
                master_l[i] += g * bus.in_l[i];
                master_r[i] += g * bus.in_r[i];
                bus.in_l[i] = 0.0; bus.in_r[i] = 0.0;   // clear for next block
            }
        }
    }
}
```

### Where it all runs in the render loop (audio_io.rs)

```
per buffer:
  zero master_l/master_r ; zero each send bus accumulator
  for each track:
      render its voices into track_l/track_r          (P1: voice_alloc)
      track.process(track_l, track_r, ctx, &mut sends) (this file: chain + send taps)
      master_l += track_l ; master_r += track_r        (dry track returns)
  apply_sidechain_ducks(...)                           (06, before sends decay matters)
  sends.process_returns(&mut master_l, &mut master_r, ctx)  (reverb/delay returns)
  master_l *= master_gain ; master_r *= master_gain
  limiter.process(master_l, master_r)                  (P1/07)
  write to cpal
```

> Routing rule of thumb baked into the TS API: **insert effects** (filter, EQ, distortion,
> bitcrush, compressor, modulation) live **in the track's chain** (`.lpf()`, `.crush()`, `.dist()`,
> `.comp()`); **parallel/shared effects** (reverb, delay) are reached via **send levels**
> (`.room(0.3)` raises this track's reverb-send level). `.delay()` can be either (a per-track insert
> delay or the shared delay send) — see [03](03-delay-reverb.md).

### Building & reconfiguring a chain without audio-thread allocation

This is the central real-time-safety problem. Building a fundsp effect graph **allocates**
(boxes, buffers, delay lines). We must never do that in the `cpal` callback. The rule (same one
Phase 1 used for voices and the [event protocol](../../event-protocol.md#control-call-surface-non-real-time)
uses for sample upload): **build off-thread, hand over by an atomic pointer swap.**

1. When a track's effect chain changes (TS re-runs `p3(...)` with a different `.dist().room()`
   stack), the TS library diffs the new chain against the current one and, for any **added or
   retyped slot**, calls a **control-call** (`engine.configureChain(track, ChainConfig)`) — an
   ordinary async napi-rs/wasm function on a **non-audio thread**, exactly the channel the protocol
   reserves for non-real-time setup.
2. On that setup thread, `graph.rs` **allocates and builds** the new `Box<dyn Effect>`s (and any
   send-bus effect) fully — every delay line and look-ahead buffer pre-sized to `MAX_BLOCK` and the
   effect's worst-case tail.
3. The freshly built chain is published into a **double buffer**: the engine holds two `TrackChain`
   configurations per track behind an `AtomicPtr` (or an `ArcSwap`-style atomic load); the setup
   thread writes the new one into the inactive buffer and **atomically flips the active index**.
4. The audio thread, at the top of each block, does a single relaxed atomic load to pick up the
   current chain pointer — **no lock, no alloc**. The flip is seen as a clean swap at a block
   boundary, so the change lands click-free (and, like a pattern hot-swap, effectively at a buffer
   edge).
5. The **old chain is not freed on the audio thread.** Its pointer is handed back to the setup
   thread (via a small lock-free retirement queue) and dropped there, *after* the engine confirms
   the audio thread has moved past it and the old effect's `tail_frames` have elapsed — so a long
   reverb tail isn't cut off mid-decay.

```rust
// One per track. Audio thread only ever LOADS; setup thread STORES + retires.
pub struct DoubleChain {
    active: AtomicPtr<TrackChain>,   // current; loaded once per block on the audio thread
    // retired chains flow to the setup thread for dropping after tail decay
}

impl DoubleChain {
    #[inline] pub fn current(&self) -> &mut TrackChain {
        // SAFETY: only the audio thread dereferences; setup thread never mutates the active one.
        unsafe { &mut *self.active.load(Ordering::Acquire) }
    }
    /// setup thread: publish a fully-built chain, return the displaced one for retirement.
    pub fn publish(&self, built: Box<TrackChain>) -> *mut TrackChain {
        let raw = Box::into_raw(built);
        self.active.swap(raw, Ordering::AcqRel)
    }
}
```

> **Smoothed reuse on swap.** When only a *parameter* changes (not the chain shape), no rebuild
> happens at all — TS just emits a `TrackFx`/`ParamSet`/`ParamRamp` and the existing smoother glides.
> Reconfiguration is reserved for **structural** changes (adding/removing/reordering/retyping a
> slot). This keeps live tweaking on the cheap hot path and the rare structural change on the
> off-thread builder.

### Parameter list & ranges (chain-level)

| Param | `fx_param` | Range | Default | Live-settable | Notes |
| --- | --- | --- | --- | --- | --- |
| wet/dry `mix` | `0x00` | 0 .. 1 | 1.0 | yes (`TrackFx`/signal) | per slot |
| `bypass` | `0x01` | 0 / 1 | 0 | yes | smoothed crossfade, not hard cut |
| send level (per bus) | chain `0xFF`, `0x10+bus` | 0 .. 1 | 0 | yes | e.g. `.room()` raises reverb send |
| `return_gain` (per bus) | global, track 0 | 0 .. 2 | 1.0 | yes | bus → master |

## Interfaces

- **Event protocol** ([event-protocol.md](../../event-protocol.md)):
  - `TrackFx (0x30)` — `param` packs `(fx_slot, fx_param)` per the encoding above; `arg0` = value.
  - `ParamSet (0x10)` / `ParamRamp (0x11)` — may target effect params using the **same packed id**,
    which is what makes effect params automatable ([07](07-parameter-automation.md)).
  - **Control call (non-RT)** — `engine.configureChain(track, ChainConfig)` and
    `engine.configureSend(bus, EffectConfig)` build graphs off-thread and publish via the double
    buffer; never on the audio thread.
- **TS chain API** — insert effects append a slot to the track's chain; `PARAM_REGISTRY`
  ([04](../phase-2/04-sound-note-api.md)) marks each effect key as `kind:"trackFx"` with its
  `(slot, param)` packer, or as a send level. Method names land in
  [02](02-filters-eq.md)–[06](06-dynamics-sidechain.md) (`.lpf`, `.delay`, `.room`, `.crush`,
  `.dist`, `.chorus`, `.comp`, `.sidechain`, …).
- **Per-track chain vs send-bus routing** — chosen per effect in the registry: inserts → chain
  slot; reverb/delay → send level + a shared bus configured once. `.room(x)` is a send level;
  `.dist(x)` is a chain insert. Documented per effect.
- **Master bus** ([07-master-bus-limiter.md](../phase-1/07-master-bus-limiter.md)) — receives the dry
  track returns plus the send returns; unchanged limiter as the final safety net.

## Edge cases & risks

- **Real-time safety (no alloc/lock/JS):** all graphs, delay lines, look-ahead buffers, and the two
  chain buffers are **pre-allocated off-thread**; the audio thread only loads an `AtomicPtr` and runs
  `process`. Verified by the alloc-counter stress check (cross-cutting DoD).
- **Click-free reconfiguration:** structural changes land via an atomic flip at a block boundary;
  parameter changes glide via smoothers; bypass is a crossfade. No hard cuts anywhere.
- **Tail handling on swap:** a removed delay/reverb keeps decaying until its `tail_frames` elapse
  before the retired chain is dropped off-thread, so swaps don't chop tails.
- **Slot/bus exhaustion:** `MAX_SLOTS`/`MAX_SENDS` are fixed; TS validates chain length and errors at
  build time (control plane) rather than overflowing — never a runtime audio-thread concern.
- **Feedback / DC / denormals:** effects with feedback (delay, reverb, flanger) own their own
  stability and denormal guards ([03](03-delay-reverb.md), [05](05-modulation-fx.md)); the global FTZ
  flag set at stream start ([phase-1/02-filters.md](../phase-1/02-filters.md)) covers the rest.
- **Ordering matters musically:** chain order is the *authoring* order of the chained methods
  (`.dist().lpf()` ≠ `.lpf().dist()`); the registry preserves it as slot index so what you write is
  what you hear.
- **CPU:** sends amortize expensive parallel effects (one reverb for N tracks). Per-block dispatch is
  one virtual call per non-empty slot; the inner loops are fundsp graphs.

## Acceptance criteria

- [ ] A track with `.dist().lpf()` builds a 2-slot chain processed in that order; reversing the
      methods reverses audible order. **Test:** golden render of both orderings differs as expected.
- [ ] Toggling a slot's `bypass` live produces **no click** (smoothed crossfade). **Test:** A/B a
      bypass toggle mid-tone, assert no step transient above −60 dB.
- [ ] Wet/dry `mix` at 0 is bit-identical to the dry input through that slot; at 1 it is fully wet.
      **Test:** null test at `mix=0`; spectrum check at `mix=1`.
- [ ] Two tracks with `.room(0.5)` both feed **one** reverb send bus and the bus is processed
      **once** per block; muting one track changes the reverb input. **Test:** instrument the send
      accumulator, assert a single reverb instance and additive input.
- [ ] A `TrackFx` with packed id `pack_fx(slot=1, param=cutoff)` reaches **only** slot 1's effect.
      **Test:** unit-decode `fx_slot`/`fx_param`; assert dispatch hits the right slot/effect.
- [ ] Reconfiguring a chain (adding a slot) happens via the **off-thread builder + atomic flip**;
      a removed reverb's tail decays before its chain is dropped. **Test:** alloc-counter shows zero
      audio-thread allocation across a reconfiguration; tail-continuity render shows no chopped tail.
- [ ] **No underruns** with a full 8-slot chain on several tracks plus two send buses. **Test:**
      `stats()` clean during the Phase 3 example.
