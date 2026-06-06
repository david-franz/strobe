//! The synthesized kick voice.
//!
//! Phase 0, doc 01: a minimal placeholder so the render loop (`audio_io::render`)
//! can call `tick()` and the drain can call `trigger_at(...)`. Doc 03 replaces this
//! with the real `fundsp`-based click→body kick (pitch envelope → sine → amp
//! envelope → tanh). The public method shapes here are the ones doc 03 keeps.

/// A monophonic, one-shot kick voice.
pub struct KickVoice {
    /// Sample rate, used by doc 03's envelope timing.
    pub sample_rate: f32,
    /// Whether the voice is currently sounding.
    pub active: bool,
    /// Samples of silence still to emit before the trigger frame within this buffer.
    pending_offset: u32,
    /// Placeholder constant output level while active (doc 03 replaces with synthesis).
    level: f32,
}

impl KickVoice {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            active: false,
            pending_offset: 0,
            level: 0.0,
        }
    }

    /// One-shot trigger from a `NoteOn`, at a sub-buffer sample offset. Pure
    /// arithmetic + flag flips — no allocation. Doc 03 fills in the synthesis params.
    #[inline]
    pub fn trigger_at(&mut self, offset: usize, _freq: f32, vel: f32, _dur_frames: f32) {
        self.pending_offset = offset as u32;
        self.level = vel.clamp(0.0, 1.0);
        self.active = true;
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
        self.level
    }
}

#[cfg(test)]
impl KickVoice {
    /// Test helper: force a constant output level (used by `audio_io` render tests
    /// to verify channel fan-out independently of the synthesis in doc 03).
    pub fn force_constant(&mut self, level: f32) {
        self.active = true;
        self.pending_offset = 0;
        self.level = level;
    }

    /// Test helper: observe the sub-buffer offset set by the last `trigger_at`
    /// (used by `scheduler::drain` tests to assert sample-accurate placement).
    pub fn test_pending_offset(&self) -> u32 {
        self.pending_offset
    }
}
