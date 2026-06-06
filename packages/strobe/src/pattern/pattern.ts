// The pattern core: a pattern is a pure function from a span of cycle-time to the
// events (haps) that happen in it (pattern-model.md "The one idea"). This file holds the
// stable primitives the scheduler queries; the concrete pattern builders live in
// `builders.ts` (and, from Phase 2, the mini-notation parser + combinators).

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
