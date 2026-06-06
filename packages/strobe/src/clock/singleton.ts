// A library singleton that lets a song's top-level `setcps(...)` reach the live Clock
// the runtime owns. The runtime registers its clock via setActiveClock; the song —
// re-imported on each hot-reload — calls the free functions, which mutate that same
// clock instance (so tempo edits are continuous across reloads).

import type { Clock } from "./clock";

let active: Clock | null = null;

/** The runtime registers its live clock here at boot. */
export function setActiveClock(clock: Clock): void {
  active = clock;
}

/** Set tempo directly (cycles per second). No-op before a clock is active. */
export function setcps(x: number): void {
  active?.setcps(x);
}

/** Set tempo in BPM (4 beats/cycle). No-op before a clock is active. */
export function setbpm(bpm: number): void {
  active?.setbpm(bpm);
}

/** Current cycle position from the live clock (0 before one is active). Used by the
 *  track registry to stage a live re-assignment at the next cycle boundary. */
export function currentCycle(): number {
  return active ? active.nowCycle() : 0;
}
