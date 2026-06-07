import { describe, it, expect, beforeEach } from "vitest";
import { loadFile, loadPcm, setActiveSampleEngine } from "../../src/sample/load";
import type { Engine } from "../../src/backend/engine";

function fakeEngine() {
  let fileCalls = 0;
  let pcmCalls = 0;
  let nextId = 0;
  const engine: Engine = {
    start: async () => {},
    stop: async () => {},
    pushEvents: () => {},
    stats: () => ({ currentFrame: 0, underruns: 0, lateEvents: 0, ringFullness: 0 }),
    loadSampleFile: (path) => {
      if (path.includes("bad")) throw new Error("couldn't decode: unsupported codec");
      fileCalls++;
      return { id: nextId++, channels: 2, frames: 1000, sampleRate: 48000 };
    },
    loadSamplePcm: (_pcm, channels, sampleRate) => {
      pcmCalls++;
      return { id: nextId++, channels, frames: 500, sampleRate };
    },
  };
  return { engine, calls: () => ({ fileCalls, pcmCalls }) };
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
});
