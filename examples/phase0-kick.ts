// The Phase 0 deliverable: a four-on-the-floor kick you can edit live.
//
// Run it (needs a working audio output device):
//   pnpm demo
// then edit the lines below and save — the loop swaps at the next cycle boundary
// with no click or dropout.

import { setcps, p1, bdEvery } from "strobe";

setcps(130 / 60 / 4); // ~130 BPM, 4 beats per cycle

p1(bdEvery(4)); // four-on-the-floor kick

// Try these instead and save while it plays:
//   p1( bdEvery(8) );        // double-time
//   p1( bdEuclid(3, 8) );    // a 3-of-8 Euclidean groove  (import bdEuclid too)
