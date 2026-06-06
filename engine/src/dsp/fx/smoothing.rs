//! A one-pole parameter smoother. Every effect parameter glides toward its target on the
//! audio thread so no setter ever steps a coefficient (zipper noise / clicks). Supports a
//! linear ParamRamp (a fixed-length glide to a target) on top of the default exponential
//! glide.

/// Default smoothing time for a `set_target` (ms).
const DEFAULT_SMOOTH_MS: f32 = 5.0;

pub struct Smoothed {
    value: f32,
    target: f32,
    coeff: f32, // exponential one-pole coefficient
    // linear ramp (ParamRamp): when ramp_left > 0, step linearly by ramp_step.
    ramp_left: u32,
    ramp_step: f32,
    sr: f32,
}

fn one_pole_coeff(ms: f32, sr: f32) -> f32 {
    if ms <= 0.0 {
        1.0
    } else {
        1.0 - (-1.0 / (ms * 0.001 * sr)).exp()
    }
}

impl Smoothed {
    pub fn new(initial: f32, sr: f32) -> Self {
        Self {
            value: initial,
            target: initial,
            coeff: one_pole_coeff(DEFAULT_SMOOTH_MS, sr),
            ramp_left: 0,
            ramp_step: 0.0,
            sr,
        }
    }

    /// Jump immediately (no glide), e.g. on reset.
    pub fn snap(&mut self, v: f32) {
        self.value = v;
        self.target = v;
        self.ramp_left = 0;
    }

    /// Set the exponential glide time.
    pub fn set_smooth_ms(&mut self, ms: f32) {
        self.coeff = one_pole_coeff(ms, self.sr);
    }

    /// New target, reached via the exponential glide.
    pub fn set_target(&mut self, t: f32) {
        self.target = t;
        self.ramp_left = 0;
    }

    /// New target reached linearly over `frames` (a ParamRamp).
    pub fn ramp_to(&mut self, target: f32, frames: u32) {
        if frames == 0 {
            self.snap(target);
            return;
        }
        self.target = target;
        self.ramp_left = frames;
        self.ramp_step = (target - self.value) / frames as f32;
    }

    /// Advance one sample and return the new value.
    #[inline]
    pub fn next(&mut self) -> f32 {
        if self.ramp_left > 0 {
            self.ramp_left -= 1;
            self.value += self.ramp_step;
            if self.ramp_left == 0 {
                self.value = self.target;
            }
        } else {
            self.value += self.coeff * (self.target - self.value);
        }
        self.value
    }

    /// Current value without advancing.
    #[inline]
    pub fn cur(&self) -> f32 {
        self.value
    }

    pub fn target(&self) -> f32 {
        self.target
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SR: f32 = 48_000.0;

    #[test]
    fn glides_toward_target_without_stepping() {
        let mut s = Smoothed::new(0.0, SR);
        s.set_target(1.0);
        let first = s.next();
        assert!(first > 0.0 && first < 0.1, "small first step ({first})");
        for _ in 0..2000 {
            s.next();
        }
        assert!((s.cur() - 1.0).abs() < 0.02, "reaches target");
    }

    #[test]
    fn linear_ramp_reaches_target_in_frames() {
        let mut s = Smoothed::new(100.0, SR);
        s.ramp_to(1100.0, 1000);
        let mut prev = s.cur();
        for _ in 0..999 {
            let v = s.next();
            assert!(v > prev, "monotone ramp");
            prev = v;
        }
        s.next();
        assert!((s.cur() - 1100.0).abs() < 1e-3, "hits target exactly");
    }

    #[test]
    fn snap_is_instant() {
        let mut s = Smoothed::new(0.0, SR);
        s.snap(5.0);
        assert_eq!(s.next(), 5.0);
    }
}
