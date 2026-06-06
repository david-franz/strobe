// The cps clock: maps cycle-time ↔ engine frames. Anchored to the engine's frame
// counter (the source of truth) so JS wall-clock jitter never detunes the groove —
// "now in cycles" is derived from stats().currentFrame, not Date.now().
//
// Feature 05 introduces the minimal mapping the scheduler needs; feature 06 adds
// setcps (with continuity re-anchoring), the BPM helpers, and their tests.

import type { Engine } from "./engine";

export class Clock {
  /** Cycles per second (default ~120 BPM at 4 beats/cycle). */
  cps = 0.5;
  private sampleRate = 48000;
  private epochCycle = 0; // cycle-time at the engine-frame epoch below
  private epochFrame = 0; // engine frame corresponding to epochCycle

  constructor(private engine: Engine) {}

  /** Capture the engine's frame/sample-rate as the clock epoch. Called at start(). */
  anchor(sampleRate: number): void {
    this.sampleRate = sampleRate;
    this.epochFrame = this.engine.stats().currentFrame;
    this.epochCycle = 0; // cycle 0 is "now" at start
  }

  /** Set tempo directly (TidalCycles `setcps`), re-anchoring so the cycle position
   *  is continuous across the change (a live tempo edit never teleports the groove). */
  setcps(x: number): void {
    const nowFrame = this.engine.stats().currentFrame;
    this.epochCycle = this.nowCycle(); // freeze current position (at the old cps)
    this.epochFrame = nowFrame;
    this.cps = x;
  }

  /** BPM helper: at 4 beats/cycle, bpm = cps*4*60. */
  get bpm(): number {
    return this.cps * 4 * 60;
  }

  setbpm(bpm: number): void {
    this.setcps(bpm / 60 / 4);
  }

  /** Current cycle-time, derived from the engine's sample clock (source of truth). */
  nowCycle(): number {
    const frame = this.engine.stats().currentFrame;
    const secs = (frame - this.epochFrame) / this.sampleRate;
    return this.epochCycle + secs * this.cps;
  }

  /** cycle-time → absolute engine frame (used to stamp events). */
  cycleToFrame(cycle: number): number {
    const secs = (cycle - this.epochCycle) / this.cps;
    return Math.round(this.epochFrame + secs * this.sampleRate);
  }

  /** a duration in cycles → a duration in frames (for note length). */
  cyclesToFrames(dCycles: number): number {
    return Math.round((dCycles / this.cps) * this.sampleRate);
  }
}
