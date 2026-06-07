import { describe, it, expect } from "vitest";
import type { Engine } from "../../src/backend/engine";
import { Clock } from "../../src/clock/clock";

function fakeEngine() {
  let currentFrame = 0;
  const engine: Engine = {
    start: async () => {},
    stop: async () => {},
    pushEvents: () => {},
    stats: () => ({ currentFrame, underruns: 0, lateEvents: 0, ringFullness: 0 }),
    loadSampleFile: () => ({ id: 0, channels: 0, frames: 0, sampleRate: 0 }),
    loadSamplePcm: () => ({ id: 0, channels: 0, frames: 0, sampleRate: 0 }),
    prepareStretched: () => ({ id: 0, channels: 0, frames: 0, sampleRate: 0 }),
  };
  return { engine, setFrame: (f: number) => (currentFrame = f) };
}

describe("Clock", () => {
  it("BPM helper round-trips", () => {
    const { engine } = fakeEngine();
    const clock = new Clock(engine);
    clock.setbpm(130);
    expect(clock.cps).toBeCloseTo(130 / 60 / 4, 10);
    expect(clock.bpm).toBeCloseTo(130, 6);
  });

  it("cycle↔frame maps one cycle to sampleRate/cps frames", () => {
    const { engine } = fakeEngine();
    const clock = new Clock(engine);
    clock.cps = 130 / 60 / 4;
    clock.anchor(48000);
    expect(clock.cycleToFrame(0)).toBe(0);
    expect(Math.abs(clock.cycleToFrame(1) - clock.cycleToFrame(0) - 48000 / clock.cps)).toBeLessThanOrEqual(1);
  });

  it("nowCycle increases monotonically as currentFrame increases", () => {
    const { engine, setFrame } = fakeEngine();
    const clock = new Clock(engine);
    clock.cps = 1;
    clock.anchor(48000);
    let prev = clock.nowCycle();
    for (const f of [12000, 24000, 48000, 96000]) {
      setFrame(f);
      const now = clock.nowCycle();
      expect(now).toBeGreaterThan(prev);
      prev = now;
    }
  });

  it("setcps is continuous and changes the subsequent rate", () => {
    const { engine, setFrame } = fakeEngine();
    const clock = new Clock(engine);
    clock.cps = 1;
    clock.anchor(48000); // epochFrame = 0
    setFrame(24000); // half a cycle in at cps=1
    expect(clock.nowCycle()).toBeCloseTo(0.5, 10);

    clock.setcps(2); // double tempo, mid-session
    expect(clock.nowCycle()).toBeCloseTo(0.5, 10); // no jump at the change

    setFrame(48000); // +24000 frames = +0.5 s → +1.0 cycle at cps=2
    expect(clock.nowCycle()).toBeCloseTo(1.5, 10);
  });
});
