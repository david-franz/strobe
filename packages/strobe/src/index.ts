// The Phase 0 public surface a song imports:
//
//   import { setcps, p1, bdEvery } from "strobe";
//   setcps(130 / 60 / 4);
//   p1( bdEvery(4) );
//
// `sound("bd*4")` (the parser-backed API) intentionally lands with the mini-notation
// parser in Phase 2; Phase 0 uses the bdEvery/bdEuclid stand-ins.

export { setcps, setbpm } from "./clock-singleton";
export { p1, p2, p3, p4, p5, p6, p7, p8 } from "./tracks";
export { seq, every, euclid } from "./patterns";
export { bdEvery, bdEuclid } from "./pattern";
export type { Pattern, Hap, TimeSpan, SoundValue } from "./pattern";
export { PRESETS, resolveNote } from "./presets";
export type { PresetName, NoteValue } from "./presets";
export { noteToHz, midiToHz, synth } from "./synth";
export { drum } from "./drums";
