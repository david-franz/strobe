//! Sample decode + resample (Phase 4, feature 01). NON-REAL-TIME: this allocates and runs for
//! ms..s; it must only ever run on a control/worker thread, never the audio thread.
//!
//! `symphonia` decodes any container/codec it supports (WAV/MP3/FLAC/OGG/…) to interleaved
//! Float32 PCM; `rubato` resamples to the engine's rate at load time so the sampler voice never
//! does source-rate conversion on the audio thread (its only runtime rate change is musical
//! speed/pitch).

use symphonia::core::audio::SampleBuffer as SymSampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use super::bank::SampleBuffer;

/// Reject absurdly long inputs — the project's posture is *short* grabs.
pub const MAX_DURATION_SEC: usize = 600;

#[derive(Debug)]
pub enum DecodeError {
    Symphonia(String),
    NoTrack,
    Empty,
    TooLong,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Symphonia(e) => write!(f, "decode failed: {e}"),
            DecodeError::NoTrack => write!(f, "no audio track in file"),
            DecodeError::Empty => write!(f, "decoded zero samples"),
            DecodeError::TooLong => write!(f, "sample longer than {MAX_DURATION_SEC}s"),
        }
    }
}
impl std::error::Error for DecodeError {}
impl From<SymError> for DecodeError {
    fn from(e: SymError) -> Self {
        DecodeError::Symphonia(e.to_string())
    }
}

/// Decoded audio, interleaved Float32, at its *source* sample rate.
pub struct DecodedPcm {
    pub samples: Vec<f32>, // interleaved
    pub channels: u16,
    pub sample_rate: u32,
}

/// Decode any container/codec symphonia supports from raw bytes. NON-REAL-TIME.
pub fn decode_bytes(bytes: Vec<u8>, ext_hint: Option<&str>) -> Result<DecodedPcm, DecodeError> {
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = ext_hint {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;
    let track = format.default_track().ok_or(DecodeError::NoTrack)?;
    let track_id = track.id;
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut out: Vec<f32> = Vec::new();
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(_)) => break, // EOF
            Err(e) => return Err(e.into()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(_)) => continue, // skip a bad packet
            Err(e) => return Err(e.into()),
        };
        let spec = *decoded.spec();
        channels = spec.channels.count() as u16;
        sample_rate = spec.rate;
        let mut buf = SymSampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        out.extend_from_slice(buf.samples());
    }

    if out.is_empty() || channels == 0 || sample_rate == 0 {
        return Err(DecodeError::Empty);
    }
    if out.len() / channels as usize > MAX_DURATION_SEC * sample_rate as usize {
        return Err(DecodeError::TooLong);
    }
    Ok(DecodedPcm { samples: out, channels, sample_rate })
}

/// Deinterleave to per-channel planes.
fn to_planar(samples: &[f32], channels: usize) -> Vec<Vec<f32>> {
    let frames = samples.len() / channels;
    let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(frames); channels];
    for f in 0..frames {
        for (c, plane) in planar.iter_mut().enumerate() {
            plane.push(samples[f * channels + c]);
        }
    }
    planar
}

/// Resample per-channel planes from `src` to `dst` rate (sinc, high quality). NON-REAL-TIME.
fn resample_planar(planar: Vec<Vec<f32>>, src: u32, dst: u32) -> Vec<Vec<f32>> {
    use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
    let ch = planar.len();
    let frames = planar.first().map(|p| p.len()).unwrap_or(0);
    if ch == 0 || frames == 0 {
        return planar;
    }
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        oversampling_factor: 128,
        interpolation: SincInterpolationType::Linear,
        window: WindowFunction::BlackmanHarris2,
    };
    let ratio = dst as f64 / src as f64;
    let mut rs = match SincFixedIn::<f32>::new(ratio, 2.0, params, frames, ch) {
        Ok(rs) => rs,
        Err(_) => return planar, // fall back to source rate rather than fail the load
    };
    match rs.process(&planar, None) {
        Ok(out) => out,
        Err(_) => planar,
    }
}

/// Prepare decoded PCM into an engine-rate, planar [`SampleBuffer`] ready for the bank.
pub fn prepare(pcm: DecodedPcm, dst_rate: u32) -> Result<SampleBuffer, DecodeError> {
    let ch = pcm.channels.max(1) as usize;
    if pcm.samples.is_empty() {
        return Err(DecodeError::Empty);
    }
    let planar = to_planar(&pcm.samples, ch);
    let planar = if pcm.sample_rate != dst_rate {
        resample_planar(planar, pcm.sample_rate, dst_rate)
    } else {
        planar
    };
    Ok(SampleBuffer::from_planar(planar, dst_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal 16-bit PCM WAV in memory (mono) at `rate` Hz containing a sine at `freq`.
    fn wav_sine(rate: u32, freq: f32, secs: f32) -> Vec<u8> {
        let n = (rate as f32 * secs) as usize;
        let mut data = Vec::new();
        for i in 0..n {
            let s = (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin();
            let v = (s * 30000.0) as i16;
            data.extend_from_slice(&v.to_le_bytes());
        }
        let mut w = Vec::new();
        let byte_rate = rate * 2;
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        w.extend_from_slice(&1u16.to_le_bytes()); // PCM
        w.extend_from_slice(&1u16.to_le_bytes()); // mono
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&byte_rate.to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes()); // block align
        w.extend_from_slice(&16u16.to_le_bytes()); // bits
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(data.len() as u32).to_le_bytes());
        w.extend_from_slice(&data);
        w
    }

    fn dominant_freq(samples: &[f32], rate: u32) -> f32 {
        // rising zero-crossing estimate (good enough for a pure tone)
        let mut crossings = 0usize;
        for w in samples.windows(2) {
            if w[0] <= 0.0 && w[1] > 0.0 {
                crossings += 1;
            }
        }
        crossings as f32 / (samples.len() as f32 / rate as f32)
    }

    #[test]
    fn decodes_a_wav_to_finite_pcm() {
        let bytes = wav_sine(44_100, 440.0, 0.5);
        let pcm = decode_bytes(bytes, Some("wav")).expect("decode");
        assert_eq!(pcm.channels, 1);
        assert_eq!(pcm.sample_rate, 44_100);
        assert!((pcm.samples.len() as i64 - 22_050).abs() < 100, "≈0.5s of frames");
        assert!(pcm.samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
    }

    #[test]
    fn resample_preserves_pitch_and_scales_length() {
        let bytes = wav_sine(44_100, 440.0, 1.0);
        let pcm = decode_bytes(bytes, Some("wav")).expect("decode");
        let buf = prepare(pcm, 48_000).expect("prepare");
        assert_eq!(buf.sample_rate, 48_000);
        // length scales by 48000/44100
        let expected = (44_100.0 * (48_000.0 / 44_100.0)) as usize;
        assert!(
            (buf.frames as i64 - expected as i64).abs() < 2000,
            "length scales (~{expected}, got {})",
            buf.frames,
        );
        // pitch unchanged (within tolerance), ignoring the resampler's warm-up edge
        let f = dominant_freq(&buf.channel(0)[2000..], 48_000);
        assert!((f - 440.0).abs() < 8.0, "440 Hz preserved (got {f})");
    }

    #[test]
    fn equal_rate_prepare_is_unchanged_length() {
        let bytes = wav_sine(48_000, 220.0, 0.25);
        let pcm = decode_bytes(bytes, Some("wav")).expect("decode");
        let n = pcm.samples.len();
        let buf = prepare(pcm, 48_000).expect("prepare");
        assert_eq!(buf.frames, n, "no resample when rates match");
    }

    #[test]
    fn garbage_bytes_fail_cleanly() {
        let err = decode_bytes(vec![0u8; 64], Some("wav"));
        assert!(err.is_err(), "corrupt input is an Err, not a panic");
    }
}
