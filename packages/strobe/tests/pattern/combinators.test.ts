import { describe, it, expect } from "vitest";
import {
  fast,
  slow,
  rev,
  every,
  iter,
  off,
  fastcat,
  slowcat,
  degrade,
  euclid,
  bjorklund,
} from "../../src/pattern/combinators";
import { parseMini } from "../../src/pattern/mini";
import { pure, queryArc, sameTime } from "../../src/pattern/pattern";

const onsetValues = (haps: { whole?: { begin: number } | undefined; part: { begin: number }; value: unknown }[]) =>
  haps
    .filter((h) => h.whole && sameTime(h.whole.begin, h.part.begin))
    .sort((a, b) => a.part.begin - b.part.begin)
    .map((h) => h.value);

describe("combinators", () => {
  it("fast(2) doubles within the cycle", () => {
    expect(queryArc(fast(2, pure("x")), 0, 1).map((h) => h.whole)).toEqual([
      { begin: 0, end: 0.5 },
      { begin: 0.5, end: 1 },
    ]);
  });

  it("slow(2) stretches over two cycles", () => {
    const haps = queryArc(slow(2, parseMini("a b")), 0, 2);
    expect(onsetValues(haps)).toEqual(["a", "b"]);
    expect(haps[0].whole).toEqual({ begin: 0, end: 1 });
  });

  it("rev reverses within a cycle", () => {
    expect(onsetValues(queryArc(rev(parseMini("a b c d")), 0, 1))).toEqual(["d", "c", "b", "a"]);
  });

  it("every applies fn only on every nth cycle", () => {
    expect(onsetValues(queryArc(every(3, rev, parseMini("a b")), 0, 1))).toEqual(["b", "a"]);
    expect(onsetValues(queryArc(every(3, rev, parseMini("a b")), 1, 2))).toEqual(["a", "b"]);
  });

  it("iter rotates by 1/n each cycle", () => {
    expect(onsetValues(queryArc(iter(4, parseMini("a b c d")), 1, 2))[0]).toBe("b");
  });

  it("off overlays a shifted copy", () => {
    const begins = queryArc(off(0.25, (x) => x, pure("x")), 0, 1)
      .filter((h) => h.whole && sameTime(h.whole.begin, h.part.begin))
      .map((h) => h.part.begin)
      .sort((a, b) => a - b);
    expect(begins).toEqual([0, 0.25]);
  });

  it("fastcat / slowcat", () => {
    expect(queryArc(fastcat(pure("a"), pure("b"), pure("c")), 0, 1).map((h) => h.part.begin)).toEqual([
      0,
      expect.closeTo(1 / 3, 9),
      expect.closeTo(2 / 3, 9),
    ]);
    expect(onsetValues(queryArc(slowcat(pure("a"), pure("b")), 0, 2))).toEqual(["a", "b"]);
  });

  it("degrade is deterministic across two queries", () => {
    const p = degrade(parseMini("hh*8"));
    expect(queryArc(p, 0, 1)).toEqual(queryArc(p, 0, 1));
  });

  it("bjorklund(3,8) is the canonical Euclid pattern", () => {
    expect(bjorklund(3, 8)).toEqual([true, false, false, true, false, false, true, false]);
    expect(onsetValues(queryArc(euclid(3, 8, 0, pure("bd")), 0, 1)).length).toBe(3);
  });

  it("standalone fn and .method() agree", () => {
    expect(queryArc(parseMini("a b").fast(2), 0, 1)).toEqual(queryArc(fast(2, parseMini("a b")), 0, 1));
    expect(queryArc(parseMini("a b c d").rev(), 0, 1)).toEqual(queryArc(rev(parseMini("a b c d")), 0, 1));
    expect(queryArc(parseMini("a b").every(2, rev), 0, 1)).toEqual(
      queryArc(every(2, rev, parseMini("a b")), 0, 1),
    );
  });
});
