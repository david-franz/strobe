import { describe, it, expect } from "vitest";
import { parseMini, MiniError } from "../../src/pattern/mini";
import { queryArc, sameTime } from "../../src/pattern/pattern";

const onsets = (src: string, begin: number, end: number) =>
  queryArc(parseMini(src), begin, end)
    .filter((h) => h.whole && sameTime(h.whole.begin, h.part.begin))
    .map((h) => ({ at: h.part.begin, v: h.value }));

describe("mini-notation", () => {
  it("a b c sequence (the canonical example)", () => {
    expect(queryArc(parseMini("bd ~ sn ~"), 0, 1)).toEqual([
      { whole: { begin: 0, end: 0.25 }, part: { begin: 0, end: 0.25 }, value: "bd" },
      { whole: { begin: 0.5, end: 0.75 }, part: { begin: 0.5, end: 0.75 }, value: "sn" },
    ]);
  });

  it("a*2 speeds up within its slot", () => {
    expect(onsets("bd*2 sn", 0, 1)).toEqual([
      { at: 0, v: "bd" },
      { at: 0.25, v: "bd" },
      { at: 0.5, v: "sn" },
    ]);
  });

  it("<a b> alternates per cycle", () => {
    expect(onsets("bd <sn cp>", 0, 1).map((o) => o.v)).toEqual(["bd", "sn"]);
    expect(onsets("bd <sn cp>", 1, 2).map((o) => o.v)).toEqual(["bd", "cp"]);
  });

  it("Euclid (3,8) → onsets at 0, 3/8, 6/8 (Bjorklund)", () => {
    const o = onsets("bd(3,8)", 0, 1);
    expect(o.map((x) => x.v)).toEqual(["bd", "bd", "bd"]);
    expect(o.map((x) => x.at)).toEqual([0, 3 / 8, 6 / 8]);
  });

  it("[bd*4, hh*8] stacks two layers in one cycle", () => {
    const o = onsets("[bd*4, hh*8]", 0, 1);
    expect(o.filter((x) => x.v === "bd").length).toBe(4);
    expect(o.filter((x) => x.v === "hh").length).toBe(8);
  });

  it("a!3 replicates into separate steps", () => {
    expect(onsets("bd!3 sn", 0, 1)).toEqual([
      { at: 0, v: "bd" },
      { at: 0.25, v: "bd" },
      { at: 0.5, v: "bd" },
      { at: 0.75, v: "sn" },
    ]);
  });

  it("a 3-element sequence lands on 0, 1/3, 2/3 (precision)", () => {
    const o = onsets("a b c", 0, 1);
    expect(sameTime(o[0].at, 0)).toBe(true);
    expect(sameTime(o[1].at, 1 / 3)).toBe(true);
    expect(sameTime(o[2].at, 2 / 3)).toBe(true);
  });

  it("degrade is deterministic across queries", () => {
    expect(queryArc(parseMini("hh*8?"), 0, 1)).toEqual(queryArc(parseMini("hh*8?"), 0, 1));
  });

  it("reports an unclosed bracket", () => {
    expect(() => parseMini("[bd sn")).toThrow(MiniError);
  });

  it("reports a postfix with no primary", () => {
    expect(() => parseMini("*2")).toThrow(MiniError);
  });
});
