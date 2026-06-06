//! The fixed 32-byte event record and its decode — the contract that crosses the
//! bridge (event-protocol.md). Little-endian, allocation-free: the audio thread casts
//! bytes straight to records via `bytemuck` and decodes into a stack enum, never
//! parsing variable-length data.

use bytemuck::{Pod, Zeroable};

/// Bytes per event record (event-protocol.md "Event record layout").
pub const REC_SIZE: usize = 32;

/// Current protocol version (byte 0 of every record).
pub const VERSION: u8 = 1;

/// Opcodes — fixed by event-protocol.md. Phase 0 interprets NoteOn + Tempo + Nop;
/// the rest are accepted (counted as unknown) for forward compatibility.
pub mod op {
    pub const NOTE_ON: u8 = 0x01;
    pub const NOTE_OFF: u8 = 0x02;
    pub const PARAM_SET: u8 = 0x10;
    pub const PARAM_RAMP: u8 = 0x11;
    pub const TRIG_SAMPLE: u8 = 0x20;
    pub const TRACK_FX: u8 = 0x30;
    pub const TEMPO: u8 = 0x40;
    pub const NOP: u8 = 0x7F;
}

/// Mirrors event-protocol.md exactly: little-endian, 32 bytes, no padding.
///
/// `#[repr(C)]` over `u8,u8,u16,u32,u32,f32,f32,f32,f32,u32` packs each field at its
/// natural offset (0,1,2,4,8,12,16,20,24,28) for exactly 32 bytes. The `const _`
/// assertion below fails the build if that ever drifts.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct EventRecord {
    pub version: u8,    // 0
    pub opcode: u8,     // 1
    pub track: u16,     // 2..4   (p1=1…; 0 = master/global)
    pub frame: u32,     // 4..8   engine-relative audio-frame timestamp (see note below)
    pub vp: u32,        // 8..12  voice id (NoteOn/Off) or param id (ParamSet)
    pub arg0: f32,      // 12..16
    pub arg1: f32,      // 16..20
    pub arg2: f32,      // 20..24
    pub arg3: f32,      // 24..28
    pub flags_seq: u32, // 28..32 bit flags + sequence counter
}

const _: () = assert!(core::mem::size_of::<EventRecord>() == REC_SIZE);
const _: () = assert!(core::mem::align_of::<EventRecord>() == 4);

// NOTE: `frame` is u32, so the engine-relative timestamp wraps after 2^32 frames — about
// 24.8 h of continuous playback at 48 kHz. The engine's own clock is u64 and does not wrap, so
// past that horizon newly scheduled events read as "late" and fire at buffer starts rather than
// sample-accurately. Acceptable for normal sessions/performances; widening `frame` to u64 (and
// REC_SIZE past 32 bytes) is the fix if a long-running installation ever needs it.

impl EventRecord {
    /// Build a `NoteOn` (0x01). `voice` is the handle TS will reuse for `NoteOff`.
    #[allow(clippy::too_many_arguments)]
    pub fn note_on(
        track: u16,
        frame: u32,
        voice: u32,
        freq: f32,
        vel: f32,
        dur_frames: f32,
        instrument: f32,
        seq: u32,
    ) -> Self {
        Self {
            version: VERSION,
            opcode: op::NOTE_ON,
            track,
            frame,
            vp: voice,
            arg0: freq,
            arg1: vel,
            arg2: dur_frames,
            arg3: instrument,
            flags_seq: seq,
        }
    }

    /// Build a `Tempo` (0x40) on the global track.
    pub fn tempo(frame: u32, cps: f32, seq: u32) -> Self {
        Self {
            version: VERSION,
            opcode: op::TEMPO,
            track: 0,
            frame,
            vp: 0,
            arg0: cps,
            arg1: 0.0,
            arg2: 0.0,
            arg3: 0.0,
            flags_seq: seq,
        }
    }
}

/// A decoded event — a stack value (no heap) the render loop branches on.
#[derive(Debug, PartialEq)]
pub enum Decoded {
    NoteOn {
        track: u16,
        voice: u32,
        freq: f32,
        vel: f32,
        dur_frames: f32,
        instrument: f32,
    },
    NoteOff {
        voice: u32,
    },
    ParamSet {
        track: u16,
        param: u32,
        value: f32,
    },
    /// Set a track-effect parameter. `param` packs `(fx_slot, fx_param)` (see `dsp::fx`).
    TrackFx {
        track: u16,
        param: u32,
        value: f32,
    },
    Tempo {
        cps: f32,
    },
    Nop,
    /// An opcode not implemented in this phase — counted and ignored.
    Unknown(u8),
}

/// Decode one record. Rejects records whose version we do not understand.
#[inline]
pub fn decode(r: &EventRecord) -> Result<Decoded, ()> {
    if r.version != VERSION {
        return Err(());
    }
    Ok(match r.opcode {
        op::NOTE_ON => Decoded::NoteOn {
            track: r.track,
            voice: r.vp,
            freq: r.arg0,
            vel: r.arg1,
            dur_frames: r.arg2,
            instrument: r.arg3,
        },
        op::NOTE_OFF => Decoded::NoteOff { voice: r.vp },
        op::PARAM_SET => Decoded::ParamSet {
            track: r.track,
            param: r.vp,
            value: r.arg0,
        },
        op::TRACK_FX => Decoded::TrackFx {
            track: r.track,
            param: r.vp,
            value: r.arg0,
        },
        op::TEMPO => Decoded::Tempo { cps: r.arg0 },
        op::NOP => Decoded::Nop,
        other => Decoded::Unknown(other),
    })
}

/// Zero-copy view of a byte buffer as records. Errors (rather than panics) on a
/// length that is not a multiple of 32 or on misalignment.
pub fn records_from_bytes(bytes: &[u8]) -> Result<&[EventRecord], ()> {
    if bytes.len() % REC_SIZE != 0 {
        return Err(());
    }
    bytemuck::try_cast_slice(bytes).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_byte_layout_matches_protocol() {
        let r = EventRecord::note_on(1, 0x01020304, 7, 55.0, 0.9, 8000.0, 0.0, 42);
        let bytes: &[u8] = bytemuck::bytes_of(&r);
        assert_eq!(bytes.len(), 32);
        assert_eq!(bytes[0], VERSION); // version
        assert_eq!(bytes[1], op::NOTE_ON); // opcode
        assert_eq!(u16::from_le_bytes([bytes[2], bytes[3]]), 1); // track
        assert_eq!(
            u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            0x01020304
        ); // frame
        assert_eq!(
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            7
        ); // voice
        assert_eq!(
            f32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            55.0
        ); // freq
        assert_eq!(
            u32::from_le_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]),
            42
        ); // seq
    }

    #[test]
    fn round_trip_bytes_to_records() {
        let recs = [
            EventRecord::note_on(1, 100, 1, 55.0, 0.9, 8000.0, 0.0, 0),
            EventRecord::tempo(0, 0.5417, 1),
        ];
        let bytes: &[u8] = bytemuck::cast_slice(&recs);
        let back = records_from_bytes(bytes).expect("cast back");
        assert_eq!(back, &recs);
    }

    #[test]
    fn bad_length_is_rejected() {
        let bytes = [0u8; 31];
        assert!(records_from_bytes(&bytes).is_err());
    }

    #[test]
    fn decode_known_and_unknown() {
        let n = EventRecord::note_on(2, 0, 3, 110.0, 0.8, 0.0, 1.0, 0);
        match decode(&n).unwrap() {
            Decoded::NoteOn { track, voice, freq, .. } => {
                assert_eq!((track, voice, freq), (2, 3, 110.0));
            }
            other => panic!("expected NoteOn, got {other:?}"),
        }
        assert_eq!(decode(&EventRecord::tempo(0, 1.0, 0)).unwrap(), Decoded::Tempo { cps: 1.0 });

        let mut bad = n;
        bad.version = 2;
        assert!(decode(&bad).is_err());

        let mut unknown = n;
        unknown.opcode = 0x99;
        assert_eq!(decode(&unknown).unwrap(), Decoded::Unknown(0x99));
    }
}
