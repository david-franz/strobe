//! Telemetry counters shared between the audio thread (writer) and `stats()` (reader).
//!
//! Every field is a plain atomic written with `Relaxed` ordering — never a lock — so
//! reading them from the control plane can never stall the real-time audio thread
//! (see architecture.md "What can go wrong (and the guardrails)").

use std::sync::atomic::AtomicU64;

/// Atomics the audio thread increments and `stats()` reads. The running frame
/// counter lives separately on the `Engine` (also an `AtomicU64`) because the
/// audio thread is its *sole writer*.
#[derive(Default)]
pub struct EngineStatsShared {
    /// Stream errors reported by cpal's error callback (treated as underruns).
    pub underruns: AtomicU64,
    /// Events that arrived past their frame and were applied immediately (doc 02).
    pub late_events: AtomicU64,
    /// Records whose protocol version the engine does not understand (doc 02).
    pub bad_version: AtomicU64,
    /// Records with an opcode not implemented in this phase (doc 02).
    pub unknown_op: AtomicU64,
    /// Events dropped because the ring was full when pushed (doc 02/04).
    pub ring_full_drops: AtomicU64,
}

/// A point-in-time snapshot returned to the control plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineStats {
    /// The engine frame counter — the source of truth for timing.
    pub current_frame: u64,
    pub underruns: u64,
    pub late_events: u64,
    /// Fraction of the ring currently occupied, `0.0..=1.0`.
    pub ring_fullness: f64,
}
