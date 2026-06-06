// The Phase 0 public surface a song imports:
//
//   import { setcps, p1, bdEvery } from "strobe";
//   setcps(130 / 60 / 4);
//   p1( bdEvery(4) );
//
// `sound("bd*4")` (the parser-backed API) intentionally lands with the mini-notation
// parser in Phase 2; Phase 0 uses the bdEvery/bdEuclid stand-ins.

export { setcps, setbpm } from "./clock/singleton";
export { p1, p2, p3, p4, p5, p6, p7, p8 } from "./runtime/tracks";
export { seq, every, euclid } from "./pattern/builders";
export { bdEvery, bdEuclid } from "./pattern/builders";
export type { Pattern, Hap, TimeSpan, SoundValue } from "./pattern/pattern";
export { PRESETS, resolveNote } from "./instruments/presets";
export type { PresetName, NoteValue } from "./instruments/presets";
export { noteToHz, midiToHz, synth } from "./instruments/synth";
export { drum } from "./instruments/drums";
