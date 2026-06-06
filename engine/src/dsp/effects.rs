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
        // Flush the recursive state out of the denormal range during long silences; denormals
        // cause per-sample CPU spikes on some targets (a soft real-time hazard).
        let y = if y.abs() < 1e-30 { 0.0 } else { y };
        self.y1 = y;
        y
    }
}

fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Sliding-window maximum of the input peak across the look-ahead window (a monotonic
/// deque — O(1) amortized, allocation-free). The limiter needs its gain target to track the
/// loudest sample anywhere in the delay, not just the newest one, so the reduction is fully
/// ramped in by the time that sample reaches the output. Without this the gain target only
/// "sees" a transient for a single sample and releases before the peak exits the delay.
struct PeakWindow {
    val: Vec<f32>,
    idx: Vec<u64>,
    head: usize,
    len: usize,
    cap: usize,
    window: u64,
    n: u64,
}

impl PeakWindow {
    fn new(window: usize) -> Self {
        let cap = window + 1;
        Self { val: vec![0.0; cap], idx: vec![0; cap], head: 0, len: 0, cap, window: window as u64, n: 0 }
    }

    /// Push the newest peak; return the maximum over the trailing look-ahead window.
    #[inline]
    fn push(&mut self, v: f32) -> f32 {
        let i = self.n;
        self.n += 1;
        // Drop front candidates that have fallen outside the window.
        while self.len > 0 && self.idx[self.head] + self.window < i {
            self.head = (self.head + 1) % self.cap;
            self.len -= 1;
        }
        // Drop tail candidates this sample dominates (they can never be the max again).
        while self.len > 0 {
            let back = (self.head + self.len - 1) % self.cap;
            if self.val[back] <= v {
                self.len -= 1;
            } else {
                break;
            }
        }
        let slot = (self.head + self.len) % self.cap;
        self.val[slot] = v;
        self.idx[slot] = i;
        self.len += 1;
        self.val[self.head]
    }
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
    peak: PeakWindow, // windowed peak detector over the look-ahead window
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
            peak: PeakWindow::new(lookahead),
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

            // Target gain from the loudest sample still in the look-ahead window (so the
            // reduction is in place before that sample exits the delay); attack fast, release slow.
            let peak = self.peak.push(xl.abs().max(xr.abs()));
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
    fn high_pass_state_does_not_linger_as_a_denormal() {
        let mut hp = OnePoleHp::new(25.0, SR);
        hp.process(1.0); // excite, then go silent
        for _ in 0..24_000 {
            hp.process(0.0);
        }
        // Without the flush the state would still be a tiny (denormal-prone) nonzero here.
        assert_eq!(hp.process(0.0), 0.0, "state flushed to exact zero");
    }

    #[test]
    fn lookahead_ducks_before_a_peak_reaches_the_output() {
        // A quiet stream with one loud spike. With working look-ahead the gain has already
        // ducked by the time the spike exits the delay, so the (quiet) samples just before
        // the spike's output position are attenuated well below their input level. The old
        // bug computed gain from the newest input and applied it to the *delayed* sample, so
        // the gain had released by the time the spike came out — caught only by the hard clip.
        let lookahead_ms = 3.0;
        let mut bus = MasterBus::with_params(SR, lookahead_ms, -0.3, 80.0, 1.0, 25.0);
        let look = (lookahead_ms * 0.001 * SR) as usize;
        let quiet = 0.1f32;
        let spike_at = 2000usize;
        let mut l: Vec<f32> = (0..4000).map(|i| if i == spike_at { 6.0 } else { quiet }).collect();
        let mut r = l.clone();
        bus.process(&mut l, &mut r);

        let ceiling = db_to_lin(-0.3);
        let out_spike = spike_at + look;
        assert!(l[out_spike].abs() <= ceiling + 1e-4, "spike limited ({})", l[out_spike]);
        let pre = l[out_spike - 10].abs();
        assert!(pre < 0.5 * quiet, "gain ducked ahead of the peak ({pre} vs {quiet})");
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
