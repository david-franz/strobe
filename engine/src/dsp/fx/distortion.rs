//! Distortion / saturation insert (Phase 3, feature 04): a drive gain into a non-linear shaper
//! (tanh / soft-clip / hard-clip / wavefold), a post-shaper tone low-pass to tame the generated
//! highs, and an output DC blocker. RT-safe: memoryless shaper + a couple of one-pole states,
//! no allocation in `process`.
//!
//! Note: this runs at the base sample rate (no oversampling yet), so hard-clip/wavefold at high
//! drive will alias somewhat; the default `tanh` + tone keep it musical. Oversampling is a
//! documented follow-up.

use super::{Effect, FxCtx, Smoothed};

/// fx_param ids (≥0x02 to avoid the chain-reserved FX_MIX/FX_BYPASS).
pub const DIST_DRIVE: u8 = 0x02; // input gain into the shaper (≥0.1)
pub const DIST_TONE: u8 = 0x03; // post-shaper low-pass cutoff (Hz)
pub const DIST_SHAPE: u8 = 0x04; // 0 tanh, 1 soft-clip, 2 hard-clip, 3 wavefold

#[inline]
fn shape(s: usize, x: f32) -> f32 {
    match s {
        1 => {
            let c = x.clamp(-1.5, 1.5);
            c - c * c * c / 3.0 // cubic soft clip
        }
        2 => x.clamp(-1.0, 1.0),                      // hard clip
        3 => (x * std::f32::consts::FRAC_PI_2).sin(), // wavefold-ish
        _ => x.tanh(),                                // 0 = tanh (default, smooth)
    }
}

pub struct Distort {
    shape: usize,
    drive: Smoothed,
    tone: Smoothed, // post-shaper LP cutoff
    sr: f32,
    lp_l: f32,
    lp_r: f32,
    // output DC blocker per channel: y[n] = x[n] - x[n-1] + R·y[n-1]
    dc_x_l: f32,
    dc_y_l: f32,
    dc_x_r: f32,
    dc_y_r: f32,
    dc_r: f32,
}

impl Distort {
    pub fn new(sr: f32) -> Self {
        Self {
            shape: 0,
            drive: Smoothed::new(1.0, sr),
            tone: Smoothed::new(8000.0, sr),
            sr,
            lp_l: 0.0,
            lp_r: 0.0,
            dc_x_l: 0.0,
            dc_y_l: 0.0,
            dc_x_r: 0.0,
            dc_y_r: 0.0,
            dc_r: 1.0 - 2.0 * std::f32::consts::PI * 10.0 / sr, // ~10 Hz high-pass
        }
    }
    #[inline]
    fn dc_block(x: f32, x1: &mut f32, y1: &mut f32, r: f32) -> f32 {
        let y = x - *x1 + r * *y1;
        *x1 = x;
        *y1 = if y.abs() < 1e-30 { 0.0 } else { y };
        *y1
    }
}

impl Effect for Distort {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], _ctx: &FxCtx) {
        let sh = self.shape;
        let nyq = self.sr * 0.5;
        for i in 0..l.len() {
            let drive = self.drive.next().max(0.1);
            let tone = self.tone.next().clamp(200.0, nyq - 1.0);
            let coef = 1.0 - (-2.0 * std::f32::consts::PI * tone / self.sr).exp();
            self.lp_l += coef * (shape(sh, l[i] * drive) - self.lp_l);
            self.lp_r += coef * (shape(sh, r[i] * drive) - self.lp_r);
            l[i] = Self::dc_block(self.lp_l, &mut self.dc_x_l, &mut self.dc_y_l, self.dc_r);
            r[i] = Self::dc_block(self.lp_r, &mut self.dc_x_r, &mut self.dc_y_r, self.dc_r);
        }
    }
    fn set_param(&mut self, p: u8, v: f32) {
        match p {
            DIST_DRIVE => self.drive.set_target(v.max(0.1)),
            DIST_TONE => self.tone.set_target(v),
            DIST_SHAPE => self.shape = core::cmp::min(v as usize, 3),
            _ => {}
        }
    }
    fn ramp_param(&mut self, p: u8, target: f32, frames: u32) {
        match p {
            DIST_DRIVE => self.drive.ramp_to(target.max(0.1), frames),
            DIST_TONE => self.tone.ramp_to(target, frames),
            _ => {}
        }
    }
    fn reset(&mut self) {
        self.lp_l = 0.0;
        self.lp_r = 0.0;
        self.dc_x_l = 0.0;
        self.dc_y_l = 0.0;
        self.dc_x_r = 0.0;
        self.dc_y_r = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::fx::sidechain::SidechainBus;
    use crate::test_util::goertzel;

    const SR: f32 = 48_000.0;

    fn ctx<'a>(sc: &'a SidechainBus) -> FxCtx<'a> {
        FxCtx { sample_rate: SR, cps: 0.5, sidechain: sc }
    }

    fn tone(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin()).collect()
    }

    #[test]
    fn tanh_saturation_adds_harmonics_and_bounds_the_peak() {
        let sc = SidechainBus::new(SR);
        let mut d = Distort::new(SR);
        d.set_param(DIST_DRIVE, 8.0);
        d.drive.snap(8.0);
        let mut l = tone(1000.0, 0.7, 8192);
        let mut r = l.clone();
        d.process(&mut l, &mut r, &ctx(&sc));
        // tanh bounds the output (heavily-driven signal does not blow up past ~1).
        let peak = l.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(peak <= 1.05, "tanh bounds the peak ({peak})");
        // odd harmonics appear (3rd harmonic at 3 kHz is well above the input's none).
        let h3 = goertzel(&l[2048..], 3000.0, SR);
        assert!(h3 > 0.02, "saturation adds a 3rd harmonic ({h3})");
    }

    #[test]
    fn unity_drive_tanh_is_nearly_clean() {
        let sc = SidechainBus::new(SR);
        let mut d = Distort::new(SR);
        let mut l = tone(1000.0, 0.2, 8192);
        let mut r = l.clone();
        d.process(&mut l, &mut r, &ctx(&sc));
        // low level + unity drive ≈ gentle: little 3rd-harmonic energy.
        let h3 = goertzel(&l[2048..], 3000.0, SR);
        assert!(h3 < 0.01, "near-clean at unity drive ({h3})");
    }
}
