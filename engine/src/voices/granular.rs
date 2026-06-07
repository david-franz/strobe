//! The granular voice (Phase 4, feature 04): plays a **cloud of short overlapping grains**
//! read from a loaded [`SampleBuffer`]. It gives textures and a cheap pseudo-time-stretch — by
//! holding the read position roughly fixed while spraying Hann-windowed grains, you sustain a
//! moment of the sample (a different aesthetic from the faithful WSOLA stretch in feature 03).
//!
//! RT-safe by the same discipline as every voice: the grain pool is a **fixed-size array** (no
//! alloc on the audio thread), grain spawn reuses a dead/oldest slot, and jitter comes from a
//! pre-seeded inline xorshift RNG (no `rand`, no lock, no syscall). One trigger starts a whole
//! cloud; the voice schedules its own grains from a few parameters, so the event ring stays light.

use std::sync::Arc;

use crate::sample::bank::SampleBuffer;
use crate::voices::pan_gains;

/// Max simultaneously-sounding grains in one cloud. A friendly cap: an over-dense request
/// reuses the oldest grain rather than allocating (or glitching).
pub const MAX_GRAINS: usize = 32;

#[derive(Clone, Copy)]
struct Grain {
    pos: f64,   // fractional read index in source frames
    age: u32,   // frames since spawn
    len: u32,   // grain length in frames (Hann window spans this)
    gain_l: f32,
    gain_r: f32,
    active: bool,
}

impl Grain {
    const DEAD: Grain =
        Grain { pos: 0.0, age: 0, len: 1, gain_l: 0.0, gain_r: 0.0, active: false };
}

pub struct GranularVoice {
    buf: Option<Arc<SampleBuffer>>,
    grains: [Grain; MAX_GRAINS],
    center: f64,          // read position (frames) the cloud hovers around
    size_frames: u32,     // grain length
    interval_frames: u32, // frames between grain spawns (= 1/density)
    scatter: f32,         // 0..1 read-position jitter (× a few grain lengths)
    spread: f32,          // 0..1 stereo pan scatter
    gain: f32,
    duration_frames: u32, // how long the cloud keeps spawning grains
    elapsed: u32,
    next_grain_in: u32,
    pending_offset: u32, // sub-buffer start offset → sample-accurate landing
    rng: u32,            // xorshift state (always nonzero)
    active: bool,
    level: f32,
}

impl Default for GranularVoice {
    fn default() -> Self {
        Self::new()
    }
}

impl GranularVoice {
    pub fn new() -> Self {
        Self {
            buf: None,
            grains: [Grain::DEAD; MAX_GRAINS],
            center: 0.0,
            size_frames: 1,
            interval_frames: 1,
            scatter: 0.0,
            spread: 0.0,
            gain: 1.0,
            duration_frames: 0,
            elapsed: 0,
            next_grain_in: 0,
            pending_offset: 0,
            rng: 0x9E3779B9,
            active: false,
            level: 0.0,
        }
    }

    /// Start a grain cloud. All inputs are rate-independent (seconds / fractions / grains-per-sec);
    /// the conversion to frames uses the buffer's own sample rate. Arithmetic + `Arc::clone` only.
    #[allow(clippy::too_many_arguments)]
    pub fn trigger_at(
        &mut self,
        offset: usize,
        buf: Arc<SampleBuffer>,
        center_frac: f32,
        gain: f32,
        size_secs: f32,
        density: f32,
        scatter: f32,
        spread: f32,
        duration_secs: f32,
    ) {
        let sr = buf.sample_rate as f64;
        let frames = buf.frames as f64;
        self.center = (center_frac.clamp(0.0, 1.0) as f64 * frames).clamp(0.0, frames);
        self.size_frames = ((size_secs.max(0.001) as f64 * sr).round() as u32).max(1);
        self.interval_frames = ((sr / density.max(0.1) as f64).round() as u32).max(1);
        self.scatter = scatter.clamp(0.0, 1.0);
        self.spread = spread.clamp(0.0, 1.0);
        self.gain = gain.clamp(0.0, 4.0);
        self.duration_frames = (duration_secs.max(0.0) as f64 * sr).round() as u32;
        self.elapsed = 0;
        self.next_grain_in = 0; // first grain on the first sounding frame
        self.pending_offset = offset as u32;
        // Seed deterministically from the trigger so a cloud is reproducible but each differs.
        self.rng = (buf.frames as u32)
            .wrapping_mul(2_654_435_761)
            ^ self.size_frames.wrapping_mul(40_503)
            ^ 0x9E3779B9;
        if self.rng == 0 {
            self.rng = 0x1234_5678;
        }
        self.grains = [Grain::DEAD; MAX_GRAINS];
        self.buf = Some(buf);
        self.active = true;
        self.level = self.gain;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }
    pub fn level(&self) -> f32 {
        if self.active {
            self.level
        } else {
            0.0
        }
    }

    /// Render `out_l.len()` frames, **adding** stereo output into the buffers. No-op when idle.
    pub fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        if !self.active {
            return;
        }
        for i in 0..out_l.len() {
            let (l, r) = self.tick();
            out_l[i] += l;
            out_r[i] += r;
            if !self.active {
                break;
            }
        }
    }

    #[inline]
    fn next_u32(&mut self) -> u32 {
        // xorshift32 — cheap, allocation-free, deterministic given the seed.
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }

    /// Uniform in [-1, 1).
    #[inline]
    fn next_bi(&mut self) -> f32 {
        (self.next_u32() as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    fn spawn_grain(&mut self) {
        let frames = match &self.buf {
            Some(b) => b.frames as f64,
            None => return,
        };
        // A free slot, else recycle the oldest grain (the soft MAX_GRAINS cap).
        let slot = self
            .grains
            .iter()
            .position(|g| !g.active)
            .unwrap_or_else(|| {
                self.grains
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, g)| g.age)
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            });
        let len = self.size_frames;
        // Jitter the read position by ±scatter × (a few grain lengths).
        let jitter = self.next_bi() as f64 * self.scatter as f64 * self.size_frames as f64 * 4.0;
        let max_pos = (frames - len as f64).max(0.0);
        let pos = (self.center + jitter).clamp(0.0, max_pos);
        // Stereo placement: spread scatters pan around center.
        let pan = (0.5 + 0.5 * self.next_bi() * self.spread).clamp(0.0, 1.0);
        let (gl, gr) = pan_gains(pan);
        self.grains[slot] = Grain { pos, age: 0, len, gain_l: gl, gain_r: gr, active: true };
    }

    #[inline]
    fn tick(&mut self) -> (f32, f32) {
        if !self.active {
            return (0.0, 0.0);
        }
        if self.pending_offset > 0 {
            self.pending_offset -= 1;
            return (0.0, 0.0);
        }
        if self.buf.is_none() {
            return (0.0, 0.0);
        }

        // Spawn on schedule while the cloud is still within its duration. Done before borrowing
        // `buf` for the summing loop so the mutable `self` borrow in spawn doesn't conflict.
        if self.elapsed < self.duration_frames {
            if self.next_grain_in == 0 {
                self.spawn_grain();
                self.next_grain_in = self.interval_frames;
            }
            self.next_grain_in = self.next_grain_in.saturating_sub(1);
        }
        self.elapsed = self.elapsed.saturating_add(1);

        // Sum active grains, each Hann-windowed and interpolated. `buf` borrows one field while
        // `grains` borrows another — disjoint fields, no clone, no atomic refcount per frame.
        let buf = self.buf.as_ref().unwrap();
        let frames = buf.frames as f64;
        let mut l = 0.0f32;
        let mut r = 0.0f32;
        for g in self.grains.iter_mut() {
            if !g.active {
                continue;
            }
            let win = hann(g.age, g.len);
            let i = g.pos.floor() as isize;
            let frac = (g.pos - i as f64) as f32;
            let s_l = read_lin(buf, 0, i, frac);
            let s_r = if buf.channels > 1 { read_lin(buf, 1, i, frac) } else { s_l };
            l += s_l * win * g.gain_l;
            r += s_r * win * g.gain_r;
            g.pos += 1.0;
            g.age += 1;
            if g.age >= g.len || g.pos >= frames {
                g.active = false;
            }
        }
        l *= self.gain;
        r *= self.gain;
        self.level = l.abs().max(r.abs());

        // Finished once the duration has elapsed and the last grain has died.
        if self.elapsed >= self.duration_frames && !self.grains.iter().any(|g| g.active) {
            self.active = false;
        }
        (l, r)
    }
}

/// Hann window value at `age` over a grain of `len` frames (0 at the edges, 1 in the middle).
#[inline]
fn hann(age: u32, len: u32) -> f32 {
    if len <= 1 {
        return 1.0;
    }
    let phase = age as f32 / (len - 1) as f32; // 0..1
    0.5 - 0.5 * (std::f32::consts::TAU * phase).cos()
}

/// Linear-interpolated read of one channel at integer index `i` + `frac`. Out-of-range → 0.0.
#[inline]
fn read_lin(buf: &SampleBuffer, c: usize, i: isize, frac: f32) -> f32 {
    let ch = buf.channel(c);
    let n = ch.len() as isize;
    let at = |k: isize| -> f32 {
        if k < 0 || k >= n {
            0.0
        } else {
            ch[k as usize]
        }
    };
    let a = at(i);
    let b = at(i + 1);
    a + (b - a) * frac
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frames: usize) -> Arc<SampleBuffer> {
        // mono buffer of all-ones so any sounding grain is obvious and windowing is visible.
        Arc::new(SampleBuffer {
            channels: 1,
            sample_rate: 48_000,
            frames,
            data: vec![1.0f32; frames].into_boxed_slice(),
        })
    }

    fn render_n(v: &mut GranularVoice, n: usize) -> (Vec<f32>, Vec<f32>) {
        let mut l = vec![0.0; n];
        let mut r = vec![0.0; n];
        v.render(&mut l, &mut r);
        (l, r)
    }

    #[test]
    fn cloud_produces_sound_then_stops_after_duration() {
        let mut v = GranularVoice::new();
        // 20 ms grains, 40/sec, 0.2 s cloud, centered, no scatter.
        v.trigger_at(0, tone(48_000), 0.0, 1.0, 0.02, 40.0, 0.0, 0.0, 0.2);
        let (l, _r) = render_n(&mut v, 48_000);
        let voiced = l.iter().filter(|&&x| x.abs() > 1e-4).count();
        assert!(voiced > 0, "the cloud makes sound");
        // Spawning stops after ~0.2 s; with ≤20 ms tails everything is silent well before 1 s.
        assert!(l[40_000..].iter().all(|&x| x.abs() < 1e-4), "cloud ends after its duration");
        assert!(!v.is_active(), "voice frees once the cloud finishes");
    }

    #[test]
    fn idle_voice_is_exactly_silent() {
        let mut v = GranularVoice::new();
        let (l, r) = render_n(&mut v, 64);
        assert!(l.iter().chain(r.iter()).all(|&x| x == 0.0));
    }

    #[test]
    fn sub_buffer_offset_delays_the_cloud() {
        let mut v = GranularVoice::new();
        v.trigger_at(100, tone(48_000), 0.0, 1.0, 0.02, 40.0, 0.0, 0.0, 0.2);
        let (l, _r) = render_n(&mut v, 2000);
        assert!(l[..100].iter().all(|&x| x == 0.0), "silent before the offset");
        assert!(l[100..].iter().any(|&x| x.abs() > 1e-4), "sounds after the offset");
    }

    #[test]
    fn density_is_capped_at_max_grains_no_overrun() {
        // Absurd density + tiny interval: must never exceed the fixed pool, never panic.
        let mut v = GranularVoice::new();
        v.trigger_at(0, tone(48_000), 0.5, 1.0, 0.05, 100_000.0, 0.5, 0.5, 0.5);
        let _ = render_n(&mut v, 24_000);
        let live = v.grains.iter().filter(|g| g.active).count();
        assert!(live <= MAX_GRAINS, "never more than the fixed pool ({live})");
    }
}
