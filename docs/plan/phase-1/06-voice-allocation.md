# 06 — Voice allocation

## Purpose

Polyphony — many notes at once — needs a pool of physical voices and a policy for what to do
when more notes arrive than there are voices. This feature is **`voice_alloc.rs`**: a
**fixed, pre-allocated pool** of voices, a **voice-stealing** policy (steal the oldest /
quietest first), and — critically — a **handle → physical-voice map** so that a `NoteOff`
arriving later finds the *right* voice even after that voice was stolen and reassigned. It is
**real-time-safe**: no allocation, no locks, on the audio thread. This is the layer that lets
the roadmap's "**16+ simultaneous voices play cleanly**" bar be met.

## Location

```
engine/src/voice_alloc.rs   # VoicePool, allocation + stealing + handle map (new)
engine/src/scheduler.rs     # NoteOn/NoteOff/ParamSet → voice_alloc calls (extended, Phase 0)
engine/src/audio_io.rs      # render loop iterates active voices via the pool (extended)
```

Allocates the voices defined in [04-synth-voice.md](04-synth-voice.md) and
[05-drum-synthesis.md](05-drum-synthesis.md); its summed output feeds
[07-master-bus-limiter.md](07-master-bus-limiter.md).

## Design

### The pool — pre-allocated, no alloc on the hot path

The pool is built **once** at `start()` (or when an instrument is first defined): every voice
slot's fundsp graph is constructed up front (see [04](04-synth-voice.md)/[05](05-drum-synthesis.md)),
so the audio thread never builds a graph. `NoteOn` *reuses* a slot by retriggering it.

```rust
const MAX_VOICES: usize = 64;   // generous; the 16-voice bar is comfortably under this

pub struct VoiceSlot {
    voice: Box<dyn Voice>,   // built once; a SynthVoice or DrumVoice graph
    state: SlotState,
    handle: u32,             // the TS-supplied voice handle (NoteOn `voice` field), or 0
    instrument: u32,         // which instrument id this slot is currently configured for
    started_frame: u64,      // for "oldest" stealing
    last_level: f32,         // running amp-env level, for "quietest" stealing
    choke_group: u8,         // 0 = none; e.g. hats share a group (05)
}

#[derive(PartialEq, Copy, Clone)]
enum SlotState { Free, Active, Releasing }

pub struct VoicePool {
    slots: [VoiceSlot; MAX_VOICES],   // fixed array, no Vec growth
    // handle → slot index, for O(1)-ish NoteOff lookup without allocation:
    handle_map: HandleMap,            // open-addressed fixed table (below)
    frame_counter: u64,
}
```

> Note: voices for **different instruments** share the pool. A slot is (re)configured to an
> instrument id on allocation. If per-instrument graphs differ structurally (synth vs drum vs
> different osc kinds), we keep **per-instrument sub-pools** (a fixed number of pre-built voices
> per defined instrument) rather than reconfiguring a graph at runtime — reconfiguration would
> allocate. Simpler and allocation-free: each defined instrument owns N pre-built slots.

### The handle map (NoteOff finds its voice after stealing)

Per the [event protocol](../../event-protocol.md#voice-ids): *"TS supplies (in `voice`) a
handle it will later use for `NoteOff`. The engine's `voice_alloc` maps that handle to a
physical voice and remembers it, so `NoteOff` can find and release the right one even after
voice stealing."* We implement that map as a **fixed-size open-addressed hash table** (no
allocation, no `HashMap`):

```rust
struct HandleMap {
    keys: [u32; MAX_VOICES * 2],   // handle (0 = empty sentinel)
    vals: [u16; MAX_VOICES * 2],   // slot index
}
impl HandleMap {
    fn insert(&mut self, handle: u32, slot: u16) { /* linear probe from hash(handle) */ }
    fn get(&self, handle: u32) -> Option<u16>    { /* linear probe; None if not present */ }
    fn remove(&mut self, handle: u32)            { /* tombstone / backshift */ }
}
```

When a voice is **stolen**, its old handle is removed from the map before the new handle is
inserted — so a late `NoteOff` for the stolen note simply finds nothing and is a harmless
no-op (the note is already gone), and the new owner's `NoteOff` resolves correctly.

### Allocation algorithm (`NoteOn`)

```rust
impl VoicePool {
    pub fn note_on(&mut self, instrument: u32, handle: u32,
                   freq: f32, vel: f32, dur_frames: u64, choke: u8) {
        // 0. choke group: a new hit silences sounding voices in the same group (e.g. hats)
        if choke != 0 { self.release_choke_group(choke); }

        // 1. find a Free slot for this instrument's sub-pool
        let slot = self.find_free(instrument)
            // 2. else steal: prefer a Releasing slot; else the "best victim" (below)
            .or_else(|| self.find_releasing(instrument))
            .unwrap_or_else(|| self.steal_victim(instrument));

        // 3. evict old handle, install new one
        let s = &mut self.slots[slot];
        if s.handle != 0 { self.handle_map.remove(s.handle); }
        self.handle_map.insert(handle, slot as u16);

        // 4. configure + trigger (no allocation: graph already built)
        s.handle = handle;
        s.instrument = instrument;
        s.started_frame = self.frame_counter;
        s.state = SlotState::Active;
        s.choke_group = choke;
        s.voice.trigger(freq, vel, dur_frames);   // sets vars, resets phase, triggers envs
    }
}
```

### Voice-stealing policy (oldest / quietest first)

When no free or releasing slot exists, choose the **best victim** within the instrument's
sub-pool:

1. **Releasing** voices first (already fading — least audible to cut).
2. Else the **quietest** active voice (lowest `last_level` from its amp-env) — cutting a quiet
   voice is least noticeable.
3. Tie-break on **oldest** (`started_frame`) — the longest-running note is most likely
   already past its peak.

```rust
fn steal_victim(&mut self, instrument: u32) -> usize {
    // among this instrument's slots, pick min by (state!=Releasing, last_level, started_frame)
    self.slots.iter().enumerate()
        .filter(|(_, s)| s.instrument == instrument)
        .min_by(|a, b| key(a.1).partial_cmp(&key(b.1)).unwrap())
        .map(|(i, _)| i).unwrap()
    // key = (releasing?0:1, last_level, started_frame as f32)  → smaller = better victim
}
```

To avoid a click when stealing an *active* (not releasing) voice, the steal **fast-releases**
the victim (a ~2–5 ms forced release ramp) before the new note's attack, rather than hard-
cutting. In practice the new note's own zero-start attack ([03](03-envelopes.md)) plus the
victim's quick fade makes the steal inaudible.

### `NoteOff` and freeing

```rust
pub fn note_off(&mut self, handle: u32) {
    if let Some(slot) = self.handle_map.get(handle) {
        self.slots[slot as usize].voice.release();        // gated env → release stage
        self.slots[slot as usize].state = SlotState::Releasing;
    } // else: stale handle (already stolen/finished) → harmless no-op
}

/// Called each buffer: reclaim finished voices, update levels for stealing decisions.
pub fn reap(&mut self) {
    for (i, s) in self.slots.iter_mut().enumerate() {
        if s.state != SlotState::Free {
            s.last_level = s.voice.current_level();        // cheap read from amp env
            if s.voice.is_finished() {
                if s.handle != 0 { self.handle_map.remove(s.handle); s.handle = 0; }
                s.state = SlotState::Free;
            }
        }
    }
}
```

### Render integration

The render loop ([audio_io.rs](../../architecture.md#the-rust-engine-internals)) iterates only
non-`Free` slots, summing each voice's stereo output into the per-track/master accumulation,
then calls `reap()` once per buffer.

## Interfaces

- **Event protocol** ([event-protocol.md](../../event-protocol.md#voice-ids)):
  - `NoteOn 0x01`: `voice`=handle, `arg0`=freq, `arg1`=vel, `arg2`=dur-frames, `arg3`=instrument
    id → `note_on(...)`.
  - `NoteOff 0x02`: `voice`=handle → `note_off(handle)`.
  - `ParamSet 0x10`: routed to a voice (or all voices of a track) via `set_param`.
- **Voices:** drives the `Voice` trait from [04](04-synth-voice.md) (synth) and
  [05](05-drum-synthesis.md) (drums); honors **choke groups** for hats (05).
- **Scheduler:** `scheduler.rs` decodes records and calls these methods at each event's frame
  offset within the buffer.
- **Master bus:** summed output → [07-master-bus-limiter.md](07-master-bus-limiter.md).

## Edge cases & risks

- **Real-time safety:** the pool, the slot array, and the handle map are all **fixed-size and
  pre-allocated**; no `Vec` growth, no `HashMap`, no `Box::new` on the hot path. This directly
  upholds the [architecture invariant](../../architecture.md#the-core-principle-two-planes)
  (no alloc/locks on the audio thread).
- **Stale `NoteOff`:** resolved to a no-op by the handle map (the stolen note's handle was
  removed). Never releases the wrong voice.
- **Handle collisions:** TS must hand out unique handles per live note. Map uses `0` as the
  empty sentinel, so TS handles start at 1. With `MAX_VOICES*2` buckets and ≤64 live voices,
  load factor stays low; linear probing is fast and bounded.
- **Click on steal:** mitigated by fast-release of the victim + zero-start attack of the new
  note.
- **Hung sustained voice:** if a `NoteOff` is dropped, the voice stays `Active`; it becomes a
  steal candidate (quietest/oldest) so the pool self-heals.
- **CPU / scaling:** stealing scans the instrument's sub-pool (≤ a few dozen slots) — O(n) but
  tiny and only on the rare full-pool path. `reap()` is O(MAX_VOICES) per buffer, negligible.

## Acceptance criteria

- [ ] **16+ simultaneous voices** allocate and render with **zero underruns** / zero late
      events. **Test:** trigger 16–32 overlapping notes under the stress harness; assert
      `stats()` clean.
- [ ] When the pool is full, a new `NoteOn` **steals** the oldest/quietest voice with no audible
      click. **Test:** saturate the pool, fire one more; assert a voice was reused and no step
      transient at the steal point.
- [ ] A `NoteOff` releases the **correct** voice **after** an unrelated steal occurred. **Test:**
      A on → (steal forces B to reuse a slot) → A's NoteOff still releases A, not B.
- [ ] A `NoteOff` for an already-stolen/finished handle is a **harmless no-op** (no wrong voice
      released, no panic). **Test:** send a stale NoteOff; assert no state change.
- [ ] Hat **choke group** works: `hh` cuts a ringing `oh`. **Test:** as in [05](05-drum-synthesis.md).
- [ ] **No allocation** on the audio thread during allocation/stealing/reaping. **Test:** the
      Phase 0 alloc-counter stress check, extended to voice churn.
