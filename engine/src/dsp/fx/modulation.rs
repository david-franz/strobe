//! Modulation FX (Phase 3, feature 05): chorus, flanger, and phaser — three insert effects that
//! all modulate a short delay (chorus/flanger) or an all-pass cascade (phaser) with a shared
//! stereo [`Lfo`]. Each produces the full dry+wet sound internally (so the chain runs them fully
//! wet). RT-safe: delay lines / all-pass state pre-allocated, no allocation in `process`.

use super::{Effect, FxCtx, Smoothed};

/// Common modulation params (≥0x02 to avoid the chain-reserved FX_MIX/FX_BYPASS).
pub const MOD_RATE: u8 = 0x02; // LFO rate (Hz)
pub const MOD_DEPTH: u8 = 0x03; // 0..1 modulation depth
pub const MOD_FEEDBACK: u8 = 0x04; // flanger/phaser feedback

#[inline]
fn frac(x: f32) -> f32 {
    x - x.floor()
}

/// A stereo sine LFO; L and R are offset in phase for width.
struct Lfo {
    rate: Smoothed, // Hz
    phase: f32,
    stereo: f32, // R phase offset (0..0.5)
}

impl Lfo {
    fn new(rate_hz: f32, sr: f32) -> Self {
        Self { rate: Smoothed::new(rate_hz, sr), phase: 0.0, stereo: 0.25 }
    }
    #[inline]
    fn next_pair(&mut self, sr: f32) -> (f32, f32) {
        let tau = 2.0 * std::f32::consts::PI;
        let l = (tau * self.phase).sin();
        let r = (tau * frac(self.phase + self.stereo)).sin();
        self.phase = frac(self.phase + (self.rate.next() / sr).max(0.0));
        (l, r)
    }
}

/// Linear-interpolated delay read, `delay_samples` behind the write head.
#[inline]
fn read_frac(buf: &[f32], write: usize, delay_samples: f32) -> f32 {
    let n = buf.len();
    let d = delay_samples.clamp(1.0, (n - 2) as f32);
    let rp = (write as f32 - d).rem_euclid(n as f32);
    let i0 = rp.floor() as usize;
    let i1 = (i0 + 1) % n;
    let f = rp - rp.floor();
    buf[i0] * (1.0 - f) + buf[i1] * f
}

// ── Chorus ──────────────────────────────────────────────────────────────────

pub struct Chorus {
    lfo: Lfo,
    depth: Smoothed,
    sr: f32,
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    write: usize,
}

impl Chorus {
    pub fn new(sr: f32) -> Self {
        let max = (0.05 * sr) as usize + 2; // 50 ms
        Self {
            lfo: Lfo::new(0.6, sr),
            depth: Smoothed::new(0.5, sr),
            sr,
            buf_l: vec![0.0; max],
            buf_r: vec![0.0; max],
            write: 0,
        }
    }
}

const CHORUS_BASE_MS: f32 = 15.0;
const CHORUS_DEPTH_MS: f32 = 7.0;

impl Effect for Chorus {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], _ctx: &FxCtx) {
        let n = self.buf_l.len();
        let ms = self.sr / 1000.0;
        for i in 0..l.len() {
            let (ml, mr) = self.lfo.next_pair(self.sr);
            let depth = self.depth.next().clamp(0.0, 1.0);
            let inl = l[i];
            let inr = r[i];
            self.buf_l[self.write] = inl;
            self.buf_r[self.write] = inr;
            let dl = (CHORUS_BASE_MS + depth * CHORUS_DEPTH_MS * ml) * ms;
            let dr = (CHORUS_BASE_MS + depth * CHORUS_DEPTH_MS * mr) * ms;
            let wl = read_frac(&self.buf_l, self.write, dl);
            let wr = read_frac(&self.buf_r, self.write, dr);
            self.write = (self.write + 1) % n;
            l[i] = inl * 0.7 + wl * 0.7;
            r[i] = inr * 0.7 + wr * 0.7;
        }
    }
    fn set_param(&mut self, p: u8, v: f32) {
        match p {
            MOD_RATE => self.lfo.rate.set_target(v.clamp(0.01, 20.0)),
            MOD_DEPTH => self.depth.set_target(v.clamp(0.0, 1.0)),
            _ => {}
        }
    }
    fn ramp_param(&mut self, p: u8, target: f32, frames: u32) {
        match p {
            MOD_RATE => self.lfo.rate.ramp_to(target.clamp(0.01, 20.0), frames),
            MOD_DEPTH => self.depth.ramp_to(target.clamp(0.0, 1.0), frames),
            _ => {}
        }
    }
    fn reset(&mut self) {
        self.buf_l.fill(0.0);
        self.buf_r.fill(0.0);
        self.write = 0;
    }
}

// ── Flanger ───────────────────────────────────────────────────────────────────

pub struct Flanger {
    lfo: Lfo,
    depth: Smoothed,
    feedback: Smoothed,
    sr: f32,
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    write: usize,
    fb_l: f32,
    fb_r: f32,
}

impl Flanger {
    pub fn new(sr: f32) -> Self {
        let max = (0.02 * sr) as usize + 2; // 20 ms
        Self {
            lfo: Lfo::new(0.3, sr),
            depth: Smoothed::new(0.6, sr),
            feedback: Smoothed::new(0.5, sr),
            sr,
            buf_l: vec![0.0; max],
            buf_r: vec![0.0; max],
            write: 0,
            fb_l: 0.0,
            fb_r: 0.0,
        }
    }
}

const FLANGER_BASE_MS: f32 = 1.0;
const FLANGER_DEPTH_MS: f32 = 5.0;

impl Effect for Flanger {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], _ctx: &FxCtx) {
        let n = self.buf_l.len();
        let ms = self.sr / 1000.0;
        for i in 0..l.len() {
            let (ml, mr) = self.lfo.next_pair(self.sr);
            let depth = self.depth.next().clamp(0.0, 1.0);
            let fb = self.feedback.next().clamp(-0.95, 0.95);
            let inl = l[i];
            let inr = r[i];
            // unipolar LFO (0..1) so the very short delay stays positive
            let dl = (FLANGER_BASE_MS + depth * FLANGER_DEPTH_MS * (0.5 + 0.5 * ml)) * ms;
            let dr = (FLANGER_BASE_MS + depth * FLANGER_DEPTH_MS * (0.5 + 0.5 * mr)) * ms;
            self.buf_l[self.write] = inl + fb * self.fb_l;
            self.buf_r[self.write] = inr + fb * self.fb_r;
            let wl = read_frac(&self.buf_l, self.write, dl);
            let wr = read_frac(&self.buf_r, self.write, dr);
            self.write = (self.write + 1) % n;
            self.fb_l = if wl.abs() < 1e-30 { 0.0 } else { wl };
            self.fb_r = if wr.abs() < 1e-30 { 0.0 } else { wr };
            l[i] = inl + wl; // dry + wet comb
            r[i] = inr + wr;
        }
    }
    fn set_param(&mut self, p: u8, v: f32) {
        match p {
            MOD_RATE => self.lfo.rate.set_target(v.clamp(0.01, 20.0)),
            MOD_DEPTH => self.depth.set_target(v.clamp(0.0, 1.0)),
            MOD_FEEDBACK => self.feedback.set_target(v.clamp(-0.95, 0.95)),
            _ => {}
        }
    }
    fn ramp_param(&mut self, p: u8, target: f32, frames: u32) {
        match p {
            MOD_RATE => self.lfo.rate.ramp_to(target.clamp(0.01, 20.0), frames),
            MOD_DEPTH => self.depth.ramp_to(target.clamp(0.0, 1.0), frames),
            MOD_FEEDBACK => self.feedback.ramp_to(target.clamp(-0.95, 0.95), frames),
            _ => {}
        }
    }
    fn reset(&mut self) {
        self.buf_l.fill(0.0);
        self.buf_r.fill(0.0);
        self.write = 0;
        self.fb_l = 0.0;
        self.fb_r = 0.0;
    }
}

// ── Phaser ────────────────────────────────────────────────────────────────────

const PHASER_STAGES: usize = 6;

#[derive(Clone, Copy, Default)]
struct Allpass1 {
    x1: f32,
    y1: f32,
}

impl Allpass1 {
    #[inline]
    fn process(&mut self, x: f32, a1: f32) -> f32 {
        let y = a1 * x + self.x1 - a1 * self.y1;
        self.x1 = x;
        self.y1 = if y.abs() < 1e-30 { 0.0 } else { y };
        y
    }
}

pub struct Phaser {
    lfo: Lfo,
    depth: Smoothed,
    feedback: Smoothed,
    sr: f32,
    stages_l: [Allpass1; PHASER_STAGES],
    stages_r: [Allpass1; PHASER_STAGES],
    fb_l: f32,
    fb_r: f32,
    center: f32,
    range: f32,
}

impl Phaser {
    pub fn new(sr: f32) -> Self {
        Self {
            lfo: Lfo::new(0.4, sr),
            depth: Smoothed::new(0.6, sr),
            feedback: Smoothed::new(0.3, sr),
            sr,
            stages_l: [Allpass1::default(); PHASER_STAGES],
            stages_r: [Allpass1::default(); PHASER_STAGES],
            fb_l: 0.0,
            fb_r: 0.0,
            center: 800.0,
            range: 2.0, // octaves
        }
    }
    #[inline]
    fn coef(&self, lfo: f32, depth: f32) -> f32 {
        let fc = (self.center * 2f32.powf(self.range * depth * lfo)).clamp(20.0, self.sr * 0.49);
        let t = (std::f32::consts::PI * fc / self.sr).tan();
        (t - 1.0) / (t + 1.0)
    }
}

impl Effect for Phaser {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], _ctx: &FxCtx) {
        for i in 0..l.len() {
            let (ml, mr) = self.lfo.next_pair(self.sr);
            let depth = self.depth.next().clamp(0.0, 1.0);
            let fb = self.feedback.next().clamp(-0.9, 0.9);
            let a1l = self.coef(ml, depth);
            let a1r = self.coef(mr, depth);
            let inl = l[i];
            let inr = r[i];
            let mut sl = inl + fb * self.fb_l;
            for s in self.stages_l.iter_mut() {
                sl = s.process(sl, a1l);
            }
            let mut sr = inr + fb * self.fb_r;
            for s in self.stages_r.iter_mut() {
                sr = s.process(sr, a1r);
            }
            self.fb_l = sl;
            self.fb_r = sr;
            l[i] = inl + sl; // dry + swept all-pass → moving notches
            r[i] = inr + sr;
        }
    }
    fn set_param(&mut self, p: u8, v: f32) {
        match p {
            MOD_RATE => self.lfo.rate.set_target(v.clamp(0.01, 20.0)),
            MOD_DEPTH => self.depth.set_target(v.clamp(0.0, 1.0)),
            MOD_FEEDBACK => self.feedback.set_target(v.clamp(-0.9, 0.9)),
            _ => {}
        }
    }
    fn ramp_param(&mut self, p: u8, target: f32, frames: u32) {
        match p {
            MOD_RATE => self.lfo.rate.ramp_to(target.clamp(0.01, 20.0), frames),
            MOD_DEPTH => self.depth.ramp_to(target.clamp(0.0, 1.0), frames),
            MOD_FEEDBACK => self.feedback.ramp_to(target.clamp(-0.9, 0.9), frames),
            _ => {}
        }
    }
    fn reset(&mut self) {
        self.stages_l = [Allpass1::default(); PHASER_STAGES];
        self.stages_r = [Allpass1::default(); PHASER_STAGES];
        self.fb_l = 0.0;
        self.fb_r = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::fx::sidechain::SidechainBus;

    const SR: f32 = 48_000.0;

    fn ctx<'a>(sc: &'a SidechainBus) -> FxCtx<'a> {
        FxCtx { sample_rate: SR, cps: 0.5, sidechain: sc }
    }

    fn tone(freq: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| 0.5 * (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin()).collect()
    }

    #[test]
    fn chorus_decorrelates_the_stereo_image() {
        let sc = SidechainBus::new(SR);
        let mut c = Chorus::new(SR);
        // identical L/R input → after chorus the two channels differ (width from the stereo LFO).
        let mono = tone(440.0, 16_000);
        let mut l = mono.clone();
        let mut r = mono.clone();
        c.process(&mut l, &mut r, &ctx(&sc));
        let diff: f32 = l[8000..].iter().zip(&r[8000..]).map(|(a, b)| (a - b).abs()).sum::<f32>()
            / l[8000..].len() as f32;
        assert!(diff > 1e-3, "L and R differ after chorus ({diff})");
        assert!(l.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn flanger_combs_and_stays_bounded() {
        let sc = SidechainBus::new(SR);
        let mut f = Flanger::new(SR);
        f.set_param(MOD_FEEDBACK, 0.9);
        f.feedback.snap(0.9);
        let input = tone(440.0, 16_000);
        let mut l = input.clone();
        let mut r = input.clone();
        f.process(&mut l, &mut r, &ctx(&sc));
        // The comb changes the signal (not a passthrough) and stays bounded even at high feedback.
        let changed = l.iter().zip(&input).any(|(a, b)| (a - b).abs() > 1e-3);
        assert!(changed, "flanger alters the signal");
        assert!(l.iter().all(|&x| x.is_finite() && x.abs() < 8.0), "bounded");
    }

    #[test]
    fn phaser_alters_signal_without_blowing_up() {
        let sc = SidechainBus::new(SR);
        let mut p = Phaser::new(SR);
        let input = tone(440.0, 16_000);
        let mut l = input.clone();
        let mut r = input.clone();
        p.process(&mut l, &mut r, &ctx(&sc));
        let changed = l.iter().zip(&input).any(|(a, b)| (a - b).abs() > 1e-3);
        assert!(changed, "phaser alters the signal");
        assert!(l.iter().all(|&x| x.is_finite() && x.abs() < 8.0), "bounded");
    }
}
