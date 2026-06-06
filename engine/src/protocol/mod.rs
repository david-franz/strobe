//! The wire format that crosses the bridge: the fixed 32-byte event record (+ decode)
//! and the lock-free SPSC ring buffer that carries it.

pub mod event;
pub mod ring;
