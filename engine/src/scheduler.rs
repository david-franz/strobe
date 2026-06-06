//! Consumer side of the event ring — the audio-thread integration point.
//!
//! Phase 0, doc 01: this module is introduced as the *seam* the render loop calls
//! once per buffer. The real lock-free SPSC ring, the 32-byte record decode, and the
//! due-this-buffer draining logic are implemented in doc 02 (`ring.rs` / `event.rs`),
//! which replaces the placeholders below. Keeping the seam here lets `audio_io::render`
//! be written and tested against its final shape now.

use crate::stats::EngineStatsShared;
use crate::voices::drum::KickVoice;

/// Consumer half of the SPSC ring, owned by the audio thread (`AudioState`).
///
/// Placeholder for doc 01 — doc 02 replaces this with the real `ring::RingConsumer`.
#[derive(Default)]
pub struct RingConsumer;

/// Drain every event due within this buffer and apply it to the voices at its
/// exact in-buffer sample offset.
///
/// Placeholder body for doc 01 (no events flow yet); doc 02 implements the real
/// drain: peek records in FIFO order, decode, handle late/not-yet-due, and call
/// `kick.trigger_at(offset, …)`.
#[inline]
pub fn drain(
    _consumer: &mut RingConsumer,
    _base: u64,
    _frames: u64,
    _kick: &mut KickVoice,
    _stats: &EngineStatsShared,
) {
    // No-op until doc 02 wires the ring through.
}
