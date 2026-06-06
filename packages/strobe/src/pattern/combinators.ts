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
  cyclePos,
  sameTime,
  hasOnset,
  type Hap,
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

// ── Time (feature 03) ─────────────────────────────────────────────────────────

/** Reverse within each cycle (so it loops). */
export const rev = <T>(p: Pat<T>): Pat<T> =>
  splitQueries(
    pattern((span) => {
      const cyc = Math.floor(span.begin);
      const reflect = (t: number) => cyc + (cyc + 1 - t); // mirror within [cyc, cyc+1]
      const q = { begin: reflect(span.end), end: reflect(span.begin) };
      return p.query(q).map((h) => ({
        whole: h.whole ? { begin: reflect(h.whole.end), end: reflect(h.whole.begin) } : undefined,
        part: { begin: reflect(h.part.end), end: reflect(h.part.begin) },
        value: h.value,
      }));
    }),
  );

export const early = <T>(t: NumArg, p: Pat<T>): Pat<T> =>
  withArg(t, (k) => withTime((x) => x + k, (x) => x - k, p));
export const late = <T>(t: NumArg, p: Pat<T>): Pat<T> =>
  withArg(t, (k) => withTime((x) => x - k, (x) => x + k, p));

/** Overlay a copy shifted by `t`, transformed by `fn` (echoes/stabs). */
export const off = <T>(t: NumArg, fn: (p: Pat<T>) => Pat<T>, p: Pat<T>): Pat<T> =>
  stack(p, fn(late(t, p)));

// ── Structure & repetition ────────────────────────────────────────────────────

export const every = <T>(n: number, fn: (p: Pat<T>) => Pat<T>, p: Pat<T>): Pat<T> =>
  splitQueries(
    pattern((span) => {
      const cyc = Math.floor(span.begin);
      return (n > 0 && cyc % n === 0 ? fn(p) : p).query(span);
    }),
  );

export const whenmod = <T>(a: number, b: number, fn: (p: Pat<T>) => Pat<T>, p: Pat<T>): Pat<T> =>
  splitQueries(
    pattern((span) => {
      const cyc = Math.floor(span.begin);
      return (((cyc % a) + a) % a >= b ? fn(p) : p).query(span);
    }),
  );

/** Rotate the pattern start by 1/n each cycle. */
export const iter = <T>(n: number, p: Pat<T>): Pat<T> =>
  splitQueries(
    pattern((span) => {
      const cyc = Math.floor(span.begin);
      return early((((cyc % n) + n) % n) / n, p).query(span);
    }),
  );

/** Apply `fn` to a different 1/n slice of the cycle each cycle (rotating fills). */
export const chunk = <T>(n: number, fn: (p: Pat<T>) => Pat<T>, p: Pat<T>): Pat<T> =>
  splitQueries(
    pattern((span) => {
      const cyc = Math.floor(span.begin);
      const k = ((cyc % n) + n) % n;
      const lo = k / n;
      const hi = (k + 1) / n;
      const inSlice = (h: Hap<T>) => {
        const pos = cyclePos(h.part.begin);
        return pos >= lo - 1e-9 && pos < hi - 1e-9;
      };
      const base = p.query(span);
      const transformed = fn(p).query(span);
      return base.map((h) =>
        inSlice(h) ? transformed.find((t) => sameTime(t.part.begin, h.part.begin)) ?? h : h,
      );
    }),
  );

// ── Layering (feature 03) ─────────────────────────────────────────────────────

export const superimpose = <T>(fn: (p: Pat<T>) => Pat<T>, p: Pat<T>): Pat<T> => stack(p, fn(p));

// ── Chance (feature 03) ───────────────────────────────────────────────────────

/** Complement of `degradeBy`: keep exactly the onsets `degradeBy` would drop (same salt),
 *  so the two partitions are disjoint and cover every onset. */
export const undegradeBy = <T>(prob: NumArg, p: Pat<T>): Pat<T> =>
  withArg(prob, (pr) =>
    pattern((span) =>
      spanCycles(span).flatMap((sub) => {
        const cyc = Math.floor(sub.begin);
        return p
          .query(sub)
          .filter((h, idx) => (!hasOnset(h) ? false : randAt(cyc, idx, SALT_DEGRADE) < pr));
      }),
    ),
  );

export const sometimesBy = <T>(prob: NumArg, fn: (p: Pat<T>) => Pat<T>, p: Pat<T>): Pat<T> =>
  stack(degradeBy(prob, p), fn(undegradeBy(prob, p)));
export const sometimes = <T>(fn: (p: Pat<T>) => Pat<T>, p: Pat<T>): Pat<T> => sometimesBy(0.5, fn, p);

export const somecycles = <T>(fn: (p: Pat<T>) => Pat<T>, p: Pat<T>, prob = 0.5): Pat<T> =>
  splitQueries(
    pattern((span) => {
      const cyc = Math.floor(span.begin);
      return (randAt(cyc, 0, SALT_DEGRADE) < prob ? fn(p) : p).query(span);
    }),
  );

// ── Chainable methods (mixed onto Pat.prototype) ──────────────────────────────

type Fn<T> = (p: Pat<T>) => Pat<T>;

declare module "./pattern" {
  interface Pat<T> {
    fast(n: NumArg): Pat<T>;
    slow(n: NumArg): Pat<T>;
    rev(): Pat<T>;
    early(t: NumArg): Pat<T>;
    late(t: NumArg): Pat<T>;
    off(t: NumArg, fn: Fn<T>): Pat<T>;
    every(n: number, fn: Fn<T>): Pat<T>;
    whenmod(a: number, b: number, fn: Fn<T>): Pat<T>;
    iter(n: number): Pat<T>;
    chunk(n: number, fn: Fn<T>): Pat<T>;
    superimpose(fn: Fn<T>): Pat<T>;
    degrade(): Pat<T>;
    degradeBy(prob: NumArg): Pat<T>;
    sometimes(fn: Fn<T>): Pat<T>;
    sometimesBy(prob: NumArg, fn: Fn<T>): Pat<T>;
    somecycles(fn: Fn<T>): Pat<T>;
  }
}

let installed = false;
/** Install the chainable methods. Idempotent; called once at module load. */
export function installCombinatorMethods(): void {
  if (installed) return;
  installed = true;
  const proto = Pat.prototype;
  proto.fast = function <T>(this: Pat<T>, n: NumArg) {
    return fast(n, this);
  };
  proto.slow = function <T>(this: Pat<T>, n: NumArg) {
    return slow(n, this);
  };
  proto.rev = function <T>(this: Pat<T>) {
    return rev(this);
  };
  proto.early = function <T>(this: Pat<T>, t: NumArg) {
    return early(t, this);
  };
  proto.late = function <T>(this: Pat<T>, t: NumArg) {
    return late(t, this);
  };
  proto.off = function <T>(this: Pat<T>, t: NumArg, fn: Fn<T>) {
    return off(t, fn, this);
  };
  proto.every = function <T>(this: Pat<T>, n: number, fn: Fn<T>) {
    return every(n, fn, this);
  };
  proto.whenmod = function <T>(this: Pat<T>, a: number, b: number, fn: Fn<T>) {
    return whenmod(a, b, fn, this);
  };
  proto.iter = function <T>(this: Pat<T>, n: number) {
    return iter(n, this);
  };
  proto.chunk = function <T>(this: Pat<T>, n: number, fn: Fn<T>) {
    return chunk(n, fn, this);
  };
  proto.superimpose = function <T>(this: Pat<T>, fn: Fn<T>) {
    return superimpose(fn, this);
  };
  proto.degrade = function <T>(this: Pat<T>) {
    return degrade(this);
  };
  proto.degradeBy = function <T>(this: Pat<T>, prob: NumArg) {
    return degradeBy(prob, this);
  };
  proto.sometimes = function <T>(this: Pat<T>, fn: Fn<T>) {
    return sometimes(fn, this);
  };
  proto.sometimesBy = function <T>(this: Pat<T>, prob: NumArg, fn: Fn<T>) {
    return sometimesBy(prob, fn, this);
  };
  proto.somecycles = function <T>(this: Pat<T>, fn: Fn<T>) {
    return somecycles(fn, this);
  };
}

installCombinatorMethods();
