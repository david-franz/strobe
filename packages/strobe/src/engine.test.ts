import { describe, it, expect } from "vitest";
import { createNativeEngine, type Engine } from "./engine";

// Encode one 32-byte NoteOn record (mirror of event-protocol.md), little-endian.
function noteOnBuffer(frame: number): ArrayBuffer {
  const buf = new ArrayBuffer(32);
  const v = new DataView(buf);
  v.setUint8(0, 1); // version
  v.setUint8(1, 0x01); // NoteOn
  v.setUint16(2, 1, true); // track
  v.setUint32(4, frame >>> 0, true); // frame
  v.setUint32(8, 1, true); // voice
  v.setFloat32(12, 55, true); // freq
  v.setFloat32(16, 0.9, true); // velocity
  return buf;
}

describe("native engine (no audio device required)", () => {
  it("constructs and reports zeroed stats", () => {
    const e = createNativeEngine();
    const s = e.stats();
    expect(s.currentFrame).toBe(0);
    expect(s.underruns).toBe(0);
    expect(s.lateEvents).toBe(0);
    expect(s.ringFullness).toBe(0);
  });

  it("stats() returns all-number fields with ringFullness in [0,1]", () => {
    const s = createNativeEngine().stats();
    for (const k of ["currentFrame", "underruns", "lateEvents", "ringFullness"] as const) {
      expect(typeof s[k]).toBe("number");
    }
    expect(s.ringFullness).toBeGreaterThanOrEqual(0);
    expect(s.ringFullness).toBeLessThanOrEqual(1);
  });

  it("pushEvents rejects a buffer whose length is not a multiple of 32", () => {
    const e = createNativeEngine();
    expect(() => e.pushEvents(new ArrayBuffer(31))).toThrow();
  });

  it("pushEvents accepts a 32-byte record and fills the ring", () => {
    const e = createNativeEngine();
    e.pushEvents(noteOnBuffer(4800));
    expect(e.stats().ringFullness).toBeGreaterThan(0);
  });

  it("pushEvents is synchronous (returns undefined, not a Promise)", () => {
    const e = createNativeEngine();
    const r = e.pushEvents(noteOnBuffer(0));
    expect(r).toBeUndefined();
  });

  it("a full ring drops without throwing or blocking", () => {
    const e = createNativeEngine();
    // Push far more than the 4096-record capacity without a running stream to drain.
    const big = new ArrayBuffer(32 * 5000);
    expect(() => e.pushEvents(big)).not.toThrow();
    // ring saturated at capacity.
    expect(e.stats().ringFullness).toBeGreaterThanOrEqual(0.99);
  });

  it("createNativeEngine() structurally satisfies the Engine interface", () => {
    const e: Engine = createNativeEngine();
    expect(typeof e.start).toBe("function");
    expect(typeof e.stop).toBe("function");
    expect(typeof e.pushEvents).toBe("function");
    expect(typeof e.stats).toBe("function");
  });
});

// The full start→push→stats→stop round-trip opens a real cpal output stream and so
// is skipped in headless/CI runs. Run locally with a working output device.
describe.skip("native engine round-trip (requires an audio device)", () => {
  it("advances the frame clock while playing with no underruns", async () => {
    const e = createNativeEngine();
    await e.start({ sampleRate: 48000, bufferFrames: 256 });
    e.pushEvents(noteOnBuffer(e.stats().currentFrame + 4800));
    await new Promise((r) => setTimeout(r, 200));
    const s = e.stats();
    expect(s.currentFrame).toBeGreaterThan(8000);
    expect(s.underruns).toBe(0);
    expect(s.lateEvents).toBe(0);
    await e.stop();
  });
});
