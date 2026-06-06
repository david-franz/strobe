// The combinator vocabulary (pattern-model.md "combinators"). Every combinator is a pure
// Pattern → Pattern function built from the core helpers (withTime/splitQueries/filter) —
// they never re-derive span math.
//
// Feature 02 lands the subset the mini-notation parser compiles to (fast/slow/fastcat/
// slowcat/stack/euclid/degrade) + patternified-argument support; feature 03 adds the rest
// (rev/every/iter/chunk/off/jux/…) and the chainable Pat methods.

import {
  Pat,
  pattern,
  silence,
  withTime,
  splitQueries,
  queryArc,
  spanCycles,
  hasOnset,
} from "./pattern";
import { randAt, SALT_DEGRADE } from "./rng";

/** A combinator argument that may itself be a pattern (`a*<2 3>`). */
export type NumArg = number | Pat<number>;

const sampleArgPerCycle = (a: NumArg, cycle: number): number =>
  typeof a === "number" ? a : (queryArc(a, cycle, cycle)[0]?.value ?? 0);

/** Build a pattern from a numeric arg; if the arg is itself a pattern, read its value once
 *  per cycle (so `*<2 3>` alternates per cycle). */
export const withArg = <T>(a: NumArg, build: (k: number) => Pat<T>): Pat<T> => {
  if (typeof a === "number") return build(a);
  return splitQueries(
    pattern((span) => build(sampleArgPerCycle(a, Math.floor(span.begin))).query(span)),
  );
};

// ── Time ──────────────────────────────────────────────────────────────────────

export const fast = <T>(n: NumArg, p: Pat<T>): Pat<T> =>
  withArg(n, (k) => (k === 0 ? silence : withTime((t) => t * k, (t) => t / k, p)));

export const slow = <T>(n: NumArg, p: Pat<T>): Pat<T> =>
  withArg(n, (k) => (k === 0 ? silence : fast(1 / k, p)));

// ── Layering ────────────────────────────────────────────────────────────────

export const stack = <T>(...ps: Pat<T>[]): Pat<T> =>
  pattern((span) => ps.flatMap((p) => p.query(span)));

/** One pattern per cycle, in turn. `<a b>` ⇒ slowcat(pure a, pure b). */
export const slowcat = <T>(...ps: Pat<T>[]): Pat<T> => {
  if (ps.length === 0) return silence;
  return splitQueries(
    pattern((span) => {
      const cyc = Math.floor(span.begin);
      const i = ((cyc % ps.length) + ps.length) % ps.length;
      // Map the outer cycle onto the chosen inner pattern's own cycle.
      const shift = cyc - Math.floor(cyc / ps.length);
      return withTime((t) => t - shift, (t) => t + shift, ps[i]).query(span);
    }),
  );
};
export const cat = slowcat;

/** Squeeze all patterns into one cycle (the sequence builder). */
export const fastcat = <T>(...ps: Pat<T>[]): Pat<T> => fast(ps.length, slowcat(...ps));

// ── Euclid ────────────────────────────────────────────────────────────────────

/** Bjorklund's algorithm: spread `pulses` evenly over `steps`. E(3,8) = [x..x..x.]. */
export function bjorklund(pulses: number, steps: number): boolean[] {
  if (steps <= 0) return [];
  const p = Math.max(0, Math.min(pulses, steps));
  let a: boolean[][] = Array.from({ length: p }, () => [true]);
  let b: boolean[][] = Array.from({ length: steps - p }, () => [false]);
  while (b.length > 1) {
    const n = Math.min(a.length, b.length);
    const merged: boolean[][] = [];
    for (let i = 0; i < n; i++) merged.push([...a[i], ...b[i]]);
    const remA = a.slice(n);
    const remB = b.slice(n);
    a = merged;
    b = remA.length ? remA : remB;
  }
  return [...a, ...b].flat();
}

const rotate = <T>(arr: T[], n: number): T[] => {
  const len = arr.length;
  if (len === 0) return arr;
  return arr.map((_, i) => arr[(((i + n) % len) + len) % len]);
};

export const euclid = <T>(pulses: number, steps: number, rotation: number, p: Pat<T>): Pat<T> =>
  fastcat(...rotate(bjorklund(pulses, steps), rotation).map((on) => (on ? p : silence)));

// ── Chance (deterministic) ──────────────────────────────────────────────────

/** Drop each onset with probability `prob`, deterministically by cycle + index. */
export const degradeBy = <T>(prob: NumArg, p: Pat<T>): Pat<T> =>
  withArg(prob, (pr) =>
    pattern((span) =>
      spanCycles(span).flatMap((sub) => {
        const cyc = Math.floor(sub.begin);
        return p
          .query(sub)
          .filter((h, idx) => (!hasOnset(h) ? true : randAt(cyc, idx, SALT_DEGRADE) >= pr));
      }),
    ),
  );
export const degrade = <T>(p: Pat<T>): Pat<T> => degradeBy(0.5, p);
