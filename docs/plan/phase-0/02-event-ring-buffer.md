# 02 — Event ring buffer & the audio-thread consumer

## Purpose

This is the queue that lets the two planes talk without ever sharing a lock. The TypeScript scheduler (the **producer**) writes frame-stamped 32-byte event records ahead of time; the audio thread (the **consumer**, living in `scheduler.rs`) drains them at exactly the right sample. It is a **lock-free single-producer / single-consumer (SPSC) ring buffer** — the central mechanism from [architecture.md](../../architecture.md#the-core-principle-two-planes) that keeps DSP off the garbage-collected control plane.

Phase 0's job here is to nail the parts that make timing *sample-accurate and glitch-free*: fixed-size records (no variable-length parsing on the audio thread), atomic head/tail indices (no locks), draining exactly the events due in this buffer and applying each at its **in-buffer sample offset**, handling late events by applying-immediately-and-counting (never silently dropping music), and handling a full ring on the producer side by **dropping and warning** rather than blocking the scheduler ([event-protocol.md](../../event-protocol.md#the-event-ring-buffer)).

## Location

```
engine/
  Cargo.toml          # + bytemuck dependency
  src/
    ring.rs           # NEW — the SPSC ring (producer + consumer halves)
    event.rs          # NEW — the #[repr(C)] 32-byte record + decode
    scheduler.rs      # NEW — the consumer-side drain(): the audio-thread integration
    audio_io.rs       # (01) calls scheduler::drain once per buffer
```

This document owns `ring.rs`, `event.rs`, and the consumer side of `scheduler.rs`. The producer side is poked by `pushEvents` in [04-napi-bridge.md](04-napi-bridge.md); the byte layout mirrors the TS encoder in [05-lookahead-scheduler.md](05-lookahead-scheduler.md).

## Design

### The record struct (`event.rs`)

The 32-byte layout is fixed by [event-protocol.md](../../event-protocol.md#event-record-layout). We mirror it as a `#[repr(C)]` struct and cast bytes with `bytemuck` — no field-by-field `from_le_bytes`, no allocation. We rely on little-endian hosts (x86-64 and aarch64, our targets, are LE), matching the protocol's "little-endian" contract.

```toml
# engine/Cargo.toml (excerpt)
[dependencies]
bytemuck = { version = "1", features = ["derive"] }
```

```rust
// engine/src/event.rs
use bytemuck::{Pod, Zeroable};

pub const REC_SIZE: usize = 32;

pub const VERSION: u8 = 1;

// Opcodes — fixed by event-protocol.md. Phase 0 implements NoteOn + Tempo + Nop.
pub mod op {
    pub const NOTE_ON:   u8 = 0x01;
    pub const NOTE_OFF:  u8 = 0x02;
    pub const PARAM_SET: u8 = 0x10;
    pub const PARAM_RAMP:u8 = 0x11;
    pub const TRIG_SAMPLE:u8= 0x20;
    pub const TRACK_FX:  u8 = 0x30;
    pub const TEMPO:     u8 = 0x40;
    pub const NOP:       u8 = 0x7F;
}

/// Mirrors event-protocol.md exactly, little-endian, 32 bytes.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct EventRecord {
    pub version: u8,     // 0
    pub opcode:  u8,     // 1
    pub track:   u16,    // 2..4   (p1=1…; 0 = master/global)
    pub frame:   u32,    // 4..8   engine-relative audio-frame timestamp
    pub vp:      u32,    // 8..12  voice id (NoteOn/Off) or param id (ParamSet)
    pub arg0:    f32,    // 12..16
    pub arg1:    f32,    // 16..20
    pub arg2:    f32,    // 20..24
    pub arg3:    f32,    // 24..28
    pub flags_seq: u32,  // 28..32 bit flags + sequence counter
}

const _: () = assert!(core::mem::size_of::<EventRecord>() == REC_SIZE);
```

> Layout note: `#[repr(C)]` with `u8,u8,u16,u32,u32,f32,f32,f32,f32,u32` packs with no padding (each field is naturally aligned at these offsets), giving exactly 32 bytes — matching the offset table in [event-protocol.md](../../event-protocol.md#event-record-layout). The `const _` assert fails the build if that ever drifts.

Casting a byte slice straight to records (the producer copy is raw bytes from the `ArrayBuffer`):

```rust
// Zero-copy view of a byte buffer as records. Errors instead of panicking on bad length.
pub fn records_from_bytes(bytes: &[u8]) -> Result<&[EventRecord], ()> {
    if bytes.len() % REC_SIZE != 0 { return Err(()); }
    // bytemuck checks alignment & size; our ArrayBuffer copies are 4-byte aligned.
    bytemuck::try_cast_slice(bytes).map_err(|_| ())
}
```

### The decoded event (`event.rs`)

The audio thread does not branch on raw fields; it decodes into a tiny enum (a stack value, no heap) so the render loop reads cleanly. A record whose `version != VERSION` is rejected and counted.

```rust
pub enum Decoded {
    NoteOn { track: u16, voice: u32, freq: f32, vel: f32, dur_frames: f32, instrument: f32 },
    Tempo  { cps: f32 },
    Nop,
    Unknown(u8), // unimplemented-in-phase-0 opcode → counted, ignored
}

#[inline]
pub fn decode(r: &EventRecord) -> Result<Decoded, ()> {
    if r.version != VERSION { return Err(()); }
    Ok(match r.opcode {
        op::NOTE_ON => Decoded::NoteOn {
            track: r.track, voice: r.vp,
            freq: r.arg0, vel: r.arg1, dur_frames: r.arg2, instrument: r.arg3,
        },
        op::TEMPO => Decoded::Tempo { cps: r.arg0 },
        op::NOP   => Decoded::Nop,
        other     => Decoded::Unknown(other),
    })
}
```

### The SPSC ring (`ring.rs`)

A pre-allocated `Box<[EventRecord]>` of fixed capacity, plus two atomic indices. Capacity is a **power of two** so index wrap is a cheap mask. We split it into a `RingProducer` (owned by the `Engine`, used by `pushEvents`) and a `RingConsumer` (moved into `AudioState`, used by `drain`). Each half holds an `Arc` to the shared buffer/indices; only the producer writes `head`, only the consumer writes `tail` — the SPSC discipline that makes `Relaxed`/`Acquire`/`Release` sufficient without a lock.

```rust
// engine/src/ring.rs
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use crate::event::EventRecord;

struct RingInner {
    buf:  Box<[std::cell::UnsafeCell<EventRecord>]>, // capacity = power of two
    mask: usize,
    head: AtomicUsize, // next write slot   (producer writes, consumer reads)
    tail: AtomicUsize, // next read slot     (consumer writes, producer reads)
}
unsafe impl Sync for RingInner {}  // SPSC discipline upheld by the two halves

pub struct RingProducer { inner: Arc<RingInner>, full_drops: Arc<std::sync::atomic::AtomicU64> }
pub struct RingConsumer { inner: Arc<RingInner> }

pub fn ring(capacity_pow2: usize, full_drops: Arc<std::sync::atomic::AtomicU64>)
    -> (RingProducer, RingConsumer)
{
    assert!(capacity_pow2.is_power_of_two());
    let buf = (0..capacity_pow2)
        .map(|_| std::cell::UnsafeCell::new(EventRecord::zeroed()))
        .collect::<Vec<_>>().into_boxed_slice();
    let inner = Arc::new(RingInner {
        buf, mask: capacity_pow2 - 1,
        head: AtomicUsize::new(0), tail: AtomicUsize::new(0),
    });
    (RingProducer { inner: inner.clone(), full_drops },
     RingConsumer { inner })
}
```

**Producer (non-audio thread, called by `pushEvents`).** Pushes one record; returns `false` and increments the drop counter if the ring is full (drop-and-warn, never block — [event-protocol.md](../../event-protocol.md#the-event-ring-buffer)).

```rust
impl RingProducer {
    #[inline]
    pub fn try_push(&self, rec: EventRecord) -> bool {
        let head = self.inner.head.load(Ordering::Relaxed);
        let tail = self.inner.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= self.inner.buf.len() {
            self.full_drops.fetch_add(1, Ordering::Relaxed);
            return false;                 // FULL → drop + warn, do not block
        }
        let slot = head & self.inner.mask;
        unsafe { *self.inner.buf[slot].get() = rec; }
        self.inner.head.store(head.wrapping_add(1), Ordering::Release);
        true
    }

    /// Push a whole batch; returns how many were dropped (full). Used by pushEvents.
    pub fn push_batch(&self, recs: &[EventRecord]) -> u32 {
        let mut dropped = 0u32;
        for r in recs { if !self.try_push(*r) { dropped += 1; } }
        dropped
    }
}
```

**Consumer (audio thread).** Reads without removing first (peek), so we only advance `tail` past events that are actually due this buffer.

```rust
impl RingConsumer {
    #[inline] fn len(&self) -> usize {
        let head = self.inner.head.load(Ordering::Acquire);
        let tail = self.inner.tail.load(Ordering::Relaxed);
        head.wrapping_sub(tail)
    }
    #[inline] fn peek_at(&self, offset: usize) -> EventRecord {
        let tail = self.inner.tail.load(Ordering::Relaxed);
        let slot = (tail.wrapping_add(offset)) & self.inner.mask;
        unsafe { *self.inner.buf[slot].get() }
    }
    #[inline] fn advance(&self, n: usize) {
        let tail = self.inner.tail.load(Ordering::Relaxed);
        self.inner.tail.store(tail.wrapping_add(n), Ordering::Release);
    }
}
```

### The drain (`scheduler.rs`, consumer side) — the heart of timing

Called once per buffer from the render loop ([01-audio-io-cpal.md](01-audio-io-cpal.md)) with the buffer-start frame `base` and the buffer length `frames`. It applies every event whose `frame ≤ base + frames` (i.e. due within this buffer), at its **in-buffer sample offset** `frame - base`, by triggering the voice with that offset so the kick starts at the exact sample, not just "somewhere in this buffer."

```rust
// engine/src/scheduler.rs  (consumer side; HOT PATH — no alloc, no locks)
use crate::event::{self, Decoded};
use crate::ring::RingConsumer;
use crate::voices::drum::KickVoice;
use crate::stats::EngineStatsShared;

pub fn drain(
    consumer: &mut RingConsumer,
    base: u64,          // engine frame at buffer start
    frames: u64,        // buffer length in frames
    kick: &mut KickVoice,
    stats: &EngineStatsShared,
) {
    let buffer_end = base + frames;        // exclusive upper bound for "due this buffer"
    let mut consumed = 0usize;
    let available = consumer.len();

    // Events are FIFO and the scheduler enqueues them in non-decreasing frame order,
    // so we can stop at the first event that is not yet due.
    while consumed < available {
        let r = consumer.peek_at(consumed);

        let dec = match event::decode(&r) {
            Ok(d) => d,
            Err(()) => { stats.bad_version.fetch_add(1, Relaxed); consumed += 1; continue; }
        };

        // Resolve the in-buffer offset, with late-event handling.
        let ev_frame = r.frame as u64; // engine-relative (see note on the 32-bit window below)
        let offset: usize = if ev_frame < base {
            // LATE: arrived past its frame. Apply immediately at offset 0, count it.
            stats.late_events.fetch_add(1, Relaxed);
            0
        } else if ev_frame >= buffer_end {
            break;                       // not due yet — leave it for a later buffer
        } else {
            (ev_frame - base) as usize;  // exact in-buffer sample offset
        };

        match dec {
            Decoded::NoteOn { vel, dur_frames, freq, .. } => {
                // (03) trigger the kick at the precise sub-buffer offset.
                kick.trigger_at(offset, freq, vel, dur_frames);
            }
            Decoded::Tempo { cps } => { /* Phase 0: store for stats/echo; no DSP effect */ }
            Decoded::Nop => {}
            Decoded::Unknown(op) => { stats.unknown_op.fetch_add(1, Relaxed); }
        }
        consumed += 1;
    }
    consumer.advance(consumed);
}
```

A few subtle but load-bearing points:

- **Ordered enqueue ⇒ early break.** The scheduler pushes events in non-decreasing `frame` order within a tick ([05-lookahead-scheduler.md](05-lookahead-scheduler.md)), so once we hit an event with `ev_frame >= buffer_end` we can stop scanning. Late events (`ev_frame < base`) are at the front and are flushed first.
- **Sub-buffer accuracy.** Passing `offset` to `kick.trigger_at(offset, …)` is what makes timing *sample-accurate* rather than *buffer-accurate*. The voice begins emitting nonzero output starting at sample `offset` within this buffer (see [03-kick-voice.md](03-kick-voice.md)).
- **The 32-bit `frame` window.** The protocol's `frame` is `u32`; the engine clock is `u64`. At 48 kHz a `u32` wraps every ~24.8 hours. Phase 0 keeps sessions far under that, and the scheduler stamps frames relative to a session epoch so `r.frame as u64` is correct. (A documented Phase-1 follow-up widens or epoch-rebases this; the record layout does not change.)

## Interfaces

- **To [01-audio-io-cpal.md](01-audio-io-cpal.md):** `scheduler::drain(consumer, base, frames, kick, stats)` is called once per buffer; `RingConsumer` lives in `AudioState`.
- **To [03-kick-voice.md](03-kick-voice.md):** drain invokes `kick.trigger_at(offset, freq, vel, dur_frames)` on `NoteOn`.
- **To [04-napi-bridge.md](04-napi-bridge.md):** `pushEvents` copies the incoming bytes, casts via `records_from_bytes`, and calls `producer.push_batch(...)`, returning the drop count into `stats`. `ringFullness` in `stats()` reads `consumer.len() / capacity` (read from the producer side's view of head/tail).
- **To [05-lookahead-scheduler.md](05-lookahead-scheduler.md):** the TS encoder writes the exact mirror of `EventRecord` via `DataView`; this file's `event.rs` is the single source of truth for offsets, kept identical to the table in [event-protocol.md](../../event-protocol.md#encoding-in-typescript).

## Edge cases & risks

- **Ring full (backpressure).** Producer drops and increments `full_drops`, surfaced as part of `ringFullness`/a drop counter in `stats()`. Mitigation: size the ring to comfortably exceed one lookahead window of events. Phase 0 default capacity **4096 records** (128 KiB) — orders of magnitude over the ~tens of events a 100 ms window holds at techno tempos.
- **Late events.** Applied immediately at offset 0 and counted in `late_events`; never musically dropped ([event-protocol.md](../../event-protocol.md#the-frame-timestamp)). A nonzero `lateEvents` under normal load fails the Phase 0 gate and signals lookahead is too short.
- **Bad version / unknown opcode.** Counted (`bad_version`, `unknown_op`) and skipped — forward-compatibility per [event-protocol.md](../../event-protocol.md#versioning); never panics on the audio thread.
- **Malformed batch length.** `pushEvents` rejects a buffer whose length isn't a multiple of 32 *before* touching the ring (in `records_from_bytes`), on the non-audio thread.
- **Memory ordering correctness.** Producer `Release`s `head` after writing the slot; consumer `Acquire`s `head` before reading slots — this publishes the record bytes. Consumer `Release`s `tail`; producer `Acquire`s `tail` before checking fullness. This is the standard SPSC fence pairing; reviewed explicitly.
- **No allocation on the audio thread.** `drain` uses only stack values and the pre-allocated ring; `Decoded` is a non-heap enum. Verified in review and the stress check.

## Acceptance criteria

- [ ] **Round-trip a record.** *Unit test:* build an `EventRecord` in Rust, `bytemuck`-cast to `&[u8]`, and assert each byte matches the offsets in [event-protocol.md](../../event-protocol.md#event-record-layout); cast back and assert equality. Cross-check against bytes produced by the TS encoder ([05](05-lookahead-scheduler.md)) for one `NoteOn`.
- [ ] **Due-this-buffer selection.** *Unit test:* push events at frames `{base-5 (late), base+0, base+10, base+frames-1, base+frames+50}`; call `drain(base, frames)`; assert the first four are applied (the late one at offset 0, counted in `lateEvents`), the last is *not* consumed and remains for the next buffer.
- [ ] **Sub-buffer offset is exact.** *Unit test:* with a spy voice, drain a single `NoteOn` at `frame = base + 37` and assert `trigger_at` received `offset == 37`.
- [ ] **Full ring drops + warns, never blocks.** *Unit test:* fill the ring to capacity, push one more, assert `try_push` returned `false`, `full_drops == 1`, and no panic/hang.
- [ ] **Version/opcode robustness.** *Unit test:* feed a record with `version = 2` (→ `bad_version` incremented, skipped) and one with `opcode = 0x99` (→ `unknown_op` incremented, skipped); assert no panic.
- [ ] **Lock-free / alloc-free.** *How to test:* code review confirms no `lock()`/`Box`/`Vec`/`String` in `drain`/`try_push`/`peek`/`advance`; the 60 s stress check ([01](01-audio-io-cpal.md)) reports zero underruns and zero late events under normal load.
