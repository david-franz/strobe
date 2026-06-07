//! Dynamics (Phase 3, feature 06): a feed-forward [`Compressor`] and the headline
//! [`SidechainDuck`] — the techno pump. The compressor follows the track's own level; the duck
//! ducks the track by a control level **tapped from another track** (read from the
//! [`SidechainBus`](super::SidechainBus) in [`FxCtx`], filled before any duck runs). Both apply
//! stereo-linked gain. RT-safe: a few floats of state, no allocation in `process`.

use super::{db_to_lin, lin_to_db, Effect, EnvelopeFollower, FxCtx, Smoothed};

// ── Compressor ──────────────────────────────────────────────────────────────

/// fx_param ids (≥0x02 to avoid the chain-reserved FX_MIX/FX_BYPASS).
pub const COMP_THRESH: u8 = 0x02; // dB
pub const COMP_RATIO: u8 = 0x03; // :1
pub const COMP_ATTACK: u8 = 0x04; // ms
pub const COMP_RELEASE: u8 = 0x05; // ms
pub const COMP_MAKEUP: u8 = 0x06; // dB

pub struct Compressor {
    thresh_db: Smoothed,
    ratio: Smoothed,
    makeup_db: Smoothed,
    follower: EnvelopeFollower,
    atk_ms: f32,
    rel_ms: f32,
    sr: f32,
}

impl Compressor {
    pub fn new(sr: f32) -> Self {
        Self {
            thresh_db: Smoothed::new(-18.0, sr),
            ratio: Smoothed::new(4.0, sr),
            makeup_db: Smoothed::new(0.0, sr),
            follower: EnvelopeFollower::new(10.0, 100.0, sr),
            atk_ms: 10.0,
            rel_ms: 100.0,
            sr,
        }
    }
}

impl Effect for Compressor {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], _ctx: &FxCtx) {
        for i in 0..l.len() {
            let det = 0.5 * (l[i].abs() + r[i].abs()); // stereo-linked detection
            let env_db = lin_to_db(self.follower.process(det));
            let thresh = self.thresh_db.next();
            let ratio = self.ratio.next().max(1.0);
            let makeup = self.makeup_db.next();
            let over = env_db - thresh;
            let gr_db = if over > 0.0 { -over * (1.0 - 1.0 / ratio) } else { 0.0 };
            let g = db_to_lin(gr_db + makeup);
            l[i] *= g;
            r[i] *= g;
        }
    }
    fn set_param(&mut self, p: u8, v: f32) {
        match p {
            COMP_THRESH => self.thresh_db.set_target(v.clamp(-60.0, 0.0)),
            COMP_RATIO => self.ratio.set_target(v.clamp(1.0, 20.0)),
            COMP_MAKEUP => self.makeup_db.set_target(v.clamp(0.0, 24.0)),
            COMP_ATTACK => {
                self.atk_ms = v.clamp(0.1, 100.0);
                self.follower.set_times(self.atk_ms, self.rel_ms, self.sr);
            }
            COMP_RELEASE => {
                self.rel_ms = v.clamp(10.0, 1000.0);
                self.follower.set_times(self.atk_ms, self.rel_ms, self.sr);
            }
            _ => {}
        }
    }
    fn ramp_param(&mut self, p: u8, target: f32, frames: u32) {
        match p {
            COMP_THRESH => self.thresh_db.ramp_to(target.clamp(-60.0, 0.0), frames),
            COMP_RATIO => self.ratio.ramp_to(target.clamp(1.0, 20.0), frames),
            COMP_MAKEUP => self.makeup_db.ramp_to(target.clamp(0.0, 24.0), frames),
            _ => {}
        }
    }
    fn reset(&mut self) {
        self.follower.reset();
    }
}

// ── Sidechain duck (the pump) ─────────────────────────────────────────────────

/// fx_param ids.
pub const DUCK_SOURCE: u8 = 0x02; // which track to follow (p1 → 1)
pub const DUCK_AMOUNT: u8 = 0x03; // 0..1 duck depth

pub struct SidechainDuck {
    source: usize,
    amount: Smoothed,
}

impl SidechainDuck {
    pub fn new(sr: f32) -> Self {
        Self { source: 0, amount: Smoothed::new(0.8, sr) }
    }
}

impl Effect for SidechainDuck {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], ctx: &FxCtx) {
        let src = ctx.sidechain.level(self.source); // the source track's tapped envelope
        for i in 0..l.len() {
            let amount = self.amount.next().clamp(0.0, 1.0);
            let level = if i < src.len() { src[i] } else { 0.0 };
            // 1 = fully open; →0 = ducked when the source is loud. Stereo-linked.
            let duck = (1.0 - amount * level.min(1.0)).max(0.0);
            l[i] *= duck;
            r[i] *= duck;
        }
    }
    fn set_param(&mut self, p: u8, v: f32) {
        match p {
            DUCK_SOURCE => self.source = v.max(0.0) as usize,
            DUCK_AMOUNT => self.amount.set_target(v.clamp(0.0, 1.0)),
            _ => {}
        }
    }
    fn ramp_param(&mut self, p: u8, target: f32, frames: u32) {
        if p == DUCK_AMOUNT {
            self.amount.ramp_to(target.clamp(0.0, 1.0), frames);
        }
    }
    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::fx::sidechain::SidechainBus;
    use crate::test_util::rms;

    const SR: f32 = 48_000.0;

    fn tone(amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / SR).sin()).collect()
    }

    #[test]
    fn compressor_reduces_level_above_threshold_only() {
        let sc = SidechainBus::new(SR);
        let ctx = FxCtx { sample_rate: SR, cps: 0.5, sidechain: &sc };
        let run = |amp: f32| -> f32 {
            let mut c = Compressor::new(SR); // thresh -18, ratio 4
            let input = tone(amp, 16_000);
            let mut l = input.clone();
            let mut r = input.clone();
            c.process(&mut l, &mut r, &ctx);
            rms(&l[8000..]) / rms(&input[8000..])
        };
        // A loud signal (well above −18 dB) is attenuated; a quiet one passes ~unity.
        assert!(run(0.5) < 0.8, "above threshold → gain reduction");
        assert!((run(0.02) - 1.0).abs() < 0.1, "below threshold → ~unity");
    }

    #[test]
    fn duck_attenuates_when_the_source_is_loud() {
        let mut sc = SidechainBus::new(SR);
        let loud = vec![1.0f32; 2000];
        sc.tap(1, &loud, &loud); // fill track 1's level with a risen envelope
        let mut duck = SidechainDuck::new(SR);
        duck.set_param(DUCK_SOURCE, 1.0);
        duck.amount.snap(0.8);
        let ctx = FxCtx { sample_rate: SR, cps: 0.5, sidechain: &sc };
        let mut l = vec![1.0f32; 2000];
        let mut r = vec![1.0f32; 2000];
        duck.process(&mut l, &mut r, &ctx);
        assert!(l[1500] < 0.5, "ducked under a loud source ({})", l[1500]);
    }

    #[test]
    fn duck_with_silent_source_is_a_passthrough() {
        let sc = SidechainBus::new(SR); // nothing tapped → all levels 0
        let mut duck = SidechainDuck::new(SR);
        duck.set_param(DUCK_SOURCE, 1.0);
        duck.amount.snap(0.8);
        let ctx = FxCtx { sample_rate: SR, cps: 0.5, sidechain: &sc };
        let mut l = vec![0.7f32; 512];
        let mut r = vec![0.7f32; 512];
        duck.process(&mut l, &mut r, &ctx);
        assert!(l.iter().all(|&x| (x - 0.7).abs() < 1e-6), "no source → no ducking");
    }
}
