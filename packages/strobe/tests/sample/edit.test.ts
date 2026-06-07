import { describe, it, expect } from "vitest";
import "../../src/sample/edit"; // installs .chop/.slice/.loop/.grain/.begin/.end + .rev override
import { pure, queryArc, type Hap } from "../../src/pattern/pattern";
import { rev } from "../../src/pattern/combinators";
import type { ControlMap } from "../../src/pattern/control";

const samp = (): ReturnType<typeof pure<ControlMap>> =>
  pure<ControlMap>({ s: "amen", sampleId: 0, _frames: 8000 });

const onsets = (haps: Hap<ControlMap>[]): Hap<ControlMap>[] =>
  haps
    .filter((h) => h.whole && Math.abs(h.part.begin - h.whole.begin) < 1e-9)
    .sort((a, b) => a.part.begin - b.part.begin);

const region = (h: Hap<ControlMap>): [number, number] => [h.value.begin as number, h.value.end as number];

describe("sample chop / slice / loop / rev", () => {
  it("chop(8) lays out 8 slices in order across the cycle", () => {
    const haps = onsets(queryArc(samp().chop(8), 0, 1));
    expect(haps.length).toBe(8);
    haps.forEach((h, k) => {
      expect(h.part.begin).toBeCloseTo(k / 8, 9);
      expect(h.value.begin).toBeCloseTo(k / 8, 9);
      expect(h.value.end).toBeCloseTo((k + 1) / 8, 9);
    });
  });

  it("chop(n) equals slice(n, '0..n-1')", () => {
    const a = onsets(queryArc(samp().chop(4), 0, 1)).map(region);
    const b = onsets(queryArc(samp().slice(4, "0 1 2 3"), 0, 1)).map(region);
    expect(a).toEqual(b);
  });

  it("slice re-sequences regions by the index pattern", () => {
    const haps = onsets(queryArc(samp().slice(8, "0 3 1 2"), 0, 1));
    expect(haps.map((h) => Math.round((h.value.begin as number) * 8))).toEqual([0, 3, 1, 2]);
  });

  it("chop reconstructs sample-exact: adjacent slices share a boundary", () => {
    const haps = onsets(queryArc(samp().chop(8), 0, 1));
    for (let k = 0; k < haps.length - 1; k++) {
      expect(haps[k].value.end).toBeCloseTo(haps[k + 1].value.begin as number, 9);
    }
  });

  it(".rev() on a chopped sample reverses order AND sets the rev flag", () => {
    const haps = onsets(queryArc(samp().chop(8).rev(), 0, 1));
    expect(haps.length).toBe(8);
    expect(haps[0].value.begin).toBeCloseTo(7 / 8, 9); // last slice comes first
    expect(haps.every((h) => h.value.rev === true)).toBe(true);
  });

  it(".rev().rev() cancels the per-slice flag", () => {
    const haps = onsets(queryArc(samp().chop(4).rev().rev(), 0, 1));
    expect(haps.every((h) => !h.value.rev)).toBe(true);
  });

  it(".rev() on a non-sample pattern only reorders (values untouched)", () => {
    const r = queryArc(pure(5).rev(), 0, 1);
    expect(r[0].value).toBe(5);
    // standalone rev combinator behaves identically for plain values
    expect(queryArc(rev(pure(9)), 0, 1)[0].value).toBe(9);
  });

  it(".loop() sets the loop flag", () => {
    expect(queryArc(samp().loop(), 0, 1)[0].value.loop).toBe(true);
  });

  it(".begin/.end set the region fractions", () => {
    const v = queryArc(samp().begin(0.25).end(0.5), 0, 1)[0].value;
    expect(v.begin).toBe(0.25);
    expect(v.end).toBe(0.5);
  });

  it(".grain(...) attaches the grain options", () => {
    const v = queryArc(samp().grain({ size: 60, density: 30, scatter: 0.1 }), 0, 1)[0].value;
    expect(v.grain).toEqual({ size: 60, density: 30, scatter: 0.1 });
  });

  it("composes with combinators (chop then every/rev) without error", () => {
    const p = samp().chop(8).every(2, (x) => x.rev());
    expect(() => queryArc(p, 0, 2)).not.toThrow();
    expect(onsets(queryArc(p, 0, 1)).length).toBe(8);
  });
});
