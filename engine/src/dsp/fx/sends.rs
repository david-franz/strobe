//! Global send buses — a shared effect (reverb/delay) many tracks feed in parallel. Each
//! track has a per-bus send level; the router sums all sends, processes the bus once, and
//! returns the result into the master bus. Far cheaper than a reverb per track.

use super::{Effect, FxCtx, Smoothed, MAX_BLOCK, MAX_SENDS};

pub struct SendBus {
    pub effect: Box<dyn Effect>,
    pub return_gain: Smoothed,
    in_l: Vec<f32>,
    in_r: Vec<f32>,
}

impl SendBus {
    pub fn new(effect: Box<dyn Effect>, sr: f32) -> Self {
        Self {
            effect,
            return_gain: Smoothed::new(1.0, sr),
            in_l: vec![0.0; MAX_BLOCK],
            in_r: vec![0.0; MAX_BLOCK],
        }
    }
}

pub struct SendRouter {
    pub buses: [Option<SendBus>; MAX_SENDS],
}

impl Default for SendRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl SendRouter {
    pub fn new() -> Self {
        Self {
            buses: std::array::from_fn(|_| None),
        }
    }

    pub fn set_bus(&mut self, idx: usize, bus: SendBus) {
        if idx < MAX_SENDS {
            self.buses[idx] = Some(bus);
        }
    }

    pub fn return_gain(&mut self, idx: usize) -> Option<&mut Smoothed> {
        self.buses.get_mut(idx).and_then(|b| b.as_mut()).map(|b| &mut b.return_gain)
    }

    /// Accumulate `track * sendLevel[bus]` into each bus's input.
    pub fn add_from_track(&mut self, l: &[f32], r: &[f32], send: &mut [Smoothed; MAX_SENDS]) {
        for (bus, lvl) in self.buses.iter_mut().zip(send.iter_mut()) {
            let Some(bus) = bus.as_mut() else {
                continue;
            };
            for i in 0..l.len().min(MAX_BLOCK) {
                let g = lvl.next();
                bus.in_l[i] += g * l[i];
                bus.in_r[i] += g * r[i];
            }
        }
    }

    /// Process each bus once and add its return into the master accumulator, then clear.
    pub fn process_returns(&mut self, master_l: &mut [f32], master_r: &mut [f32], ctx: &FxCtx) {
        let n = master_l.len().min(MAX_BLOCK);
        for bus in self.buses.iter_mut().flatten() {
            bus.effect.process(&mut bus.in_l[..n], &mut bus.in_r[..n], ctx);
            for i in 0..n {
                let g = bus.return_gain.next();
                master_l[i] += g * bus.in_l[i];
                master_r[i] += g * bus.in_r[i];
                bus.in_l[i] = 0.0;
                bus.in_r[i] = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::fx::sidechain::SidechainBus;

    /// A bus effect that doubles its input — lets us see the send sum.
    struct Double;
    impl Effect for Double {
        fn process(&mut self, l: &mut [f32], r: &mut [f32], _: &FxCtx) {
            for x in l.iter_mut().chain(r.iter_mut()) {
                *x *= 2.0;
            }
        }
        fn set_param(&mut self, _: u8, _: f32) {}
        fn ramp_param(&mut self, _: u8, _: f32, _: u32) {}
        fn reset(&mut self) {}
    }

    #[test]
    fn two_tracks_feed_one_bus_processed_once() {
        let sr = 48_000.0;
        let mut router = SendRouter::new();
        router.set_bus(0, SendBus::new(Box::new(Double), sr));

        let mut send_a: [Smoothed; MAX_SENDS] = std::array::from_fn(|_| Smoothed::new(0.0, sr));
        send_a[0].snap(0.5);
        let mut send_b: [Smoothed; MAX_SENDS] = std::array::from_fn(|_| Smoothed::new(0.0, sr));
        send_b[0].snap(0.5);

        let a = vec![1.0f32; 64];
        let b = vec![1.0f32; 64];
        router.add_from_track(&a, &a, &mut send_a); // 0.5 into bus
        router.add_from_track(&b, &b, &mut send_b); // +0.5 → bus input 1.0

        let sc = SidechainBus::new(sr);
        let ctx = FxCtx { sample_rate: sr, cps: 0.5, sidechain: &sc };
        let mut ml = vec![0.0f32; 64];
        let mut mr = vec![0.0f32; 64];
        router.process_returns(&mut ml, &mut mr, &ctx);
        // bus input was 1.0 (0.5+0.5), doubled → 2.0, return_gain 1.0.
        assert!((ml[10] - 2.0).abs() < 1e-5, "summed then processed once ({})", ml[10]);
    }
}
