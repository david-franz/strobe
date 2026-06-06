// Phase 1 demo — the synthesis core: a synthesized drum kit, a pitched subtractive
// synth bassline, real polyphony, and the master limiter keeping it all from clipping.
//
// Run it (needs a working audio output device):
//   pnpm demo:phase1
// then edit any line and save — the loop swaps at the next cycle boundary, no dropout.
//
// What it exercises:
//   • drum kit (feature 05): kick / snare / closed+open hats, each a synthesized voice
//   • oscillators + filter + ADSR (features 01–03): the saw bass is osc → SVF → amp env
//   • voice allocation (feature 06): four tracks + overlapping bass notes share the pool
//   • master limiter (feature 07): the full mix sums well below clipping

import { setcps, p1, p2, p3, p4, PRESETS, noteToHz, every, euclid, seq } from "strobe";

setcps(130 / 60 / 4); // ~130 BPM, 4 beats per cycle

// Four-on-the-floor kick.
p1(every({ s: "bd" }, 4));

// Off-beat closed hats as a 5-of-8 Euclidean groove (slightly softer).
p2(euclid({ s: "hh", vel: 0.6 }, 5, 8));

// Snare backbeat on 2 and 4.
p3(seq([null, { s: "sn" }, null, { s: "sn" }]));

// Rolling saw bassline (preset 5 = saw → low-pass → amp envelope), 8 steps per cycle.
const line = ["c2", "c2", "eb2", "g2", "c2", "bb1", "eb2", "f2"];
p4(seq(line.map((n) => ({ instrument: PRESETS.saw, freq: noteToHz(n), vel: 0.8 }))));

// Try while it plays:
//   p2( euclid({ s: "hh" }, 7, 16) );   // busier hats
//   p4( seq(line.map((n) => ({ instrument: PRESETS.square, freq: noteToHz(n) }))) );  // square bass
//   p3( seq([{ s: "cp" }, null, { s: "sn" }, { s: "oh" }]) );  // clap + open-hat variation
