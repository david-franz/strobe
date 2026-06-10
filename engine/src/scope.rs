//! A lock-free scope tap: the audio thread pushes recent master-output samples; the control
//! thread (the napi bridge) reads the latest window to draw a waveform in the terminal.
//!
//! RT-safe by the same discipline as the rest of the audio plane: a fixed `AtomicU32` ring,
//! written and read with `Relaxed` atomics. The single audio-thread writer never locks or
//! allocates; the reader copies the latest N samples. A torn read across the wrap is visually
//! negligible for a scope (and never UB, since every slot access is atomic).

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::Relaxed};

pub struct Scope {
    buf: Box<[AtomicU32]>, // f32 bits, indexed by `head & mask`
    head: AtomicUsize,     // total samples written (wraps); newest is at head-1
    mask: usize,
}

impl Scope {
    /// `capacity` is rounded up to a power of two (min 64) so indexing is a mask.
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.next_power_of_two().max(64);
        let buf = (0..cap).map(|_| AtomicU32::new(0)).collect::<Vec<_>>().into_boxed_slice();
        Self { buf, head: AtomicUsize::new(0), mask: cap - 1 }
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Audio thread: append one sample. A single writer, so the load/store of `head` needs no CAS.
    #[inline]
    pub fn push(&self, sample: f32) {
        let h = self.head.load(Relaxed);
        self.buf[h & self.mask].store(sample.to_bits(), Relaxed);
        self.head.store(h.wrapping_add(1), Relaxed);
    }

    /// Control thread: fill `out` with the most recent `out.len()` samples, oldest first. Before
    /// the ring has filled, the not-yet-written slots read as the zeros they were initialised to.
    pub fn read_latest(&self, out: &mut [f32]) {
        let n = out.len().min(self.buf.len());
        let h = self.head.load(Relaxed);
        let start = h.wrapping_sub(n); // window [h-n, h)
        for (i, slot) in out[..n].iter_mut().enumerate() {
            *slot = f32::from_bits(self.buf[start.wrapping_add(i) & self.mask].load(Relaxed));
        }
        for slot in out[n..].iter_mut() {
            *slot = 0.0;
        }
    }

    /// Convenience for the napi bridge: the latest `n` samples as a `Vec` (control thread → alloc
    /// is fine here).
    pub fn latest(&self, n: usize) -> Vec<f32> {
        let mut out = vec![0.0; n];
        self.read_latest(&mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_rounds_up_to_a_power_of_two() {
        assert_eq!(Scope::new(1000).capacity(), 1024);
        assert_eq!(Scope::new(4096).capacity(), 4096);
        assert_eq!(Scope::new(1).capacity(), 64); // floor
    }

    #[test]
    fn reads_back_the_latest_window_in_order() {
        let s = Scope::new(8); // tiny ring
        for i in 0..6 {
            s.push(i as f32);
        }
        let mut out = [0.0; 4];
        s.read_latest(&mut out);
        assert_eq!(out, [2.0, 3.0, 4.0, 5.0], "the last 4 pushed, oldest first");
    }

    #[test]
    fn survives_wraparound() {
        let s = Scope::new(4); // capacity 4
        for i in 0..10 {
            s.push(i as f32); // 6,7,8,9 remain
        }
        let mut out = [0.0; 4];
        s.read_latest(&mut out);
        assert_eq!(out, [6.0, 7.0, 8.0, 9.0]);
    }

    #[test]
    fn empty_ring_reads_zeros() {
        let s = Scope::new(64);
        assert!(s.latest(16).iter().all(|&x| x == 0.0));
    }

    #[test]
    fn requesting_more_than_capacity_is_clamped_not_panicked() {
        let s = Scope::new(64);
        s.push(1.0);
        let v = s.latest(1000); // > capacity
        assert_eq!(v.len(), 1000);
    }
}
