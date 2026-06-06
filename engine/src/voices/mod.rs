//! Synthesized voices. Phase 0 had a single monophonic kick; Phase 1 adds the
//! oscillator bank, the configurable synth voice, the synthesized drum kit, and
//! polyphonic voice allocation.

pub mod drum;
pub mod oscillator;

pub use oscillator::{OscConfig, OscKind, Oscillator, WavetableBank};
