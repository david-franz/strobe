import { describe, it, expect } from "vitest";
import {
  pure,
  silence,
  signal,
  queryArc,
  spanCycles,
  withTime,
  hasOnset,
  sameTime,
} from "./pattern";

describe("pattern core", () => {
  it("pure plays one value per cycle", () => {
    expect(queryArc(pure("bd"), 0, 1)).toEqual([
      { whole: { begin: 0, end: 1 }, part: { begin: 0, end: 1 }, value: "bd" },
    ]);
  });

  it("pure splits across cycles", () => {
    const haps = queryArc(pure("bd"), 0, 2);
    expect(haps.map((h) => h.whole)).toEqual([
      { begin: 0, end: 1 },
      { begin: 1, end: 2 },
    ]);
  });

  it("a leading fragment has no onset", () => {
    const [h] = queryArc(pure("bd"), 0.5, 1);
    expect(h.whole).toEqual({ begin: 0, end: 1 });
    expect(h.part).toEqual({ begin: 0.5, end: 1 });
    expect(hasOnset(h)).toBe(false);
  });

  it("silence is empty", () => {
    expect(queryArc(silence, 0, 4)).toEqual([]);
  });

  it("signal point-samples with no whole", () => {
    expect(queryArc(signal((t) => t), 0.25, 0.25)).toEqual([
      { whole: undefined, part: { begin: 0.25, end: 0.25 }, value: 0.25 },
    ]);
  });

  it("spanCycles splits a multi-cycle span", () => {
    expect(spanCycles({ begin: 0.5, end: 2.5 })).toEqual([
      { begin: 0.5, end: 1 },
      { begin: 1, end: 2 },
      { begin: 2, end: 2.5 },
    ]);
  });

  it("withTime realises the fast(2) mechanic", () => {
    const haps = queryArc(withTime((t) => t * 2, (t) => t / 2, pure("x")), 0, 1);
    expect(haps.map((h) => h.whole)).toEqual([
      { begin: 0, end: 0.5 },
      { begin: 0.5, end: 1 },
    ]);
  });

  it("is pure — querying the same arc twice is deep-equal", () => {
    const p = pure("bd");
    expect(queryArc(p, 0, 4)).toEqual(queryArc(p, 0, 4));
  });

  it("fmap maps values, preserving time", () => {
    const haps = queryArc(pure(3).fmap((v) => v * 2), 0, 1);
    expect(haps[0].value).toBe(6);
    expect(haps[0].whole).toEqual({ begin: 0, end: 1 });
  });

  it("sameTime tolerates float error", () => {
    expect(sameTime(1 / 3, 0.3333333333)).toBe(true);
    expect(sameTime(0, 0.01)).toBe(false);
  });
});
