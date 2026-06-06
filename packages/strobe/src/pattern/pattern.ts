// The pattern core (pattern-model.md "The one idea"): a pattern is a pure function from
// a span of cycle-time to the events (haps) that happen in it. This file is the leaf every
// other pattern file builds on — the `Pat<T>` wrapper, the three primitives, `queryArc`,
// the fundamental constructors (pure/silence/signal), cycle splitting, and the
// time-manipulation helpers (withTime/splitQueries) the combinators reuse.

/** A half-open interval of cycle-time, [begin, end). */
export interface TimeSpan {
  begin: number;
  end: number;
}

/** One thing that happens: a value, when it happens, and the visible part. */
export interface Hap<T> {
  /** Full extent of the event; `undefined` for continuous/analog signals. */
  whole: TimeSpan | undefined;
  /** Portion of the event intersecting the queried span. */
  part: TimeSpan;
  value: T;
}

/** The one idea: a pattern is a pure function from a span to the haps in it. */
export type Pattern<T> = (span: TimeSpan) => Hap<T>[];

/** The chainable wrapper. The raw `query` function is the source of truth; combinator and
 *  modifier methods are mixed onto the prototype by combinators.ts / sound.ts / signals.ts
 *  (each `declare module`-augments this interface). */
export class Pat<T> {
  constructor(public readonly query: Pattern<T>) {}

  /** Map over each hap's value, preserving all time structure (whole/part). */
  fmap<U>(f: (v: T) => U): Pat<U> {
    return new Pat<U>((span) =>
      this.query(span).map((h) => ({ whole: h.whole, part: h.part, value: f(h.value) })),
    );
  }
}

/** Construct a `Pat` from a raw query function. */
export function pattern<T>(query: Pattern<T>): Pat<T> {
  return new Pat(query);
}

/** The universal entry point used by tests and the scheduler. Accepts a `Pat` or a bare
 *  query function (so Phase-0/1 bare patterns keep working). */
export function queryArc<T>(pat: Pat<T> | Pattern<T>, begin: number, end: number): Hap<T>[] {
  const query = typeof pat === "function" ? pat : pat.query;
  return query({ begin, end });
}

// ── TimeSpan helpers ──────────────────────────────────────────────────────────

export const sameTime = (a: number, b: number): boolean => Math.abs(a - b) < 1e-9;

export function spanIntersect(a: TimeSpan, b: TimeSpan): TimeSpan | undefined {
  const begin = Math.max(a.begin, b.begin);
  const end = Math.min(a.end, b.end);
  if (begin > end) return undefined; // disjoint
  if (begin === end && a.begin !== a.end && b.begin !== b.end) return undefined; // zero-width touch
  return { begin, end };
}

/** Split a span at every integer cycle boundary it crosses. A zero-width span returns
 *  itself (so point queries still reach `pure`). */
export function spanCycles(span: TimeSpan): TimeSpan[] {
  const out: TimeSpan[] = [];
  let { begin } = span;
  const { end } = span;
  if (begin >= end) {
    return begin === end ? [span] : [];
  }
  while (begin < end) {
    const nextBoundary = Math.floor(begin) + 1;
    const segEnd = Math.min(nextBoundary, end);
    out.push({ begin, end: segEnd });
    begin = segEnd;
  }
  return out;
}

/** Fractional part within a cycle (0..1). */
export const cyclePos = (t: number): number => t - Math.floor(t);

// ── Fundamental constructors ──────────────────────────────────────────────────

/** A discrete pattern that plays `value` once per cycle, its `whole` spanning the cycle. */
export function pure<T>(value: T): Pat<T> {
  return pattern((span) =>
    spanCycles(span).map((sub) => {
      const cycle = Math.floor(sub.begin);
      const whole: TimeSpan = { begin: cycle, end: cycle + 1 };
      return { whole, part: sub, value };
    }),
  );
}
export const atom = pure;

/** The empty pattern. */
export const silence: Pat<never> = pattern(() => []);

/** A continuous pattern: sampled at a point in time, with no `whole`. A zero-width query
 *  reads exactly `sample(t)`; a positive-width query samples once at the midpoint. */
export function signal<T>(sample: (t: number) => T): Pat<T> {
  return pattern((span) => {
    if (span.begin === span.end) {
      return [{ whole: undefined, part: span, value: sample(span.begin) }];
    }
    const mid = (span.begin + span.end) / 2;
    return [{ whole: undefined, part: span, value: sample(mid) }];
  });
}

// ── Time-manipulation helpers (the basis of every time combinator) ────────────

type TimeFn = (t: number) => number;

const withQueryTime = <T>(f: TimeFn, p: Pat<T>): Pattern<T> => (span) =>
  p.query({ begin: f(span.begin), end: f(span.end) });

const withHapTime = <T>(f: TimeFn, haps: Hap<T>[]): Hap<T>[] =>
  haps.map((h) => ({
    whole: h.whole ? { begin: f(h.whole.begin), end: f(h.whole.end) } : undefined,
    part: { begin: f(h.part.begin), end: f(h.part.end) },
    value: h.value,
  }));

/** Query through `fq`, then map the resulting haps through `fh`. `fast(n)` is
 *  `withTime(t => t*n, t => t/n, p)`. */
export const withTime = <T>(fq: TimeFn, fh: TimeFn, p: Pat<T>): Pat<T> =>
  pattern((span) => withHapTime(fh, withQueryTime(fq, p)(span)));

/** Evaluate each integer cycle of a query separately (for `<>`, `every`, `iter`, …). */
export const splitQueries = <T>(p: Pat<T>): Pat<T> =>
  pattern((span) => spanCycles(span).flatMap((sub) => p.query(sub)));

// ── Hap filtering ─────────────────────────────────────────────────────────────

export const filterHaps = <T>(pred: (h: Hap<T>) => boolean, p: Pat<T>): Pat<T> =>
  pattern((span) => p.query(span).filter(pred));

/** Onset = the hap's part begins exactly at its whole's begin (no leading fragment). */
export const hasOnset = <T>(h: Hap<T>): boolean =>
  h.whole !== undefined && sameTime(h.whole.begin, h.part.begin);

export const filterOnsets = <T>(p: Pat<T>): Pat<T> => filterHaps(hasOnset, p);

/** The Phase 0 control value: a sound name. The full ControlMap (sound.ts) supersedes it. */
export interface SoundValue {
  s: string;
}
