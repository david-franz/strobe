//! The effects rack framework (Phase 3): the `Effect` trait every effect implements, the
//! per-track ordered insert chain, global send buses, the cross-track sidechain bus, and the
//! `TrackFx` `(fx_slot, fx_param)` id encoding.
//!
//! Pragmatic real-time-safe design: every effect, delay line, and buffer is **pre-allocated**
//! at engine start and driven purely by smoothed parameters — no audio-thread allocation, no
//! dynamic structural reconfiguration (the doc's off-thread chain rebuild is deferred). Each
//! track owns a fixed set of insert slots in a fixed order; a slot is active once its `mix`
//! parameter is raised above 0.

pub mod bitcrush;
pub mod chain;
pub mod delay;
pub mod distortion;
pub mod dynamics;
pub mod filter_eq;
pub mod modulation;
pub mod reverb;
pub mod sends;
pub mod sidechain;
pub mod smoothing;

pub use chain::TrackEffects;
pub use sends::{SendBus, SendRouter};
pub use sidechain::{EnvelopeFollower, SidechainBus};
pub use smoothing::Smoothed;

/// Tracks p1..pN (id 0 reserved for master/global).
pub const MAX_TRACKS: usize = 8;
/// Insert slots per track chain (one per effect kind, see the fixed layout below).
pub const MAX_SLOTS: usize = 9;
/// Shared send buses (0 = reverb, 1 = delay).
pub const MAX_SENDS: usize = 2;
/// Largest render block we pre-allocate effect scratch for.
pub const MAX_BLOCK: usize = 4096;

// ── Fixed insert-slot layout (processing order) ────────────────────────────────
// One slot per effect kind, a sensible fixed order (NOT authoring order). Each track
// pre-allocates all of these; a slot only processes once it has been activated (a param
// raised its mix above 0), so unused effects cost nothing.
pub const SLOT_FILTER: u8 = 0;
pub const SLOT_EQ: u8 = 1;
pub const SLOT_DIST: u8 = 2;
pub const SLOT_CRUSH: u8 = 3;
pub const SLOT_CHORUS: u8 = 4;
pub const SLOT_FLANGER: u8 = 5;
pub const SLOT_PHASER: u8 = 6;
pub const SLOT_COMP: u8 = 7;
pub const SLOT_DUCK: u8 = 8;

// ── Fixed send-bus layout ──────────────────────────────────────────────────────
pub const SEND_REVERB: usize = 0;
pub const SEND_DELAY: usize = 1;
/// CHAIN_SLOT param base for send levels: send `bus` is addressed as `0x10 + bus`.
pub const CHAIN_SEND_BASE: u8 = 0x10;

/// Build a track's insert chain with every effect kind pre-allocated and inactive. Later
/// features populate more slots; feature 02 installs the filter and EQ.
pub fn build_track_chain(sr: f32) -> TrackEffects {
    let mut fx = TrackEffects::new(sr);
    fx.install(SLOT_FILTER as usize, Box::new(filter_eq::PerfFilter::new(sr)));
    fx.install(SLOT_EQ as usize, Box::new(filter_eq::Eq::new(sr)));
    fx.install(SLOT_DIST as usize, Box::new(distortion::Distort::new(sr)));
    fx.install(SLOT_CRUSH as usize, Box::new(bitcrush::Bitcrush::new(sr)));
    fx.install(SLOT_CHORUS as usize, Box::new(modulation::Chorus::new(sr)));
    fx.install(SLOT_FLANGER as usize, Box::new(modulation::Flanger::new(sr)));
    fx.install(SLOT_PHASER as usize, Box::new(modulation::Phaser::new(sr)));
    fx.install(SLOT_COMP as usize, Box::new(dynamics::Compressor::new(sr)));
    fx.install(SLOT_DUCK as usize, Box::new(dynamics::SidechainDuck::new(sr)));
    fx
}

/// Build the shared send router: bus 0 = reverb, bus 1 = delay (feature 03).
pub fn build_send_router(sr: f32) -> SendRouter {
    let mut router = SendRouter::new();
    router.set_bus(SEND_REVERB, SendBus::new(Box::new(reverb::Reverb::new(sr)), sr));
    router.set_bus(SEND_DELAY, SendBus::new(Box::new(delay::Delay::new(sr)), sr));
    router
}

/// Read-only per-block context handed to every effect.
pub struct FxCtx<'a> {
    pub sample_rate: f32,
    pub cps: f32,
    pub sidechain: &'a SidechainBus,
}

/// One stereo, block-based, real-time-safe effect. No method may allocate, lock, or block.
pub trait Effect: Send {
    /// Process one block in place (input on entry, output on exit), fully wet — the chain
    /// applies wet/dry.
    fn process(&mut self, l: &mut [f32], r: &mut [f32], ctx: &FxCtx);
    /// Set one parameter (already decoded). Pushes a new smoother target; never reallocs.
    fn set_param(&mut self, fx_param: u8, value: f32);
    /// Begin a linear ParamRamp on one parameter.
    fn ramp_param(&mut self, fx_param: u8, target: f32, ramp_frames: u32);
    /// Clear internal state (delay lines, filter memory, envelopes).
    fn reset(&mut self);
    /// Tail length in frames (delay/reverb); 0 for memoryless effects.
    fn tail_frames(&self) -> u32 {
        0
    }
}

// ── TrackFx param id encoding: bits [15:8] = fx_slot, [7:0] = fx_param ─────────

/// Slot 0xFF addresses chain-level params (send levels, chain bypass).
pub const CHAIN_SLOT: u8 = 0xFF;
/// Well-known per-slot param ids.
pub const FX_MIX: u8 = 0x00;
pub const FX_BYPASS: u8 = 0x01;

#[inline]
pub fn fx_slot(param_id: u32) -> u8 {
    ((param_id >> 8) & 0xFF) as u8
}
#[inline]
pub fn fx_param(param_id: u32) -> u8 {
    (param_id & 0xFF) as u8
}
#[inline]
pub fn pack_fx(slot: u8, p: u8) -> u32 {
    ((slot as u32) << 8) | (p as u32)
}

#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}
#[inline]
pub fn lin_to_db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fx_id_pack_round_trips() {
        let id = pack_fx(3, 0x12);
        assert_eq!(fx_slot(id), 3);
        assert_eq!(fx_param(id), 0x12);
        let chain = pack_fx(CHAIN_SLOT, 0x10);
        assert_eq!(fx_slot(chain), CHAIN_SLOT);
        assert_eq!(fx_param(chain), 0x10);
    }
}
