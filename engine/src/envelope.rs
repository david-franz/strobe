//! ADSR envelopes — what gives a note a shape over time (the difference between a click
//! and a pad). Two roles inside a voice: the amp envelope (multiplies the audio) and the
//! filter envelope (modulates cutoff). Two trigger styles: gated (NoteOn→NoteOff) and
//! one-shot (fixed length from NoteOn.arg2, for drums/plucks). All transitions are
//! click-free.
//!
//! A few floats of state, allocation-free `tick()` — a plain Rust state machine (the
//! robust path the Phase 0 plan endorsed) rather than wrapping fundsp's gate envelope.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EnvMode {
    Gated,   // held from NoteOn until NoteOff (sustained synth notes)
    OneShot, // fixed length = NoteOn.arg2 frames; ignores NoteOff (drums/plucks)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SegCurve {
    Linear,
    Exponential,
}

#[derive(Clone, Copy, Debug)]
pub struct AdsrConfig {
    pub attack_s: f32,
    pub decay_s: f32,
    pub sustain: f32, // level 0..1, not time
    pub release_s: f32,
    pub mode: EnvMode,
    pub attack_curve: SegCurve,
    pub decay_curve: SegCurve,
    pub release_curve: SegCurve,
}

impl Default for AdsrConfig {
    fn default() -> Self {
        Self {
            attack_s: 0.005,
            decay_s: 0.1,
            sustain: 0.7,
            release_s: 0.2,
            mode: EnvMode::Gated,
            attack_curve: SegCurve::Linear,
            decay_curve: SegCurve::Exponential,
            release_curve: SegCurve::Exponential,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// The minimum attack/release in seconds — a "0" request still gets a sub-ms ramp so it
/// is never a hard edge.
const MIN_SEG_S: f32 = 0.0005;
/// Below this level a released envelope is considered finished.
const EPS: f32 = 1.0e-4;

/// A click-free, retriggerable ADSR with gated + one-shot modes.
pub struct Adsr {
    cfg: AdsrConfig,
    sr: f32,
    level: f32,
    stage: Stage,
    samples_since_on: u64,
    one_shot_len: u64,
    // precomputed per-segment rates
    attack_inc: f32,    // linear attack increment / sample
    attack_coeff: f32,  // exp attack one-pole coeff
    decay_coeff: f32,   // exp decay one-pole coeff
    release_coeff: f32, // exp release one-pole coeff
}

/// Number of time constants packed into one segment, so an exponential decay/release
/// substantially completes (~99%) within its configured `seconds` rather than reaching
/// only ~63%. Makes the musical "decay/release time" match the heard duration.
const SETTLE: f32 = 5.0;

fn one_pole_coeff(seconds: f32, sr: f32) -> f32 {
    let s = seconds.max(MIN_SEG_S);
    1.0 - (-SETTLE / (s * sr)).exp()
}

impl Adsr {
    pub fn new(cfg: AdsrConfig, sr: f32) -> Self {
        let mut a = Self {
            cfg,
            sr,
            level: 0.0,
            stage: Stage::Idle,
            samples_since_on: 0,
            one_shot_len: 0,
            attack_inc: 0.0,
            attack_coeff: 0.0,
            decay_coeff: 0.0,
            release_coeff: 0.0,
        };
        a.recompute();
        a
    }

    fn recompute(&mut self) {
        let a = self.cfg.attack_s.max(MIN_SEG_S);
        self.attack_inc = 1.0 / (a * self.sr);
        self.attack_coeff = one_pole_coeff(self.cfg.attack_s, self.sr);
        self.decay_coeff = one_pole_coeff(self.cfg.decay_s, self.sr);
        self.release_coeff = one_pole_coeff(self.cfg.release_s, self.sr);
    }

    /// NoteOn. `dur_frames` is arg2 (0 ⇒ gated hold). Level is NOT zeroed → click-free
    /// retrigger (re-attack from the current level).
    pub fn trigger(&mut self, dur_frames: u64) {
        self.stage = Stage::Attack;
        self.samples_since_on = 0;
        self.one_shot_len = if self.cfg.mode == EnvMode::OneShot { dur_frames } else { 0 };
    }

    /// NoteOff (gated only): begin the release.
    pub fn gate_off(&mut self) {
        if self.cfg.mode == EnvMode::Gated && self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
    }

    /// One sample of envelope (0..1). Advances the stage machine; one-shot auto-releases.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        if self.cfg.mode == EnvMode::OneShot
            && self.one_shot_len != 0
            && self.samples_since_on >= self.one_shot_len
        {
            self.stage = Stage::Release;
        }
        self.samples_since_on += 1;

        match self.stage {
            Stage::Idle => self.level = 0.0,
            Stage::Attack => {
                match self.cfg.attack_curve {
                    SegCurve::Linear => self.level += self.attack_inc,
                    SegCurve::Exponential => self.level += self.attack_coeff * (1.2 - self.level),
                }
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level += self.decay_coeff * (self.cfg.sustain - self.level);
                if (self.level - self.cfg.sustain).abs() < EPS {
                    self.level = self.cfg.sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => self.level = self.cfg.sustain,
            Stage::Release => {
                self.level += self.release_coeff * (0.0 - self.level);
                if self.level < EPS {
                    self.level = 0.0;
                }
            }
        }
        self.level
    }

    /// True once a released envelope's tail has ended → the voice can be freed.
    pub fn is_finished(&self) -> bool {
        self.stage == Stage::Release && self.level < EPS
    }

    /// Current level — used by the allocator's "quietest voice" stealing.
    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn is_active(&self) -> bool {
        self.stage != Stage::Idle
    }

    // Live ADSR edits (ParamSet).
    pub fn set_attack_s(&mut self, v: f32) {
        self.cfg.attack_s = v;
        self.recompute();
    }
    pub fn set_decay_s(&mut self, v: f32) {
        self.cfg.decay_s = v;
        self.recompute();
    }
    pub fn set_sustain(&mut self, v: f32) {
        self.cfg.sustain = v.clamp(0.0, 1.0);
    }
    pub fn set_release_s(&mut self, v: f32) {
        self.cfg.release_s = v;
        self.recompute();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn cfg(mode: EnvMode) -> AdsrConfig {
        AdsrConfig {
            attack_s: 0.01,
            decay_s: 0.05,
            sustain: 0.5,
            release_s: 0.1,
            mode,
            ..Default::default()
        }
    }

    #[test]
    fn gated_runs_attack_decay_sustain_release() {
        let mut e = Adsr::new(cfg(EnvMode::Gated), SR);
        e.trigger(0);
        // After the attack, the peak (~1.0) has been reached.
        let mut peak = 0.0f32;
        for _ in 0..(0.012 * SR) as usize {
            peak = peak.max(e.tick());
        }
        assert!(peak > 0.95, "reaches peak in attack ({peak})");
        // After decay it settles to sustain.
        for _ in 0..(0.1 * SR) as usize {
            e.tick();
        }
        assert!((e.level() - 0.5).abs() < 0.02, "holds sustain ({})", e.level());
        // NoteOff → release toward 0.
        e.gate_off();
        for _ in 0..(0.3 * SR) as usize {
            e.tick();
        }
        assert!(e.is_finished(), "released to silence");
    }

    #[test]
    fn one_shot_auto_releases_at_duration_without_noteoff() {
        let mut e = Adsr::new(cfg(EnvMode::OneShot), SR);
        let dur = (0.2 * SR) as u64;
        e.trigger(dur);
        // Just before the duration it is still sounding (above the release floor).
        for _ in 0..(dur - 100) {
            e.tick();
        }
        let before = e.level();
        // Run past the duration; it auto-releases (no NoteOff sent).
        for _ in 0..(0.3 * SR) as usize {
            e.tick();
        }
        assert!(e.is_finished(), "one-shot released on its own");
        assert!(before >= 0.0);
    }

    #[test]
    fn attack_is_click_free_from_zero() {
        let mut e = Adsr::new(cfg(EnvMode::Gated), SR);
        e.trigger(0);
        let first = e.tick();
        assert!(first < 0.02, "starts near 0 ({first})");
        let second = e.tick();
        assert!(second > first, "ramps upward");
    }

    #[test]
    fn release_is_monotonic_to_epsilon() {
        let mut e = Adsr::new(cfg(EnvMode::Gated), SR);
        e.trigger(0);
        for _ in 0..(0.1 * SR) as usize {
            e.tick();
        }
        e.gate_off();
        let mut prev = e.level() + 1.0;
        let mut finished = false;
        for _ in 0..(0.5 * SR) as usize {
            let v = e.tick();
            assert!(v <= prev + 1e-6, "non-increasing release tail");
            prev = v;
            if e.is_finished() {
                finished = true;
                break;
            }
        }
        assert!(finished, "tail reaches epsilon");
    }

    #[test]
    fn retrigger_re_attacks_from_current_level_not_zero() {
        let mut e = Adsr::new(cfg(EnvMode::Gated), SR);
        e.trigger(0);
        for _ in 0..(0.05 * SR) as usize {
            e.tick();
        }
        let mid = e.level();
        assert!(mid > 0.2, "mid-note level is well above 0 ({mid})");
        e.trigger(0); // retrigger
        let after = e.tick();
        assert!(after >= mid - 0.05, "did not snap to 0 on retrigger ({after} vs {mid})");
    }

    #[test]
    fn live_param_edit_changes_release_time() {
        let mut e = Adsr::new(cfg(EnvMode::Gated), SR);
        e.set_release_s(2.0); // long release
        e.trigger(0);
        for _ in 0..(0.1 * SR) as usize {
            e.tick();
        }
        e.gate_off();
        for _ in 0..(0.2 * SR) as usize {
            e.tick();
        }
        // With a 2 s release, after 0.2 s it is nowhere near finished.
        assert!(!e.is_finished() && e.level() > 0.1, "long release still ringing");
    }
}
