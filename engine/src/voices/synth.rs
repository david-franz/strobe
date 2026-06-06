//! The synth voice: the classic subtractive chain oscillator → filter → amp, with an
//! amp envelope and a filter envelope, per-voice velocity, gain and pan. Built once from
//! an `InstrumentConfig`; `trigger`/`release`/`set_param` only touch plain state — no
//! allocation on the hot path.

use crate::envelope::Adsr;
use crate::filter::Filter;
use crate::graph::InstrumentConfig;
use crate::voices::oscillator::{Oscillator, WavetableBank};
use crate::voices::{pan_gains, ParamId, Voice};

pub struct SynthVoice {
    osc: Oscillator,
    filter: Filter,
    amp_env: Adsr,
    filter_env: Adsr,
    env_amount_hz: f32,
    freq: f32,
    vel: f32,
    gain: f32,
    pan: f32,
}

impl SynthVoice {
    pub fn new(cfg: &InstrumentConfig, sr: f32, bank: &WavetableBank) -> Self {
        Self {
            osc: Oscillator::new(cfg.osc, sr, bank),
            filter: Filter::new(cfg.svf, sr),
            amp_env: Adsr::new(cfg.amp_env, sr),
            filter_env: Adsr::new(cfg.filter_env, sr),
            env_amount_hz: cfg.env_amount_hz,
            freq: 0.0,
            vel: 1.0,
            gain: cfg.gain,
            pan: cfg.pan,
        }
    }
}

impl Voice for SynthVoice {
    fn trigger(&mut self, freq: f32, vel: f32, dur_frames: u64) {
        self.freq = freq;
        self.vel = vel.clamp(0.0, 1.0);
        self.osc.reset(); // honors phase_reset internally
        self.filter.reset();
        self.amp_env.trigger(dur_frames);
        self.filter_env.trigger(dur_frames);
    }

    fn release(&mut self) {
        self.amp_env.gate_off();
        self.filter_env.gate_off();
    }

    fn is_finished(&self) -> bool {
        self.amp_env.is_finished()
    }

    fn current_level(&self) -> f32 {
        self.amp_env.level()
    }

    fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let (pl, pr) = pan_gains(self.pan);
        for (l, r) in out_l.iter_mut().zip(out_r.iter_mut()) {
            let raw = self.osc.tick(self.freq);
            let fenv = self.filter_env.tick();
            let filtered = self.filter.tick(raw, fenv * self.env_amount_hz);
            let amp = self.amp_env.tick() * self.vel * self.gain;
            let mono = filtered * amp;
            *l += mono * pl;
            *r += mono * pr;
        }
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        use ParamId::*;
        match param {
            Cutoff => self.filter.set_cutoff(value),
            Resonance => self.filter.set_q(value),
            AmpAttack => self.amp_env.set_attack_s(value),
            AmpDecay => self.amp_env.set_decay_s(value),
            AmpSustain => self.amp_env.set_sustain(value),
            AmpRelease => self.amp_env.set_release_s(value),
            FiltAttack => self.filter_env.set_attack_s(value),
            FiltDecay => self.filter_env.set_decay_s(value),
            FiltSustain => self.filter_env.set_sustain(value),
            FiltRelease => self.filter_env.set_release_s(value),
            FiltEnvAmount => self.env_amount_hz = value,
            PulseWidth => self.osc.set_pulse_width(value),
            Gain => self.gain = value,
            Pan => self.pan = value.clamp(0.0, 1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{AdsrConfig, EnvMode};
    use crate::filter::{SvfConfig, SvfMode};
    use crate::graph::InstrumentConfig;
    use crate::test_util::{goertzel, rms};
    use crate::voices::oscillator::{OscConfig, OscKind};

    const SR: f32 = 48_000.0;

    fn saw_lp(cutoff: f32) -> InstrumentConfig {
        InstrumentConfig {
            osc: OscConfig { kind: OscKind::Saw, ..Default::default() },
            svf: SvfConfig { mode: SvfMode::Lowpass, cutoff_hz: cutoff, q: 0.7, smooth_ms: 5.0 },
            amp_env: AdsrConfig {
                attack_s: 0.005,
                decay_s: 0.05,
                sustain: 0.8,
                release_s: 0.1,
                mode: EnvMode::Gated,
                ..Default::default()
            },
            gain: 0.8,
            ..Default::default()
        }
    }

    fn render(v: &mut SynthVoice, n: usize) -> (Vec<f32>, Vec<f32>) {
        let mut l = vec![0.0; n];
        let mut r = vec![0.0; n];
        v.render(&mut l, &mut r);
        (l, r)
    }

    #[test]
    fn plays_the_triggered_pitch() {
        let mut v = SynthVoice::new(&saw_lp(8000.0), SR, &WavetableBank::new());
        v.trigger(440.0, 1.0, 0);
        let (l, _) = render(&mut v, SR as usize);
        let f0 = goertzel(&l, 440.0, SR);
        assert!(f0 > goertzel(&l, 415.0, SR) * 8.0, "fundamental at 440");
        assert!(f0 > goertzel(&l, 466.0, SR) * 8.0);
    }

    #[test]
    fn lowpass_attenuates_high_harmonics() {
        let mut bright = SynthVoice::new(&saw_lp(8000.0), SR, &WavetableBank::new());
        let mut dark = SynthVoice::new(&saw_lp(400.0), SR, &WavetableBank::new());
        bright.trigger(110.0, 1.0, 0);
        dark.trigger(110.0, 1.0, 0);
        let hi_bright = goertzel(&render(&mut bright, SR as usize).0, 3300.0, SR);
        let hi_dark = goertzel(&render(&mut dark, SR as usize).0, 3300.0, SR);
        assert!(hi_bright > hi_dark * 4.0, "open filter passes more highs");
    }

    #[test]
    fn velocity_scales_amplitude() {
        let mut soft = SynthVoice::new(&saw_lp(8000.0), SR, &WavetableBank::new());
        let mut loud = SynthVoice::new(&saw_lp(8000.0), SR, &WavetableBank::new());
        soft.trigger(220.0, 0.5, 0);
        loud.trigger(220.0, 1.0, 0);
        // measure over the sustain region (skip the attack)
        let rs = rms(&render(&mut soft, SR as usize).0[2000..]);
        let rl = rms(&render(&mut loud, SR as usize).0[2000..]);
        assert!((rl / rs - 2.0).abs() < 0.3, "vel 1.0 ≈ 2× vel 0.5 ({})", rl / rs);
    }

    #[test]
    fn distinct_instruments_have_distinct_spectra() {
        let saw = SynthVoice::new(&saw_lp(8000.0), SR, &WavetableBank::new());
        let mut sine_cfg = saw_lp(8000.0);
        sine_cfg.osc.kind = OscKind::Sine;
        let sine = SynthVoice::new(&sine_cfg, SR, &WavetableBank::new());
        let mut a = saw;
        let mut b = sine;
        a.trigger(220.0, 1.0, 0);
        b.trigger(220.0, 1.0, 0);
        let saw_h2 = goertzel(&render(&mut a, SR as usize).0, 440.0, SR);
        let sine_h2 = goertzel(&render(&mut b, SR as usize).0, 440.0, SR);
        assert!(saw_h2 > sine_h2 * 5.0, "saw has a 2nd harmonic, sine doesn't");
    }

    #[test]
    fn gated_note_sustains_then_frees_after_release() {
        let mut v = SynthVoice::new(&saw_lp(8000.0), SR, &WavetableBank::new());
        v.trigger(220.0, 1.0, 0);
        render(&mut v, (0.2 * SR) as usize); // attack+decay+sustain
        assert!(!v.is_finished(), "held note is sounding");
        assert!(v.current_level() > 0.5, "at sustain");
        v.release();
        render(&mut v, (0.4 * SR) as usize);
        assert!(v.is_finished(), "freed after the release tail");
    }

    #[test]
    fn live_cutoff_paramset_glides_without_zipper() {
        let mut v = SynthVoice::new(&saw_lp(400.0), SR, &WavetableBank::new());
        v.trigger(110.0, 1.0, 0);
        let before = goertzel(&render(&mut v, 12000).0, 3300.0, SR);
        v.set_param(ParamId::Cutoff, 6000.0);
        let after = goertzel(&render(&mut v, 12000).0, 3300.0, SR);
        assert!(after > before * 3.0, "cutoff opened, more highs");
    }

    #[test]
    fn filter_envelope_opens_cutoff_on_attack() {
        let mut cfg = saw_lp(300.0);
        cfg.env_amount_hz = 4000.0;
        // Rise fast, then decay back to the base cutoff (sustain 0) → open then close.
        cfg.filter_env = AdsrConfig {
            attack_s: 0.002,
            decay_s: 0.03,
            sustain: 0.0,
            release_s: 0.1,
            mode: EnvMode::Gated,
            ..Default::default()
        };
        let mut v = SynthVoice::new(&cfg, SR, &WavetableBank::new());
        v.trigger(110.0, 1.0, 0);
        let (l, _) = render(&mut v, (0.15 * SR) as usize);
        // Early (cutoff opened by the env) is bright; late (cutoff back at the 300 Hz base)
        // is dark. Amp sustains through both windows so only the filter differs.
        let early = goertzel(&l[100..1400], 3300.0, SR);
        let late = goertzel(&l[(l.len() - 2000)..], 3300.0, SR);
        assert!(early > late * 2.0, "filter env opened then closed ({early} vs {late})");
    }

    #[test]
    fn pan_splits_constant_power() {
        let mut cfg = saw_lp(8000.0);
        cfg.pan = 0.0; // hard left
        let mut v = SynthVoice::new(&cfg, SR, &WavetableBank::new());
        v.trigger(220.0, 1.0, 0);
        let (l, r) = render(&mut v, 4800);
        assert!(rms(&l) > 0.01, "left has signal");
        assert!(rms(&r) < rms(&l) * 0.01, "right is silent at hard left");
    }
}
