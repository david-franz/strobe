//! Consumer side of the event ring — the audio-thread integration point.
//!
//! `drain` is called once per buffer from the render loop (doc 01) with the
//! buffer-start frame `base` and the buffer length `frames`. It applies every event
//! due within this buffer (`frame ≤ base + frames`) at its exact in-buffer sample
//! offset (`frame - base`), so the kick lands on its sample-accurate frame rather
//! than merely at the start of the buffer. HOT PATH — no alloc, no locks.

use std::sync::atomic::Ordering::Relaxed;

use crate::event::{self, Decoded};
use crate::ring::RingConsumer;
use crate::stats::EngineStatsShared;
use crate::voices::drum::KickVoice;

pub fn drain(
    consumer: &mut RingConsumer,
    base: u64,   // engine frame at buffer start
    frames: u64, // buffer length in frames
    kick: &mut KickVoice,
    stats: &EngineStatsShared,
) {
    let buffer_end = base + frames; // exclusive upper bound for "due this buffer"
    let mut consumed = 0usize;
    let available = consumer.len();

    // Events are FIFO and enqueued in non-decreasing frame order within a tick, so we
    // can stop at the first event that is not yet due. Late events sit at the front.
    while consumed < available {
        let r = consumer.peek_at(consumed);

        let dec = match event::decode(&r) {
            Ok(d) => d,
            Err(()) => {
                stats.bad_version.fetch_add(1, Relaxed);
                consumed += 1;
                continue;
            }
        };

        let ev_frame = r.frame as u64;
        let offset: usize = if ev_frame < base {
            // LATE: arrived past its frame. Apply immediately at offset 0, count it.
            stats.late_events.fetch_add(1, Relaxed);
            0
        } else if ev_frame >= buffer_end {
            break; // not due yet — leave it for a later buffer
        } else {
            (ev_frame - base) as usize // exact in-buffer sample offset
        };

        match dec {
            Decoded::NoteOn {
                freq,
                vel,
                dur_frames,
                ..
            } => {
                kick.trigger_at(offset, freq, vel, dur_frames);
            }
            Decoded::Tempo { .. } => { /* Phase 0: no DSP effect; timing is baked into frames */ }
            Decoded::Nop => {}
            Decoded::Unknown(_op) => {
                stats.unknown_op.fetch_add(1, Relaxed);
            }
        }
        consumed += 1;
    }

    consumer.advance(consumed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventRecord;
    use crate::ring::ring;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    fn setup() -> (
        crate::ring::RingProducer,
        RingConsumer,
        KickVoice,
        Arc<EngineStatsShared>,
    ) {
        let drops = Arc::new(AtomicU64::new(0));
        let (p, c) = ring(64, drops);
        (p, c, KickVoice::new(48_000.0), Arc::new(EngineStatsShared::default()))
    }

    #[test]
    fn due_this_buffer_selection() {
        let (p, mut c, mut kick, stats) = setup();
        let base = 1000u64;
        let frames = 256u64;
        // late, on-time, mid, last-in-buffer, future
        p.try_push(EventRecord::note_on(1, (base - 5) as u32, 0, 55.0, 0.9, 0.0, 0.0, 0));
        p.try_push(EventRecord::note_on(1, base as u32, 1, 55.0, 0.9, 0.0, 0.0, 1));
        p.try_push(EventRecord::note_on(1, (base + 10) as u32, 2, 55.0, 0.9, 0.0, 0.0, 2));
        p.try_push(EventRecord::note_on(1, (base + frames - 1) as u32, 3, 55.0, 0.9, 0.0, 0.0, 3));
        p.try_push(EventRecord::note_on(1, (base + frames + 50) as u32, 4, 55.0, 0.9, 0.0, 0.0, 4));

        drain(&mut c, base, frames, &mut kick, &stats);

        // Four applied (incl. the late one), the future one remains for the next buffer.
        assert_eq!(c.len(), 1);
        assert_eq!(c.peek_at(0).frame, (base + frames + 50) as u32);
        assert_eq!(stats.late_events.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn sub_buffer_offset_is_exact() {
        let (p, mut c, mut kick, stats) = setup();
        let base = 500u64;
        p.try_push(EventRecord::note_on(1, (base + 37) as u32, 0, 55.0, 0.9, 0.0, 0.0, 0));
        drain(&mut c, base, 128, &mut kick, &stats);
        assert!(kick.active);
        assert_eq!(kick.test_pending_offset(), 37);
    }

    #[test]
    fn version_and_opcode_robustness() {
        let (p, mut c, mut kick, stats) = setup();
        let mut bad_ver = EventRecord::note_on(1, 0, 0, 55.0, 0.9, 0.0, 0.0, 0);
        bad_ver.version = 2;
        let mut unknown = EventRecord::note_on(1, 1, 0, 55.0, 0.9, 0.0, 0.0, 0);
        unknown.opcode = 0x99;
        p.try_push(bad_ver);
        p.try_push(unknown);
        drain(&mut c, 0, 128, &mut kick, &stats);
        assert_eq!(stats.bad_version.load(Ordering::Relaxed), 1);
        assert_eq!(stats.unknown_op.load(Ordering::Relaxed), 1);
        assert_eq!(c.len(), 0); // both consumed, no panic
    }
}
