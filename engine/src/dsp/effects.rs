//! Master bus + limiter — the last stage before cpal. Sums (already done by the pool),
//! applies master gain and a gentle DC/sub high-pass, then a stereo-linked look-ahead
//! brick-wall limiter so the output can never clip while staying transparent on a normal
//! mix. (Per-track FX chains and sends are Phase 3; this is the single master bus.)

/// One-pole DC/sub high-pass (removes inaudible sub-bass that would eat headroom).
pub struct OnePoleHp {
    r: f32,
    x1: f32,
    y1: f32,
}

impl OnePoleHp {
    pub fn new(fc_hz: f32, sr: f32) -> Self {
        let r = (1.0 - 2.0 * std::f32::consts::PI * fc_hz / sr).clamp(0.0, 0.9999);
        Self { r, x1: 0.0, y1: 0.0 }
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.r * (self.y1 + x - self.x1);
        self.x1 = x;
        self.y1 = y;
        y
    }
}

fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// The master bus: gain → DC high-pass → look-ahead limiter → safety ceiling.
pub struct MasterBus {
    master_gain: f32,
    ceiling: f32, // linear
    attack_coef: f32,
    release_coef: f32,
    hp_l: OnePoleHp,
    hp_r: OnePoleHp,
    // look-ahead delay (ring of `lookahead` samples per channel)
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    didx: usize,
    lookahead: usize,
    gain: f32, // current limiter gain (stereo-linked)
    sr: f32,
}

impl MasterBus {
    pub fn new(sr: f32) -> Self {
        Self::with_params(sr, 3.0, -0.3, 80.0, 0.8, 25.0)
    }

    pub fn with_params(
        sr: f32,
        lookahead_ms: f32,
        ceiling_db: f32,
        release_ms: f32,
        master_gain: f32,
        dc_hp_hz: f32,
    ) -> Self {
        let lookahead = ((lookahead_ms * 0.001 * sr) as usize).max(1);
        Self {
            master_gain,
            ceiling: db_to_lin(ceiling_db),
            // Attack reaches the target within the look-ahead window (so a peak is already
            // attenuated by the time it exits the delay); the safety clip catches residue.
            attack_coef: 1.0 - (-5.0 / lookahead as f32).exp(),
            release_coef: 1.0 - (-1.0 / (release_ms * 0.001 * sr)).exp(),
            hp_l: OnePoleHp::new(dc_hp_hz, sr),
            hp_r: OnePoleHp::new(dc_hp_hz, sr),
            delay_l: vec![0.0; lookahead],
            delay_r: vec![0.0; lookahead],
            didx: 0,
            lookahead,
            gain: 1.0,
            sr,
        }
    }

    pub fn set_master_gain(&mut self, g: f32) {
        self.master_gain = g.max(0.0);
    }
    pub fn set_ceiling_db(&mut self, db: f32) {
        self.ceiling = db_to_lin(db);
    }
    pub fn set_release_ms(&mut self, ms: f32) {
        self.release_coef = 1.0 - (-1.0 / (ms.max(1.0) * 0.001 * self.sr)).exp();
    }

    /// Master/global `ParamSet` (track 0): 0 = gain, 1 = ceiling_db, 2 = release_ms.
    pub fn set_master_param(&mut self, param: u32, value: f32) {
        match param {
            0 => self.set_master_gain(value),
            1 => self.set_ceiling_db(value),
            2 => self.set_release_ms(value),
            _ => {}
        }
    }

    /// Process the master mix in place. Stereo-linked: L and R share one gain so a centered
    /// signal stays centered.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (sl, sr) in l.iter_mut().zip(r.iter_mut()) {
            // master gain + DC high-pass
            let xl = self.hp_l.process(*sl * self.master_gain);
            let xr = self.hp_r.process(*sr * self.master_gain);

            // delayed output samples (look-ahead)
            let dl = self.delay_l[self.didx];
            let dr = self.delay_r[self.didx];
            self.delay_l[self.didx] = xl;
            self.delay_r[self.didx] = xr;
            self.didx = (self.didx + 1) % self.lookahead;

            // target gain from the incoming (future) peak; attack fast, release slow.
            let peak = xl.abs().max(xr.abs());
            let target = if peak > self.ceiling { self.ceiling / peak } else { 1.0 };
            let coef = if target < self.gain { self.attack_coef } else { self.release_coef };
            self.gain += coef * (target - self.gain);

            // apply linked gain, then a hard safety clip that guarantees |out| <= ceiling.
            *sl = (dl * self.gain).clamp(-self.ceiling, self.ceiling);
            *sr = (dr * self.gain).clamp(-self.ceiling, self.ceiling);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn output_never_exceeds_the_ceiling_on_a_hot_mix() {
        let mut bus = MasterBus::new(SR);
        let ceiling = db_to_lin(-0.3);
        // A deliberately hot signal well over 0 dBFS.
        let mut l: Vec<f32> = (0..4800).map(|i| 4.0 * (i as f32 * 0.05).sin()).collect();
        let mut r = l.clone();
        bus.process(&mut l, &mut r);
        let peak = l.iter().chain(r.iter()).fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(peak <= ceiling + 1e-6, "peak {peak} within ceiling {ceiling}");
    }

    #[test]
    fn transparent_on_a_moderate_mix() {
        // A -12 dBFS sine: the limiter idles (gain ~1), output ≈ input * master_gain.
        let mut bus = MasterBus::with_params(SR, 3.0, -0.3, 80.0, 1.0, 25.0);
        let input: Vec<f32> = (0..SR as usize).map(|i| 0.25 * (i as f32 * 0.06).sin()).collect();
        let mut l = input.clone();
        let mut r = input.clone();
        bus.process(&mut l, &mut r);
        // Compare RMS away from the initial look-ahead delay; should match the input.
        let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
        let ratio = rms(&l[1000..]) / rms(&input[1000..]);
        assert!((ratio - 1.0).abs() < 0.05, "near-unity (idle limiter), ratio {ratio}");
    }

    #[test]
    fn stereo_linked_keeps_a_centered_signal_centered() {
        let mut bus = MasterBus::new(SR);
        let mut l: Vec<f32> = (0..4800).map(|i| 3.0 * (i as f32 * 0.05).sin()).collect();
        let mut r = l.clone();
        bus.process(&mut l, &mut r);
        for (a, b) in l.iter().zip(r.iter()) {
            assert!((a - b).abs() < 1e-6, "L and R stay identical through limiting");
        }
    }

    #[test]
    fn master_gain_param_scales_the_output() {
        let sig: Vec<f32> = (0..SR as usize).map(|i| 0.2 * (i as f32 * 0.06).sin()).collect();
        let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();

        let mut a = MasterBus::with_params(SR, 3.0, -0.3, 80.0, 0.8, 25.0);
        let (mut al, mut ar) = (sig.clone(), sig.clone());
        a.process(&mut al, &mut ar);

        let mut b = MasterBus::with_params(SR, 3.0, -0.3, 80.0, 0.8, 25.0);
        b.set_master_param(0, 0.4); // half gain
        let (mut bl, mut br) = (sig.clone(), sig.clone());
        b.process(&mut bl, &mut br);

        let ratio = rms(&bl[1000..]) / rms(&al[1000..]);
        assert!((ratio - 0.5).abs() < 0.05, "0.4/0.8 ≈ 0.5 ({ratio})");
    }

    #[test]
    fn dc_high_pass_removes_sub_offset() {
        let mut bus = MasterBus::with_params(SR, 3.0, -0.3, 80.0, 1.0, 25.0);
        // Constant DC + an audible tone.
        let mut l: Vec<f32> = (0..SR as usize).map(|i| 0.5 + 0.2 * (i as f32 * 0.06).sin()).collect();
        let mut r = l.clone();
        bus.process(&mut l, &mut r);
        let mean: f32 = l[2000..].iter().sum::<f32>() / l[2000..].len() as f32;
        assert!(mean.abs() < 0.02, "DC removed (mean {mean})");
    }
}
