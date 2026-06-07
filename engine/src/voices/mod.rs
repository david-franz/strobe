//! Synthesized voices. Phase 0 had a single monophonic kick; Phase 1 adds the
//! oscillator bank, the configurable synth voice, the synthesized drum kit, and
//! polyphonic voice allocation.

pub mod drum;
pub mod oscillator;
pub mod sampler;
pub mod synth;

pub use oscillator::{OscConfig, OscKind, Oscillator, WavetableBank};
pub use synth::SynthVoice;

/// Live-settable parameter ids (the `ParamSet` namespace). Values match the protocol's
/// `param` field; master/global params live on track 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ParamId {
    Cutoff = 0,
    Resonance = 1,
    AmpAttack = 2,
    AmpDecay = 3,
    AmpSustain = 4,
    AmpRelease = 5,
    FiltAttack = 6,
    FiltDecay = 7,
    FiltSustain = 8,
    FiltRelease = 9,
    FiltEnvAmount = 10,
    PulseWidth = 11,
    Gain = 12,
    Pan = 13,
}

impl ParamId {
    pub fn from_u32(v: u32) -> Option<ParamId> {
        use ParamId::*;
        Some(match v {
            0 => Cutoff,
            1 => Resonance,
            2 => AmpAttack,
            3 => AmpDecay,
            4 => AmpSustain,
            5 => AmpRelease,
            6 => FiltAttack,
            7 => FiltDecay,
            8 => FiltSustain,
            9 => FiltRelease,
            10 => FiltEnvAmount,
            11 => PulseWidth,
            12 => Gain,
            13 => Pan,
            _ => return None,
        })
    }
}

/// A playable voice — a synth or a drum. The allocator (feature 06) owns a pool of
/// these and drives them from `NoteOn`/`NoteOff`/`ParamSet`.
pub trait Voice: Send {
    /// Start a note (from `NoteOn`): freq (Hz), velocity (0..1), one-shot duration in
    /// frames (0 = gated/held).
    fn trigger(&mut self, freq: f32, vel: f32, dur_frames: u64);
    /// Begin release (from `NoteOff`); a no-op for one-shot drums.
    fn release(&mut self);
    /// True once the amp tail has ended and the slot can be reclaimed.
    fn is_finished(&self) -> bool;
    /// Current amp level — used by the allocator's "quietest voice" stealing.
    fn current_level(&self) -> f32;
    /// Render `out_l.len()` frames, **adding** stereo output into the buffers.
    fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]);
    /// Live-edit a parameter (from `ParamSet`).
    fn set_param(&mut self, param: ParamId, value: f32);
}

/// Constant-power pan: `pan` in 0..1 (0 = hard left, 0.5 = center, 1 = hard right).
#[inline]
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let angle = pan.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
    (angle.cos(), angle.sin())
}
