//! Ground effect along a path (ISO 9613-2), where the floor has no image.

use crate::audio::filters::Biquad;
use crate::audio::propagation::{ground_effect_bands, GROUND_BAND_FREQUENCIES};
use crate::pipeline::path::{PathEffect, PathEffectContext};

/// Per-path frequency-dependent ground effect (ISO 9613-2, Table 3).
///
/// Fits three stages to the octave-band attenuation from
/// [`ground_effect_bands`], which is the single source of truth shared with
/// the broadband `ground_effect_gain` used by the WorldLocked renderer:
///
/// 1. **Broadband gain** = the 2–8 kHz band value (identical in Table 3).
///    Hard ground: +3 dB (two regions × 1.5 dB of coherent reflection).
///    Porous ground: 0 dB.
/// 2. **Low shelf (~177 Hz)** = the 125 Hz band relative to the high bands.
/// 3. **Mid peak** = the 250 Hz–1 kHz ground dip relative to the line between
///    the 125 Hz and 2 kHz values, centred on the dip's log-frequency centroid.
///
/// The attenuation is a loss in dB, so every stage gain is its negative.
/// For short distances (< 0.5 m) the filter passes through unchanged.
///
/// It also passes through unchanged whenever the floor material reflects:
/// that floor is rendered as an image-source path, which is the same ground
/// bounce the ISO term stands for. Only an `open` floor leaves this stage on.
pub struct GroundEffectFilter {
    low_shelf: Biquad,
    mid_peak: Biquad,
    broadband_gain: f32,
    sample_rate: f32,
}

impl GroundEffectFilter {
    /// Corner between the 125 Hz and 250 Hz octave bands.
    const LOW_SHELF_HZ: f32 = 177.0;
    /// Roughly two octaves wide: the ISO dip spans 250 Hz–1 kHz.
    const MID_PEAK_Q: f32 = 0.7;
    /// Below this the dip is inaudible and the peak stage stays at unity.
    const MIN_MID_EXCESS_DB: f32 = 0.25;

    pub fn new(sample_rate: f32) -> Self {
        Self {
            low_shelf: Biquad::unity(),
            mid_peak: Biquad::unity(),
            broadband_gain: 1.0,
            sample_rate,
        }
    }

    fn set_unity(&mut self) {
        self.low_shelf.set_unity();
        self.mid_peak.set_unity();
        self.broadband_gain = 1.0;
    }
}

impl PathEffect for GroundEffectFilter {
    fn update(&mut self, ctx: &PathEffectContext) {
        self.sample_rate = ctx.sample_rate;

        // The ISO 9613-2 ground term is the direct ray plus the ray bounced
        // off the ground, summed as energy (+3 dB over hard ground). Where
        // that bounce is rendered as an image-source path of its own,
        // applying the ground term as well counts the floor twice, so the
        // explicit reflection wins. Everywhere else this stage is the only
        // floor there is: an `open` floor outdoors, and any source or
        // receiver standing on the floor, whose bounce has no delay and so
        // no path.
        if ctx
            .environment
            .floor_bounce_is_rendered(ctx.source_pos, ctx.target_pos)
        {
            self.set_unity();
            return;
        }

        let dx = ctx.source_pos.x - ctx.target_pos.x;
        let dy = ctx.source_pos.y - ctx.target_pos.y;
        let horizontal_dist = (dx * dx + dy * dy).sqrt();

        if horizontal_dist < 0.5 {
            // Too close — ground effect negligible.
            self.set_unity();
            return;
        }

        let bands = ground_effect_bands(
            horizontal_dist,
            ctx.source_pos.z,
            ctx.target_pos.z,
            &ctx.environment.ground,
        );
        // Band indices: 1 = 125 Hz, 2..=4 = 250 Hz–1 kHz, 5..=7 = 2–8 kHz.
        let low_db = bands[1];
        let high_db = bands[5];

        self.broadband_gain = 10.0_f32.powf(-high_db / 20.0);
        self.low_shelf
            .set_low_shelf(Self::LOW_SHELF_HZ, -(low_db - high_db), ctx.sample_rate);

        // Mid dip: how far 250 Hz–1 kHz sit above the 125 Hz → 2 kHz line.
        let span = (GROUND_BAND_FREQUENCIES[5] / GROUND_BAND_FREQUENCIES[1]).ln();
        let mut largest_excess_db = 0.0f32;
        let mut weight_sum = 0.0f32;
        let mut weighted_log_frequency = 0.0f32;
        for band in 2..=4 {
            let frequency = GROUND_BAND_FREQUENCIES[band];
            let fraction = (frequency / GROUND_BAND_FREQUENCIES[1]).ln() / span;
            let baseline_db = low_db + (high_db - low_db) * fraction;
            let excess_db = bands[band] - baseline_db;
            if excess_db.abs() > largest_excess_db.abs() {
                largest_excess_db = excess_db;
            }
            weight_sum += excess_db.abs();
            weighted_log_frequency += excess_db.abs() * frequency.ln();
        }

        if largest_excess_db.abs() < Self::MIN_MID_EXCESS_DB {
            self.mid_peak.set_unity();
        } else {
            let centre_hz = (weighted_log_frequency / weight_sum).exp();
            self.mid_peak.set_peak(
                centre_hz,
                -largest_excess_db,
                Self::MID_PEAK_Q,
                ctx.sample_rate,
            );
        }
    }

    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        let shelved = self.low_shelf.process(sample * self.broadband_gain);
        self.mid_peak.process(shelved)
    }

    fn name(&self) -> &str {
        "ground_effect"
    }

    fn reset(&mut self) {
        self.low_shelf.reset();
        self.mid_peak.reset();
    }
}
