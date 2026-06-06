// Generic pattern builders (a thin Phase 1 slice of the full Phase 2 mini-notation):
// place values evenly across the cycle, with `null` meaning a rest. Each value is a
// `NoteValue` the scheduler turns into a NoteOn.

import type { Pattern, Hap } from "./pattern";

/** Place `slots` evenly across the cycle (one slot each), repeating every cycle; `null`
 *  slots are rests. Queryable over any span, emitting each onset exactly once. */
function placeEvenly<T>(slots: (T | null)[]): Pattern<T> {
  const n = Math.max(1, slots.length);
  const step = 1 / n;
  return ({ begin, end }) => {
    const haps: Hap<T>[] = [];
    let k = Math.ceil(begin / step - 1e-9);
    for (;;) {
      const onset = k * step + 0; // normalize -0 → +0
      if (onset >= end) break;
      const slot = ((k % n) + n) % n;
      const v = slots[slot];
      if (v != null) {
        const wholeEnd = onset + step;
        haps.push({
          whole: { begin: onset, end: wholeEnd },
          part: { begin: Math.max(onset, begin), end: Math.min(wholeEnd, end) },
          value: v,
        });
      }
      k++;
    }
    return haps;
  };
}

/** A sequence: one value per evenly-spaced step (`null` = rest). */
export function seq<T>(values: (T | null)[]): Pattern<T> {
  return placeEvenly(values);
}

/** `n` evenly-spaced hits of `value` per cycle. */
export function every<T>(value: T, n: number): Pattern<T> {
  return placeEvenly(Array(n).fill(value));
}

/** A Euclidean rhythm: `k` hits of `value` spread over `n` slots (Bresenham). */
export function euclid<T>(value: T, k: number, n: number): Pattern<T> {
  const slots: (T | null)[] = Array.from({ length: n }, (_, i) => ((i * k) % n < k ? value : null));
  return placeEvenly(slots);
}
