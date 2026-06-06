// The pattern core: a pattern is a pure function from a span of cycle-time to the
// events (haps) that happen in it (pattern-model.md "The one idea"). This file holds
// the stable primitives the scheduler queries; feature 06 adds the Phase 0 `bd`
// pattern helpers on top.

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
  /** What plays (Phase 0: `{ s: "bd" }`). */
  value: T;
}

/** A pattern is a pure function from a span to the haps in it. */
export type Pattern<T> = (span: TimeSpan) => Hap<T>[];

/** Query a pattern over [begin, end) in cycle-time. */
export function queryArc<T>(pat: Pattern<T>, begin: number, end: number): Hap<T>[] {
  return pat({ begin, end });
}

/** The Phase 0 control value: a sound name. The full control map arrives in Phase 2. */
export interface SoundValue {
  s: string;
}

/** Phase 0 stand-in for `sound("bd*n")`: `n` evenly spaced "bd" onsets per cycle,
 *  queryable over any span (multi-cycle and fractional) and emitting each onset
 *  exactly once. `sound("bd*4")` ⇒ `bdEvery(4)`. */
export function bdEvery(n: number): Pattern<SoundValue> {
  const step = 1 / n;
  return ({ begin, end }) => {
    const haps: Hap<SoundValue>[] = [];
    let k = Math.ceil(begin / step - 1e-9); // first slot at or after `begin`
    for (;;) {
      const onset = k * step + 0; // `+ 0` normalizes a -0 (from ceil) to +0
      if (onset >= end) break;
      const wholeEnd = onset + step;
      haps.push({
        whole: { begin: onset, end: wholeEnd },
        part: { begin: Math.max(onset, begin), end: Math.min(wholeEnd, end) },
        value: { s: "bd" },
      });
      k++;
    }
    return haps;
  };
}

/** Phase 0 stand-in for `sound("bd(k,n)")`: a Euclidean rhythm of `k` hits spread
 *  over `n` slots (Bresenham distribution — E(3,8) = x..x..x). */
export function bdEuclid(k: number, n: number): Pattern<SoundValue> {
  const step = 1 / n;
  // mask[i] is a hit when (i*k mod n) < k — the standard Euclidean (Bresenham) spread.
  const isHit = (slot: number) => ((slot * k) % n) < k;
  return ({ begin, end }) => {
    const haps: Hap<SoundValue>[] = [];
    let j = Math.ceil(begin / step - 1e-9);
    for (;;) {
      const onset = j * step + 0; // `+ 0` normalizes a -0 (from ceil) to +0
      if (onset >= end) break;
      const slot = ((j % n) + n) % n; // slot index within the cycle
      if (isHit(slot)) {
        const wholeEnd = onset + step;
        haps.push({
          whole: { begin: onset, end: wholeEnd },
          part: { begin: Math.max(onset, begin), end: Math.min(wholeEnd, end) },
          value: { s: "bd" },
        });
      }
      j++;
    }
    return haps;
  };
}
