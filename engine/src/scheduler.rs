//! Consumer side of the event ring — the audio-thread integration point.
//!
//! `dispatch_and_render` drains the events due in this buffer and renders the voice pool
//! in **sample-accurate segments**: it renders the active voices up to the next event's
//! in-buffer offset, applies that event (which may trigger/steal a voice), then continues
//! — so a note lands on its exact frame, not merely at the start of the buffer. HOT PATH:
//! no alloc, no locks.

use std::sync::atomic::Ordering::Relaxed;

use crate::dsp::effects::MasterBus;
use crate::dsp::fx::TrackEffects;
use crate::protocol::event::{self, Decoded};
use crate::protocol::ring::RingConsumer;
use crate::stats::EngineStatsShared;
use crate::voice_alloc::VoicePool;
use crate::voices::ParamId;

fn apply(
    dec: Decoded,
    pool: &mut VoicePool,
    bus: &mut MasterBus,
    chains: &mut [TrackEffects],
    stats: &EngineStatsShared,
) {
    match dec {
        Decoded::NoteOn { track, voice, freq, vel, dur_frames, instrument } => {
            pool.note_on(track, instrument as u32, voice, freq, vel, dur_frames.max(0.0) as u64);
        }
        Decoded::NoteOff { voice } => pool.note_off(voice),
        Decoded::ParamSet { track, param, value } => {
            if track == 0 {
                bus.set_master_param(param, value); // master/global params live on track 0
            } else if let Some(p) = ParamId::from_u32(param) {
                pool.set_param_track(track, p, value);
            }
        }
        Decoded::TrackFx { track, param, value } => {
            if let Some(chain) = chains.get_mut(track as usize) {
                chain.set_param(param, value); // param packs (fx_slot, fx_param)
            }
        }
        Decoded::ParamRamp { track, param, target, ramp_frames } => {
            // Smooth automation of an effect param (e.g. .lpf(sine…)) — glides on the audio thread.
            if let Some(chain) = chains.get_mut(track as usize) {
                chain.ramp_param(param, target, ramp_frames);
            }
        }
        Decoded::Tempo { .. } | Decoded::Nop => {}
        Decoded::Unknown(_) => {
            stats.unknown_op.fetch_add(1, Relaxed);
        }
    }
}

/// Apply every event whose in-buffer offset is `<= pos` (including late events), then
/// return the offset of the next event due *later* in this buffer (or `None`).
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn apply_due(
    consumer: &mut RingConsumer,
    base: u64,
    buffer_end: u64,
    pos: usize,
    pool: &mut VoicePool,
    bus: &mut MasterBus,
    chains: &mut [TrackEffects],
    stats: &EngineStatsShared,
) -> Option<usize> {
    while consumer.len() > 0 {
        let r = consumer.peek_at(0);
        let dec = match event::decode(&r) {
            Ok(d) => d,
            Err(()) => {
                stats.bad_version.fetch_add(1, Relaxed);
                consumer.advance(1);
                continue;
            }
        };
        let evf = r.frame as u64;
        if evf >= buffer_end {
            return None; // not due this buffer — leave for a later one
        }
        let off = if evf < base { 0 } else { (evf - base) as usize };
        if off <= pos {
            if evf < base {
                stats.late_events.fetch_add(1, Relaxed);
            }
            apply(dec, pool, bus, chains, stats);
            consumer.advance(1);
        } else {
            return Some(off);
        }
    }
    None
}

/// Drain the events due this buffer and render the voice pool into the **per-track** stereo
/// buffers (`tracks_l`/`tracks_r`, indexed by track id) in sample-accurate segments. Effect
/// params (`TrackFx`) update their track's chain here; the chains/sends/master themselves are
/// run once per block by the caller (`audio_io::render`).
#[allow(clippy::too_many_arguments)]
pub fn dispatch_and_render(
    consumer: &mut RingConsumer,
    base: u64,
    frames: u64,
    pool: &mut VoicePool,
    bus: &mut MasterBus,
    chains: &mut [TrackEffects],
    stats: &EngineStatsShared,
    tracks_l: &mut [Vec<f32>],
    tracks_r: &mut [Vec<f32>],
) {
    let buffer_end = base + frames;
    let frames = frames as usize;
    let mut pos = 0usize;
    loop {
        let next =
            apply_due(consumer, base, buffer_end, pos, pool, bus, chains, stats).unwrap_or(frames);
        let seg_end = next.min(frames);
        pool.render_segment(tracks_l, tracks_r, pos, seg_end);
        pos = seg_end;
        if pos >= frames {
            break;
        }
    }
    pool.reap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::event::EventRecord;
    use crate::graph::InstrumentConfig;
    use crate::protocol::ring::{ring, RingProducer};
    use crate::voice_alloc::{InstrumentDef, VoicePool};
    use crate::voices::drum::DrumConfig;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    const SR: f32 = 48_000.0;

    fn setup(def: InstrumentDef) -> (RingProducer, RingConsumer, VoicePool, Arc<EngineStatsShared>) {
        let drops = Arc::new(AtomicU64::new(0));
        let (p, c) = ring(256, drops);
        let pool = VoicePool::new(&[(0, def, 0)], SR, 8);
        (p, c, pool, Arc::new(EngineStatsShared::default()))
    }

    fn note_on(track: u16, frame: u32, voice: u32, freq: f32, dur: f32) -> EventRecord {
        EventRecord::note_on(track, frame, voice, freq, 0.9, dur, 0.0, voice)
    }

    use crate::dsp::fx::MAX_TRACKS;

    fn per_track(frames: usize) -> Vec<Vec<f32>> {
        (0..=MAX_TRACKS).map(|_| vec![0.0; frames]).collect()
    }
    fn empty_chains() -> Vec<TrackEffects> {
        (0..=MAX_TRACKS).map(|_| TrackEffects::new(SR)).collect()
    }

    /// Drive one buffer and return track 1's left channel (where the test notes render).
    fn dispatch(c: &mut RingConsumer, base: u64, frames: u64, pool: &mut VoicePool, st: &EngineStatsShared) -> Vec<f32> {
        let mut bus = crate::dsp::effects::MasterBus::new(SR);
        let mut chains = empty_chains();
        let mut tl = per_track(frames as usize);
        let mut tr = per_track(frames as usize);
        dispatch_and_render(c, base, frames, pool, &mut bus, &mut chains, st, &mut tl, &mut tr);
        tl.swap_remove(1)
    }

    #[test]
    fn note_on_triggers_a_voice_due_this_buffer() {
        let (p, mut c, mut pool, st) = setup(InstrumentDef::Drum(DrumConfig::kick()));
        let base = 1000u64;
        p.try_push(note_on(1, base as u32, 1, 0.0, 0.0)); // due now
        p.try_push(note_on(1, (base + 5000) as u32, 2, 0.0, 0.0)); // far future
        dispatch(&mut c, base, 256, &mut pool, &st);
        assert_eq!(pool.active_count(), 1, "only the due note triggered");
        assert_eq!(c.len(), 1, "future event remains for a later buffer");
    }

    #[test]
    fn sub_buffer_offset_places_the_onset_exactly() {
        let (p, mut c, mut pool, st) = setup(InstrumentDef::Drum(DrumConfig::kick()));
        let base = 0u64;
        let offset = 100u32;
        p.try_push(note_on(1, offset, 1, 0.0, 0.0));
        let l = dispatch(&mut c, base, 256, &mut pool, &st);
        assert!(l[..offset as usize].iter().all(|&s| s == 0.0), "silent before the onset");
        assert!(l[offset as usize..].iter().any(|&s| s.abs() > 1e-6), "sound from the onset");
    }

    #[test]
    fn late_event_is_applied_and_counted() {
        let (p, mut c, mut pool, st) = setup(InstrumentDef::Drum(DrumConfig::kick()));
        let base = 1000u64;
        p.try_push(note_on(1, (base - 50) as u32, 1, 0.0, 0.0)); // late
        dispatch(&mut c, base, 256, &mut pool, &st);
        assert_eq!(pool.active_count(), 1, "late note still played");
        assert_eq!(st.late_events.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn note_off_releases_a_held_synth_voice() {
        let (p, mut c, mut pool, st) =
            setup(InstrumentDef::Synth(InstrumentConfig::default()));
        let base = 0u64;
        p.try_push(note_on(1, 0, 7, 220.0, 0.0)); // held (dur 0)
        // NoteOff for handle 7 later in the same buffer.
        p.try_push(EventRecord {
            version: 1,
            opcode: 0x02,
            track: 1,
            frame: 100,
            vp: 7,
            arg0: 0.0,
            arg1: 0.0,
            arg2: 0.0,
            arg3: 0.0,
            flags_seq: 0,
        });
        dispatch(&mut c, base, 256, &mut pool, &st);
        // Render a long tail; the released voice frees.
        let mut bus = crate::dsp::effects::MasterBus::new(SR);
        let mut chains = empty_chains();
        let mut tl = per_track(SR as usize);
        let mut tr = per_track(SR as usize);
        dispatch_and_render(&mut c, base + 256, SR as u64, &mut pool, &mut bus, &mut chains, &st, &mut tl, &mut tr);
        assert_eq!(pool.active_count(), 0, "held voice released by NoteOff then freed");
    }

    #[test]
    fn unknown_opcode_is_counted_and_skipped() {
        let (p, mut c, mut pool, st) = setup(InstrumentDef::Drum(DrumConfig::kick()));
        let mut rec = note_on(1, 0, 1, 0.0, 0.0);
        rec.opcode = 0x55; // unimplemented
        p.try_push(rec);
        dispatch(&mut c, 0, 256, &mut pool, &st);
        assert_eq!(st.unknown_op.load(Ordering::Relaxed), 1);
        assert_eq!(pool.active_count(), 0);
    }
}
