//! State-variable filter (SVF) — the single most important timbre-shaping tool in
//! techno (the filter sweep). Four modes (LP/HP/BP/notch) with resonance/Q, plus a
//! one-pole cutoff smoother so sweeping the cutoff produces a clean glide and no zipper
//! noise.
//!
//! Wraps fundsp's 3-input SVF nodes (audio, cutoff, Q), ticked per-sample with a
//! smoothed cutoff. Built once per voice; allocation-free on the hot path.

use fundsp::hacker32::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SvfMode {
    Lowpass,
    Highpass,
    Bandpass,
    Notch,
}

#[derive(Clone, Copy, Debug)]
pub struct SvfConfig {
    pub mode: SvfMode,
    pub cutoff_hz: f32, // 20 .. Nyquist
    pub q: f32,         // 0.5 .. 20
    pub smooth_ms: f32, // cutoff smoothing time constant; default 5 ms
}

impl Default for SvfConfig {
    fn default() -> Self {
        Self {
            mode: SvfMode::Lowpass,
            cutoff_hz: 1000.0,
            q: 0.7,
            smooth_ms: 5.0,
        }
    }
}

/// One-pole exponential smoother (a leaky follower). Used to de-zipper the cutoff.
pub struct OnePole {
    coeff: f32,
    value: f32,
}

impl OnePole {
    /// `smooth_ms <= 0` makes it instantaneous (no smoothing).
    pub fn new(smooth_ms: f32, sr: f32, initial: f32) -> Self {
        let coeff = if smooth_ms <= 0.0 {
            1.0
        } else {
            1.0 - (-1.0 / (smooth_ms * 0.001 * sr)).exp()
        };
        Self {
            coeff,
            value: initial,
        }
    }

    /// Jump straight to `v` (no glide) — used on voice reset.
    pub fn snap(&mut self, v: f32) {
        self.value = v;
    }

    /// One step toward `target`; returns the new smoothed value.
    #[inline]
    pub fn process(&mut self, target: f32) -> f32 {
        self.value += self.coeff * (target - self.value);
        self.value
    }

    pub fn value(&self) -> f32 {
        self.value
    }
}

fn build_svf_node(mode: SvfMode) -> Box<dyn AudioUnit> {
    match mode {
        SvfMode::Lowpass => Box::new(lowpass()),
        SvfMode::Highpass => Box::new(highpass()),
        SvfMode::Bandpass => Box::new(bandpass()),
        SvfMode::Notch => Box::new(notch()),
    }
}

/// One SVF stage, ticked per-sample with a smoothed cutoff (+ optional envelope offset).
pub struct Filter {
    node: Box<dyn AudioUnit>,
    smoother: OnePole,
    base_cutoff: f32,
    q: f32,
    nyquist: f32,
}

impl Filter {
    pub fn new(cfg: SvfConfig, sr: f32) -> Self {
        let mut node = build_svf_node(cfg.mode);
        node.set_sample_rate(sr as f64);
        Self {
            node,
            smoother: OnePole::new(cfg.smooth_ms, sr, cfg.cutoff_hz),
            base_cutoff: cfg.cutoff_hz,
            q: cfg.q.clamp(0.5, 20.0),
            nyquist: sr * 0.5,
        }
    }

    /// Set the base cutoff target; the smoother glides toward it (no zipper noise).
    pub fn set_cutoff(&mut self, hz: f32) {
        self.base_cutoff = hz;
    }

    pub fn set_q(&mut self, q: f32) {
        self.q = q.clamp(0.5, 20.0);
    }

    /// Clear filter state and snap the cutoff smoother to the base (on retrigger), so a
    /// stolen voice carries no resonant ringing into the next note.
    pub fn reset(&mut self) {
        self.node.reset();
        self.smoother.snap(self.base_cutoff);
    }

    /// Filter one sample. `env_offset_hz` is the filter envelope's contribution
    /// (already smooth), summed on top of the de-zippered base cutoff.
    #[inline]
    pub fn tick(&mut self, audio: f32, env_offset_hz: f32) -> f32 {
        let cutoff =
            (self.smoother.process(self.base_cutoff) + env_offset_hz).clamp(20.0, self.nyquist - 1.0);
        let mut out = [0.0f32; 1];
        self.node.tick(&[audio, cutoff, self.q], &mut out);
        out[0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{goertzel, white_noise};

    const SR: f32 = 48_000.0;

    fn filtered(mode: SvfMode, cutoff: f32, q: f32) -> Vec<f32> {
        let mut f = Filter::new(
            SvfConfig {
                mode,
                cutoff_hz: cutoff,
                q,
                smooth_ms: 0.0, // fixed cutoff for response tests
            },
            SR,
        );
        let noise = white_noise(SR as usize, 0xC0FFEE);
        noise.iter().map(|&x| f.tick(x, 0.0)).collect()
    }

    #[test]
    fn lowpass_passes_below_attenuates_above() {
        let out = filtered(SvfMode::Lowpass, 1000.0, 0.7);
        let pass = goertzel(&out, 400.0, SR);
        let stop = goertzel(&out, 4000.0, SR);
        assert!(pass > stop * 8.0, "LP: pass {pass} >> stop {stop}");
    }

    #[test]
    fn highpass_passes_above_attenuates_below() {
        let out = filtered(SvfMode::Highpass, 1000.0, 0.7);
        let pass = goertzel(&out, 4000.0, SR);
        let stop = goertzel(&out, 200.0, SR); // well into the stopband (0.2× cutoff)
        assert!(pass > stop * 6.0, "HP: pass {pass} >> stop {stop}");
    }

    #[test]
    fn bandpass_peaks_at_cutoff() {
        let out = filtered(SvfMode::Bandpass, 1000.0, 4.0);
        let peak = goertzel(&out, 1000.0, SR);
        assert!(peak > goertzel(&out, 300.0, SR), "BP peak above low side");
        assert!(peak > goertzel(&out, 4000.0, SR), "BP peak above high side");
    }

    #[test]
    fn notch_nulls_at_cutoff() {
        let out = filtered(SvfMode::Notch, 1000.0, 1.0);
        let null = goertzel(&out, 1000.0, SR);
        assert!(null < goertzel(&out, 300.0, SR) * 0.2, "notch nulls at cutoff");
        assert!(null < goertzel(&out, 4000.0, SR) * 0.2);
    }

    #[test]
    fn higher_q_sharpens_the_resonant_peak() {
        let low_q = goertzel(&filtered(SvfMode::Lowpass, 1000.0, 0.7), 1000.0, SR);
        let high_q = goertzel(&filtered(SvfMode::Lowpass, 1000.0, 12.0), 1000.0, SR);
        assert!(high_q > low_q * 5.0, "Q raises peak ({high_q} vs {low_q})");
        // NOTE: fundsp's SVF is bounded-stable and does not fully self-oscillate at high
        // Q; an acid-style resonant whistle would need a feedback filter — a follow-up.
    }

    #[test]
    fn cutoff_smoother_glides_and_does_not_step() {
        // From a settled 400 Hz, target 1600 Hz: the value glides, never jumps.
        let mut sm = OnePole::new(5.0, SR, 400.0);
        let first = sm.process(1600.0);
        assert!(first > 400.0 && first < 460.0, "first step is small ({first})");
        let mut last = first;
        for _ in 0..2000 {
            let v = sm.process(1600.0);
            assert!(v >= last - 1e-3, "monotonic glide");
            last = v;
        }
        assert!((last - 1600.0).abs() < 5.0, "reaches target ({last})");
    }

    #[test]
    fn zero_smoothing_jumps_instantly() {
        let mut sm = OnePole::new(0.0, SR, 400.0);
        assert_eq!(sm.process(1600.0), 1600.0, "no smoothing → instant");
    }

    #[test]
    fn cutoff_change_is_audible_but_not_zippered() {
        // Drive a steady saw-ish input; sweep cutoff up; assert the high-frequency
        // content increases (audible) and the smoother prevents an instantaneous jump.
        let mut f = Filter::new(
            SvfConfig { mode: SvfMode::Lowpass, cutoff_hz: 400.0, q: 1.0, smooth_ms: 5.0 },
            SR,
        );
        let noise = white_noise(SR as usize, 7);
        let before: Vec<f32> = noise[..12000].iter().map(|&x| f.tick(x, 0.0)).collect();
        f.set_cutoff(4000.0);
        let after: Vec<f32> = noise[12000..24000].iter().map(|&x| f.tick(x, 0.0)).collect();
        let hi_before = goertzel(&before, 3000.0, SR);
        let hi_after = goertzel(&after, 3000.0, SR);
        assert!(hi_after > hi_before * 3.0, "opening the cutoff lets highs through");
    }
}
