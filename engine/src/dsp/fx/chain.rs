//! A track's ordered insert-effect chain: a fixed array of pre-allocated slots processed in
//! order (output of slot i feeds slot i+1), plus per-send levels. Wet/dry and bypass are
//! handled uniformly here (a smoothed crossfade against a retained dry copy) so individual
//! effects always render fully wet. Empty slots are skipped.

use super::{
    fx_param, fx_slot, Effect, FxCtx, Smoothed, CHAIN_SLOT, FX_BYPASS, FX_MIX, MAX_BLOCK,
    MAX_SENDS, MAX_SLOTS,
};

struct Slot {
    effect: Option<Box<dyn Effect>>,
    mix: Smoothed,   // wet/dry, 0..1
    xfade: Smoothed, // bypass crossfade, 1 = active, 0 = bypassed
}

pub struct TrackEffects {
    slots: [Slot; MAX_SLOTS],
    send: [Smoothed; MAX_SENDS],
    dry_l: Vec<f32>,
    dry_r: Vec<f32>,
}

impl TrackEffects {
    pub fn new(sr: f32) -> Self {
        Self {
            slots: std::array::from_fn(|_| Slot {
                effect: None,
                mix: Smoothed::new(1.0, sr),
                xfade: Smoothed::new(1.0, sr),
            }),
            send: std::array::from_fn(|_| Smoothed::new(0.0, sr)),
            dry_l: vec![0.0; MAX_BLOCK],
            dry_r: vec![0.0; MAX_BLOCK],
        }
    }

    /// Install a pre-built effect into a slot (done once at engine start).
    pub fn set_effect(&mut self, slot: usize, effect: Box<dyn Effect>) {
        if slot < MAX_SLOTS {
            self.slots[slot].effect = Some(effect);
        }
    }

    /// Install a pre-built effect into a slot **inactive** (mix 0) — it stays a dry passthrough
    /// until a param activates it. Used to pre-allocate the whole rack at engine start so no
    /// audio-thread allocation is ever needed.
    pub fn install(&mut self, slot: usize, effect: Box<dyn Effect>) {
        if slot < MAX_SLOTS {
            self.slots[slot].effect = Some(effect);
            self.slots[slot].mix.snap(0.0);
        }
    }

    /// Per-send levels, tapped by the SendRouter after the chain runs.
    pub fn send_levels(&mut self) -> &mut [Smoothed; MAX_SENDS] {
        &mut self.send
    }

    pub fn reset(&mut self) {
        for s in &mut self.slots {
            if let Some(fx) = s.effect.as_mut() {
                fx.reset();
            }
        }
    }

    /// Process the chain in place over one block.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], ctx: &FxCtx) {
        let n = l.len().min(MAX_BLOCK);
        let Self { slots, dry_l, dry_r, .. } = self;
        for slot in slots.iter_mut() {
            // Skip an inactive slot (mix fully 0): pure dry passthrough, so a pre-installed but
            // unused effect costs nothing.
            if slot.mix.cur() <= 1e-6 && slot.mix.target() <= 1e-6 {
                continue;
            }
            let Some(fx) = slot.effect.as_mut() else {
                continue;
            };
            dry_l[..n].copy_from_slice(&l[..n]);
            dry_r[..n].copy_from_slice(&r[..n]);
            fx.process(&mut l[..n], &mut r[..n], ctx);
            for i in 0..n {
                let mix = slot.mix.next();
                let xf = slot.xfade.next();
                let wetl = dry_l[i] + mix * (l[i] - dry_l[i]);
                let wetr = dry_r[i] + mix * (r[i] - dry_r[i]);
                l[i] = dry_l[i] + xf * (wetl - dry_l[i]);
                r[i] = dry_r[i] + xf * (wetr - dry_r[i]);
            }
        }
    }

    /// Apply a decoded TrackFx/ParamSet value (packed slot+param id).
    pub fn set_param(&mut self, param_id: u32, value: f32) {
        let slot = fx_slot(param_id);
        let p = fx_param(param_id);
        if slot == CHAIN_SLOT {
            self.apply_chain(p, value);
            return;
        }
        let s = slot as usize;
        if s >= MAX_SLOTS {
            return;
        }
        match p {
            FX_MIX => self.slots[s].mix.set_target(value),
            // value >= 0.5 bypasses the slot (xfade → 0 = dry); < 0.5 makes it active (→ 1 = wet).
            FX_BYPASS => self.slots[s].xfade.set_target(if value >= 0.5 { 0.0 } else { 1.0 }),
            _ => {
                self.activate(s);
                if let Some(fx) = self.slots[s].effect.as_mut() {
                    fx.set_param(p, value);
                }
            }
        }
    }

    /// Using an effect activates its slot: fade the wet mix up from the inactive 0 (no-op if it
    /// is already active or the user set an explicit mix).
    fn activate(&mut self, s: usize) {
        if self.slots[s].mix.target() <= 0.0 {
            self.slots[s].mix.set_target(1.0);
        }
    }

    /// Apply a decoded ParamRamp (smooth linear glide).
    pub fn ramp_param(&mut self, param_id: u32, target: f32, frames: u32) {
        let slot = fx_slot(param_id);
        let p = fx_param(param_id);
        if slot == CHAIN_SLOT {
            if p >= 0x10 && (p as usize - 0x10) < MAX_SENDS {
                self.send[p as usize - 0x10].ramp_to(target, frames);
            }
            return;
        }
        let s = slot as usize;
        if s >= MAX_SLOTS {
            return;
        }
        match p {
            FX_MIX => self.slots[s].mix.ramp_to(target, frames),
            _ => {
                self.activate(s);
                if let Some(fx) = self.slots[s].effect.as_mut() {
                    fx.ramp_param(p, target, frames);
                }
            }
        }
    }

    fn apply_chain(&mut self, p: u8, value: f32) {
        // p in 0x10.. addresses send levels (0x10 + bus).
        if p >= 0x10 && (p as usize - 0x10) < MAX_SENDS {
            self.send[p as usize - 0x10].set_target(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::fx::pack_fx;
    use crate::dsp::fx::sidechain::SidechainBus;

    /// A gain effect controlled by param 0x07.
    struct Gain {
        g: Smoothed,
    }
    impl Effect for Gain {
        fn process(&mut self, l: &mut [f32], r: &mut [f32], _: &FxCtx) {
            for i in 0..l.len() {
                let g = self.g.next();
                l[i] *= g;
                r[i] *= g;
            }
        }
        fn set_param(&mut self, p: u8, v: f32) {
            if p == 0x07 {
                self.g.set_target(v);
            }
        }
        fn ramp_param(&mut self, _: u8, _: f32, _: u32) {}
        fn reset(&mut self) {}
    }

    fn ctx<'a>(sr: f32, sc: &'a SidechainBus) -> FxCtx<'a> {
        FxCtx { sample_rate: sr, cps: 0.5, sidechain: sc }
    }

    #[test]
    fn empty_chain_is_passthrough() {
        let sr = 48_000.0;
        let mut fx = TrackEffects::new(sr);
        let sc = SidechainBus::new(sr);
        let mut l = vec![0.3f32; 64];
        let mut r = vec![0.3f32; 64];
        fx.process(&mut l, &mut r, &ctx(sr, &sc));
        assert!(l.iter().all(|&x| (x - 0.3).abs() < 1e-6));
    }

    #[test]
    fn an_effect_in_a_slot_processes_and_its_param_routes_by_slot() {
        let sr = 48_000.0;
        let mut fx = TrackEffects::new(sr);
        let mut g = Gain { g: Smoothed::new(1.0, sr) };
        g.g.snap(0.5);
        fx.set_effect(2, Box::new(g));
        let sc = SidechainBus::new(sr);

        let mut l = vec![1.0f32; 64];
        let mut r = vec![1.0f32; 64];
        fx.process(&mut l, &mut r, &ctx(sr, &sc));
        assert!((l[40] - 0.5).abs() < 1e-4, "slot 2 halved the signal");

        // a param packed for slot 2 reaches that effect.
        fx.set_param(pack_fx(2, 0x07), 0.25);
        let mut l2 = vec![1.0f32; 4096];
        let mut r2 = vec![1.0f32; 4096];
        fx.process(&mut l2, &mut r2, &ctx(sr, &sc)); // long enough for the smoother to settle
        assert!((l2[4095] - 0.25).abs() < 0.01, "param glided to 0.25");
    }

    #[test]
    fn mix_zero_is_dry() {
        let sr = 48_000.0;
        let mut fx = TrackEffects::new(sr);
        let mut g = Gain { g: Smoothed::new(0.0, sr) }; // would zero the signal
        g.g.snap(0.0);
        fx.set_effect(2, Box::new(g));
        fx.set_param(pack_fx(2, FX_MIX), 0.0); // fully dry
        // settle the mix smoother
        let sc = SidechainBus::new(sr);
        let mut l = vec![1.0f32; 8192];
        let mut r = vec![1.0f32; 8192];
        for _ in 0..3 {
            l.iter_mut().for_each(|x| *x = 1.0);
            r.iter_mut().for_each(|x| *x = 1.0);
            fx.process(&mut l[..4096], &mut r[..4096], &ctx(sr, &sc));
        }
        assert!((l[4000] - 1.0).abs() < 1e-3, "mix=0 → dry passthrough");
    }

    #[test]
    fn chain_send_level_set_via_chain_slot() {
        let sr = 48_000.0;
        let mut fx = TrackEffects::new(sr);
        fx.set_param(pack_fx(CHAIN_SLOT, 0x10), 0.7); // reverb send level
        let levels = fx.send_levels();
        let target = levels[0].target();
        levels[0].snap(target);
        assert!((levels[0].cur() - 0.7).abs() < 1e-6);
    }
}
