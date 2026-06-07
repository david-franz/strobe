//! Bitcrush insert (Phase 3, feature 04): bit-depth reduction (quantization "fuzz") plus
//! sample-rate decimation (a sample-and-hold for the lo-fi / aliased character). Aliasing here
//! is the *feature*, so this is intentionally not oversampled. RT-safe: a few floats of state,
//! no allocation in `process`.

use super::{Effect, FxCtx, Smoothed};

/// fx_param ids (≥0x02 to avoid the chain-reserved FX_MIX/FX_BYPASS).
pub const BC_BITS: u8 = 0x02; // effective bit depth, 1..16 (16 = transparent)
pub const BC_DOWNSAMPLE: u8 = 0x03; // sample-and-hold factor, 1..64 (1 = no decimation)

pub struct Bitcrush {
    bits: Smoothed,
    downsample: Smoothed,
    hold_l: f32,
    hold_r: f32,
    phase: f32,
}

impl Bitcrush {
    pub fn new(sr: f32) -> Self {
        Self {
            bits: Smoothed::new(16.0, sr),
            downsample: Smoothed::new(1.0, sr),
            hold_l: 0.0,
            hold_r: 0.0,
            phase: 0.0,
        }
    }
}

impl Effect for Bitcrush {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], _ctx: &FxCtx) {
        for i in 0..l.len() {
            let ds = self.downsample.next().max(1.0);
            let bits = self.bits.next().clamp(1.0, 16.0);
            self.phase += 1.0;
            if self.phase >= ds {
                self.phase -= ds;
                let q = 2.0f32.powf(bits);
                self.hold_l = (l[i] * q).round() / q; // quantize + capture
                self.hold_r = (r[i] * q).round() / q;
            }
            l[i] = self.hold_l; // hold between captures (decimate)
            r[i] = self.hold_r;
        }
    }
    fn set_param(&mut self, p: u8, v: f32) {
        match p {
            BC_BITS => self.bits.set_target(v.clamp(1.0, 16.0)),
            BC_DOWNSAMPLE => self.downsample.set_target(v.clamp(1.0, 64.0)),
            _ => {}
        }
    }
    fn ramp_param(&mut self, p: u8, target: f32, frames: u32) {
        match p {
            BC_BITS => self.bits.ramp_to(target.clamp(1.0, 16.0), frames),
            BC_DOWNSAMPLE => self.downsample.ramp_to(target.clamp(1.0, 64.0), frames),
            _ => {}
        }
    }
    fn reset(&mut self) {
        self.hold_l = 0.0;
        self.hold_r = 0.0;
        self.phase = 0.0;
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
        (0..n).map(|i| 0.8 * (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin()).collect()
    }

    #[test]
    fn low_bits_quantize_to_a_coarse_grid() {
        let sc = SidechainBus::new(SR);
        let mut bc = Bitcrush::new(SR);
        bc.set_param(BC_BITS, 2.0);
        bc.bits.snap(2.0); // q = 2^2 = 4 → multiples of 0.25
        let mut l = tone(500.0, 4096);
        let mut r = l.clone();
        bc.process(&mut l, &mut r, &ctx(&sc));
        let q = 4.0f32;
        assert!(
            l.iter().all(|&x| ((x * q).round() / q - x).abs() < 1e-5),
            "every output sits on the 1/4 quantization grid",
        );
        // genuinely coarse: very few distinct levels.
        let mut levels: Vec<i32> = l.iter().map(|&x| (x * q).round() as i32).collect();
        levels.sort_unstable();
        levels.dedup();
        assert!(levels.len() <= 9, "few quantization levels ({})", levels.len());
    }

    #[test]
    fn sixteen_bits_is_transparent() {
        let sc = SidechainBus::new(SR);
        let mut bc = Bitcrush::new(SR); // bits=16, downsample=1
        let input = tone(500.0, 4096);
        let mut l = input.clone();
        let mut r = input.clone();
        bc.process(&mut l, &mut r, &ctx(&sc));
        let err = l.iter().zip(&input).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(err < 1e-3, "16-bit depth, no decimation ≈ transparent ({err})");
    }

    #[test]
    fn downsample_holds_samples() {
        let sc = SidechainBus::new(SR);
        let mut bc = Bitcrush::new(SR);
        bc.set_param(BC_DOWNSAMPLE, 8.0);
        bc.downsample.snap(8.0);
        let mut l = tone(500.0, 4096);
        let mut r = l.clone();
        bc.process(&mut l, &mut r, &ctx(&sc));
        // With a hold of 8, ~7 of every 8 samples equal the previous (decimated staircase).
        let held = (1..l.len()).filter(|&i| (l[i] - l[i - 1]).abs() < 1e-9).count();
        let frac = held as f32 / (l.len() - 1) as f32;
        assert!(frac > 0.8, "most samples are held (decimated): {frac}");
    }
}
