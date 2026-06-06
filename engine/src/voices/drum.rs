//! The synthesized drum kit (bd / sn / cp / hh / oh).
//!
//! Each drum is a one-shot `Voice` built from the synth primitives (oscillator, filter)
//! plus white noise and direct percussive envelopes, so a one-shot self-finishes without
//! a NoteOff. (The Phase 0 single `KickVoice` was retired when the voice pool took over.)

use crate::filter::{Filter, SvfConfig, SvfMode};
use crate::voices::oscillator::{OscConfig, OscKind, Oscillator, WavetableBank};
use crate::voices::{pan_gains, ParamId, Voice};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DrumKind {
    Kick,
    Snare,
    Clap,
    ClosedHat,
    OpenHat,
}

/// One config struct covers every kind; irrelevant fields are simply unused. Use the
/// named constructors for sensible per-kind defaults.
#[derive(Clone, Copy, Debug)]
pub struct DrumConfig {
    pub kind: DrumKind,
    pub gain: f32,
    pub pan: f32,
    // kick
    pub start_hz: f32,
    pub body_hz: f32,
    pub pitch_decay_s: f32,
    pub amp_decay_s: f32,
    pub drive: f32,
    // snare
    pub bp_hz: f32,
    pub bp_q: f32,
    pub tone_hz: f32,
    pub noise_decay_s: f32,
    pub body_decay_s: f32,
    pub noise_gain: f32,
    pub body_gain: f32,
    // clap
    pub taps: u32,
    pub spread_s: f32,
    pub tail_s: f32,
    // hat
    pub hp_hz: f32,
    pub hp_q: f32,
    pub decay_s: f32,
}

impl DrumConfig {
    fn base(kind: DrumKind) -> Self {
        Self {
            kind,
            gain: 0.9,
            pan: 0.5,
            start_hz: 150.0,
            body_hz: 50.0,
            pitch_decay_s: 0.04,
            amp_decay_s: 0.25,
            drive: 0.5,
            bp_hz: 1800.0,
            bp_q: 1.5,
            tone_hz: 180.0,
            noise_decay_s: 0.12,
            body_decay_s: 0.08,
            noise_gain: 0.7,
            body_gain: 0.5,
            taps: 3,
            spread_s: 0.012,
            tail_s: 0.2,
            hp_hz: 7000.0,
            hp_q: 0.7,
            decay_s: 0.04,
        }
    }
    pub fn kick() -> Self {
        Self::base(DrumKind::Kick)
    }
    pub fn snare() -> Self {
        Self::base(DrumKind::Snare)
    }
    pub fn clap() -> Self {
        let mut c = Self::base(DrumKind::Clap);
        c.bp_hz = 1200.0;
        c.bp_q = 1.2;
        c.gain = 0.8;
        c
    }
    pub fn closed_hat() -> Self {
        let mut c = Self::base(DrumKind::ClosedHat);
        c.hp_hz = 7000.0;
        c.decay_s = 0.04;
        c.gain = 0.6;
        c
    }
    pub fn open_hat() -> Self {
        let mut c = Self::base(DrumKind::OpenHat);
        c.hp_hz = 6000.0;
        c.decay_s = 0.3;
        c.gain = 0.6;
        c
    }
}

/// Percussive amp envelope: linear attack then exponential decay. ~`decay_f` sets the
/// −17 dB point; output is effectively 0 after `attack_f + decay_f*4`.
#[inline]
fn perc_amp(t: f32, attack_f: f32, decay_f: f32) -> f32 {
    if t < attack_f {
        t / attack_f.max(1.0)
    } else {
        (-(t - attack_f) / (decay_f * 0.5).max(1.0)).exp()
    }
}

/// Clap multi-burst envelope: `taps` spikes spaced `spread_f` frames apart, the last with
/// a long room tail. Each spike re-attacks to 1.0 at its onset, so the envelope dips
/// between taps (distinct hand-claps).
#[inline]
fn clap_env(t: f32, taps: u32, spread_f: f32, burst_decay_f: f32, tail_f: f32) -> f32 {
    let idx = core::cmp::min((t / spread_f) as u32, taps.saturating_sub(1));
    let onset = idx as f32 * spread_f;
    let decay = if idx == taps - 1 { tail_f } else { burst_decay_f };
    (-(t - onset) / decay.max(1.0)).exp()
}

pub struct DrumVoice {
    kind: DrumKind,
    sr: f32,
    cfg: DrumConfig,
    osc1: Option<Oscillator>, // sine: kick body / snare tone partial
    osc2: Option<Oscillator>, // sine: snare 2nd partial
    filter: Option<Filter>,   // bandpass (snare/clap) or highpass (hat)
    rng: u32,
    active: bool,
    t: f32,
    end_frames: f32,
    vel: f32,
    last_level: f32,
}

impl DrumVoice {
    pub fn new(cfg: &DrumConfig, sr: f32) -> Self {
        let bank = WavetableBank::new();
        let sine = |freq_unused: f32| {
            let _ = freq_unused;
            Oscillator::new(
                OscConfig { kind: OscKind::Sine, ..Default::default() },
                sr,
                &bank,
            )
        };
        let (osc1, osc2, filter) = match cfg.kind {
            DrumKind::Kick => (Some(sine(0.0)), None, None),
            DrumKind::Snare => (
                Some(sine(0.0)),
                Some(sine(0.0)),
                Some(Filter::new(
                    SvfConfig { mode: SvfMode::Bandpass, cutoff_hz: cfg.bp_hz, q: cfg.bp_q, smooth_ms: 0.0 },
                    sr,
                )),
            ),
            DrumKind::Clap => (
                None,
                None,
                Some(Filter::new(
                    SvfConfig { mode: SvfMode::Bandpass, cutoff_hz: cfg.bp_hz, q: cfg.bp_q, smooth_ms: 0.0 },
                    sr,
                )),
            ),
            DrumKind::ClosedHat | DrumKind::OpenHat => (
                None,
                None,
                Some(Filter::new(
                    SvfConfig { mode: SvfMode::Highpass, cutoff_hz: cfg.hp_hz, q: cfg.hp_q, smooth_ms: 0.0 },
                    sr,
                )),
            ),
        };
        let mut v = Self {
            kind: cfg.kind,
            sr,
            cfg: *cfg,
            osc1,
            osc2,
            filter,
            rng: 0x9E3779B9,
            active: false,
            t: 0.0,
            end_frames: 0.0,
            vel: 1.0,
            last_level: 0.0,
        };
        v.recompute_end();
        v
    }

    fn recompute_end(&mut self) {
        let s = self.sr;
        self.end_frames = match self.kind {
            DrumKind::Kick => 0.001 * s + self.cfg.amp_decay_s * 4.0 * s,
            DrumKind::Snare => {
                0.001 * s + self.cfg.noise_decay_s.max(self.cfg.body_decay_s) * 4.0 * s
            }
            DrumKind::Clap => {
                self.cfg.spread_s * self.cfg.taps as f32 * s + self.cfg.tail_s * 4.0 * s
            }
            DrumKind::ClosedHat | DrumKind::OpenHat => 0.0005 * s + self.cfg.decay_s * 4.0 * s,
        };
    }

    #[inline]
    fn next_noise(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// One mono sample, or 0.0 when idle.
    #[inline]
    fn tick_mono(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }
        let s = self.sr;
        let t = self.t;
        let out = match self.kind {
            DrumKind::Kick => {
                let k = (-t / (self.cfg.pitch_decay_s * s)).exp();
                let pitch = self.cfg.body_hz + (self.cfg.start_hz - self.cfg.body_hz) * k;
                let raw = self.osc1.as_mut().unwrap().tick(pitch);
                let amp = perc_amp(t, 0.001 * s, self.cfg.amp_decay_s * s);
                self.last_level = amp;
                ((raw * amp) * (1.0 + self.cfg.drive)).tanh()
            }
            DrumKind::Snare => {
                let noise = self.next_noise();
                let rattle = self.filter.as_mut().unwrap().tick(noise, 0.0);
                let n_amp = perc_amp(t, 0.001 * s, self.cfg.noise_decay_s * s);
                let body = self.osc1.as_mut().unwrap().tick(self.cfg.tone_hz)
                    + 0.5 * self.osc2.as_mut().unwrap().tick(self.cfg.tone_hz * 1.6);
                let b_amp = perc_amp(t, 0.001 * s, self.cfg.body_decay_s * s);
                self.last_level = n_amp.max(b_amp);
                rattle * n_amp * self.cfg.noise_gain + body * b_amp * self.cfg.body_gain
            }
            DrumKind::Clap => {
                let noise = self.next_noise();
                let bp = self.filter.as_mut().unwrap().tick(noise, 0.0);
                let env = clap_env(
                    t,
                    self.cfg.taps,
                    self.cfg.spread_s * s,
                    self.cfg.spread_s * 0.4 * s,
                    self.cfg.tail_s * s,
                );
                self.last_level = env;
                bp * env
            }
            DrumKind::ClosedHat | DrumKind::OpenHat => {
                let noise = self.next_noise();
                let hp = self.filter.as_mut().unwrap().tick(noise, 0.0);
                let amp = perc_amp(t, 0.0005 * s, self.cfg.decay_s * s);
                self.last_level = amp;
                hp * amp
            }
        };
        self.t += 1.0;
        if self.t > self.end_frames {
            self.active = false;
        }
        out * self.cfg.gain * self.vel
    }
}

impl Voice for DrumVoice {
    fn trigger(&mut self, freq: f32, vel: f32, _dur_frames: u64) {
        // Optional pitch tuning of the kick (sound("bd").note("c1")).
        if self.kind == DrumKind::Kick && freq > 20.0 {
            let ratio = self.cfg.start_hz / self.cfg.body_hz;
            self.cfg.body_hz = freq;
            self.cfg.start_hz = freq * ratio;
        }
        self.vel = vel.clamp(0.0, 1.0);
        self.t = 0.0;
        self.active = true;
        if let Some(o) = self.osc1.as_mut() {
            o.reset();
        }
        if let Some(o) = self.osc2.as_mut() {
            o.reset();
        }
        if let Some(f) = self.filter.as_mut() {
            f.reset();
        }
    }

    fn release(&mut self) {} // one-shot: no-op

    fn is_finished(&self) -> bool {
        !self.active
    }

    fn current_level(&self) -> f32 {
        if self.active {
            self.last_level
        } else {
            0.0
        }
    }

    fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let (pl, pr) = pan_gains(self.cfg.pan);
        for (l, r) in out_l.iter_mut().zip(out_r.iter_mut()) {
            let mono = self.tick_mono();
            *l += mono * pl;
            *r += mono * pr;
        }
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        match param {
            ParamId::Gain => self.cfg.gain = value,
            ParamId::Pan => self.cfg.pan = value.clamp(0.0, 1.0),
            ParamId::AmpDecay => {
                // Maps to the kind's primary decay control.
                match self.kind {
                    DrumKind::Kick => self.cfg.amp_decay_s = value,
                    DrumKind::Snare => self.cfg.noise_decay_s = value,
                    DrumKind::Clap => self.cfg.tail_s = value,
                    DrumKind::ClosedHat | DrumKind::OpenHat => self.cfg.decay_s = value,
                }
                self.recompute_end();
            }
            ParamId::Cutoff => {
                if let Some(f) = self.filter.as_mut() {
                    f.set_cutoff(value);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod drumkit_tests {
    use super::*;
    use crate::test_util::{goertzel, rms, zero_crossing_freq};

    const SR: f32 = 48_000.0;

    fn render(v: &mut DrumVoice, n: usize) -> Vec<f32> {
        let mut l = vec![0.0; n];
        let mut r = vec![0.0; n];
        v.render(&mut l, &mut r);
        l
    }

    #[test]
    fn kick_pitch_drops_from_start_to_body() {
        let mut v = DrumVoice::new(&DrumConfig::kick(), SR);
        v.trigger(0.0, 1.0, 0);
        let sig = render(&mut v, (0.2 * SR) as usize);
        let early = zero_crossing_freq(&sig[..(0.02 * SR) as usize], SR);
        let late = zero_crossing_freq(&sig[(0.08 * SR) as usize..(0.18 * SR) as usize], SR);
        assert!(early > late * 1.5, "pitch slides down ({early} → {late})");
    }

    #[test]
    fn snare_has_bandpass_noise_and_a_tonal_body() {
        let mut v = DrumVoice::new(&DrumConfig::snare(), SR);
        v.trigger(0.0, 1.0, 0);
        let sig = render(&mut v, (0.2 * SR) as usize);
        // tonal partial at tone_hz (180) present.
        assert!(goertzel(&sig, 180.0, SR) > 0.001, "tonal body present");
        // Averaged band energy is emphasised around the bandpass centre (1800) vs a high
        // band (single-bin noise is too variable to compare directly).
        let band = |fs: &[f32]| fs.iter().map(|&f| goertzel(&sig, f, SR)).sum::<f32>() / fs.len() as f32;
        let center = band(&[1500.0, 1800.0, 2100.0]);
        let high = band(&[5000.0, 6000.0, 7000.0]);
        assert!(center > high, "noise emphasised around the bandpass centre ({center} vs {high})");
    }

    #[test]
    fn clap_envelope_has_distinct_taps() {
        let spread_f = 0.012 * SR;
        let burst = 0.012 * 0.4 * SR;
        let tail = 0.2 * SR;
        // Sampled just before each of the 3 onsets the envelope has dipped; at each onset
        // it re-attacks to ~1.0.
        let at_onset = |k: f32| clap_env(k * spread_f, 3, spread_f, burst, tail);
        let before = |k: f32| clap_env(k * spread_f - 1.0, 3, spread_f, burst, tail);
        for k in [1.0, 2.0] {
            assert!(at_onset(k) > 0.95, "tap {k} re-attacks");
            assert!(before(k) < 0.6, "envelope dips before tap {k} ({})", before(k));
        }
    }

    fn decay_time(sig: &[f32], sr: f32) -> f32 {
        // Time until the smoothed amplitude falls below 10% of its peak.
        let win = (0.005 * sr) as usize;
        let peak = sig
            .chunks(win)
            .map(|c| rms(c))
            .fold(0.0f32, f32::max)
            .max(1e-9);
        for (i, c) in sig.chunks(win).enumerate() {
            if i > 0 && rms(c) < peak * 0.1 {
                return i as f32 * win as f32 / sr;
            }
        }
        sig.len() as f32 / sr
    }

    #[test]
    fn closed_hat_is_short_open_hat_is_long() {
        let mut closed = DrumVoice::new(&DrumConfig::closed_hat(), SR);
        let mut open = DrumVoice::new(&DrumConfig::open_hat(), SR);
        closed.trigger(0.0, 1.0, 0);
        open.trigger(0.0, 1.0, 0);
        let dc = decay_time(&render(&mut closed, (0.6 * SR) as usize), SR);
        let dop = decay_time(&render(&mut open, SR as usize), SR);
        assert!(dc < dop * 0.5, "closed ({dc}) ≪ open ({dop})");
    }

    #[test]
    fn hats_are_high_frequency_biased() {
        let mut v = DrumVoice::new(&DrumConfig::closed_hat(), SR);
        v.trigger(0.0, 1.0, 0);
        let sig = render(&mut v, (0.05 * SR) as usize);
        assert!(goertzel(&sig, 10000.0, SR) > goertzel(&sig, 500.0, SR), "bright noise");
    }

    #[test]
    fn one_shot_finishes_on_its_own() {
        let mut v = DrumVoice::new(&DrumConfig::closed_hat(), SR);
        v.trigger(0.0, 1.0, 0);
        render(&mut v, SR as usize);
        assert!(v.is_finished(), "closed hat self-terminates");
    }

    #[test]
    fn live_gain_param_changes_level() {
        let mut v = DrumVoice::new(&DrumConfig::kick(), SR);
        v.trigger(0.0, 1.0, 0);
        let loud = rms(&render(&mut v, 2000));
        v.set_param(ParamId::Gain, 0.2);
        v.trigger(0.0, 1.0, 0);
        let soft = rms(&render(&mut v, 2000));
        assert!(soft < loud * 0.5, "gain cut lowered the level");
    }

    #[test]
    fn kick_tuning_via_freq_lowers_body() {
        let mut hi = DrumVoice::new(&DrumConfig::kick(), SR);
        let mut lo = DrumVoice::new(&DrumConfig::kick(), SR);
        hi.trigger(80.0, 1.0, 0);
        lo.trigger(40.0, 1.0, 0);
        let hi_body = zero_crossing_freq(&render(&mut hi, (0.18 * SR) as usize)[(0.1 * SR) as usize..], SR);
        let lo_body = zero_crossing_freq(&render(&mut lo, (0.18 * SR) as usize)[(0.1 * SR) as usize..], SR);
        assert!(hi_body > lo_body, "higher tuning → higher body ({hi_body} vs {lo_body})");
    }
}
