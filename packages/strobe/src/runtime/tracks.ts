// Named track slots p1..pN. A track is a mutable slot holding one pattern; re-running
// pN(...) stages a new pattern that swaps in at the next cycle boundary (architecture.md
// "Tracks and hot-swapping"). Seamless because patterns are pure — swapping a slot just
// points it at a new function; the committed lookahead window is already bytes in the ring.

import type { Pat, Pattern } from "../pattern/pattern";
import { silence } from "../pattern/pattern";
import { currentCycle } from "../clock/singleton";

/** A track may hold a Pat (the sound/note API) or a bare query function (Phase-0/1). */
export type AnyPat = Pat<unknown> | Pattern<unknown>;

interface TrackSlot {
  id: number;
  current: AnyPat | null; // what the scheduler queries now
  pending: AnyPat | null; // staged; promoted at the next integer cycle
  pendingFromCycle: number;
  muted: boolean;
  soloed: boolean;
}

const MAX_TRACKS = 8;
const slots = new Map<number, TrackSlot>();

function slot(id: number): TrackSlot {
  let s = slots.get(id);
  if (!s) {
    s = { id, current: null, pending: null, pendingFromCycle: Infinity, muted: false, soloed: false };
    slots.set(id, s);
  }
  return s;
}

/** Assign a track's pattern. The first assignment to an empty slot takes effect
 *  immediately (a freshly started song plays at once); a live re-assignment is staged for
 *  the next cycle boundary, so the committed window is never rewritten. */
export function assign(id: number, pat: AnyPat): void {
  if (id < 1 || id > MAX_TRACKS) return;
  const s = slot(id);
  if (s.current === null) {
    s.current = pat;
  } else {
    s.pending = pat;
    s.pendingFromCycle = Math.ceil(currentCycle() + 1e-9);
  }
}

/** Immediate low-level set (bypasses staging) — used by tests and `null` clears. */
export function setTrack(id: number, pat: AnyPat | null): void {
  if (id < 1 || id > MAX_TRACKS) return;
  const s = slot(id);
  s.current = pat;
  s.pending = null;
  s.pendingFromCycle = Infinity;
}

export function getTrack(id: number): AnyPat | null {
  return slots.get(id)?.current ?? null;
}

/** Promote any staged swaps due by `atCycle`. Called by the scheduler at the top of each
 *  tick, before querying. */
export function promotePending(atCycle: number): void {
  for (const s of slots.values()) {
    if (s.pending && atCycle >= s.pendingFromCycle) {
      s.current = s.pending;
      s.pending = null;
      s.pendingFromCycle = Infinity;
    }
  }
}

/** Every track the scheduler should query: non-muted with a current pattern, restricted to
 *  soloed tracks if any are soloed. */
export function activeTracks(): { id: number; pattern: AnyPat }[] {
  const all = [...slots.values()];
  const soloed = all.filter((s) => s.soloed);
  const pool = soloed.length ? soloed : all;
  return pool.filter((s) => !s.muted && s.current).map((s) => ({ id: s.id, pattern: s.current! }));
}

export function setMuted(id: number, muted: boolean): void {
  slot(id).muted = muted;
}
export function setSoloed(id: number, soloed: boolean): void {
  slot(id).soloed = soloed;
}

/** Silence everything immediately (panic). The committed ~100 ms already in the ring still
 *  plays out (consistent with the architecture's commit rule). */
export function hush(): void {
  for (const s of slots.values()) {
    s.current = silence;
    s.pending = null;
    s.pendingFromCycle = Infinity;
  }
}

/** Clear the whole registry (used by tests). */
export function clearAllTracks(): void {
  slots.clear();
}

const assignTo = (id: number) => (pat: AnyPat): void => assign(id, pat);

/** The live-coding entry points: re-running pN(...) stages that track's pattern. */
export const p1 = assignTo(1);
export const p2 = assignTo(2);
export const p3 = assignTo(3);
export const p4 = assignTo(4);
export const p5 = assignTo(5);
export const p6 = assignTo(6);
export const p7 = assignTo(7);
export const p8 = assignTo(8);

export const mute = (id: number): void => setMuted(id, true);
export const unmute = (id: number): void => setMuted(id, false);
export const solo = (id: number): void => setSoloed(id, true);
export const unsolo = (id: number): void => setSoloed(id, false);
