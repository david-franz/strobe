//! The sampler voice (Phase 4, feature 02): plays a loaded [`SampleBuffer`] with variable
//! speed/pitch (a fractional read index + interpolation), gain, a start offset, one-shot or
//! looped playback, and reverse. Triggered by `TrigSample`. RT-safe: it holds a cloned
//! `Arc<SampleBuffer>` and does only arithmetic in `tick` — no decode/resample/alloc/lock.
//!
//! Speed *is* pitch here (varispeed/tape): `step = 2.0` is an octave up at half length. Decoupled
//! stretch/pitch (feature 03) prepares a new buffer offline that this voice plays at `step = 1.0`.

use std::sync::Arc;

use crate::sample::bank::SampleBuffer;

#[derive(Clone, Copy, PartialEq)]
pub enum Interp {
    Linear,
    Cubic,
}

/// Trigger options carried in the `TrigSample` record's flags / region field.
#[derive(Clone, Copy)]
pub struct TrigOpts {
    pub reverse: bool,
    pub looped: bool,
    pub interp: Interp,
    /// Region length in source frames; `None` = to the end of the buffer.
    pub region_len: Option<f64>,
}

impl Default for TrigOpts {
    fn default() -> Self {
        Self { reverse: false, looped: false, interp: Interp::Cubic, region_len: None }
    }
}

/// `TrigSample` flag bits (the record's `flags_seq` field).
pub const FLAG_REVERSE: u32 = 1 << 0;
pub const FLAG_LOOP: u32 = 1 << 1;
pub const FLAG_LINEAR: u32 = 1 << 2; // else cubic (the default, higher quality)

impl TrigOpts {
    /// Decode the flags + region length carried by a `TrigSample`.
    pub fn from_flags(flags: u32, region_len_frames: u32) -> Self {
        Self {
            reverse: flags & FLAG_REVERSE != 0,
            looped: flags & FLAG_LOOP != 0,
            interp: if flags & FLAG_LINEAR != 0 { Interp::Linear } else { Interp::Cubic },
            region_len: if region_len_frames > 0 { Some(region_len_frames as f64) } else { None },
        }
    }
}

pub struct SamplerVoice {
    buf: Option<Arc<SampleBuffer>>,
    pos: f64,  // fractional read index, in source frames
    step: f64, // per-output-frame increment = speed
    gain: f32,
    region_start: f64,
    region_end: f64, // exclusive
    looped: bool,
    reverse: bool,
    interp: Interp,
    pending_offset: u32, // sub-buffer start offset → sample-accurate landing
    active: bool,
    fade_frames: f64,
    level: f32, // last output magnitude, for voice-stealing decisions
}

impl Default for SamplerVoice {
    fn default() -> Self {
        Self::new()
    }
}

impl SamplerVoice {
    pub fn new() -> Self {
        Self {
            buf: None,
            pos: 0.0,
            step: 1.0,
            gain: 1.0,
            region_start: 0.0,
            region_end: 0.0,
            looped: false,
            reverse: false,
            interp: Interp::Cubic,
            pending_offset: 0,
            active: false,
            fade_frames: 1.0,
            level: 0.0,
        }
    }

    /// Trigger from a `TrigSample` at a sub-buffer sample `offset`. Arithmetic + `Arc::clone` only.
    #[allow(clippy::too_many_arguments)]
    pub fn trigger_at(
        &mut self,
        offset: usize,
        buf: Arc<SampleBuffer>,
        speed: f32,
        gain: f32,
        start_frame: f32,
        opts: TrigOpts,
    ) {
        let frames = buf.frames as f64;
        self.reverse = opts.reverse;
        self.looped = opts.looped;
        self.interp = opts.interp;
        self.gain = gain.clamp(0.0, 4.0);
        self.step = (speed.abs() as f64).max(1e-6);
        self.region_start = (start_frame as f64).clamp(0.0, frames);
        self.region_end = match opts.region_len {
            Some(len) if len > 0.0 => (self.region_start + len).min(frames),
            _ => frames,
        };
        if self.region_end <= self.region_start {
            self.region_end = frames.max(self.region_start + 1.0);
        }
        self.pos = if self.reverse { self.region_end - 1.0 } else { self.region_start };
        self.pending_offset = offset as u32;
        self.fade_frames = (0.002 * buf.sample_rate as f64).max(1.0);
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
    fn tick(&mut self) -> (f32, f32) {
        if !self.active {
            return (0.0, 0.0);
        }
        if self.pending_offset > 0 {
            self.pending_offset -= 1;
            return (0.0, 0.0);
        }
        let buf = match &self.buf {
            Some(b) => b,
            None => return (0.0, 0.0),
        };
        let i = self.pos.floor() as isize;
        let frac = (self.pos - i as f64) as f32;
        let env = self.boundary_env() as f32 * self.gain;
        let l = read_channel(buf, 0, i, frac, self.interp) * env;
        let r = if buf.channels > 1 {
            read_channel(buf, 1, i, frac, self.interp) * env
        } else {
            l
        };
        self.level = l.abs().max(r.abs());

        self.pos += if self.reverse { -self.step } else { self.step };
        if !self.reverse && self.pos >= self.region_end - 1.0 {
            if self.looped {
                self.pos = self.region_start + (self.pos - (self.region_end - 1.0));
            } else {
                self.active = false;
            }
        } else if self.reverse && self.pos <= self.region_start {
            if self.looped {
                self.pos = self.region_end - 1.0 - (self.region_start - self.pos);
            } else {
                self.active = false;
            }
        }
        (l, r)
    }

    /// Short linear fade near region edges (start/end/loop-wrap) to suppress boundary clicks.
    #[inline]
    fn boundary_env(&self) -> f64 {
        let from_start = (self.pos - self.region_start).abs();
        let to_end = (self.region_end - 1.0 - self.pos).abs();
        let edge = from_start.min(to_end).min(self.fade_frames);
        (edge / self.fade_frames).clamp(0.0, 1.0)
    }
}

/// Interpolated read of one channel at integer index `i` + `frac`. Out-of-range → 0.0.
#[inline]
fn read_channel(buf: &SampleBuffer, c: usize, i: isize, frac: f32, interp: Interp) -> f32 {
    let ch = buf.channel(c);
    let n = ch.len() as isize;
    let at = |k: isize| -> f32 {
        if k < 0 || k >= n {
            0.0
        } else {
            ch[k as usize]
        }
    };
    match interp {
        Interp::Linear => {
            let a = at(i);
            let b = at(i + 1);
            a + (b - a) * frac
        }
        Interp::Cubic => {
            let x0 = at(i - 1);
            let x1 = at(i);
            let x2 = at(i + 1);
            let x3 = at(i + 2);
            let t = frac;
            let a0 = -0.5 * x0 + 1.5 * x1 - 1.5 * x2 + 0.5 * x3;
            let a1 = x0 - 2.5 * x1 + 2.0 * x2 - 0.5 * x3;
            let a2 = -0.5 * x0 + 0.5 * x2;
            ((a0 * t + a1) * t + a2) * t + x1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(frames: usize) -> Arc<SampleBuffer> {
        // mono ramp 0,1,2,... so order/position is obvious in tests
        Arc::new(SampleBuffer {
            channels: 1,
            sample_rate: 48_000,
            frames,
            data: (0..frames).map(|i| i as f32).collect::<Vec<_>>().into_boxed_slice(),
        })
    }

    fn render_n(v: &mut SamplerVoice, n: usize) -> Vec<f32> {
        let mut l = vec![0.0; n];
        let mut r = vec![0.0; n];
        v.render(&mut l, &mut r);
        l
    }

    // Buffers are long enough that the ~2 ms (96-frame) boundary fade is a small fraction.
    #[test]
    fn unity_playback_reproduces_the_buffer() {
        let mut v = SamplerVoice::new();
        v.trigger_at(0, ramp(2000), 1.0, 1.0, 0.0, TrigOpts { interp: Interp::Linear, ..Default::default() });
        let out = render_n(&mut v, 2000);
        for i in (200..1800).step_by(137) {
            assert!((out[i] - i as f32).abs() < 0.5, "frame {i}: {} vs {i}", out[i]);
        }
        assert!(!v.is_active(), "one-shot finished at the region end");
    }

    #[test]
    fn sub_buffer_offset_is_silent_then_plays() {
        let mut v = SamplerVoice::new();
        v.trigger_at(16, ramp(2000), 1.0, 1.0, 0.0, TrigOpts::default());
        let out = render_n(&mut v, 2000);
        assert!(out[..16].iter().all(|&x| x == 0.0), "silent before the offset");
        assert!(out[200..400].iter().any(|&x| x.abs() > 0.5), "plays after the offset");
    }

    #[test]
    fn double_speed_halves_duration() {
        let mut v = SamplerVoice::new();
        v.trigger_at(0, ramp(2000), 2.0, 1.0, 0.0, TrigOpts { interp: Interp::Linear, ..Default::default() });
        let out = render_n(&mut v, 2000);
        let voiced = out.iter().filter(|&&x| x != 0.0).count();
        assert!(voiced < 1100, "double speed ends in ~half ({voiced} voiced frames)");
    }

    #[test]
    fn start_offset_plays_from_the_region() {
        let mut v = SamplerVoice::new();
        v.trigger_at(0, ramp(2000), 1.0, 1.0, 500.0, TrigOpts { interp: Interp::Linear, region_len: Some(500.0), ..Default::default() });
        let out = render_n(&mut v, 700);
        assert!((out[200] - 700.0).abs() < 2.0, "reads source frame ~700 ({})", out[200]);
        assert_eq!(out[600], 0.0, "stops at the 500-frame region end");
    }

    #[test]
    fn reverse_plays_the_region_backwards() {
        let mut v = SamplerVoice::new();
        v.trigger_at(0, ramp(2000), 1.0, 1.0, 0.0, TrigOpts { reverse: true, interp: Interp::Linear, ..Default::default() });
        let out = render_n(&mut v, 2000);
        assert!(out[200] > out[800], "reversed descends: {} > {}", out[200], out[800]);
    }

    #[test]
    fn idle_voice_is_exactly_silent() {
        let mut v = SamplerVoice::new();
        let out = render_n(&mut v, 16);
        assert!(out.iter().all(|&x| x == 0.0));
    }
}
