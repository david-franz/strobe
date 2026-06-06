import { describe, it, expect } from "vitest";
import { drum, drumKind, isDrumName } from "./drums";

describe("drum name mapping", () => {
  it("maps the kit tokens to kinds", () => {
    expect(drumKind("bd")).toBe("kick");
    expect(drumKind("sn")).toBe("snare");
    expect(drumKind("cp")).toBe("clap");
    expect(drumKind("hh")).toBe("closedHat");
    expect(drumKind("oh")).toBe("openHat");
  });

  it("recognises drum tokens", () => {
    expect(isDrumName("bd")).toBe(true);
    expect(isDrumName("c2")).toBe(false);
  });
});

describe("DrumBuilder", () => {
  it("carries defaults and applies overrides", () => {
    const cfg = drum("bd").gain(0.8).decay(0.4).tone(55).config();
    expect(cfg).toMatchObject({ type: "drum", kind: "kick", gain: 0.8 });
    expect(cfg.params).toEqual({ decay: 0.4, tone: 55 });
  });

  it("returns an isolated copy", () => {
    const b = drum("hh");
    const a = b.config();
    b.gain(0.1);
    expect(a.gain).toBe(0.9);
  });
});
