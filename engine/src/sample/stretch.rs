//! Decoupled time-stretch + pitch-shift (Phase 4, feature 03), done **offline** at prepare time
//! (never on the audio thread). Hidden behind a [`StretchEngine`] trait so a higher-quality
//! backend (Signalsmith Stretch / `ssstretch`) can be swapped in later; the bundled engine is a
//! pure-Rust WSOLA time-stretcher + linear resampler.
//!
//! Time-stretch changes duration while preserving pitch (WSOLA overlap-add with correlation
//! alignment). Pitch-shift = stretch by the pitch ratio then resample back, so pitch changes
//! while duration is preserved. The result is a new buffer the sampler plays at `speed = 1.0`.

/// A swappable offline stretch/pitch backend.
pub trait StretchEngine: Send + Sync {
    /// Process per-channel planes: scale duration by `stretch` (>0) and shift pitch by
    /// `pitch_semitones`. Returns new planes. `stretch == 1.0 && pitch == 0.0` is a passthrough.
    fn process(&self, planes: &[Vec<f32>], stretch: f64, pitch_semitones: f64) -> Vec<Vec<f32>>;
}

/// WSOLA time-stretch + linear-resample pitch — pure Rust, decent quality with the artifacts
/// expected of a time-domain stretcher (the Signalsmith backend is the higher-quality swap-in).
pub struct Wsola;

const WIN: usize = 1024;
const HOP_SYN: usize = WIN / 4; // 75% overlap
const SEARCH: isize = (WIN / 4) as isize;

fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos()).collect()
}

/// WSOLA time-stretch of one channel by `stretch` (output ≈ input.len()·stretch frames).
fn wsola(input: &[f32], stretch: f64, window: &[f32]) -> Vec<f32> {
    let n = input.len();
    if (stretch - 1.0).abs() < 1e-6 || n < WIN * 2 {
        // passthrough / too short to stretch meaningfully
        return scale_len(input, stretch);
    }
    let ha = (HOP_SYN as f64 / stretch).max(1.0).round() as usize; // analysis hop
    let out_len = (n as f64 * stretch) as usize + WIN;
    let mut out = vec![0.0f32; out_len];
    let mut wsum = vec![0.0f32; out_len];

    let mut a: isize = 0;
    let mut s: usize = 0;
    // `target` = the samples the next grain should continue smoothly from (the previous grain
    // advanced by the synthesis hop). Start with the head.
    let mut target: Vec<f32> = (0..WIN).map(|i| input.get(i).copied().unwrap_or(0.0)).collect();

    while (a as usize) + WIN < n && s + WIN < out_len {
        let delta = best_offset(input, a, &target);
        let start = (a + delta).clamp(0, (n as isize - WIN as isize).max(0)) as usize;
        for i in 0..WIN {
            let v = input.get(start + i).copied().unwrap_or(0.0);
            out[s + i] += v * window[i];
            wsum[s + i] += window[i];
        }
        let tstart = start + HOP_SYN;
        for (i, t) in target.iter_mut().enumerate() {
            *t = input.get(tstart + i).copied().unwrap_or(0.0);
        }
        s += HOP_SYN;
        a += ha as isize;
    }
    for i in 0..out_len {
        if wsum[i] > 1e-6 {
            out[i] /= wsum[i];
        }
    }
    out.truncate((n as f64 * stretch) as usize);
    out
}

/// Best alignment offset in `[-SEARCH, SEARCH]` around `a` maximizing correlation with `target`.
fn best_offset(input: &[f32], a: isize, target: &[f32]) -> isize {
    let n = input.len() as isize;
    let mut best_delta = 0isize;
    let mut best_score = f32::NEG_INFINITY;
    let mut delta = -SEARCH;
    while delta <= SEARCH {
        let start = a + delta;
        if start >= 0 && start + WIN as isize <= n {
            let mut score = 0.0f32;
            // a coarse correlation (every 4th sample) — plenty for alignment, 4× cheaper
            let mut i = 0usize;
            while i < WIN {
                score += input[(start as usize) + i] * target[i];
                i += 4;
            }
            if score > best_score {
                best_score = score;
                best_delta = delta;
            }
        }
        delta += 1;
    }
    best_delta
}

/// Linear resample to `ratio`× the length (ratio>1 = longer/lower if used as a pitch step).
fn scale_len(input: &[f32], ratio: f64) -> Vec<f32> {
    if (ratio - 1.0).abs() < 1e-9 {
        return input.to_vec();
    }
    let out_len = ((input.len() as f64) * ratio).max(1.0) as usize;
    (0..out_len)
        .map(|j| {
            let pos = j as f64 / ratio;
            let i = pos.floor() as usize;
            let f = (pos - i as f64) as f32;
            let a = input.get(i).copied().unwrap_or(0.0);
            let b = input.get(i + 1).copied().unwrap_or(a);
            a + (b - a) * f
        })
        .collect()
}

impl StretchEngine for Wsola {
    fn process(&self, planes: &[Vec<f32>], stretch: f64, pitch_semitones: f64) -> Vec<Vec<f32>> {
        let stretch = stretch.max(0.01);
        let p = 2f64.powf(pitch_semitones / 12.0); // pitch ratio
        let window = hann(WIN);
        planes
            .iter()
            .map(|ch| {
                // time-stretch by stretch·p, then resample by 1/p → duration ×stretch, pitch ×p.
                let stretched = wsola(ch, stretch * p, &window);
                if (p - 1.0).abs() < 1e-6 {
                    stretched
                } else {
                    scale_len(&stretched, 1.0 / p)
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(freq: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin()).collect()
    }

    fn dominant_freq(s: &[f32]) -> f32 {
        let mut crossings = 0usize;
        for w in s.windows(2) {
            if w[0] <= 0.0 && w[1] > 0.0 {
                crossings += 1;
            }
        }
        crossings as f32 / (s.len() as f32 / SR)
    }

    #[test]
    fn stretch_changes_duration_keeps_pitch() {
        let inp = sine(440.0, 24_000); // 0.5 s
        let out = Wsola.process(&[inp], 2.0, 0.0);
        let ch = &out[0];
        assert!((ch.len() as i64 - 48_000).abs() < 2000, "≈2× length ({})", ch.len());
        let f = dominant_freq(&ch[2000..ch.len() - 2000]);
        assert!((f - 440.0).abs() < 12.0, "pitch preserved (got {f})");
    }

    #[test]
    fn pitch_up_octave_keeps_duration() {
        let inp = sine(440.0, 24_000);
        let n = inp.len();
        let out = Wsola.process(&[inp], 1.0, 12.0); // +12 semitones
        let ch = &out[0];
        assert!((ch.len() as i64 - n as i64).abs() < 2500, "≈same length ({})", ch.len());
        let f = dominant_freq(&ch[2000..ch.len() - 2000]);
        assert!((f - 880.0).abs() < 25.0, "pitch ≈ 880 Hz (got {f})");
    }

    #[test]
    fn passthrough_when_neutral() {
        let inp = sine(220.0, 8000);
        let out = Wsola.process(&[inp.clone()], 1.0, 0.0);
        assert_eq!(out[0].len(), inp.len());
    }

    #[test]
    fn pitch_down_keeps_duration() {
        let inp = sine(440.0, 24_000);
        let n = inp.len();
        let out = Wsola.process(&[inp], 1.0, -12.0); // octave down
        let ch = &out[0];
        assert!((ch.len() as i64 - n as i64).abs() < 2500, "≈same length");
        let f = dominant_freq(&ch[2000..ch.len() - 2000]);
        assert!((f - 220.0).abs() < 15.0, "pitch ≈ 220 Hz (got {f})");
    }
}
