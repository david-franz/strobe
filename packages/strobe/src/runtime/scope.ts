// Terminal waveform: turn a window of master-output samples (from engine.scope) into an ASCII
// oscilloscope. `renderScope` is pure and testable; `startScopeDisplay` drives the in-place
// redraw + color while a song plays. The samples are the real engine output, so the trace is a
// faithful picture of what you're hearing, not a decorative animation.

import type { Engine } from "../backend/engine";

export interface ScopeOpts {
  width?: number; // columns (default 80)
  height?: number; // rows; forced odd so there's a true center axis (default 9)
}

const FILL = "█";
const AXIS = "─";
const BLANK = " ";

/** Render `samples` as `height` rows × `width` columns. Each column is filled across the
 *  min..max extent of its slice of the window (a filled oscilloscope trace), with a center axis
 *  line where a column is empty. Amplitude is clamped to [-1, 1] (no auto-gain — it shows the
 *  true level). */
export function renderScope(samples: ArrayLike<number>, opts: ScopeOpts = {}): string[] {
  const width = Math.max(1, Math.floor(opts.width ?? 80));
  let height = Math.max(3, Math.floor(opts.height ?? 9));
  if (height % 2 === 0) height += 1; // odd → a real center row
  const mid = (height - 1) / 2;

  const rowOf = (v: number): number => {
    const c = v < -1 ? -1 : v > 1 ? 1 : v;
    return Math.round((1 - (c + 1) / 2) * (height - 1)); // +1 → top row 0, -1 → bottom row
  };

  const grid: string[][] = Array.from({ length: height }, () => new Array<string>(width).fill(BLANK));
  for (let c = 0; c < width; c++) grid[mid][c] = AXIS; // zero axis

  const n = samples.length;
  if (n > 0) {
    const per = n / width;
    for (let col = 0; col < width; col++) {
      const lo = Math.floor(col * per);
      const hi = Math.max(lo + 1, Math.floor((col + 1) * per));
      let mn = Infinity;
      let mx = -Infinity;
      for (let i = lo; i < hi && i < n; i++) {
        const s = samples[i];
        if (s < mn) mn = s;
        if (s > mx) mx = s;
      }
      if (mn === Infinity) continue;
      const top = rowOf(mx);
      const bot = rowOf(mn);
      for (let r = top; r <= bot; r++) grid[r][col] = FILL;
    }
  }
  return grid.map((row) => row.join(""));
}

const GREEN = "\x1b[32m";
const DIM = "\x1b[2m";
const RESET = "\x1b[0m";
const HIDE_CURSOR = "\x1b[?25l";
const SHOW_CURSOR = "\x1b[?25h";

/** Drive a live oscilloscope + stats line in the terminal, redrawing in place. Returns a stop
 *  function. Intended for a TTY; the caller falls back to a plain stats line when piped. */
export function startScopeDisplay(
  engine: Engine,
  opts: { width?: number; height?: number; fps?: number } = {},
): () => void {
  const out = process.stdout;
  const width = opts.width ?? Math.max(20, Math.min((out.columns ?? 80) - 1, 120));
  let height = opts.height ?? 9;
  if (height % 2 === 0) height += 1;
  const blockLines = height + 1; // scope rows + one stats line
  const nSamples = Math.max(width * 4, 512); // a few samples per column for min/max detail
  const period = Math.round(1000 / (opts.fps ?? 24));

  let first = true;
  out.write(HIDE_CURSOR);
  const timer = setInterval(() => {
    const lines = renderScope(engine.scope(nSamples), { width, height });
    const s = engine.stats();
    const stat = `frame=${s.currentFrame}  underruns=${s.underruns}  late=${s.lateEvents}  ring=${s.ringFullness.toFixed(3)}`;
    let frame = first ? "" : `\x1b[${blockLines}A`; // move up over the previous block
    first = false;
    for (const ln of lines) frame += `\r\x1b[2K${GREEN}${ln}${RESET}\n`;
    frame += `\r\x1b[2K${DIM}${stat}${RESET}\n`;
    out.write(frame);
  }, period);

  return () => {
    clearInterval(timer);
    out.write(SHOW_CURSOR + "\n");
  };
}
