// Named track slots. Phase 0 has one slot (p1). Assigning a slot is a plain variable
// write — cheap and atomic from JS's perspective (architecture.md "Tracks and
// hot-swapping"); the scheduler reads the current pattern on its next query.
//
// Feature 05 introduces the slot store + getTrack the scheduler reads; feature 06
// adds the public `p1` live-coding entry point on top of setTrack.

import type { Pattern } from "./pattern";

// index 1 = p1; index 0 reserved for master/global.
const slots: (Pattern<unknown> | null)[] = [null, null];

/** Assign the pattern for a track id (the primitive `p1` is built on this in 06). */
export function setTrack(id: number, pattern: Pattern<unknown> | null): void {
  slots[id] = pattern;
}

/** Scheduler reads the current pattern for a track id. */
export function getTrack(id: number): Pattern<unknown> | null {
  return slots[id] ?? null;
}
