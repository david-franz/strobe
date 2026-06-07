//! The engine-side sample bank (Phase 4, feature 01): immutable decoded buffers addressed by
//! a numeric id, with a **lock-free, wait-free lookup** on the audio thread.
//!
//! The control thread decodes/resamples a sample (non-real-time) and `insert`s it, building a
//! fresh snapshot and swapping it in atomically (`arc_swap`). The audio thread only ever does
//! `lookup` = one wait-free atomic load + index + `Arc::clone` (a refcount bump). A voice that
//! has cloned its `Arc<SampleBuffer>` keeps playing the same buffer even if the bank is later
//! swapped; the buffer's memory frees only when the last `Arc` drops (off the audio thread,
//! since the bank holds the master reference for the buffer's whole life in v1 — no eviction yet).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;

/// An immutable, ready-to-play buffer at the engine's sample rate. Stored **planar**
/// (`[ch0 frames…][ch1 frames…]`) so the sampler voice reads one contiguous channel slice with
/// a fractional index.
pub struct SampleBuffer {
    pub channels: u16,
    pub sample_rate: u32,
    pub frames: usize, // per-channel length
    pub data: Box<[f32]>,
}

impl SampleBuffer {
    /// Build a planar buffer from per-channel planes (already at the engine rate).
    pub fn from_planar(planes: Vec<Vec<f32>>, sample_rate: u32) -> Self {
        let channels = planes.len().max(1) as u16;
        let frames = planes.first().map(|p| p.len()).unwrap_or(0);
        let mut data = Vec::with_capacity(frames * channels as usize);
        for plane in &planes {
            data.extend_from_slice(plane);
        }
        Self { channels, sample_rate, frames, data: data.into_boxed_slice() }
    }

    /// One channel's contiguous samples (clamped to the available channels).
    #[inline]
    pub fn channel(&self, c: usize) -> &[f32] {
        let c = c.min(self.channels as usize - 1);
        let n = self.frames;
        &self.data[c * n..c * n + n]
    }
}

struct BankSnapshot {
    /// Index = sample id. `None` = empty/evicted slot.
    slots: Vec<Option<Arc<SampleBuffer>>>,
}

/// The bank: an atomically-swapped snapshot the audio thread reads wait-free.
pub struct SampleBank {
    current: ArcSwap<BankSnapshot>,
    next_id: AtomicU32,
}

impl Default for SampleBank {
    fn default() -> Self {
        Self::new()
    }
}

impl SampleBank {
    pub fn new() -> Self {
        Self {
            current: ArcSwap::from_pointee(BankSnapshot { slots: Vec::new() }),
            next_id: AtomicU32::new(0),
        }
    }

    /// NON-REAL-TIME (control thread): assign an id, build a new snapshot (Arc-refcount clones
    /// only), and swap it in. The audio thread sees the old or new snapshot, never a torn state.
    pub fn insert(&self, buf: Arc<SampleBuffer>) -> u32 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let cur = self.current.load();
        let mut slots = cur.slots.clone();
        if id as usize >= slots.len() {
            slots.resize((id as usize) + 1, None);
        }
        slots[id as usize] = Some(buf);
        self.current.store(Arc::new(BankSnapshot { slots }));
        id
    }

    /// REAL-TIME-SAFE (audio thread): one wait-free load + index + Arc clone. No alloc, no lock.
    #[inline]
    pub fn lookup(&self, id: u32) -> Option<Arc<SampleBuffer>> {
        let snap = self.current.load();
        snap.slots.get(id as usize).and_then(|s| s.clone())
    }

    /// How many ids have been assigned (test/telemetry).
    pub fn len(&self) -> usize {
        self.current.load().slots.iter().filter(|s| s.is_some()).count()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(frames: usize, ch: u16) -> Arc<SampleBuffer> {
        Arc::new(SampleBuffer {
            channels: ch,
            sample_rate: 48_000,
            frames,
            data: vec![0.25f32; frames * ch as usize].into_boxed_slice(),
        })
    }

    #[test]
    fn insert_then_lookup_round_trips() {
        let bank = SampleBank::new();
        let id = bank.insert(buf(100, 2));
        let got = bank.lookup(id).expect("present");
        assert_eq!(got.frames, 100);
        assert_eq!(got.channels, 2);
    }

    #[test]
    fn unknown_id_is_none_not_panic() {
        let bank = SampleBank::new();
        assert!(bank.lookup(999).is_none());
        let _ = bank.insert(buf(10, 1));
        assert!(bank.lookup(999).is_none());
    }

    #[test]
    fn ids_are_distinct_and_both_resolve() {
        let bank = SampleBank::new();
        let a = bank.insert(buf(10, 1));
        let b = bank.insert(buf(20, 1));
        assert_ne!(a, b);
        assert_eq!(bank.lookup(a).unwrap().frames, 10);
        assert_eq!(bank.lookup(b).unwrap().frames, 20);
    }

    #[test]
    fn an_inflight_handle_survives_a_later_swap() {
        // A voice that cloned its Arc keeps a valid buffer even after more inserts swap the bank.
        let bank = SampleBank::new();
        let id = bank.insert(buf(48, 2));
        let held = bank.lookup(id).unwrap(); // the "voice" holds this
        for _ in 0..5 {
            let _ = bank.insert(buf(10, 1)); // more loads → snapshot swaps
        }
        assert_eq!(held.frames, 48, "held buffer is still valid after swaps");
    }

    #[test]
    fn planar_channel_access() {
        let s = SampleBuffer::from_planar(vec![vec![1.0, 2.0], vec![3.0, 4.0]], 48_000);
        assert_eq!(s.channel(0), &[1.0, 2.0]);
        assert_eq!(s.channel(1), &[3.0, 4.0]);
        assert_eq!(s.channel(5), &[3.0, 4.0], "channel index clamps");
    }
}
