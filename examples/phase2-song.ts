// Phase 2 demo — the pattern language. This is the README quickstart song, now runnable:
// mini-notation, the sound/note API, chained modifiers, and a continuous filter sweep.
//
//   pnpm demo:phase2
// then edit any line and save — the swap lands at the next cycle boundary, no dropout.
//
// What it exercises:
//   • mini-notation (feature 02): "bd*4", "~ hh ~ hh", "c2 c2 eb2 g2"
//   • sound/note + chained modifiers (feature 04): .gain/.lpf/.saw/.room
//   • continuous signals (feature 05): sine.range(...).slow(8) sweeping the filter
//   • multi-track hot-swap (feature 07): three live tracks
//
// Note: room/sidechain and the audio-rate filter ramp are accepted and compiled now, but
// only become audible with the Phase 3 effects rack; the kick + offbeat hats + saw bass
// play today.

import { setcps, p1, p2, p3, sound, note, sine } from "strobe";

setcps(130 / 60 / 4); // ~130 BPM, 4 beats per cycle

p1(sound("bd*4").gain(0.9)); // four-on-the-floor kick

p2(sound("~ hh ~ hh").lpf(6000)); // offbeat hats

p3(
  note("c2 c2 eb2 g2") // rolling bassline…
    .saw()
    .lpf(sine.range(300, 1200).slow(8)) // …with an 8-cycle filter sweep
    .room(0.3)
    .sidechain("p1"), // ducked by the kick for that techno pump (Phase 3)
);

// Try while it plays:
//   p2( sound("hh(5,8)").gain(0.6) );          // a Euclidean hat groove
//   p3( note("<c2 eb2> g2 bb1 f2").saw().lpf(800) );  // alternating bass root
//   p1( sound("bd*4, ~ cp").gain(0.9) );        // add a clap layer
