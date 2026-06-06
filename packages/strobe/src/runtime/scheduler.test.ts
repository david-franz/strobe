import { describe, it, expect, beforeEach } from "vitest";
import type { Engine } from "../backend/engine";
import type { Pattern } from "../pattern/pattern";
import { Clock } from "../clock/clock";
import { Scheduler, writeNoteOn, writeTempo } from "./scheduler";
import { setTrack } from "./tracks";

function fakeEngine() {
  let currentFrame = 0;
  const pushed: ArrayBuffer[] = [];
  const engine: Engine = {
    start: async () => {},
    stop: async () => {},
    pushEvents: (b) => {
      pushed.push(b);
    },
    stats: () => ({ currentFrame, underruns: 0, lateEvents: 0, ringFullness: 0 }),
  };
  return { engine, pushed, setFrame: (f: number) => (currentFrame = f) };
}

// A quarter-cycle "bd" pattern: 4 evenly spaced onsets per cycle, queryable over any
// span (the Phase 0 `bdEvery(4)` stand-in feature 06 formalizes).
const bd4: Pattern<{ s: string }> = ({ begin, end }) => {
  const step = 0.25;
  const haps = [];
  let k = Math.ceil(begin / step - 1e-9);
  for (;;) {
    const onset = k * step;
    if (onset >= end) break;
    haps.push({
      whole: { begin: onset, end: onset + step },
      part: { begin: Math.max(onset, begin), end: Math.min(onset + step, end) },
      value: { s: "bd" },
    });
    k++;
  }
  return haps;
};

function framesFrom(pushed: ArrayBuffer[]): number[] {
  const frames: number[] = [];
  for (const b of pushed) {
    const v = new DataView(b);
    for (let o = 0; o < b.byteLength; o += 32) {
      expect(v.getUint8(o + 0)).toBe(1); // version
      expect(v.getUint8(o + 1)).toBe(0x01); // NoteOn
      frames.push(v.getUint32(o + 4, true));
    }
  }
  return frames;
}

beforeEach(() => setTrack(1, null));

describe("event encoders", () => {
  it("writeNoteOn produces the exact little-endian 32-byte layout", () => {
    const v = new DataView(new ArrayBuffer(32));
    writeNoteOn(v, 0, 1, 0x01020304, 7, 55, 0.9, 8000, 0, 42);
    expect(v.getUint8(0)).toBe(1);
    expect(v.getUint8(1)).toBe(0x01);
    expect(v.getUint16(2, true)).toBe(1);
    expect(v.getUint32(4, true)).toBe(0x01020304);
    expect(v.getUint32(8, true)).toBe(7);
    expect(v.getFloat32(12, true)).toBe(55);
    expect(v.getFloat32(16, true)).toBeCloseTo(0.9, 6);
    expect(v.getFloat32(20, true)).toBe(8000);
    expect(v.getUint32(28, true)).toBe(42);
  });

  it("writeTempo encodes the global-track Tempo opcode", () => {
    const v = new DataView(new ArrayBuffer(32));
    writeTempo(v, 0, 10, 0.5417, 1);
    expect(v.getUint8(1)).toBe(0x40);
    expect(v.getUint16(2, true)).toBe(0);
    expect(v.getUint32(4, true)).toBe(10);
    expect(v.getFloat32(12, true)).toBeCloseTo(0.5417, 4);
  });
});

describe("frame math", () => {
  it("cycleToFrame advances by sampleRate/cps per cycle", () => {
    const { engine } = fakeEngine();
    const clock = new Clock(engine);
    clock.cps = 130 / 60 / 4;
    clock.anchor(48000);
    const f0 = clock.cycleToFrame(0);
    const f1 = clock.cycleToFrame(1);
    expect(f0).toBe(0);
    expect(Math.abs(f1 - f0 - 48000 / clock.cps)).toBeLessThanOrEqual(1);
  });
});

describe("look-ahead scheduling", () => {
  it("schedules each cycle-time slice exactly once across overlapping windows", () => {
    const { engine, pushed, setFrame } = fakeEngine();
    const clock = new Clock(engine);
    clock.cps = 1; // 1 cycle/sec → 1 cycle = 48000 frames, quarter = 12000
    clock.anchor(48000);
    setTrack(1, bd4);
    const sched = new Scheduler(engine, clock, { lookaheadS: 0.1 });

    // 60 overlapping ticks of ~25 ms each (~1.5 cycles of coverage).
    for (let i = 0; i < 60; i++) {
      setFrame(Math.round(i * 0.025 * 48000));
      sched.tick();
    }

    const frames = framesFrom(pushed);
    // No duplicate onsets.
    expect(new Set(frames).size).toBe(frames.length);
    // Exactly the quarter-cycle grid (multiples of 12000), none missing.
    const max = Math.max(...frames);
    const expected: number[] = [];
    for (let f = 0; f <= max; f += 12000) expected.push(f);
    expect([...frames].sort((a, b) => a - b)).toEqual(expected);
  });

  it("emits at most one pushEvents per tick, each a whole number of records", () => {
    const { engine, pushed, setFrame } = fakeEngine();
    const clock = new Clock(engine);
    clock.cps = 1;
    clock.anchor(48000);
    setTrack(1, bd4);
    const sched = new Scheduler(engine, clock, { lookaheadS: 0.1 });

    setFrame(0);
    sched.tick();
    const before = pushed.length;
    setFrame(24000); // half a cycle later
    sched.tick();
    expect(pushed.length - before).toBeLessThanOrEqual(1);
    for (const b of pushed) expect(b.byteLength % 32).toBe(0);
  });

  it("packs the hap value's instrument id, pitch and velocity into the NoteOn", () => {
    const { engine, pushed, setFrame } = fakeEngine();
    const clock = new Clock(engine);
    clock.cps = 1;
    clock.anchor(48000);
    // A one-event pattern carrying a saw note at 220 Hz.
    const note: Pattern<{ instrument: number; freq: number; vel: number }> = ({ begin, end }) =>
      begin <= 0 && 0 < end
        ? [{ whole: { begin: 0, end: 1 }, part: { begin: 0, end: 1 }, value: { instrument: 5, freq: 220, vel: 0.7 } }]
        : [];
    setTrack(1, note);
    const sched = new Scheduler(engine, clock, { lookaheadS: 0.1 });
    setFrame(0);
    sched.tick();
    const v = new DataView(pushed[0]);
    expect(v.getFloat32(12, true)).toBeCloseTo(220, 3); // arg0 freq
    expect(v.getFloat32(16, true)).toBeCloseTo(0.7, 5); // arg1 vel
    expect(v.getFloat32(24, true)).toBe(5); // arg3 instrument
  });

  it("does not emit events when no pattern is assigned", () => {
    const { engine, pushed, setFrame } = fakeEngine();
    const clock = new Clock(engine);
    clock.cps = 1;
    clock.anchor(48000);
    const sched = new Scheduler(engine, clock, { lookaheadS: 0.1 });
    setFrame(0);
    sched.tick();
    setFrame(48000);
    sched.tick();
    expect(pushed.length).toBe(0);
  });
});
