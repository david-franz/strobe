// Named track slots p1..pN. Phase 0 had one slot; Phase 1 supports several so a song can
// layer a kick, hats, snare and a bassline at once. Assigning a slot is a plain variable
// write; the scheduler queries every active slot each tick (architecture.md "Tracks").

import type { Pattern } from "./pattern";

const MAX_TRACKS = 8;
// index 0 reserved for master/global; 1..MAX_TRACKS = p1..pN.
const slots: (Pattern<unknown> | null)[] = Array(MAX_TRACKS + 1).fill(null);

export function setTrack(id: number, pattern: Pattern<unknown> | null): void {
  if (id >= 1 && id <= MAX_TRACKS) slots[id] = pattern;
}

export function getTrack(id: number): Pattern<unknown> | null {
  return slots[id] ?? null;
}

/** Every assigned track, in id order — what the scheduler iterates. */
export function activeTracks(): { id: number; pattern: Pattern<unknown> }[] {
  const out: { id: number; pattern: Pattern<unknown> }[] = [];
  for (let id = 1; id <= MAX_TRACKS; id++) {
    const pattern = slots[id];
    if (pattern) out.push({ id, pattern });
  }
  return out;
}

const assign = (id: number) => (pattern: Pattern<unknown>): void => setTrack(id, pattern);

/** The live-coding entry points: re-running `pN(...)` swaps that track's pattern. */
export const p1 = assign(1);
export const p2 = assign(2);
export const p3 = assign(3);
export const p4 = assign(4);
export const p5 = assign(5);
export const p6 = assign(6);
export const p7 = assign(7);
export const p8 = assign(8);
