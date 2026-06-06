//! Instrument configuration — the `defineInstrument()` payload the TS layer sends over
//! the (non-real-time) control channel. The engine builds a voice's DSP graph from this
//! once, at pool-creation time; `NoteOn` only retriggers it (feature 06).

use crate::envelope::AdsrConfig;
use crate::filter::SvfConfig;
use crate::voices::oscillator::OscConfig;

/// A subtractive synth instrument: oscillator → filter → amp, with amp + filter
/// envelopes, gain and pan.
#[derive(Clone, Copy, Debug)]
pub struct InstrumentConfig {
    pub osc: OscConfig,
    pub svf: SvfConfig,
    /// Filter-envelope depth in Hz (added to the cutoff over the envelope).
    pub env_amount_hz: f32,
    pub amp_env: AdsrConfig,
    pub filter_env: AdsrConfig,
    pub gain: f32, // 0..1
    pub pan: f32,  // 0..1, 0.5 = center
}

impl Default for InstrumentConfig {
    fn default() -> Self {
        use crate::envelope::EnvMode;
        Self {
            osc: OscConfig::default(),
            svf: SvfConfig::default(),
            env_amount_hz: 0.0,
            amp_env: AdsrConfig::default(),
            filter_env: AdsrConfig {
                sustain: 0.0,
                mode: EnvMode::Gated,
                ..AdsrConfig::default()
            },
            gain: 0.8,
            pan: 0.5,
        }
    }
}
