//! Track-insert filters & EQ (Phase 3, feature 02).
//!
//! - [`PerfFilter`]: a stereo, sweepable SVF "performance filter" — the genre-defining filter
//!   sweep applied to a *whole track's* summed signal (distinct from the per-voice SVF, which
//!   carves each note). LP/HP/BP nodes are all pre-built; `mode` selects which is ticked, so
//!   switching type never allocates. Cutoff/Q are smoothed so a stepped or ramped automation
//!   arrives as a clean glide.
//! - [`Eq`]: a 3-band EQ (low shelf + peaking mid + high shelf), hand-rolled RBJ biquads with
//!   smoothed freq/gain/Q. A band at 0 dB is transparent.
//!
//! RT-safe: pre-built nodes, smoothed params, no allocation in `process`.

use fundsp::hacker32::*;

use super::{Effect, FxCtx, Smoothed};

// ── Performance filter ──────────────────────────────────────────────────────

/// fx_param ids. Cutoff is addressed per mode — setting one also selects the SVF mode, so a
/// single TS event (`.lpf`/`.hpf`/`.bpf`) both picks the type and sets/ramps the frequency.
/// All ≥ 0x02 to avoid the chain-reserved `FX_MIX` (0x00) / `FX_BYPASS` (0x01).
pub const PF_CUTOFF_LP: u8 = 0x02;
pub const PF_Q: u8 = 0x03;
pub const PF_CUTOFF_HP: u8 = 0x04;
pub const PF_CUTOFF_BP: u8 = 0x05;

pub struct PerfFilter {
    nodes_l: [Box<dyn AudioUnit>; 3], // [lp, hp, bp]
    nodes_r: [Box<dyn AudioUnit>; 3],
    mode: usize,
    cutoff: Smoothed,
    q: Smoothed,
    nyquist: f32,
}

impl PerfFilter {
    pub fn new(sr: f32) -> Self {
        Self {
            nodes_l: [Box::new(lowpass()), Box::new(highpass()), Box::new(bandpass())],
            nodes_r: [Box::new(lowpass()), Box::new(highpass()), Box::new(bandpass())],
            mode: 0,
            cutoff: Smoothed::new(1000.0, sr),
            q: Smoothed::new(0.7, sr),
            nyquist: sr * 0.5,
        }
    }
}

impl Effect for PerfFilter {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], _ctx: &FxCtx) {
        let mode = self.mode;
        let mut o = [0.0f32; 1];
        for i in 0..l.len() {
            // `max`/`min` (not `clamp`) so a non-finite swept value can't propagate as NaN.
            let c = self.cutoff.next().max(20.0).min(self.nyquist - 1.0);
            let q = self.q.next().max(0.5).min(20.0);
            self.nodes_l[mode].tick(&[l[i], c, q], &mut o);
            l[i] = o[0];
            self.nodes_r[mode].tick(&[r[i], c, q], &mut o);
            r[i] = o[0];
        }
    }
    fn set_param(&mut self, p: u8, v: f32) {
        match p {
            PF_CUTOFF_LP => {
                self.mode = 0;
                self.cutoff.set_target(v);
            }
            PF_CUTOFF_HP => {
                self.mode = 1;
                self.cutoff.set_target(v);
            }
            PF_CUTOFF_BP => {
                self.mode = 2;
                self.cutoff.set_target(v);
            }
            PF_Q => self.q.set_target(v),
            _ => {}
        }
    }
    fn ramp_param(&mut self, p: u8, target: f32, frames: u32) {
        match p {
            PF_CUTOFF_LP => {
                self.mode = 0;
                self.cutoff.ramp_to(target, frames);
            }
            PF_CUTOFF_HP => {
                self.mode = 1;
                self.cutoff.ramp_to(target, frames);
            }
            PF_CUTOFF_BP => {
                self.mode = 2;
                self.cutoff.ramp_to(target, frames);
            }
            PF_Q => self.q.ramp_to(target, frames),
            _ => {}
        }
    }
    fn reset(&mut self) {
        for n in self.nodes_l.iter_mut().chain(self.nodes_r.iter_mut()) {
            n.reset();
        }
    }
}

// ── 3-band EQ ───────────────────────────────────────────────────────────────

/// EQ bands: 0 = low shelf, 1 = peaking mid, 2 = high shelf.
pub const EQ_BANDS: usize = 3;
/// Per-band params start here; band `b` uses `EQ_PARAM_BASE + b*4` (freq), `+1` (gain dB), `+2` (Q).
pub const EQ_PARAM_BASE: u8 = 0x10;

#[derive(Clone, Copy)]
enum BandKind {
    LowShelf,
    Peak,
    HighShelf,
}

/// A transposed-direct-form-II biquad.
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn identity() -> Self {
        Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        // Flush denormals during silence (cheap; avoids per-sample CPU spikes).
        self.y1 = if y.abs() < 1e-30 { 0.0 } else { y };
        self.y1
    }
    fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// RBJ cookbook coefficients, normalized by a0. Returns `(b0, b1, b2, a1, a2)`.
fn rbj_coeffs(kind: BandKind, freq: f32, gain_db: f32, q: f32, sr: f32) -> (f32, f32, f32, f32, f32) {
    let f = freq.max(20.0).min(sr * 0.5 - 1.0);
    let a = 10f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * f / sr;
    let (sn, cs) = w0.sin_cos();
    let alpha = sn / (2.0 * q.max(0.1));
    let (b0, b1, b2, a0, a1, a2) = match kind {
        BandKind::Peak => (
            1.0 + alpha * a,
            -2.0 * cs,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cs,
            1.0 - alpha / a,
        ),
        BandKind::LowShelf => {
            let s = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) - (a - 1.0) * cs + s),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                a * ((a + 1.0) - (a - 1.0) * cs - s),
                (a + 1.0) + (a - 1.0) * cs + s,
                -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                (a + 1.0) + (a - 1.0) * cs - s,
            )
        }
        BandKind::HighShelf => {
            let s = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) + (a - 1.0) * cs + s),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                a * ((a + 1.0) + (a - 1.0) * cs - s),
                (a + 1.0) - (a - 1.0) * cs + s,
                2.0 * ((a - 1.0) - (a + 1.0) * cs),
                (a + 1.0) - (a - 1.0) * cs - s,
            )
        }
    };
    (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
}

struct EqBand {
    kind: BandKind,
    freq: Smoothed,
    gain_db: Smoothed,
    q: Smoothed,
    bl: Biquad,
    br: Biquad,
    sr: f32,
    last: (f32, f32, f32), // (freq, gain, q) the current coeffs were computed for
}

impl EqBand {
    fn new(kind: BandKind, freq: f32, gain_db: f32, q: f32, sr: f32) -> Self {
        Self {
            kind,
            freq: Smoothed::new(freq, sr),
            gain_db: Smoothed::new(gain_db, sr),
            q: Smoothed::new(q, sr),
            bl: Biquad::identity(),
            br: Biquad::identity(),
            sr,
            last: (-1.0, f32::NAN, -1.0), // force a recompute on the first sample
        }
    }
    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let f = self.freq.next();
        let g = self.gain_db.next();
        let qv = self.q.next();
        // Recompute coeffs only when a smoothed param actually moved (free when static).
        if (f - self.last.0).abs() > 1e-3 || (g - self.last.1).abs() > 1e-4 || (qv - self.last.2).abs() > 1e-4 {
            let (b0, b1, b2, a1, a2) = rbj_coeffs(self.kind, f, g, qv, self.sr);
            self.bl.b0 = b0;
            self.bl.b1 = b1;
            self.bl.b2 = b2;
            self.bl.a1 = a1;
            self.bl.a2 = a2;
            self.br.b0 = b0;
            self.br.b1 = b1;
            self.br.b2 = b2;
            self.br.a1 = a1;
            self.br.a2 = a2;
            self.last = (f, g, qv);
        }
        (self.bl.process(l), self.br.process(r))
    }
}

/// A serial 3-band EQ. Each band's freq/gain/Q is smoothed; a band at 0 dB is transparent.
pub struct Eq {
    bands: [EqBand; EQ_BANDS],
}

impl Eq {
    pub fn new(sr: f32) -> Self {
        Self {
            bands: [
                EqBand::new(BandKind::LowShelf, 120.0, 0.0, 0.7, sr),
                EqBand::new(BandKind::Peak, 1000.0, 0.0, 1.0, sr),
                EqBand::new(BandKind::HighShelf, 6000.0, 0.0, 0.7, sr),
            ],
        }
    }
}

impl Effect for Eq {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], _ctx: &FxCtx) {
        for i in 0..l.len() {
            let (mut sl, mut sr) = (l[i], r[i]);
            for band in self.bands.iter_mut() {
                let (a, b) = band.process(sl, sr);
                sl = a;
                sr = b;
            }
            l[i] = sl;
            r[i] = sr;
        }
    }
    fn set_param(&mut self, p: u8, v: f32) {
        if let Some((band, off)) = eq_param(p) {
            match off {
                0 => self.bands[band].freq.set_target(v),
                1 => self.bands[band].gain_db.set_target(v),
                2 => self.bands[band].q.set_target(v),
                _ => {}
            }
        }
    }
    fn ramp_param(&mut self, p: u8, target: f32, frames: u32) {
        if let Some((band, off)) = eq_param(p) {
            match off {
                0 => self.bands[band].freq.ramp_to(target, frames),
                1 => self.bands[band].gain_db.ramp_to(target, frames),
                2 => self.bands[band].q.ramp_to(target, frames),
                _ => {}
            }
        }
    }
    fn reset(&mut self) {
        for b in self.bands.iter_mut() {
            b.bl.reset();
            b.br.reset();
        }
    }
}

/// Decode an EQ param id into `(band, offset)` where offset is 0=freq, 1=gain, 2=Q.
fn eq_param(p: u8) -> Option<(usize, usize)> {
    if p < EQ_PARAM_BASE {
        return None;
    }
    let rel = (p - EQ_PARAM_BASE) as usize;
    let band = rel / 4;
    if band < EQ_BANDS {
        Some((band, rel % 4))
    } else {
        None
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

    fn white(n: usize, seed: u32) -> Vec<f32> {
        crate::test_util::white_noise(n, seed)
    }

    /// Band energy via a one-bin Goertzel-ish RMS after the effect at a probe frequency.
    fn tone(freq: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin()).collect()
    }

    #[test]
    fn lowpass_attenuates_above_cutoff() {
        let sc = SidechainBus::new(SR);
        let mut f = PerfFilter::new(SR);
        f.set_param(PF_CUTOFF_LP, 500.0); // LP @ 500 Hz
        f.cutoff.snap(500.0);
        // a high tone (5 kHz) should be strongly attenuated; a low tone (100 Hz) passes.
        let mut run = |hz: f32| {
            let mut l = tone(hz, 4096);
            let mut r = l.clone();
            f.reset();
            f.process(&mut l, &mut r, &ctx(&sc));
            rms(&l[2048..])
        };
        let low = run(100.0);
        let high = run(5000.0);
        assert!(high < low * 0.3, "LP attenuates 5kHz vs 100Hz ({high} vs {low})");
    }

    #[test]
    fn eq_all_bands_flat_is_transparent() {
        let sc = SidechainBus::new(SR);
        let mut eq = Eq::new(SR);
        let input = white(8192, 0xEEE);
        let mut l = input.clone();
        let mut r = input.clone();
        eq.process(&mut l, &mut r, &ctx(&sc));
        // 0 dB on every band → output ≈ input (transparent within a tight tolerance).
        let err: f32 = l.iter().zip(&input).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(err < 1e-3, "flat EQ is transparent (max err {err})");
    }

    #[test]
    fn eq_peak_boost_raises_band_energy() {
        let sc = SidechainBus::new(SR);
        let mut eq = Eq::new(SR);
        // +12 dB peak at 1 kHz.
        eq.set_param(EQ_PARAM_BASE + 4, 1000.0); // band 1 freq
        eq.set_param(EQ_PARAM_BASE + 5, 12.0); // band 1 gain
        for b in eq.bands.iter_mut() {
            b.freq.snap(b.freq.target());
            b.gain_db.snap(b.gain_db.target());
        }
        let mut l = tone(1000.0, 8192);
        let mut r = l.clone();
        let before = rms(&l[4096..]);
        eq.process(&mut l, &mut r, &ctx(&sc));
        let after = rms(&l[4096..]);
        assert!(after > before * 2.0, "a +12 dB peak boosts 1 kHz ({after} vs {before})");
    }
}
