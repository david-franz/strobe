//! Sampling (Phase 4): decode + resample recorded audio **off the audio thread** and store it
//! in a real-time-safe [`bank::SampleBank`] addressed by numeric id. The sampler voice
//! (`voices/sampler`) reads these immutable buffers; chop/slice address regions of them.

pub mod bank;
pub mod decode;
pub mod stretch;

pub use bank::{SampleBank, SampleBuffer};
pub use decode::{decode_bytes, prepare, DecodeError, DecodedPcm};
pub use stretch::{StretchEngine, Wsola};
