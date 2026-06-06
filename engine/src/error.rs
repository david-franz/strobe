//! Engine error types shared across modules.

use std::fmt;

/// Errors from opening / running the audio device.
#[derive(Debug)]
pub enum AudioError {
    /// No default output device is available.
    NoDevice,
    /// The default output config could not be queried.
    DefaultConfig(String),
    /// The device cannot provide an `f32` output stream (Phase 0 keeps the path narrow).
    UnsupportedFormat,
    /// `cpal` failed to build the output stream.
    BuildStream(String),
    /// `cpal` failed to start the stream.
    PlayStream(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::NoDevice => write!(f, "no default output device"),
            AudioError::DefaultConfig(e) => write!(f, "default output config error: {e}"),
            AudioError::UnsupportedFormat => write!(f, "device does not support f32 output"),
            AudioError::BuildStream(e) => write!(f, "failed to build output stream: {e}"),
            AudioError::PlayStream(e) => write!(f, "failed to start output stream: {e}"),
        }
    }
}

impl std::error::Error for AudioError {}
