//! The napi-rs native bridge — in-process TypeScript ↔ the Rust engine.
//!
//! Satisfies the "native" row of architecture.md's bridge table: Rust spawns/owns the
//! cpal audio thread (inside `engine::Engine`); TS calls these four methods on Node's
//! thread. The hot path (`push_events`) reaches the audio thread only through the
//! lock-free ring — never a shared lock — so it can never stall the real-time side.

use napi::bindgen_prelude::*;
use napi_derive::napi;

/// The Phase 0 control-call surface: `start` / `stop` / `pushEvents` / `stats`.
#[napi(js_name = "Engine")]
pub struct Engine {
    inner: engine::Engine,
}

#[napi]
impl Engine {
    #[napi(constructor)]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Engine {
            inner: engine::Engine::new(),
        }
    }

    /// Open the cpal output stream. cpal spawns/owns the audio thread.
    #[napi]
    pub fn start(&mut self, opts: Option<StartOpts>) -> Result<()> {
        let (sr, bf) = match opts {
            Some(o) => (
                o.sample_rate.map(|x| x as u32),
                o.buffer_frames.map(|x| x as u32),
            ),
            None => (None, None),
        };
        self.inner
            .start(sr, bf)
            .map_err(|e| Error::from_reason(e.to_string()))
    }

    #[napi]
    pub fn stop(&mut self) {
        self.inner.stop(); // dropping the Stream tears the audio thread down cleanly
    }

    /// Hot path: copy N×32 bytes into the ring. Non-blocking; synchronous; one call
    /// per scheduler tick. A full ring drops + counts rather than blocking.
    #[napi]
    pub fn push_events(&self, buffer: Buffer) -> Result<()> {
        let bytes: &[u8] = buffer.as_ref();
        self.inner
            .push_events(bytes)
            .map_err(|e| Error::from_reason(e.to_string()))
    }

    #[napi]
    pub fn stats(&self) -> Stats {
        let s = self.inner.stats();
        Stats {
            // u64 → f64 is exact below 2^53 (~570 years at 48 kHz) — safe.
            current_frame: s.current_frame as f64,
            underruns: s.underruns as f64,
            late_events: s.late_events as f64,
            ring_fullness: s.ring_fullness,
        }
    }
}

#[napi(object)]
pub struct StartOpts {
    pub sample_rate: Option<f64>,
    pub buffer_frames: Option<f64>,
}

#[napi(object)]
pub struct Stats {
    pub current_frame: f64,
    pub underruns: f64,
    pub late_events: f64,
    pub ring_fullness: f64,
}
