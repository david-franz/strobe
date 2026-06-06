//! The lock-free single-producer / single-consumer (SPSC) ring buffer.
//!
//! The TS scheduler (producer, via `pushEvents`) writes frame-stamped records ahead
//! of time; the audio thread (consumer, via `scheduler::drain`) reads them at exactly
//! the right sample. No locks: head/tail are atomic indices and only the producer
//! writes `head`, only the consumer writes `tail`. A full ring drops + counts rather
//! than blocking the scheduler (event-protocol.md "The event ring buffer").

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use bytemuck::Zeroable;

use crate::protocol::event::EventRecord;

struct RingInner {
    buf: Box<[UnsafeCell<EventRecord>]>, // capacity = power of two
    mask: usize,
    head: AtomicUsize, // next write slot  (producer writes, consumer reads)
    tail: AtomicUsize, // next read slot    (consumer writes, producer reads)
}

// SAFETY: the SPSC discipline is upheld by handing out exactly one `RingProducer`
// (sole writer of `head` and of slots ahead of `tail`) and one `RingConsumer` (sole
// writer of `tail`). The Acquire/Release pairing on head/tail publishes slot bytes.
unsafe impl Sync for RingInner {}

/// Producer half — owned by the `Engine`, used by `pushEvents`.
pub struct RingProducer {
    inner: Arc<RingInner>,
    full_drops: Arc<AtomicU64>,
}

/// Consumer half — moved into `AudioState`, used by the audio-thread `drain`.
pub struct RingConsumer {
    inner: Arc<RingInner>,
}

/// Create a ring of `capacity_pow2` records. `full_drops` is shared so the producer
/// can count dropped events without the `Engine` reaching into the ring.
pub fn ring(capacity_pow2: usize, full_drops: Arc<AtomicU64>) -> (RingProducer, RingConsumer) {
    assert!(capacity_pow2.is_power_of_two());
    let buf = (0..capacity_pow2)
        .map(|_| UnsafeCell::new(EventRecord::zeroed()))
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let inner = Arc::new(RingInner {
        buf,
        mask: capacity_pow2 - 1,
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
    });
    (
        RingProducer {
            inner: inner.clone(),
            full_drops,
        },
        RingConsumer { inner },
    )
}

impl RingProducer {
    pub fn capacity(&self) -> usize {
        self.inner.buf.len()
    }

    /// Push one record; returns `false` and counts a drop if the ring is full.
    /// Never blocks.
    #[inline]
    pub fn try_push(&self, rec: EventRecord) -> bool {
        let head = self.inner.head.load(Ordering::Relaxed);
        let tail = self.inner.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= self.inner.buf.len() {
            self.full_drops.fetch_add(1, Ordering::Relaxed);
            return false; // FULL → drop + warn, do not block
        }
        let slot = head & self.inner.mask;
        // SAFETY: SPSC — this is the only writer of this slot, and `tail`'s Acquire
        // above guarantees the consumer has finished reading it.
        unsafe {
            *self.inner.buf[slot].get() = rec;
        }
        self.inner.head.store(head.wrapping_add(1), Ordering::Release);
        true
    }

    /// Push a whole batch; returns how many were dropped because the ring was full.
    pub fn push_batch(&self, recs: &[EventRecord]) -> u32 {
        let mut dropped = 0u32;
        for r in recs {
            if !self.try_push(*r) {
                dropped += 1;
            }
        }
        dropped
    }

    /// Current occupancy as a fraction `0.0..=1.0` (read from the producer side).
    pub fn fullness(&self) -> f64 {
        let head = self.inner.head.load(Ordering::Relaxed);
        let tail = self.inner.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail) as f64 / self.inner.buf.len() as f64
    }
}

impl RingConsumer {
    /// Number of records currently available to read.
    #[inline]
    pub fn len(&self) -> usize {
        let head = self.inner.head.load(Ordering::Acquire);
        let tail = self.inner.tail.load(Ordering::Relaxed);
        head.wrapping_sub(tail)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read the record `offset` ahead of `tail` without removing it.
    #[inline]
    pub fn peek_at(&self, offset: usize) -> EventRecord {
        let tail = self.inner.tail.load(Ordering::Relaxed);
        let slot = tail.wrapping_add(offset) & self.inner.mask;
        // SAFETY: SPSC — slots in `[tail, head)` are published (Acquire on `head` in
        // `len`) and the producer will not overwrite them until we advance `tail`.
        unsafe { *self.inner.buf[slot].get() }
    }

    /// Advance `tail` past `n` consumed records.
    #[inline]
    pub fn advance(&self, n: usize) {
        let tail = self.inner.tail.load(Ordering::Relaxed);
        self.inner.tail.store(tail.wrapping_add(n), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_ring(cap: usize) -> (RingProducer, RingConsumer, Arc<AtomicU64>) {
        let drops = Arc::new(AtomicU64::new(0));
        let (p, c) = ring(cap, drops.clone());
        (p, c, drops)
    }

    #[test]
    fn push_peek_advance() {
        let (p, c, _drops) = new_ring(8);
        for i in 0..3 {
            assert!(p.try_push(EventRecord::note_on(1, i, i, 55.0, 0.9, 0.0, 0.0, i)));
        }
        assert_eq!(c.len(), 3);
        assert_eq!(c.peek_at(0).frame, 0);
        assert_eq!(c.peek_at(2).frame, 2);
        c.advance(2);
        assert_eq!(c.len(), 1);
        assert_eq!(c.peek_at(0).frame, 2);
    }

    #[test]
    fn full_ring_drops_and_counts_never_blocks() {
        let (p, _c, drops) = new_ring(4);
        for i in 0..4 {
            assert!(p.try_push(EventRecord::tempo(i, 0.5, i)));
        }
        // One past capacity → dropped, counted, no panic/hang.
        assert!(!p.try_push(EventRecord::tempo(99, 0.5, 99)));
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        assert!((p.fullness() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn batch_push_reports_drops() {
        let (p, _c, _drops) = new_ring(4);
        let recs: Vec<_> = (0..6)
            .map(|i| EventRecord::tempo(i, 0.5, i))
            .collect();
        let dropped = p.push_batch(&recs);
        assert_eq!(dropped, 2); // 4 fit, 2 dropped
    }

    #[test]
    fn wraps_around_capacity() {
        let (p, c, _drops) = new_ring(4);
        // Fill, drain, refill across the wrap boundary.
        for i in 0..4 {
            assert!(p.try_push(EventRecord::tempo(i, 0.5, i)));
        }
        c.advance(4);
        for i in 4..8 {
            assert!(p.try_push(EventRecord::tempo(i, 0.5, i)));
        }
        assert_eq!(c.len(), 4);
        assert_eq!(c.peek_at(0).frame, 4);
        assert_eq!(c.peek_at(3).frame, 7);
    }
}
