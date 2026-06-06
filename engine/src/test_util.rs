//! Test-only DSP measurement helpers (no FFT crate needed): single-bin DFT (Goertzel),
//! RMS, and zero-crossing pitch estimation. Shared across the voice/filter tests.

#![cfg(test)]
#![allow(dead_code)] // helpers accumulate across features; not all used in every build

use std::f32::consts::PI;

/// Magnitude of `signal` at frequency `freq` (a single-bin DFT via Goertzel).
pub fn goertzel(signal: &[f32], freq: f32, sr: f32) -> f32 {
    let w = 2.0 * PI * freq / sr;
    let cos = w.cos();
    let coeff = 2.0 * cos;
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for &x in signal {
        let s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let real = s1 - s2 * cos;
    let imag = s2 * w.sin();
    (real * real + imag * imag).sqrt() / (signal.len() as f32 / 2.0)
}

/// RMS level of a signal.
pub fn rms(signal: &[f32]) -> f32 {
    if signal.is_empty() {
        return 0.0;
    }
    (signal.iter().map(|&x| x * x).sum::<f32>() / signal.len() as f32).sqrt()
}

/// Deterministic white noise in [-1, 1] (xorshift), for reproducible filter tests.
pub fn white_noise(n: usize, seed: u32) -> Vec<f32> {
    let mut st = seed | 1;
    (0..n)
        .map(|_| {
            st ^= st << 13;
            st ^= st >> 17;
            st ^= st << 5;
            (st as f32 / u32::MAX as f32) * 2.0 - 1.0
        })
        .collect()
}

/// Estimate fundamental frequency by counting rising zero-crossings.
pub fn zero_crossing_freq(signal: &[f32], sr: f32) -> f32 {
    let mut crossings = 0usize;
    for w in signal.windows(2) {
        if w[0] <= 0.0 && w[1] > 0.0 {
            crossings += 1;
        }
    }
    let secs = signal.len() as f32 / sr;
    crossings as f32 / secs
}
