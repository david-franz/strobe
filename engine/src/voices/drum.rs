//! The synthesized kick voice.
//!
//! The classic 808/909 kick: a pitched sine with a fast downward *pitch envelope*
//! (the "click" attack that snaps from a high frequency to the body fundamental),
//! shaped by a fast *amplitude envelope* (sharp attack, exponential decay), softened
//! by a `tanh` drive for weight. The sine oscillator is a `fundsp` node built once at
//! construction; `trigger_at`/`tick` do only arithmetic and flag flips — allocation-
//! free per trigger, as required on the hard-real-time audio thread.

use fundsp::hacker32::*;

pub struct KickVoice {
    /// Sine oscillator (frequency-in, sample-out), built once. Allocation happens
    /// here at construction only — never per trigger.
    osc: Box<dyn AudioUnit>,

    // --- per-trigger state (just numbers; reset on trigger, no alloc) ---
    /// Whether the voice is currently sounding.
    pub active: bool,
    /// Samples of silence still to emit before the trigger frame within this buffer.
    pending_offset: u32,
    /// Frames since trigger.
    t: u32,

    body_hz: f32,
    click_hz: f32,
    pitch_decay_frames: f32,
    amp_attack_frames: f32,
    amp_decay_frames: f32,
    gain: f32,
    drive: f32,
}

impl KickVoice {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            osc: Box::new(sine()),
            active: false,
            pending_offset: 0,
            t: 0,
            body_hz: 55.0,
            click_hz: 440.0,
            pitch_decay_frames: 0.03 * sample_rate,
            amp_attack_frames: 0.001 * sample_rate,
            amp_decay_frames: 0.18 * sample_rate,
            gain: 0.9,
            drive: 1.5,
        }
    }

    /// One-shot trigger from a `NoteOn`, at a sub-buffer sample offset (doc 02 passes
    /// this). Pure arithmetic + flag flips — no allocation.
    #[inline]
    pub fn trigger_at(&mut self, offset: usize, freq: f32, vel: f32, dur_frames: f32) {
        self.body_hz = if freq > 0.0 { freq } else { 55.0 };
        self.click_hz = self.body_hz * 8.0;
        self.gain = vel.clamp(0.0, 1.0);
        if dur_frames > 0.0 {
            self.amp_decay_frames = dur_frames;
        }
        self.t = 0;
        self.pending_offset = offset as u32;
        self.active = true;
        self.osc.reset(); // reset phase so the click starts cleanly; no allocation
    }

    /// Instantaneous pitch: starts at `click_hz`, decays toward `body_hz`.
    #[inline]
    fn pitch_at(&self, t: f32) -> f32 {
        let k = (-t / self.pitch_decay_frames).exp();
        self.body_hz + (self.click_hz - self.body_hz) * k
    }

    /// Amplitude envelope: linear attack then exponential decay.
    #[inline]
    fn amp_at(&self, t: f32) -> f32 {
        if t < self.amp_attack_frames {
            t / self.amp_attack_frames
        } else {
            let d = t - self.amp_attack_frames;
            (-d / (self.amp_decay_frames * 0.5)).exp()
        }
    }

    /// Frame at which the voice is considered finished (output is exactly 0 after).
    #[inline]
    fn end_frame(&self) -> f32 {
        self.amp_attack_frames + self.amp_decay_frames * 4.0
    }

    /// One sample of output. `0.0` when idle. Honors the sub-buffer start offset so
    /// the hit lands on its exact frame, not merely at the start of the buffer.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }
        if self.pending_offset > 0 {
            self.pending_offset -= 1;
            return 0.0;
        }

        let t = self.t as f32;
        let freq = self.pitch_at(t);
        let mut out = [0.0f32; 1];
        self.osc.tick(&[freq], &mut out); // sine at the instantaneous frequency
        let amp = self.amp_at(t);
        let mut s = out[0] * amp * self.gain;
        s = (s * self.drive).tanh(); // weight / saturation

        self.t += 1;
        // End the voice once well past the decay so idle output is exactly 0.0
        // (avoids a denormal tail).
        if t > self.end_frame() {
            self.active = false;
        }
        s
    }
}

#[cfg(test)]
impl KickVoice {
    /// Test helper: observe the sub-buffer offset set by the last `trigger_at`
    /// (used by `scheduler::drain` tests to assert sample-accurate placement).
    pub fn test_pending_offset(&self) -> u32 {
        self.pending_offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn pitch_envelope_snaps_from_click_to_body() {
        let mut v = KickVoice::new(SR);
        v.trigger_at(0, 55.0, 1.0, 0.0);
        // At t=0 the pitch is the click (~8× body); after ~100 ms it has settled to body.
        assert!((v.pitch_at(0.0) - 440.0).abs() < 1.0, "click ~440Hz");
        // ~100 ms in (several pitch-decay time-constants) it is most of the way to body.
        let mid = v.pitch_at(0.1 * SR);
        assert!(mid < 80.0, "well below the click toward body, got {mid}");
        // By ~150 ms it has effectively settled to the body fundamental.
        let late = v.pitch_at(0.15 * SR);
        assert!((late - 55.0).abs() < 10.0, "settles to body ~55Hz, got {late}");
        assert!(v.pitch_at(0.0) > v.pitch_at(0.01 * SR), "pitch falls over time");
    }

    #[test]
    fn amp_envelope_attacks_then_decays_to_zero() {
        let mut v = KickVoice::new(SR);
        v.trigger_at(0, 55.0, 1.0, 0.05 * SR); // short body
        assert_eq!(v.amp_at(0.0), 0.0, "attack starts at 0");
        let peak = v.amp_at(v.amp_attack_frames); // end of attack ~ peak
        assert!((peak - 1.0).abs() < 1e-6, "peak ~1.0");
        // Monotonic decay after the peak.
        let a = v.amp_at(v.amp_attack_frames + 1.0);
        let b = v.amp_at(v.amp_attack_frames + 1000.0);
        assert!(a > b, "decays after peak");
    }

    #[test]
    fn one_shot_ends_on_its_own_without_noteoff() {
        let mut v = KickVoice::new(SR);
        v.trigger_at(0, 55.0, 1.0, 0.05 * SR);
        assert!(v.active);
        // Render past the end; the voice deactivates itself.
        let total = (v.end_frame() as usize) + 100;
        let mut last = 0.0;
        for _ in 0..total {
            last = v.tick();
        }
        assert!(!v.active, "voice ended on its own");
        assert_eq!(last, 0.0, "idle output is exactly 0.0");
        assert_eq!(v.tick(), 0.0);
    }

    #[test]
    fn sub_buffer_offset_emits_silence_until_onset() {
        let mut v = KickVoice::new(SR);
        v.trigger_at(64, 55.0, 1.0, 0.0);
        let mut buf = [0.0f32; 128];
        for s in buf.iter_mut() {
            *s = v.tick();
        }
        // Silence for the first 64 samples (the onset offset)...
        assert!(buf[..64].iter().all(|&s| s == 0.0), "silent until offset");
        // ...then the voice produces signal within the attack window.
        assert!(buf[64..].iter().any(|&s| s.abs() > 1e-6), "signal after onset");
    }

    #[test]
    fn output_is_bounded() {
        let mut v = KickVoice::new(SR);
        v.trigger_at(0, 55.0, 1.0, 0.0);
        for _ in 0..(0.3 * SR) as usize {
            let s = v.tick();
            assert!(s.abs() <= 1.0, "tanh-bounded output stays within [-1,1]");
        }
    }
}
