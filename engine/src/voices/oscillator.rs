//! The oscillator bank: the "base frequency" every voice picks one of. sine / saw /
//! square (PWM) / triangle / noise via fundsp's band-limited generators (so high notes
//! don't alias), plus a simple interpolating wavetable reader.
//!
//! Each oscillator is a fundsp `AudioUnit` built once and driven per-sample with the
//! note frequency (no allocation per tick). The voice (feature 04) and drum kit
//! (feature 05) own an `Oscillator`; it is never instantiated on its own.

use std::sync::Arc;

use fundsp::hacker32::*;

/// Selected per-instrument by the TS layer (`.saw()`, `.square()`, …).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OscKind {
    Sine,
    Saw,
    Square, // pulse width in OscConfig.pulse_width (0.0..1.0, 0.5 = square)
    Triangle,
    Noise, // white noise; frequency-independent
    Wavetable(u32),
}

#[derive(Clone, Copy, Debug)]
pub struct OscConfig {
    pub kind: OscKind,
    pub pulse_width: f32, // Square only; 0.01..0.99, default 0.5
    pub phase_reset: bool, // reset phase to 0 on retrigger? default true
}

impl Default for OscConfig {
    fn default() -> Self {
        Self {
            kind: OscKind::Sine,
            pulse_width: 0.5,
            phase_reset: true,
        }
    }
}

/// Pre-loaded single-cycle wavetables, uploaded off the audio thread. The audio thread
/// only `Arc`-clones a table (no audio-data allocation) and reads it.
#[derive(Default)]
pub struct WavetableBank {
    tables: Vec<Arc<Vec<f32>>>,
}

impl WavetableBank {
    pub fn new() -> Self {
        Self::default()
    }

    /// Upload a single-cycle table; returns its id.
    pub fn add(&mut self, table: Vec<f32>) -> u32 {
        self.tables.push(Arc::new(table));
        (self.tables.len() - 1) as u32
    }

    pub fn get(&self, id: u32) -> Option<Arc<Vec<f32>>> {
        self.tables.get(id as usize).cloned()
    }
}

enum Backend {
    /// 1-input (frequency) fundsp generator: sine / saw / triangle.
    Pitched(Box<dyn AudioUnit>),
    /// fundsp `pulse()` — 2 inputs (frequency, width) — used for square/PWM.
    Pulse(Box<dyn AudioUnit>),
    /// fundsp `noise()` — 0 inputs.
    Noise(Box<dyn AudioUnit>),
    /// Simple interpolating single-cycle wavetable reader (own phase accumulator).
    /// Band-limiting at very high notes (mip-mapping) is a documented follow-up; not
    /// needed by the Phase 1 synth/drum "done when" criteria.
    Wavetable { table: Arc<Vec<f32>>, phase: f32 },
}

/// One oscillator instance, built once and ticked per-sample.
pub struct Oscillator {
    backend: Backend,
    pulse_width: f32,
    phase_reset: bool,
    sr: f32,
    nyquist: f32,
}

impl Oscillator {
    /// Build the oscillator for `cfg` at sample rate `sr`. Allocation happens here (at
    /// pool-creation time), never per trigger.
    pub fn new(cfg: OscConfig, sr: f32, bank: &WavetableBank) -> Self {
        let mk = |mut u: Box<dyn AudioUnit>| {
            u.set_sample_rate(sr as f64);
            u
        };
        let backend = match cfg.kind {
            OscKind::Sine => Backend::Pitched(mk(Box::new(sine()))),
            OscKind::Saw => Backend::Pitched(mk(Box::new(saw()))),
            OscKind::Triangle => Backend::Pitched(mk(Box::new(triangle()))),
            OscKind::Square => Backend::Pulse(mk(Box::new(pulse()))),
            OscKind::Noise => Backend::Noise(mk(Box::new(noise()))),
            OscKind::Wavetable(id) => Backend::Wavetable {
                table: bank.get(id).unwrap_or_else(|| Arc::new(vec![0.0])),
                phase: 0.0,
            },
        };
        Self {
            backend,
            pulse_width: cfg.pulse_width.clamp(0.01, 0.99),
            phase_reset: cfg.phase_reset,
            sr,
            nyquist: sr * 0.5,
        }
    }

    /// Reset phase on retrigger (when `phase_reset` is set), for a consistent attack.
    pub fn reset(&mut self) {
        if !self.phase_reset {
            return;
        }
        match &mut self.backend {
            Backend::Pitched(u) | Backend::Pulse(u) | Backend::Noise(u) => u.reset(),
            Backend::Wavetable { phase, .. } => *phase = 0.0,
        }
    }

    /// Live-set the square pulse width.
    pub fn set_pulse_width(&mut self, pw: f32) {
        self.pulse_width = pw.clamp(0.01, 0.99);
    }

    /// One sample at `freq_hz` (clamped below Nyquist for the pitched kinds).
    #[inline]
    pub fn tick(&mut self, freq_hz: f32) -> f32 {
        let f = freq_hz.clamp(0.0, self.nyquist - 1.0);
        let mut out = [0.0f32; 1];
        match &mut self.backend {
            Backend::Pitched(u) => {
                u.tick(&[f], &mut out);
                out[0]
            }
            Backend::Pulse(u) => {
                u.tick(&[f, self.pulse_width], &mut out);
                out[0]
            }
            Backend::Noise(u) => {
                u.tick(&[], &mut out);
                out[0]
            }
            Backend::Wavetable { table, phase } => {
                let len = table.len();
                let pos = *phase * len as f32;
                let i0 = pos as usize % len;
                let i1 = (i0 + 1) % len;
                let frac = pos - pos.floor();
                let s = table[i0] * (1.0 - frac) + table[i1] * frac;
                *phase += f / self.sr;
                if *phase >= 1.0 {
                    *phase -= phase.floor();
                }
                s
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{goertzel, rms, zero_crossing_freq};

    const SR: f32 = 48_000.0;

    fn osc(kind: OscKind) -> Oscillator {
        Oscillator::new(
            OscConfig {
                kind,
                ..Default::default()
            },
            SR,
            &WavetableBank::new(),
        )
    }

    fn render(o: &mut Oscillator, freq: f32, n: usize) -> Vec<f32> {
        (0..n).map(|_| o.tick(freq)).collect()
    }

    #[test]
    fn sine_is_a_single_partial() {
        let mut o = osc(OscKind::Sine);
        let sig = render(&mut o, 440.0, SR as usize);
        let f0 = goertzel(&sig, 440.0, SR);
        let h2 = goertzel(&sig, 880.0, SR);
        assert!(f0 > 0.4, "fundamental present ({f0})");
        assert!(h2 < f0 * 0.02, "no 2nd harmonic ({h2} vs {f0})");
    }

    #[test]
    fn saw_has_all_harmonics_falling_as_one_over_n() {
        let mut o = osc(OscKind::Saw);
        let sig = render(&mut o, 220.0, SR as usize);
        let h1 = goertzel(&sig, 220.0, SR);
        let h2 = goertzel(&sig, 440.0, SR);
        let h3 = goertzel(&sig, 660.0, SR);
        assert!(h1 > 0.1 && h2 > 0.05 && h3 > 0.03, "harmonics present");
        // ~1/n falloff: h2 ≈ h1/2, h3 ≈ h1/3 (loose tolerance).
        assert!((h2 / h1 - 0.5).abs() < 0.2, "h2/h1 ≈ 0.5 ({})", h2 / h1);
        assert!((h3 / h1 - 0.3333).abs() < 0.2, "h3/h1 ≈ 1/3 ({})", h3 / h1);
    }

    #[test]
    fn square_has_odd_harmonics_only() {
        let mut o = osc(OscKind::Square);
        let sig = render(&mut o, 220.0, SR as usize);
        let h1 = goertzel(&sig, 220.0, SR);
        let h2 = goertzel(&sig, 440.0, SR);
        let h3 = goertzel(&sig, 660.0, SR);
        assert!(h1 > 0.1 && h3 > 0.03, "odd harmonics present");
        assert!(h2 < h1 * 0.05, "even harmonic suppressed ({h2} vs {h1})");
    }

    #[test]
    fn triangle_odd_harmonics_fall_as_one_over_n_squared() {
        let mut o = osc(OscKind::Triangle);
        let sig = render(&mut o, 220.0, SR as usize);
        let h1 = goertzel(&sig, 220.0, SR);
        let h3 = goertzel(&sig, 660.0, SR);
        // 3rd harmonic of a triangle ≈ 1/9 of the fundamental.
        assert!(h3 < h1 * 0.25 && h3 > h1 * 0.02, "h3 ≈ h1/9 ({h3} vs {h1})");
    }

    #[test]
    fn noise_is_broadband_with_no_dominant_partial() {
        let mut o = osc(OscKind::Noise);
        let sig = render(&mut o, 0.0, SR as usize);
        assert!(rms(&sig) > 0.1, "noise has energy");
        // No single bin dominates; a few bins all carry comparable, small energy.
        for f in [500.0, 2000.0, 8000.0] {
            let m = goertzel(&sig, f, SR);
            assert!(m < 0.1, "bin {f} not dominant ({m})");
        }
    }

    #[test]
    fn pitch_follows_note_frequency() {
        let mut o = osc(OscKind::Sine);
        let sig = render(&mut o, 440.0, SR as usize);
        // The fundamental sits at 440, not at neighbouring frequencies.
        let at440 = goertzel(&sig, 440.0, SR);
        assert!(at440 > goertzel(&sig, 420.0, SR) * 10.0);
        assert!(at440 > goertzel(&sig, 460.0, SR) * 10.0);
        // Coarse zero-crossing estimate corroborates (±2 Hz over a 1 s window).
        let est = zero_crossing_freq(&sig, SR);
        assert!((est - 440.0).abs() < 2.0, "pitch ≈ 440 Hz ({est})");
    }

    #[test]
    fn saw_is_band_limited_no_inharmonic_aliasing() {
        let mut o = osc(OscKind::Saw);
        let sig = render(&mut o, 4000.0, SR as usize); // harmonics: 4k,8k,12k,16k,20k
        let f0 = goertzel(&sig, 4000.0, SR);
        // Inharmonic frequencies should carry essentially no energy (band-limited).
        for f in [5000.0, 7000.0, 9000.0] {
            let alias = goertzel(&sig, f, SR);
            assert!(alias < f0 * 0.01, "no alias at {f} ({alias} vs {f0})");
        }
    }

    #[test]
    fn pulse_width_changes_harmonic_content() {
        // A 0.5 (square) duty cycle nulls even harmonics; an asymmetric duty restores them.
        let mut sq = Oscillator::new(
            OscConfig { kind: OscKind::Square, pulse_width: 0.5, phase_reset: true },
            SR,
            &WavetableBank::new(),
        );
        let mut narrow = Oscillator::new(
            OscConfig { kind: OscKind::Square, pulse_width: 0.1, phase_reset: true },
            SR,
            &WavetableBank::new(),
        );
        let even_sq = goertzel(&render(&mut sq, 220.0, SR as usize), 440.0, SR);
        let even_nw = goertzel(&render(&mut narrow, 220.0, SR as usize), 440.0, SR);
        assert!(even_nw > even_sq * 3.0, "narrow pulse has more even-harmonic energy");
    }

    #[test]
    fn phase_reset_makes_retriggers_align() {
        let mut o = osc(OscKind::Saw);
        let a = render(&mut o, 220.0, 64);
        o.reset();
        let b = render(&mut o, 220.0, 64);
        for (x, y) in a.iter().zip(b.iter()) {
            assert!((x - y).abs() < 1e-6, "reset retrigger aligns");
        }
    }

    #[test]
    fn wavetable_reads_uploaded_table() {
        let mut bank = WavetableBank::new();
        // A single-cycle sine table.
        let table: Vec<f32> = (0..1024)
            .map(|i| (2.0 * std::f32::consts::PI * i as f32 / 1024.0).sin())
            .collect();
        let id = bank.add(table);
        let mut o = Oscillator::new(
            OscConfig { kind: OscKind::Wavetable(id), ..Default::default() },
            SR,
            &bank,
        );
        let sig = render(&mut o, 440.0, SR as usize);
        let f0 = goertzel(&sig, 440.0, SR);
        let h2 = goertzel(&sig, 880.0, SR);
        assert!(f0 > 0.4, "table fundamental present ({f0})");
        assert!(h2 < f0 * 0.05, "single-cycle sine table → ~one partial");
    }
}
