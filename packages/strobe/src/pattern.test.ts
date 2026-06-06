import { describe, it, expect } from "vitest";
import { bdEvery, bdEuclid, queryArc } from "./pattern";

describe("bdEvery", () => {
  it("queryArc(bdEvery(4), 0, 1) yields 4 quarter-cycle onsets", () => {
    const haps = queryArc(bdEvery(4), 0, 1);
    expect(haps.map((h) => h.whole?.begin)).toEqual([0, 0.25, 0.5, 0.75]);
    for (const h of haps) {
      expect(h.value).toEqual({ s: "bd" });
      expect(h.whole?.end).toBeCloseTo((h.whole?.begin ?? 0) + 0.25, 10);
    }
  });

  it("emits each onset exactly once across adjacent windows (no dup, no gap)", () => {
    const onsets = [
      ...queryArc(bdEvery(4), 0, 0.3),
      ...queryArc(bdEvery(4), 0.3, 0.6),
      ...queryArc(bdEvery(4), 0.6, 1.0),
    ].map((h) => h.whole?.begin);
    expect(onsets).toEqual([0, 0.25, 0.5, 0.75]);
  });

  it("repeats shifted on a multi-cycle query", () => {
    const haps = queryArc(bdEvery(4), 3.0, 4.0);
    expect(haps.map((h) => h.whole?.begin)).toEqual([3.0, 3.25, 3.5, 3.75]);
  });
});

describe("bdEuclid", () => {
  it("E(3,8) places hits at slots 0, 3, 6", () => {
    const haps = queryArc(bdEuclid(3, 8), 0, 1);
    expect(haps.map((h) => h.whole?.begin)).toEqual([0, 0.375, 0.75]);
    expect(haps.every((h) => h.value.s === "bd")).toBe(true);
  });

  it("repeats across cycles", () => {
    const haps = queryArc(bdEuclid(3, 8), 1.0, 2.0);
    expect(haps.map((h) => h.whole?.begin)).toEqual([1.0, 1.375, 1.75]);
  });
});
