import { describe, it, expect, beforeEach } from "vitest";
import { loadFile, loadPcm, sample, setActiveSampleEngine } from "../../src/sample/load";
import { queryArc } from "../../src/pattern/pattern";
import type { Engine } from "../../src/backend/engine";
import { Clock } from "../../src/clock/clock";
import { setActiveClock } from "../../src/clock/singleton";
import "../../src/sample/edit"; // installs .pitch()/.stretch()/.fit()

function fakeEngine() {
  let fileCalls = 0;
  let pcmCalls = 0;
  let stretchCalls = 0;
  let nextId = 0;
  const engine: Engine = {
    start: async () => {},
    stop: async () => {},
    pushEvents: () => {},
    stats: () => ({ currentFrame: 0, underruns: 0, lateEvents: 0, ringFullness: 0 }),
    scope: () => new Float32Array(0),
    loadSampleFile: (path) => {
      if (path.includes("bad")) throw new Error("couldn't decode: unsupported codec");
      fileCalls++;
      return { id: nextId++, channels: 2, frames: 1000, sampleRate: 48000 };
    },
    loadSamplePcm: (_pcm, channels, sampleRate) => {
      pcmCalls++;
      return { id: nextId++, channels, frames: 500, sampleRate };
    },
    prepareStretched: (_id, stretch) => {
      stretchCalls++;
      return { id: nextId++, channels: 2, frames: Math.round(1000 * stretch), sampleRate: 48000 };
    },
  };
  return { engine, calls: () => ({ fileCalls, pcmCalls, stretchCalls }) };
}

describe("sample load", () => {
  beforeEach(() => setActiveSampleEngine(null)); // clears the idempotency cache

  it("loadFile returns a handle with the engine's metadata", () => {
    const { engine } = fakeEngine();
    setActiveSampleEngine(engine);
    const h = loadFile("/x/amen.wav");
    expect(h.id).toBe(0);
    expect(h.channels).toBe(2);
    expect(h.frames).toBe(1000);
    expect(h.name).toBe("/x/amen.wav");
  });

  it("loadFile is idempotent per path (decodes once)", () => {
    const f = fakeEngine();
    setActiveSampleEngine(f.engine);
    const a = loadFile("/x/amen.wav");
    const b = loadFile("/x/amen.wav");
    expect(a).toBe(b);
    expect(f.calls().fileCalls).toBe(1);
  });

  it("a bad file surfaces a clear error rather than crashing", () => {
    const { engine } = fakeEngine();
    setActiveSampleEngine(engine);
    expect(() => loadFile("/x/bad.wav")).toThrow(/decode/);
  });

  it("loadPcm with a key is idempotent", () => {
    const f = fakeEngine();
    setActiveSampleEngine(f.engine);
    const pcm = new Float32Array(10);
    loadPcm(pcm, 1, 48000, { key: "hash1" });
    loadPcm(pcm, 1, 48000, { key: "hash1" });
    expect(f.calls().pcmCalls).toBe(1);
  });

  it("loading before the runtime starts throws a clear error", () => {
    expect(() => loadFile("/x/amen.wav")).toThrow(/no active engine/);
  });

  it("sample() loads and produces a one-shot trigger pattern", () => {
    const { engine } = fakeEngine();
    setActiveSampleEngine(engine);
    const pat = sample("/x/amen.wav");
    const haps = queryArc(pat, 0, 1);
    expect(haps.length).toBe(1);
    expect(haps[0].value.sampleId).toBe(0);
    expect(haps[0].value._frames).toBe(1000);
  });

  it(".stretch prepares a variant and remaps sampleId + frame count", () => {
    const { engine } = fakeEngine();
    setActiveSampleEngine(engine);
    const v = sample("/x/amen.wav").stretch(2);
    const cm = queryArc(v, 0, 1)[0].value;
    expect(cm.sampleId).not.toBe(0); // remapped to the prepared variant id
    expect(cm._frames).toBe(2000); // 1000 frames * stretch 2
  });

  it(".pitch keeps the frame count (constant duration) but remaps the id", () => {
    const { engine } = fakeEngine();
    setActiveSampleEngine(engine);
    const v = sample("/x/amen.wav").pitch(12);
    const cm = queryArc(v, 0, 1)[0].value;
    expect(cm.sampleId).not.toBe(0);
    expect(cm._frames).toBe(1000); // stretch factor 1 → unchanged length
  });

  it(".fit(n) stretches a sample to span exactly n cycles at the current tempo", () => {
    const { engine } = fakeEngine();
    setActiveSampleEngine(engine);
    const clock = new Clock(engine);
    clock.cps = 1;
    clock.anchor(48000); // one cycle = 48000 frames
    setActiveClock(clock);
    // sample() loads a 1000-frame buffer; fit(1) must stretch it to one cycle = 48000 frames.
    const cm = queryArc(sample("/x/amen.wav").fit(1), 0, 1)[0].value;
    expect(cm.sampleId).not.toBe(0); // remapped to the fitted variant
    expect(cm._frames).toBe(48000); // 1000 × (48000 / 1000)
  });

  it("a stretch edit prepares once per distinct id (cached across reloads)", () => {
    const f = fakeEngine();
    setActiveSampleEngine(f.engine);
    const before = f.calls().stretchCalls;
    sample("/x/amen.wav").stretch(2);
    sample("/x/amen.wav").stretch(2); // same (id, factor) → cache hit
    expect(f.calls().stretchCalls - before).toBe(1);
  });
});
