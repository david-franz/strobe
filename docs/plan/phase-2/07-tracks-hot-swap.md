# Phase 2.07 — Tracks & hot-swapping (`tracks.ts`)

## Purpose

A **track** (`p1`, `p2`, … `pN`) is a mutable slot that holds one pattern; re-running `p1(...)` swaps the slot's pattern at the **next cycle boundary** ([architecture.md](../../architecture.md#tracks-and-hot-swapping)). This is the live-coding workflow: each track is a loop; reassigning it layers and reshapes the sound without ever stopping the engine.

Phase 0 had a single track slot ([phase-0/06-clock-and-tracks.md](../phase-0/06-clock-and-tracks.md)). Phase 2 generalizes it to the `p1..pN` **registry** the scheduler queries every tick ([06-scheduler-integration.md](06-scheduler-integration.md)), implements the swap-at-cycle-boundary timing, and adds the live helpers `hush()`, `solo`, and `mute`. The whole thing is seamless **because patterns are pure** — swapping a slot is just pointing it at a new function; nothing in flight is mutated ([pattern-model.md](../../pattern-model.md#why-pure-functions-not-a-sequencer-object)).

## Location

```
packages/tekno/src/
  tracks.ts        # the registry, p1..pN, swap timing, hush/solo/mute
  tracks.test.ts
```

`tracks.ts` is imported by the song surface (re-exported from the package index alongside `sound`/`note`/`setcps`) and read by `scheduler.ts` ([06](06-scheduler-integration.md)) via `registry.activeTracks()`. It depends only on `Pat` ([01](01-pattern-core.md)) and the cps clock (Phase 0).

## Design

### The track registry

Each slot holds the **current** pattern plus an optional **pending** pattern staged to swap in at the next cycle boundary, plus mute/solo flags.

```ts
interface TrackSlot {
  id: number;                 // protocol track field: p1 = 1, p2 = 2, … (0 = master/global)
  current: Pat<ControlMap>;   // what the scheduler queries now
  pending?: Pat<ControlMap>;  // staged; promoted to `current` at the next integer cycle
  pendingFromCycle?: number;  // the cycle at/after which `pending` takes effect
  muted: boolean;
}

class TrackRegistry {
  private slots = new Map<number, TrackSlot>();

  // p1..pN are bound to assign(1), assign(2), …
  assign(id: number, pat: Pat<ControlMap>): void {
    const slot = this.slots.get(id) ?? this.makeSlot(id);
    if (slot.current === undefined) {
      slot.current = pat;                        // first assignment: take effect immediately
    } else {
      slot.pending = pat;                        // live re-assignment: stage for the boundary
      slot.pendingFromCycle = Math.ceil(clock.nowCycle() + 1e-9);  // next integer cycle
    }
  }

  // Called by the scheduler each tick, *before* querying, with the window's begin cycle.
  promotePending(atCycle: number): void {
    for (const slot of this.slots.values()) {
      if (slot.pending && atCycle >= (slot.pendingFromCycle ?? Infinity)) {
        slot.current = slot.pending;
        slot.pending = undefined;
        slot.pendingFromCycle = undefined;
      }
    }
  }

  activeTracks(): TrackSlot[] {
    const soloed = [...this.slots.values()].filter(s => s.soloed);
    const pool = soloed.length ? soloed : [...this.slots.values()];
    return pool.filter(s => !s.muted && s.current);
  }
}

export const registry = new TrackRegistry();
export const p1 = (pat: Pat<ControlMap>) => registry.assign(1, pat);
export const p2 = (pat: Pat<ControlMap>) => registry.assign(2, pat);
// … p3..pN generated up to a fixed N (e.g. 16); ids map 1:1 to the protocol `track` field.
```

### Swap timing — why "next cycle boundary"

The scheduler works ~100 ms ahead ([architecture.md](../../architecture.md#the-look-ahead-scheduler-two-clocks)). When you re-run a `p1(...)` line:

1. **The pending reference is staged immediately** — a cheap, atomic variable assignment from JS's perspective. The *current* pattern keeps playing.
2. **The scheduler does not retro-actively rewrite already-enqueued events** — the next ~100 ms is already committed to the ring buffer ([architecture.md](../../architecture.md#tracks-and-hot-swapping)). Staging (not overwriting `current`) is exactly what preserves that committed window.
3. **From the next cycle boundary, the scheduler queries the new pattern.** `promotePending(atCycle)` runs at the top of each tick; once the window's begin cycle reaches `pendingFromCycle` (the next integer cycle), `current` becomes `pending`. Swapping at the boundary keeps the music in time and click-free.

```
tick loop (each ~25ms):
  const winBegin = clock.windowBeginCycle();
  registry.promotePending(winBegin);     // ← promote any staged swaps due by this cycle
  for (track of registry.activeTracks()) queryArc(track.current, winBegin, winEnd) …
```

Because a window may straddle a boundary (e.g. `2.9 → 3.1`), `spanCycles` ([01](01-pattern-core.md)) splits it; the pre-boundary slice is queried from `current` and the post-boundary slice from the just-promoted pattern only if promotion happens at the boundary cycle. In practice we promote when the window's begin cycle ≥ `pendingFromCycle`, so the swap lands cleanly on the integer cycle and the already-committed pre-boundary events are never touched.

### Why purity makes this seamless

A traditional sequencer stores the grid as state; editing it live mutates shared state that races with playback ([pattern-model.md](../../pattern-model.md#why-pure-functions-not-a-sequencer-object)). Tracks store **no playback state** — a slot is just a reference to a pure function. Swapping is pointing the reference at a new function; the in-flight committed window was produced by querying the *old* function and is already bytes in the ring buffer, wholly independent of the slot now. There is nothing to tear down, no notes to reconcile, no race. This is the property that makes `tekno` safe to edit while it plays.

### Live helpers

```ts
// Silence everything immediately (panic) — clears all slots; next tick queries nothing.
export function hush(): void {
  for (const slot of registry.allSlots()) { slot.current = silence; slot.pending = undefined; }
}

// Mute / unmute a track from the next cycle boundary (consistent with swap timing).
export const mute = (id: number) => registry.setMuted(id, true);
export const unmute = (id: number) => registry.setMuted(id, false);

// Solo: only soloed tracks play (others are implicitly silenced) until un-soloed.
export const solo = (id: number) => registry.setSoloed(id, true);
export const unsolo = (id: number) => registry.setSoloed(id, false);
```

`mute`/`solo` flip flags read by `activeTracks()`; like swaps, their effect is observed at the next query (next cycle boundary for musical consistency). `hush()` is the one *immediate* action (a live "panic"), accepting that it acts on the next tick's window rather than waiting for a boundary — silence has no timing to keep.

> **Note on `mute` and the committed window.** Muting hides a track from `activeTracks()`, so it stops being *queried*. Events already in the ring (the committed ~100 ms) still play out — consistent with the "already-enqueued lookahead is committed" rule. For a hard immediate cut you would also need an engine-level voice kill; that is out of Phase 2 scope (Phase 3 effects) and noted as a follow-up.

### Worked timing example

```
cps such that 1 cycle ≈ 1s; lookahead 100ms. Currently at cycle 2.4.
p1(sound("bd*4"))     // first assign → current immediately
… plays bd*4 …
at cycle 2.7 you edit to p1(sound("bd*8"))
  → assign() stages pending = bd*8, pendingFromCycle = ceil(2.7) = 3
  → scheduler keeps querying bd*4 for the rest of cycle 2 (windows up to ~2.8→2.9 etc.)
  → first tick whose window begins at/after cycle 3 → promotePending(3) swaps current = bd*8
  → from cycle 3 onward, bd*8 plays; no event from cycle 2 was rewritten → no click
```

## Interfaces

- **[06-scheduler-integration.md](06-scheduler-integration.md):** calls `registry.promotePending(winBegin)` then iterates `registry.activeTracks()`, querying `slot.current` and using `slot.id` as the protocol `track` field. Mute/solo simply change which slots `activeTracks()` returns.
- **[01-pattern-core.md](01-pattern-core.md):** slots hold `Pat<ControlMap>`; `hush` uses `silence`; window straddling uses `spanCycles`.
- **[04-sound-note-api.md](04-sound-note-api.md):** the patterns assigned to slots are exactly what `sound`/`note`/`freq` chains produce (`Pat<ControlMap>`).
- **cps clock (Phase 0):** `clock.nowCycle()` / `windowBeginCycle()` drive `pendingFromCycle` and `promotePending`. Cross-reference [architecture.md](../../architecture.md#tracks-and-hot-swapping).
- **Live runtime (Phase 5):** the file-watcher re-evaluates a song file and re-runs `pN(...)` lines, which is exactly the `assign` → stage → boundary-swap path here; Phase 2 makes that mechanism correct, Phase 5 wires the watcher.

## Edge cases & risks

- **Purity / atomicity:** `assign` is a single reference write; even if a scheduler tick interleaves, it reads either the old or the new reference, never a torn state. The pending/promote split guarantees the committed window is produced entirely from one pattern.
- **Boundary alignment & fractional precision:** `pendingFromCycle = ceil(nowCycle)` with an epsilon nudge avoids re-assigning *exactly* on a boundary swapping too early/late; promotion compares `winBegin >= pendingFromCycle` with `sameTime` tolerance ([01](01-pattern-core.md)). If a window already straddles the boundary at assign time, the pre-boundary slice still uses `current` (already partly committed), the post-boundary the new one at the next tick.
- **Rapid re-assignment:** editing the same `pN` twice within one cycle just overwrites `pending` (last edit wins), still promoting once at the boundary. No queue build-up.
- **Continuous patterns as a track:** assigning a bare continuous signal to a track produces no onsets → silence; the scheduler warns ([06](06-scheduler-integration.md)). Tracks expect `Pat<ControlMap>`; the type system steers users to `sound`/`note`.
- **Solo/mute interaction:** if any track is soloed, non-soloed tracks are silent regardless of their mute flag; un-soloing restores the mute flags. Tested.
- **`hush` during committed window:** `hush()` empties slots immediately but the ~100 ms already in the ring still plays — documented, consistent with the architecture's commit rule. A future engine "all-notes-off" would make it instant.
- **Performance:** the registry is a small map (≤ N slots); `promotePending`/`activeTracks` are O(N) per tick, negligible.

## Acceptance criteria

- `p1(sound("bd*4"))` then querying via the scheduler at cycle 0 plays `bd*4`; re-running `p1(sound("bd*8"))` mid-cycle leaves cycle's remaining events as `bd*4` and switches to `bd*8` **only from the next integer cycle** (assert via `promotePending` + `queryArc` on `slot.current`).
- A staged swap does **not** alter `slot.current` until `promotePending(atCycle)` with `atCycle >= pendingFromCycle`.
- `registry.activeTracks()` excludes muted tracks and, when any track is soloed, includes only soloed tracks.
- `hush()` sets every slot's `current` to `silence`, so the next tick's `activeTracks()` produces no onsets.
- The first assignment to an empty slot takes effect immediately (no staging), so a freshly started song doesn't wait a cycle to begin.
- Track ids map 1:1 to the protocol `track` field (p1→1, p2→2, …), verified against the records [06](06-scheduler-integration.md) emits.
- A purity/atomicity test: interleaving an `assign` between two `queryArc`s of the same window yields a window built from exactly one pattern (old or new, never mixed).
