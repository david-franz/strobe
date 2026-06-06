//! Algorithmic reverb send effect (Phase 3, feature 03): a Freeverb-style topology — 8 damped
//! comb filters in parallel into 4 series all-pass diffusers per channel, with a stereo spread.
//! `size` sets the tail length (comb feedback, clamped < 1 for stability) and `damping` the HF
//! absorption in the tail. All delay lines pre-allocated; no allocation in `process`.

use super::{Effect, FxCtx, Smoothed};

/// fx_param ids (≥0x02 to avoid the chain-reserved FX_MIX/FX_BYPASS).
pub const RV_SIZE: u8 = 0x02; // 0..1 → tail length
pub const RV_DAMP: u8 = 0x03; // 0..1 → HF absorption

// Freeverb tunings (samples at 44.1 kHz), scaled to the actual sample rate.
const COMB_TUNINGS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNINGS: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;
const FIXED_GAIN: f32 = 0.015;

struct Comb {
    buf: Vec<f32>,
    idx: usize,
    store: f32, // one-pole low-pass state in the feedback (damping)
}

impl Comb {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)], idx: 0, store: 0.0 }
    }
    #[inline]
    fn process(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let out = self.buf[self.idx];
        self.store = out * (1.0 - damp) + self.store * damp;
        let mut v = input + self.store * feedback;
        if v.abs() < 1e-30 {
            v = 0.0; // flush denormals on a decaying tail
        }
        self.buf[self.idx] = v;
        self.idx = (self.idx + 1) % self.buf.len();
        out
    }
    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.store = 0.0;
        self.idx = 0;
    }
}

struct Allpass {
    buf: Vec<f32>,
    idx: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)], idx: 0 }
    }
    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let bufout = self.buf[self.idx];
        let out = -input + bufout;
        let mut v = input + bufout * 0.5;
        if v.abs() < 1e-30 {
            v = 0.0;
        }
        self.buf[self.idx] = v;
        self.idx = (self.idx + 1) % self.buf.len();
        out
    }
    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.idx = 0;
    }
}

pub struct Reverb {
    combs_l: Vec<Comb>,
    combs_r: Vec<Comb>,
    aps_l: Vec<Allpass>,
    aps_r: Vec<Allpass>,
    size: Smoothed,
    damp: Smoothed,
    sr: f32,
}

impl Reverb {
    pub fn new(sr: f32) -> Self {
        let scale = sr / 44_100.0;
        let combs = |extra: usize| -> Vec<Comb> {
            COMB_TUNINGS.iter().map(|&t| Comb::new(((t + extra) as f32 * scale) as usize)).collect()
        };
        let aps = |extra: usize| -> Vec<Allpass> {
            ALLPASS_TUNINGS.iter().map(|&t| Allpass::new(((t + extra) as f32 * scale) as usize)).collect()
        };
        Self {
            combs_l: combs(0),
            combs_r: combs(STEREO_SPREAD),
            aps_l: aps(0),
            aps_r: aps(STEREO_SPREAD),
            size: Smoothed::new(0.5, sr),
            damp: Smoothed::new(0.4, sr),
            sr,
        }
    }
}

impl Effect for Reverb {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], _ctx: &FxCtx) {
        for i in 0..l.len() {
            let size = self.size.next().clamp(0.0, 1.0);
            let damp = self.damp.next().clamp(0.0, 1.0);
            let feedback = 0.7 + size * 0.28; // 0.70 .. 0.98 (RT60); < 1 for stability
            let input = (l[i] + r[i]) * FIXED_GAIN;
            let mut outl = 0.0;
            let mut outr = 0.0;
            for c in self.combs_l.iter_mut() {
                outl += c.process(input, feedback, damp);
            }
            for c in self.combs_r.iter_mut() {
                outr += c.process(input, feedback, damp);
            }
            for a in self.aps_l.iter_mut() {
                outl = a.process(outl);
            }
            for a in self.aps_r.iter_mut() {
                outr = a.process(outr);
            }
            l[i] = outl;
            r[i] = outr;
        }
    }
    fn set_param(&mut self, p: u8, v: f32) {
        match p {
            RV_SIZE => self.size.set_target(v.clamp(0.0, 1.0)),
            RV_DAMP => self.damp.set_target(v.clamp(0.0, 1.0)),
            _ => {}
        }
    }
    fn ramp_param(&mut self, p: u8, target: f32, frames: u32) {
        match p {
            RV_SIZE => self.size.ramp_to(target.clamp(0.0, 1.0), frames),
            RV_DAMP => self.damp.ramp_to(target.clamp(0.0, 1.0), frames),
            _ => {}
        }
    }
    fn reset(&mut self) {
        for c in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            c.reset();
        }
        for a in self.aps_l.iter_mut().chain(self.aps_r.iter_mut()) {
            a.reset();
        }
    }
    fn tail_frames(&self) -> u32 {
        (2.0 * self.sr) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::fx::sidechain::SidechainBus;
    use crate::test_util::rms;

    const SR: f32 = 48_000.0;

    fn ctx<'a>(sc: &'a SidechainBus) -> FxCtx<'a> {
        FxCtx { sample_rate: SR, cps: 0.5, sidechain: sc }
    }

    #[test]
    fn impulse_produces_a_decaying_tail() {
        let sc = SidechainBus::new(SR);
        let mut rv = Reverb::new(SR);
        let n = SR as usize; // 1 s
        let mut l = vec![0.0f32; n];
        let mut r = vec![0.0f32; n];
        l[0] = 1.0;
        r[0] = 1.0;
        rv.process(&mut l, &mut r, &ctx(&sc));
        // There is energy well after the impulse (a tail), and it decays over time.
        let early = rms(&l[2000..6000]);
        let late = rms(&l[(n - 6000)..(n - 2000)]);
        assert!(early > 1e-5, "a reverb tail is present ({early})");
        assert!(late < early, "the tail decays ({late} < {early})");
        assert!(l.iter().all(|x| x.is_finite()), "stable, finite tail");
    }

    #[test]
    fn more_damping_darkens_the_tail() {
        let sc = SidechainBus::new(SR);
        let tail_hf = |damp: f32| -> f32 {
            let mut rv = Reverb::new(SR);
            rv.set_param(RV_DAMP, damp);
            rv.damp.snap(damp);
            let n = SR as usize / 2;
            let mut l = vec![0.0f32; n];
            let mut r = vec![0.0f32; n];
            l[0] = 1.0;
            r[0] = 1.0;
            rv.process(&mut l, &mut r, &ctx(&sc));
            // crude HF measure: mean absolute first difference of the tail.
            let tail = &l[4000..];
            tail.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>() / tail.len() as f32
        };
        assert!(tail_hf(0.9) < tail_hf(0.1), "heavier damping → less HF in the tail");
    }
}
