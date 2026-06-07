//! Voice allocation: a fixed, pre-allocated pool of physical voices with per-instrument
//! sub-pools, oldest/quietest voice stealing, choke groups (hats), and handle→voice
//! resolution so a `NoteOff` finds the right voice even after a steal. Real-time-safe:
//! no allocation, no locks on the audio thread (the slot array is built once at start).

use std::sync::Arc;

use crate::graph::InstrumentConfig;
use crate::sample::bank::SampleBuffer;
use crate::voices::drum::{DrumConfig, DrumVoice};
use crate::voices::granular::GranularVoice;
use crate::voices::oscillator::WavetableBank;
use crate::voices::sampler::{SamplerVoice, TrigOpts};
use crate::voices::synth::SynthVoice;
use crate::voices::{ParamId, Voice};

/// Pre-built voice slots per instrument. Generous — the roadmap's 16-voice bar fits with
/// headroom.
pub const SLOTS_PER_INSTRUMENT: usize = 24;
/// Pre-built sampler voices shared across all tracks (Phase 4).
pub const SAMPLER_VOICES: usize = 24;
/// Pre-built granular voices shared across all tracks (Phase 4). Fewer — each owns a grain pool
/// and is heavier; a granular cloud is rarely needed on many tracks at once.
pub const GRANULAR_VOICES: usize = 6;

/// A pre-allocated sampler-voice slot (Phase 4). Triggered by `TrigSample`, not `NoteOn`.
struct SamplerSlot {
    voice: SamplerVoice,
    track: u16,
    started: u64,
}

/// A pre-allocated granular-voice slot (Phase 4). Triggered by `TrigGrain`.
struct GranularSlot {
    voice: GranularVoice,
    track: u16,
    started: u64,
}

/// How the engine builds a voice for an instrument id.
pub enum InstrumentDef {
    Synth(InstrumentConfig),
    Drum(DrumConfig),
}

fn build_voice(def: &InstrumentDef, sr: f32, bank: &WavetableBank) -> Box<dyn Voice> {
    match def {
        InstrumentDef::Synth(c) => Box::new(SynthVoice::new(c, sr, bank)),
        InstrumentDef::Drum(c) => Box::new(DrumVoice::new(c, sr)),
    }
}

#[derive(PartialEq, Copy, Clone, Debug)]
enum SlotState {
    Free,
    Active,
    Releasing,
}

struct Slot {
    voice: Box<dyn Voice>,
    instrument: u32,
    track: u16, // which track (p1=1..) this voice currently belongs to; routes its audio + params
    choke: u8,  // 0 = none
    state: SlotState,
    handle: u32, // TS-supplied NoteOn handle; 0 = none
    started: u64,
    last_level: f32,
}

/// The fixed pool. Built once; `note_on`/`note_off`/`set_param`/`reap`/`render_segment`
/// only touch the pre-allocated slots.
pub struct VoicePool {
    slots: Vec<Slot>,
    samplers: Vec<SamplerSlot>,
    granulars: Vec<GranularSlot>,
    frame: u64,
}

impl VoicePool {
    /// Build `slots_per` voices for each `(instrument_id, def, choke_group)`.
    pub fn new(entries: &[(u32, InstrumentDef, u8)], sr: f32, slots_per: usize) -> Self {
        let bank = WavetableBank::new();
        let mut slots = Vec::with_capacity(entries.len() * slots_per);
        for (id, def, choke) in entries {
            for _ in 0..slots_per {
                slots.push(Slot {
                    voice: build_voice(def, sr, &bank),
                    instrument: *id,
                    track: 0,
                    choke: *choke,
                    state: SlotState::Free,
                    handle: 0,
                    started: 0,
                    last_level: 0.0,
                });
            }
        }
        let samplers =
            (0..SAMPLER_VOICES).map(|_| SamplerSlot { voice: SamplerVoice::new(), track: 0, started: 0 }).collect();
        let granulars = (0..GRANULAR_VOICES)
            .map(|_| GranularSlot { voice: GranularVoice::new(), track: 0, started: 0 })
            .collect();
        Self { slots, samplers, granulars, frame: 0 }
    }

    pub fn active_count(&self) -> usize {
        self.slots.iter().filter(|s| s.state != SlotState::Free).count()
    }

    fn find(&self, instrument: u32, state: SlotState) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.instrument == instrument && s.state == state)
    }

    /// Best steal victim within an instrument's sub-pool: prefer releasing, then quietest,
    /// then oldest.
    fn steal_victim(&self, instrument: u32) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.instrument == instrument)
            .min_by(|(_, a), (_, b)| {
                let ka = (
                    if a.state == SlotState::Releasing { 0u8 } else { 1 },
                    a.last_level,
                    a.started,
                );
                let kb = (
                    if b.state == SlotState::Releasing { 0u8 } else { 1 },
                    b.last_level,
                    b.started,
                );
                ka.0
                    .cmp(&kb.0)
                    .then(ka.1.partial_cmp(&kb.1).unwrap_or(std::cmp::Ordering::Equal))
                    .then(ka.2.cmp(&kb.2))
            })
            .map(|(i, _)| i)
    }

    fn release_choke_group(&mut self, choke: u8) {
        for s in self.slots.iter_mut() {
            if s.choke == choke && s.state == SlotState::Active {
                s.voice.release();
                s.state = SlotState::Releasing;
            }
        }
    }

    /// Apply a `NoteOn`: choke, find/steal a slot, trigger the voice on its track.
    pub fn note_on(
        &mut self,
        track: u16,
        instrument: u32,
        handle: u32,
        freq: f32,
        vel: f32,
        dur_frames: u64,
    ) {
        let choke = self
            .slots
            .iter()
            .find(|s| s.instrument == instrument)
            .map(|s| s.choke)
            .unwrap_or(0);
        if choke != 0 {
            self.release_choke_group(choke);
        }

        let slot = self
            .find(instrument, SlotState::Free)
            .or_else(|| self.find(instrument, SlotState::Releasing))
            .or_else(|| self.steal_victim(instrument));
        let Some(slot) = slot else {
            return; // unknown instrument → drop
        };

        self.frame += 1;
        let frame = self.frame;
        let s = &mut self.slots[slot];
        s.handle = handle;
        s.track = track;
        s.state = SlotState::Active;
        s.started = frame;
        s.last_level = vel;
        s.voice.trigger(freq, vel, dur_frames);
    }

    /// Apply a `NoteOff` by handle. A stale handle (already stolen/finished) is a no-op.
    pub fn note_off(&mut self, handle: u32) {
        if handle == 0 {
            return;
        }
        if let Some(s) = self
            .slots
            .iter_mut()
            .find(|s| s.handle == handle && s.state != SlotState::Free)
        {
            s.voice.release();
            s.state = SlotState::Releasing;
        }
    }

    /// Live-edit a parameter on every active voice **on a given track** (so a per-voice
    /// `ParamSet` from one track doesn't bleed into another).
    pub fn set_param_track(&mut self, track: u16, param: ParamId, value: f32) {
        for s in self.slots.iter_mut() {
            if s.state != SlotState::Free && s.track == track {
                s.voice.set_param(param, value);
            }
        }
    }

    /// Render `[start, end)` of this buffer for every sounding voice into **its track's** stereo
    /// buffer (indexed by track id; voices **add** into the buffer). `tracks_l`/`tracks_r` are
    /// the per-track scratch buffers owned by the audio thread.
    pub fn render_segment(
        &mut self,
        tracks_l: &mut [Vec<f32>],
        tracks_r: &mut [Vec<f32>],
        start: usize,
        end: usize,
    ) {
        if end <= start || tracks_l.is_empty() {
            return;
        }
        let last = tracks_l.len() - 1;
        for s in self.slots.iter_mut() {
            if s.state != SlotState::Free {
                let t = (s.track as usize).min(last);
                s.voice.render(&mut tracks_l[t][start..end], &mut tracks_r[t][start..end]);
            }
        }
    }

    /// Trigger a loaded sample on `track` (Phase 4). Allocates a free sampler voice, stealing the
    /// oldest if all are busy, at sub-buffer `offset`. Arithmetic + `Arc::clone` only.
    #[allow(clippy::too_many_arguments)]
    pub fn trigger_sample(
        &mut self,
        track: u16,
        buf: Arc<SampleBuffer>,
        speed: f32,
        gain: f32,
        start_frame: f32,
        opts: TrigOpts,
        offset: usize,
    ) {
        if self.samplers.is_empty() {
            return;
        }
        let idx = self.samplers.iter().position(|s| !s.voice.is_active()).unwrap_or_else(|| {
            self.samplers
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| s.started)
                .map(|(i, _)| i)
                .unwrap_or(0)
        });
        self.frame += 1;
        let frame = self.frame;
        let s = &mut self.samplers[idx];
        s.track = track;
        s.started = frame;
        s.voice.trigger_at(offset, buf, speed, gain, start_frame, opts);
    }

    /// Render every active sampler voice into its track's buffer over the full block (each voice
    /// applies its own sub-buffer offset). Call once per buffer.
    pub fn render_samplers(
        &mut self,
        tracks_l: &mut [Vec<f32>],
        tracks_r: &mut [Vec<f32>],
        frames: usize,
    ) {
        if tracks_l.is_empty() {
            return;
        }
        let last = tracks_l.len() - 1;
        for s in self.samplers.iter_mut() {
            if s.voice.is_active() {
                let t = (s.track as usize).min(last);
                s.voice.render(&mut tracks_l[t][..frames], &mut tracks_r[t][..frames]);
            }
        }
    }

    /// Active sampler voices (telemetry/tests).
    pub fn active_sampler_count(&self) -> usize {
        self.samplers.iter().filter(|s| s.voice.is_active()).count()
    }

    /// Start a granular cloud on `track` (Phase 4). Allocates a free granular voice, stealing the
    /// oldest if all are busy, at sub-buffer `offset`. Arithmetic + `Arc::clone` only.
    #[allow(clippy::too_many_arguments)]
    pub fn trigger_grain(
        &mut self,
        track: u16,
        buf: Arc<SampleBuffer>,
        center: f32,
        gain: f32,
        size_secs: f32,
        density: f32,
        scatter: f32,
        spread: f32,
        duration_secs: f32,
        offset: usize,
    ) {
        if self.granulars.is_empty() {
            return;
        }
        let idx = self.granulars.iter().position(|s| !s.voice.is_active()).unwrap_or_else(|| {
            self.granulars
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| s.started)
                .map(|(i, _)| i)
                .unwrap_or(0)
        });
        self.frame += 1;
        let frame = self.frame;
        let s = &mut self.granulars[idx];
        s.track = track;
        s.started = frame;
        s.voice
            .trigger_at(offset, buf, center, gain, size_secs, density, scatter, spread, duration_secs);
    }

    /// Render every active granular voice into its track's buffer over the full block. Call once
    /// per buffer (after the samplers).
    pub fn render_granulars(
        &mut self,
        tracks_l: &mut [Vec<f32>],
        tracks_r: &mut [Vec<f32>],
        frames: usize,
    ) {
        if tracks_l.is_empty() {
            return;
        }
        let last = tracks_l.len() - 1;
        for s in self.granulars.iter_mut() {
            if s.voice.is_active() {
                let t = (s.track as usize).min(last);
                s.voice.render(&mut tracks_l[t][..frames], &mut tracks_r[t][..frames]);
            }
        }
    }

    /// Active granular voices (telemetry/tests).
    pub fn active_granular_count(&self) -> usize {
        self.granulars.iter().filter(|s| s.voice.is_active()).count()
    }

    /// Reclaim finished voices and refresh levels for stealing decisions. Once per buffer.
    pub fn reap(&mut self) {
        for s in self.slots.iter_mut() {
            if s.state != SlotState::Free {
                s.last_level = s.voice.current_level();
                if s.voice.is_finished() {
                    s.state = SlotState::Free;
                    s.handle = 0;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::voices::drum::DrumConfig;

    const SR: f32 = 48_000.0;

    /// A minimal Voice for exercising the allocator's bookkeeping precisely.
    struct MockVoice {
        level: f32,
    }
    impl MockVoice {
        fn boxed(level: f32) -> Box<dyn Voice> {
            Box::new(MockVoice { level })
        }
    }
    impl Voice for MockVoice {
        fn trigger(&mut self, _f: f32, vel: f32, _d: u64) {
            self.level = vel;
        }
        fn release(&mut self) {}
        fn is_finished(&self) -> bool {
            false
        }
        fn current_level(&self) -> f32 {
            self.level
        }
        fn render(&mut self, _l: &mut [f32], _r: &mut [f32]) {}
        fn set_param(&mut self, _p: ParamId, _v: f32) {}
    }

    fn pool_of(entries: Vec<(u32, u8, Box<dyn Voice>)>) -> VoicePool {
        let slots = entries
            .into_iter()
            .map(|(instrument, choke, voice)| Slot {
                voice,
                instrument,
                track: 0,
                choke,
                state: SlotState::Free,
                handle: 0,
                started: 0,
                last_level: 0.0,
            })
            .collect();
        VoicePool { slots, samplers: Vec::new(), granulars: Vec::new(), frame: 0 }
    }

    /// A pool with `n` voices of instrument 0 (choke 0).
    fn synth_pool(n: usize) -> VoicePool {
        pool_of((0..n).map(|_| (0u32, 0u8, MockVoice::boxed(1.0))).collect())
    }

    #[test]
    fn allocates_sixteen_simultaneous_voices() {
        let mut p = synth_pool(24);
        for h in 1..=16 {
            p.note_on(1, 0, h, 220.0, 1.0, 0);
        }
        assert_eq!(p.active_count(), 16);
    }

    #[test]
    fn full_subpool_steals_rather_than_growing() {
        let mut p = synth_pool(4);
        for h in 1..=4 {
            p.note_on(1, 0, h, 220.0, 1.0, 0);
        }
        assert_eq!(p.active_count(), 4);
        p.note_on(1, 0, 99, 220.0, 1.0, 0); // 5th → must steal
        assert_eq!(p.active_count(), 4, "never exceeds the sub-pool");
        // handle 99 now owns a slot.
        assert!(p.slots.iter().any(|s| s.handle == 99));
    }

    #[test]
    fn note_off_releases_correct_voice_after_an_unrelated_steal() {
        let mut p = synth_pool(2);
        // A (handle 1) loud & newest so it is not the steal victim.
        p.note_on(1, 0, 1, 220.0, 1.0, 0);
        p.note_on(1, 0, 2, 220.0, 0.1, 0); // B, quiet → preferred victim
        p.note_on(1, 0, 3, 220.0, 1.0, 0); // steals B (quietest)
        assert!(!p.slots.iter().any(|s| s.handle == 2), "B was stolen");
        // A's NoteOff still releases A.
        p.note_off(1);
        let a = p.slots.iter().find(|s| s.handle == 1).unwrap();
        assert_eq!(a.state, SlotState::Releasing);
    }

    #[test]
    fn stale_note_off_is_a_harmless_no_op() {
        let mut p = synth_pool(2);
        p.note_on(1, 0, 1, 220.0, 1.0, 0);
        let before: Vec<_> = p.slots.iter().map(|s| s.state).collect();
        p.note_off(999); // never allocated
        let after: Vec<_> = p.slots.iter().map(|s| s.state).collect();
        assert_eq!(before, after, "no state changed");
    }

    #[test]
    fn reap_frees_finished_voices() {
        // A real one-shot closed hat finishes on its own after its short decay.
        let mut p = VoicePool::new(
            &[(3, InstrumentDef::Drum(DrumConfig::closed_hat()), 1)],
            SR,
            2,
        );
        p.note_on(1, 3, 1, 0.0, 1.0, 0);
        assert_eq!(p.active_count(), 1);
        let mut tl = vec![vec![0.0; SR as usize]; 2];
        let mut tr = vec![vec![0.0; SR as usize]; 2];
        p.render_segment(&mut tl, &mut tr, 0, SR as usize); // render 1 s → hat ends
        p.reap();
        assert_eq!(p.active_count(), 0, "finished voice reclaimed");
    }

    #[test]
    fn choke_group_cuts_a_ringing_voice() {
        // Two instruments sharing choke group 1 (closed + open hat).
        let mut p = pool_of(vec![
            (3, 1, MockVoice::boxed(1.0)), // closed hat
            (4, 1, MockVoice::boxed(1.0)), // open hat
        ]);
        p.note_on(1, 4, 1, 0.0, 1.0, 0); // open hat rings
        assert_eq!(p.active_count(), 1);
        p.note_on(1, 3, 2, 0.0, 1.0, 0); // closed hat → chokes the open hat
        let oh = p.slots.iter().find(|s| s.instrument == 4).unwrap();
        assert_eq!(oh.state, SlotState::Releasing, "open hat choked by closed hat");
    }
}
