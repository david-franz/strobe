// The public surface a song imports:
//
//   import { setcps, p1, p2, p3, sound, note, sine } from "strobe";
//   setcps(130 / 60 / 4);
//   p1( sound("bd*4").gain(0.9) );
//   p3( note("c2 c2 eb2 g2").saw().lpf(sine.range(300,1200).slow(8)) );

// Tempo + live tracks
export { setcps, setbpm } from "./clock/singleton";
export { p1, p2, p3, p4, p5, p6, p7, p8, mute, unmute, solo, unsolo, hush } from "./runtime/tracks";

// The sound/note API
export { sound, s, note, freq, jux, hurry } from "./pattern/sound";

// Combinators (standalone forms). `every`/`euclid` are reached via the `.every()` method
// and mini-notation `(3,8)` so the barrel names stay free for the Phase 1 builders below.
export {
  fast,
  slow,
  rev,
  early,
  late,
  off,
  whenmod,
  iter,
  chunk,
  stack,
  cat,
  slowcat,
  fastcat,
  superimpose,
  degrade,
  degradeBy,
  sometimes,
  sometimesBy,
  somecycles,
} from "./pattern/combinators";

// Continuous signals
export { sine, cosine, saw, isaw, tri, square, rand, perlin } from "./pattern/signals";

// Pattern primitives + the parser (advanced use / testing)
export { Pat, pure, silence, signal, queryArc, type Pattern, type Hap, type TimeSpan } from "./pattern/pattern";
export { parseMini } from "./pattern/mini";
export { parseNote, noteToFreq, type ControlMap } from "./pattern/control";

// ── Phase 1 helpers (still exported for the Phase 1 example / instrument config) ──
export { seq, every, euclid, bdEvery, bdEuclid } from "./pattern/builders";
export { PRESETS, resolveNote } from "./instruments/presets";
export type { PresetName, NoteValue } from "./instruments/presets";
export { noteToHz, midiToHz, synth } from "./instruments/synth";
export { drum } from "./instruments/drums";
