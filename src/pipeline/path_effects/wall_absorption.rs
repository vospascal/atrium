//! Wall material colouring of a reflection path.

use crate::audio::filters::Biquad;
use crate::pipeline::path::{PathEffect, PathEffectContext};

/// Per-path wall absorption based on surface material (Yeoward 2021).
///
/// For reflection paths, shapes the spectral response based on the wall material's
/// absorption coefficients. Uses two shelving filters:
///
/// - **Low shelf (~250 Hz)**: gain derived from α at 125–250 Hz bands.
/// - **High shelf (~2 kHz)**: gain derived from α at 2000–4000 Hz bands.
///
/// Direct paths pass through unfiltered.
/// The broadband `path.gain` already accounts for overall wall reflectivity;
/// these filters add the *frequency-dependent* coloring on top.
///
/// `path.gain` is the mean over 250 Hz–4 kHz, while the shelves here are
/// measured from the 500 Hz–1 kHz mid band. Those two references differ for
/// materials that absorb unevenly, so `mid_level` moves the mid band from the
/// broadband level to its own coefficient. Without it every band inherits the
/// offset: ceiling tile came out 2.25 dB too loud, and above unity at 63 Hz.
pub struct WallAbsorptionEffect {
    low_shelf: Biquad,
    high_shelf: Biquad,
    /// Mid-band reflection gain relative to the broadband gain in `path.gain`.
    mid_level: f32,
    sample_rate: f32,
}

impl WallAbsorptionEffect {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            low_shelf: Biquad::unity(),
            high_shelf: Biquad::unity(),
            mid_level: 1.0,
            sample_rate,
        }
    }
}

impl PathEffect for WallAbsorptionEffect {
    fn update(&mut self, ctx: &PathEffectContext) {
        self.sample_rate = ctx.sample_rate;

        // Only apply to reflections with a known wall.
        let wall_idx = match ctx.path.wall_index {
            Some(idx) if (idx as usize) < ctx.environment.wall_materials.len() => idx as usize,
            _ => {
                self.low_shelf.set_unity();
                self.high_shelf.set_unity();
                self.mid_level = 1.0;
                return;
            }
        };

        let material = &ctx.environment.wall_materials[wall_idx];
        // α[0..6] at [125, 250, 500, 1000, 2000, 4000] Hz.
        // Average low-freq bands (125, 250 Hz) and high-freq bands (2000, 4000 Hz).
        let alpha_low = (material.alpha[0] + material.alpha[1]) * 0.5;
        let alpha_high = (material.alpha[4] + material.alpha[5]) * 0.5;

        // Convert absorption coefficient to reflection gain in dB.
        // Reflection coefficient r = sqrt(1 - α), gain_db = 20·log10(r).
        // We express this *relative* to the broadband gain already in path.gain,
        // using the mid-band average as the reference.
        let alpha_mid = (material.alpha[2] + material.alpha[3]) * 0.5;
        let r_mid = (1.0 - alpha_mid.clamp(0.0, 0.99)).sqrt();
        let r_low = (1.0 - alpha_low.clamp(0.0, 0.99)).sqrt();
        let r_high = (1.0 - alpha_high.clamp(0.0, 0.99)).sqrt();

        let broadband_gain = material.broadband_reflection_gain();
        self.mid_level = if broadband_gain > 1e-6 {
            r_mid / broadband_gain
        } else {
            1.0
        };

        // Relative gain in dB (positive = boost, negative = cut relative to mid).
        let low_db = if r_mid > 1e-6 {
            20.0 * (r_low / r_mid).log10()
        } else {
            0.0
        };
        let high_db = if r_mid > 1e-6 {
            20.0 * (r_high / r_mid).log10()
        } else {
            0.0
        };

        // Only set filters if the difference is audible (> 0.5 dB).
        if low_db.abs() > 0.5 {
            self.low_shelf.set_low_shelf(250.0, low_db, ctx.sample_rate);
        } else {
            self.low_shelf.set_unity();
        }
        if high_db.abs() > 0.5 {
            self.high_shelf
                .set_high_shelf(2000.0, high_db, ctx.sample_rate);
        } else {
            self.high_shelf.set_unity();
        }
    }

    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        let s = self.low_shelf.process(sample * self.mid_level);
        self.high_shelf.process(s)
    }

    fn name(&self) -> &str {
        "wall_absorption"
    }

    fn reset(&mut self) {
        self.low_shelf.reset();
        self.high_shelf.reset();
    }
}
