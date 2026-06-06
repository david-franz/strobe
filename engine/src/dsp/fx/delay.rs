//! Delay send effect (Phase 3, feature 03): a ping-pong stereo echo with a damped feedback
//! path (the classic "dub" delay that loses highs each repeat). Used as the shared delay send
//! bus. The delay line is pre-allocated to its maximum time; nothing allocates in `process`.

use super::{Effect, FxCtx, Smoothed};

/// fx_param ids (≥0x02 to avoid the chain-reserved FX_MIX/FX_BYPASS).
pub const DLY_TIME: u8 = 0x02; // delay time in seconds
pub const DLY_FEEDBACK: u8 = 0x03; // 0 .. 0.95
pub const DLY_DAMP: u8 = 0x04; // 0 (bright) .. ~0.95 (dark) feedback low-pass

const MAX_SECS: f32 = 2.0;

pub struct Delay {
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    write: usize,
    sr: f32,
    time: Smoothed,     // seconds
    feedback: Smoothed, // 0..0.95
    damp: Smoothed,     // 0..~0.95 (one-pole retention in the feedback path)
    fb_lp: f32,         // feedback-path low-pass state (the R→L return)
}

impl Delay {
    pub fn new(sr: f32) -> Self {
        let max = (MAX_SECS * sr) as usize + 1;
        Self {
            buf_l: vec![0.0; max],
            buf_r: vec![0.0; max],
            write: 0,
            sr,
            time: Smoothed::new(0.375, sr), // ≈ dotted-eighth at ~120 BPM
            feedback: Smoothed::new(0.4, sr),
            damp: Smoothed::new(0.4, sr),
            fb_lp: 0.0,
        }
    }
}

impl Effect for Delay {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], _ctx: &FxCtx) {
        let n = self.buf_l.len();
        for i in 0..l.len() {
            let inl = l[i];
            let inr = r[i];
            let dsamp = ((self.time.next() * self.sr) as usize).clamp(1, n - 1);
            let ri = (self.write + n - dsamp) % n;
            let dl = self.buf_l[ri];
            let dr = self.buf_r[ri];
            let damp = self.damp.next().clamp(0.0, 0.99);
            let fb = self.feedback.next().clamp(0.0, 0.95);
            // Damp the signal that returns from R into L (loses highs each round-trip).
            self.fb_lp = damp * self.fb_lp + (1.0 - damp) * dr;
            if self.fb_lp.abs() < 1e-30 {
                self.fb_lp = 0.0; // flush denormals on a decaying tail
            }
            // Ping-pong: input enters L; L's tap feeds R; R's (damped) tap feeds back into L.
            self.buf_l[self.write] = inl + inr + fb * self.fb_lp;
            self.buf_r[self.write] = dl;
            self.write = (self.write + 1) % n;
            l[i] = dl;
            r[i] = dr;
        }
    }
    fn set_param(&mut self, p: u8, v: f32) {
        match p {
            DLY_TIME => self.time.set_target(v.clamp(0.0, MAX_SECS)),
            DLY_FEEDBACK => self.feedback.set_target(v.clamp(0.0, 0.95)),
            DLY_DAMP => self.damp.set_target(v.clamp(0.0, 0.99)),
            _ => {}
        }
    }
    fn ramp_param(&mut self, p: u8, target: f32, frames: u32) {
        match p {
            DLY_TIME => self.time.ramp_to(target.clamp(0.0, MAX_SECS), frames),
            DLY_FEEDBACK => self.feedback.ramp_to(target.clamp(0.0, 0.95), frames),
            DLY_DAMP => self.damp.ramp_to(target.clamp(0.0, 0.99), frames),
            _ => {}
        }
    }
    fn reset(&mut self) {
        self.buf_l.fill(0.0);
        self.buf_r.fill(0.0);
        self.fb_lp = 0.0;
        self.write = 0;
    }
    fn tail_frames(&self) -> u32 {
        // a few repeats of the longest plausible delay
        (MAX_SECS * self.sr) as u32
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

    #[test]
    fn ping_pong_bounces_left_then_right() {
        let sc = SidechainBus::new(SR);
        let mut d = Delay::new(SR);
        let dsamp = 100usize;
        d.set_param(DLY_TIME, dsamp as f32 / SR);
        d.time.snap(dsamp as f32 / SR);
        d.set_param(DLY_FEEDBACK, 0.5);
        d.feedback.snap(0.5);

        // A single left-channel impulse, then silence.
        let len = dsamp * 3;
        let mut lch = vec![0.0f32; len];
        let mut rch = vec![0.0f32; len];
        lch[0] = 1.0;
        d.process(&mut lch, &mut rch, &ctx(&sc));

        // First echo lands on the LEFT around `dsamp`, the bounce on the RIGHT around `2*dsamp`.
        assert!(lch[dsamp].abs() > 0.5, "left echo at dsamp ({})", lch[dsamp]);
        assert!(rch[2 * dsamp].abs() > 0.2, "right bounce at 2·dsamp ({})", rch[2 * dsamp]);
        assert!(rch[dsamp].abs() < 1e-3, "right is quiet at the first echo");
    }

    #[test]
    fn high_feedback_decays_and_stays_bounded() {
        let sc = SidechainBus::new(SR);
        let mut d = Delay::new(SR);
        d.set_param(DLY_TIME, 0.005);
        d.time.snap(0.005);
        d.set_param(DLY_FEEDBACK, 0.95);
        d.feedback.snap(0.95);
        let mut lch = vec![0.0f32; (SR as usize) / 2];
        let mut rch = vec![0.0f32; (SR as usize) / 2];
        lch[0] = 1.0;
        d.process(&mut lch, &mut rch, &ctx(&sc));
        let tail = |s: &[f32]| s.iter().rev().take(2000).fold(0.0f32, |m, &x| m.max(x.abs()));
        let early = lch[..2000].iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(tail(&lch) < early, "feedback decays rather than runs away");
        assert!(lch.iter().all(|&x| x.abs() <= 2.0), "stays bounded");
    }
}
