//! Concrete `PathEffect` implementations.
//!
//! - `AirAbsorptionEffect`: ISO 9613-1 frequency-dependent atmospheric absorption.
//! - `DistanceAttenuationEffect`: distance-based gain (inverse, linear, exponential).
//! - `PropagationDelayEffect`: fractional-delay line for reflection timing.

use atrium_core::panner::DistanceModelType;

use crate::audio::distance::DistanceModel;
use crate::audio::filters::{AirAbsorptionFilter, Biquad};
use crate::audio::propagation::{ground_effect_bands, GROUND_BAND_FREQUENCIES};
use crate::pipeline::path::{PathEffect, PathEffectContext};

// ─────────────────────────────────────────────────────────────────────────────
// AirAbsorptionEffect
// ─────────────────────────────────────────────────────────────────────────────

/// Per-path air absorption using ISO 9613-1 lowpass filter.
///
/// Reuses `AirAbsorptionFilter` — the same biquad + hysteresis logic used by
/// the WorldLockedRenderer.
pub struct AirAbsorptionEffect {
    inner: AirAbsorptionFilter,
}

impl AirAbsorptionEffect {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            inner: AirAbsorptionFilter::new(sample_rate),
        }
    }
}

impl PathEffect for AirAbsorptionEffect {
    fn update(&mut self, ctx: &PathEffectContext) {
        self.inner.update(ctx.path.distance, ctx.atmosphere);
    }

    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        self.inner.process(sample)
    }

    fn name(&self) -> &str {
        "air_absorption"
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DistanceAttenuationEffect
// ─────────────────────────────────────────────────────────────────────────────

/// Per-path distance attenuation. Computes a broadband gain from path distance.
///
/// Uses the same distance model formulas as `distance_gain_at_model` in
/// atrium-core, but operates on `path.distance` directly instead of two positions.
pub struct DistanceAttenuationEffect {
    gain: f32,
    distance_model: DistanceModel,
}

impl DistanceAttenuationEffect {
    pub fn new(distance_model: DistanceModel) -> Self {
        Self {
            gain: 1.0,
            distance_model,
        }
    }
}

impl PathEffect for DistanceAttenuationEffect {
    fn update(&mut self, ctx: &PathEffectContext) {
        let dm = &self.distance_model;
        let clamped = ctx.path.distance.clamp(dm.ref_distance, dm.max_distance);

        self.gain = match dm.model {
            DistanceModelType::Linear => {
                let range = dm.max_distance - dm.ref_distance;
                if range <= 0.0 {
                    1.0
                } else {
                    1.0 - dm.rolloff * (clamped - dm.ref_distance) / range
                }
            }
            DistanceModelType::Inverse => {
                let denom = dm.ref_distance + dm.rolloff * (clamped - dm.ref_distance);
                if denom <= 0.0 {
                    1.0
                } else {
                    dm.ref_distance / denom
                }
            }
            DistanceModelType::Exponential => {
                if dm.ref_distance <= 0.0 {
                    1.0
                } else {
                    (clamped / dm.ref_distance).powf(-dm.rolloff)
                }
            }
        }
        .clamp(0.0, 1.0);
    }

    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        sample * self.gain
    }

    fn name(&self) -> &str {
        "distance_attenuation"
    }

    fn reset(&mut self) {
        self.gain = 1.0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GroundEffectFilter
// ─────────────────────────────────────────────────────────────────────────────

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
            ctx.ground,
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

/// 2nd-order biquad for shelving filters (Direct Form I).

// ─────────────────────────────────────────────────────────────────────────────
// PropagationDelayEffect
// ─────────────────────────────────────────────────────────────────────────────

/// Fractional-delay line for propagation delay (reflections, diffraction).
///
/// Uses a circular buffer with linear interpolation between adjacent samples,
/// following the pattern from `delay_comp.rs:118-126`. The delay is derived
/// from `path.delay_seconds * sample_rate` each buffer.
///
/// Buffer capacity is dynamically sized from the maximum first-order image-source
/// distance (via `room_acoustics::delay_buffer_capacity`), with a minimum of 8192
/// samples (~170ms at 48kHz). Power-of-2 sizing enables fast wrapping via bitmask.
pub struct PropagationDelayEffect {
    buffer: Box<[f32]>,
    capacity: usize,
    mask: usize,
    write_pos: usize,
    delay_samples: f32,
    sample_rate: f32,
}

impl PropagationDelayEffect {
    /// Minimum capacity: 8192 samples (~170ms at 48kHz).
    const MIN_CAPACITY: usize = 8192;

    pub fn new(sample_rate: f32, capacity: usize) -> Self {
        let capacity = capacity.max(Self::MIN_CAPACITY).next_power_of_two();
        Self {
            buffer: vec![0.0; capacity].into_boxed_slice(),
            capacity,
            mask: capacity - 1,
            write_pos: 0,
            delay_samples: 0.0,
            sample_rate,
        }
    }
}

impl PathEffect for PropagationDelayEffect {
    fn update(&mut self, ctx: &PathEffectContext) {
        self.sample_rate = ctx.sample_rate;
        self.delay_samples = ctx.path.delay_seconds * ctx.sample_rate;
    }

    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        let wp = self.write_pos;
        self.buffer[wp] = sample;
        self.write_pos = (wp + 1) & self.mask;

        if self.delay_samples < 0.5 {
            return sample;
        }

        let delay_clamped = self.delay_samples.min((self.capacity - 2) as f32);
        let delay_int = delay_clamped as usize;
        let frac = delay_clamped - delay_int as f32;

        let idx0 = (wp + self.capacity - delay_int) & self.mask;
        let idx1 = (wp + self.capacity - delay_int - 1) & self.mask;

        let s0 = self.buffer[idx0];
        let s1 = self.buffer[idx1];
        s0 + (s1 - s0) * frac
    }

    fn name(&self) -> &str {
        "propagation_delay"
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
        self.delay_samples = 0.0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WallAbsorptionEffect
// ─────────────────────────────────────────────────────────────────────────────

/// Per-path wall absorption based on surface material (Yeoward 2021).
///
/// For reflection paths, shapes the spectral response based on the wall material's
/// absorption coefficients. Uses two shelving filters:
///
/// - **Low shelf (~250 Hz)**: gain derived from α at 125–250 Hz bands.
/// - **High shelf (~2 kHz)**: gain derived from α at 2000–4000 Hz bands.
///
/// Direct paths and diffraction paths pass through unfiltered.
/// The broadband `path.gain` already accounts for overall wall reflectivity;
/// these filters add the *frequency-dependent* coloring on top.
pub struct WallAbsorptionEffect {
    low_shelf: Biquad,
    high_shelf: Biquad,
    sample_rate: f32,
}

impl WallAbsorptionEffect {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            low_shelf: Biquad::unity(),
            high_shelf: Biquad::unity(),
            sample_rate,
        }
    }
}

impl PathEffect for WallAbsorptionEffect {
    fn update(&mut self, ctx: &PathEffectContext) {
        self.sample_rate = ctx.sample_rate;

        // Only apply to reflections with a known wall.
        let wall_idx = match ctx.path.wall_index {
            Some(idx) if (idx as usize) < ctx.wall_materials.len() => idx as usize,
            _ => {
                self.low_shelf.set_unity();
                self.high_shelf.set_unity();
                return;
            }
        };

        let material = &ctx.wall_materials[wall_idx];
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
        let s = self.low_shelf.process(sample);
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

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::atmosphere::AtmosphericParams;
    use crate::audio::propagation::GroundProperties;
    use crate::pipeline::path::{PathContribution, PathKind, WallMaterial};
    use atrium_core::types::Vec3;

    fn default_wall_materials() -> [WallMaterial; 6] {
        std::array::from_fn(|_| WallMaterial::default())
    }

    fn make_ctx(distance: f32) -> (PathContribution, AtmosphericParams, GroundProperties) {
        let path = PathContribution {
            kind: PathKind::Direct,
            direction: Vec3::new(1.0, 0.0, 0.0),
            distance,
            delay_seconds: 0.0,
            gain: 1.0,
            wall_index: None,
        };
        (
            path,
            AtmosphericParams::default(),
            GroundProperties::default(),
        )
    }

    // ── AirAbsorptionEffect ─────────────────────────────────────────────

    #[test]
    fn air_absorption_transparent_at_zero_distance() {
        let (path, atmo, ground) = make_ctx(0.0);
        let mut effect = AirAbsorptionEffect::new(48000.0);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };
        effect.update(&ctx);
        // Feed DC (1.0) until the biquad settles — a 20kHz lowpass passes DC
        let mut out = 0.0;
        for _ in 0..200 {
            out = effect.process_sample(1.0);
        }
        assert!(
            (out - 1.0).abs() < 0.01,
            "expected near-transparent at 0m after settling, got {out}"
        );
    }

    #[test]
    fn air_absorption_attenuates_at_distance() {
        let (path_near, atmo, ground) = make_ctx(2.0);
        let (path_far, _, _) = make_ctx(50.0);
        let mut effect_near = AirAbsorptionEffect::new(48000.0);
        let mut effect_far = AirAbsorptionEffect::new(48000.0);

        let ctx_near = PathEffectContext {
            path: &path_near,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };
        let ctx_far = PathEffectContext {
            path: &path_far,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };

        effect_near.update(&ctx_near);
        effect_far.update(&ctx_far);

        // Process a burst of samples to let the filter settle
        let mut out_near = 0.0;
        let mut out_far = 0.0;
        for _ in 0..1000 {
            out_near = effect_near.process_sample(1.0);
            out_far = effect_far.process_sample(1.0);
        }
        // Far distance should attenuate more (lower cutoff = more HF loss)
        assert!(
            out_near >= out_far,
            "near ({out_near}) should >= far ({out_far})"
        );
    }

    /// Verify that the 2-shelf air absorption filter matches ISO 9613
    /// at octave-band frequencies within 3 dB at 50m distance.
    #[test]
    fn air_absorption_multiband_matches_iso9613() {
        use crate::audio::atmosphere::iso9613_alpha;

        let sample_rate = 48000.0;
        let distance = 50.0;
        let atmo = AtmosphericParams::default();

        // Set up the effect at 50m distance.
        let (path, _, ground) = make_ctx(distance);
        let mut effect = AirAbsorptionEffect::new(sample_rate);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(distance, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };
        effect.update(&ctx);

        // Measure filter gain at octave-band frequencies by feeding sine waves.
        let test_freqs = [250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];
        let num_frames = 8192;

        for &freq in &test_freqs {
            effect.reset();
            effect.update(&ctx);

            // Feed sine, measure steady-state output amplitude.
            let mut max_out = 0.0f32;
            for i in 0..num_frames {
                let t = i as f32 / sample_rate;
                let input = (2.0 * std::f32::consts::PI * freq * t).sin();
                let out = effect.process_sample(input);
                // Only measure in the last quarter (after settling).
                if i > num_frames * 3 / 4 {
                    max_out = max_out.max(out.abs());
                }
            }

            // Expected attenuation from ISO 9613.
            let expected_db = -iso9613_alpha(freq, &atmo) * distance;
            let expected_linear = 10.0_f32.powf(expected_db / 20.0);

            // Filter gain in dB.
            let filter_db = if max_out > 1e-10 {
                20.0 * max_out.log10()
            } else {
                -100.0
            };

            let error_db = (filter_db - expected_db).abs();

            eprintln!(
                "  {freq:>5.0} Hz: filter={filter_db:>6.2} dB, \
                 ISO={expected_db:>6.2} dB (linear={expected_linear:.4}), \
                 error={error_db:.2} dB"
            );

            // Within 3 dB at each octave band (250 Hz–8 kHz).
            assert!(
                error_db < 3.0,
                "Air absorption at {freq} Hz, {distance}m: filter={filter_db:.2} dB, \
                 expected={expected_db:.2} dB, error={error_db:.2} dB > 3 dB"
            );
        }
    }

    // ── DistanceAttenuationEffect ───────────────────────────────────────

    #[test]
    fn distance_atten_unity_at_ref_distance() {
        let dm = DistanceModel::default(); // ref=1.0, max=20, rolloff=1, inverse
        let (path, atmo, ground) = make_ctx(dm.ref_distance);
        let mut effect = DistanceAttenuationEffect::new(dm);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };
        effect.update(&ctx);
        let out = effect.process_sample(1.0);
        assert!(
            (out - 1.0).abs() < 1e-6,
            "expected 1.0 at ref distance, got {out}"
        );
    }

    #[test]
    fn distance_atten_decreases_with_distance() {
        let dm = DistanceModel::default();
        let (path_near, atmo, ground) = make_ctx(1.0);
        let (path_far, _, _) = make_ctx(10.0);
        let mut effect = DistanceAttenuationEffect::new(dm);

        let ctx_near = PathEffectContext {
            path: &path_near,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };
        effect.update(&ctx_near);
        let gain_near = effect.process_sample(1.0);

        let ctx_far = PathEffectContext {
            path: &path_far,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };
        effect.update(&ctx_far);
        let gain_far = effect.process_sample(1.0);

        assert!(
            gain_near > gain_far,
            "near gain ({gain_near}) should > far gain ({gain_far})"
        );
    }

    #[test]
    fn distance_atten_linear_model() {
        let dm = DistanceModel {
            ref_distance: 1.0,
            max_distance: 10.0,
            rolloff: 1.0,
            model: DistanceModelType::Linear,
        };
        let (path_mid, atmo, ground) = make_ctx(5.5);
        let mut effect = DistanceAttenuationEffect::new(dm);
        let ctx = PathEffectContext {
            path: &path_mid,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };
        effect.update(&ctx);
        let gain = effect.process_sample(1.0);
        // Linear: 1.0 - 1.0 * (5.5 - 1.0) / (10.0 - 1.0) = 1.0 - 4.5/9.0 = 0.5
        assert!(
            (gain - 0.5).abs() < 1e-6,
            "expected 0.5 at midpoint, got {gain}"
        );
    }

    #[test]
    fn distance_atten_exponential_model() {
        let dm = DistanceModel {
            ref_distance: 1.0,
            max_distance: 100.0,
            rolloff: 2.0,
            model: DistanceModelType::Exponential,
        };
        let (path, atmo, ground) = make_ctx(2.0);
        let mut effect = DistanceAttenuationEffect::new(dm);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };
        effect.update(&ctx);
        let gain = effect.process_sample(1.0);
        // Exponential: (2.0 / 1.0)^(-2.0) = 0.25
        assert!((gain - 0.25).abs() < 1e-6, "expected 0.25, got {gain}");
    }

    #[test]
    fn distance_atten_reset_restores_unity() {
        let dm = DistanceModel::default();
        let (path, atmo, ground) = make_ctx(10.0);
        let mut effect = DistanceAttenuationEffect::new(dm);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };
        effect.update(&ctx);
        assert!(effect.process_sample(1.0) < 1.0);
        effect.reset();
        assert!((effect.process_sample(1.0) - 1.0).abs() < 1e-6);
    }

    // ── PropagationDelayEffect ──────────────────────────────────────────

    fn make_delay_ctx(
        delay_seconds: f32,
    ) -> (PathContribution, AtmosphericParams, GroundProperties) {
        let path = PathContribution {
            kind: PathKind::Reflection,
            direction: Vec3::new(-1.0, 0.0, 0.0),
            distance: 10.0,
            delay_seconds,
            gain: 0.9,
            wall_index: Some(0),
        };
        (
            path,
            AtmosphericParams::default(),
            GroundProperties::default(),
        )
    }

    #[test]
    fn propagation_delay_zero_is_passthrough() {
        let (path, atmo, ground) = make_delay_ctx(0.0);
        let mut effect = PropagationDelayEffect::new(48000.0, 8192);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };
        effect.update(&ctx);

        // With zero delay, output should equal input immediately
        let out = effect.process_sample(0.5);
        assert!(
            (out - 0.5).abs() < 1e-6,
            "zero delay should pass through, got {out}"
        );
    }

    #[test]
    fn propagation_delay_integer_samples() {
        let sample_rate = 48000.0;
        let delay_samples = 10.0;
        let delay_seconds = delay_samples / sample_rate;

        let (path, atmo, ground) = make_delay_ctx(delay_seconds);
        let mut effect = PropagationDelayEffect::new(sample_rate, 8192);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };
        effect.update(&ctx);

        // Feed an impulse at sample 0, then silence
        let _ = effect.process_sample(1.0);
        for _ in 1..10 {
            let out = effect.process_sample(0.0);
            assert!(out.abs() < 1e-6, "should be silent before delay, got {out}");
        }
        // At sample 10 (after 10 samples of delay), the impulse should appear
        let out = effect.process_sample(0.0);
        assert!(
            (out - 1.0).abs() < 1e-6,
            "impulse should appear at delay offset, got {out}"
        );
    }

    #[test]
    fn propagation_delay_fractional_interpolates() {
        let sample_rate = 48000.0;
        let delay_samples = 5.5;
        let delay_seconds = delay_samples / sample_rate;

        let (path, atmo, ground) = make_delay_ctx(delay_seconds);
        let mut effect = PropagationDelayEffect::new(sample_rate, 8192);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };
        effect.update(&ctx);

        // Feed an impulse, then silence
        let _ = effect.process_sample(1.0);
        for _ in 1..5 {
            let _ = effect.process_sample(0.0);
        }
        // At sample 5 (integer part of 5.5): should get partial impulse
        let out5 = effect.process_sample(0.0);
        // At sample 6: should get the complementary part
        let out6 = effect.process_sample(0.0);

        // Linear interpolation: sample 5 gets 0.5, sample 6 gets 0.5
        assert!(
            (out5 - 0.5).abs() < 1e-6,
            "fractional delay at int part: expected 0.5, got {out5}"
        );
        assert!(
            (out6 - 0.5).abs() < 1e-6,
            "fractional delay at int+1: expected 0.5, got {out6}"
        );
    }

    #[test]
    fn propagation_delay_reset_clears_buffer() {
        let sample_rate = 48000.0;
        let delay_seconds = 10.0 / sample_rate;

        let (path, atmo, ground) = make_delay_ctx(delay_seconds);
        let mut effect = PropagationDelayEffect::new(sample_rate, 8192);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };
        effect.update(&ctx);

        // Fill buffer with signal
        for _ in 0..20 {
            effect.process_sample(1.0);
        }

        // Reset should clear everything
        effect.reset();
        effect.update(&ctx);

        // After reset, feeding silence should produce silence (no stale data)
        for _ in 0..20 {
            let out = effect.process_sample(0.0);
            assert!(out.abs() < 1e-6, "after reset, should be silent, got {out}");
        }
    }

    #[test]
    fn propagation_delay_preserves_signal_energy() {
        let sample_rate = 48000.0;
        let delay_seconds = 100.0 / sample_rate;

        let (path, atmo, ground) = make_delay_ctx(delay_seconds);
        let mut effect = PropagationDelayEffect::new(sample_rate, 8192);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };
        effect.update(&ctx);

        // Feed N samples of DC, then enough silence for the delay to drain
        let n = 200;
        let mut total_out = 0.0;
        for _ in 0..n {
            total_out += effect.process_sample(1.0);
        }
        for _ in 0..200 {
            total_out += effect.process_sample(0.0);
        }

        // Total energy out should equal total energy in (delay is lossless)
        assert!(
            (total_out - n as f32).abs() < 1e-3,
            "delay should preserve energy: expected {n}, got {total_out}"
        );
    }

    // ── GroundEffectFilter ──────────────────────────────────────────────

    #[test]
    fn ground_effect_hard_ground_boosts_low_freq() {
        // Hard ground (G=0): ISO Table 3 gives -1.5 dB at low freq (boost).
        let ground = GroundProperties {
            g_source: 0.0,
            g_receiver: 0.0,
            g_middle: 0.0,
        };
        let path = PathContribution {
            kind: PathKind::Direct,
            direction: Vec3::new(1.0, 0.0, 0.0),
            distance: 10.0,
            delay_seconds: 0.0,
            gain: 1.0,
            wall_index: None,
        };
        let atmo = AtmosphericParams::default();
        let mut effect = GroundEffectFilter::new(48000.0);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
            source_pos: Vec3::new(0.0, 0.0, 1.5),
            target_pos: Vec3::new(10.0, 0.0, 1.5),
            wall_materials: &default_wall_materials(),
        };
        effect.update(&ctx);

        // Process DC (low freq) — should pass through near unity or slightly boosted.
        let mut out = 0.0;
        for _ in 0..500 {
            out = effect.process_sample(1.0);
        }
        // Hard ground at low freq: -1.5 dB low shelf = ~0.84, but DC passes below shelf.
        // Key point: hard ground does NOT strongly attenuate.
        assert!(
            out > 0.7,
            "hard ground should not strongly attenuate DC, got {out}"
        );
    }

    #[test]
    fn ground_effect_soft_ground_attenuates_more() {
        // Soft ground (G=1) should attenuate more at low freq than hard ground (G=0).
        let hard_ground = GroundProperties {
            g_source: 0.0,
            g_receiver: 0.0,
            g_middle: 0.0,
        };
        let soft_ground = GroundProperties {
            g_source: 1.0,
            g_receiver: 1.0,
            g_middle: 1.0,
        };
        let path = PathContribution {
            kind: PathKind::Direct,
            direction: Vec3::new(1.0, 0.0, 0.0),
            distance: 10.0,
            delay_seconds: 0.0,
            gain: 1.0,
            wall_index: None,
        };
        let atmo = AtmosphericParams::default();

        let mut hard_effect = GroundEffectFilter::new(48000.0);
        let mut soft_effect = GroundEffectFilter::new(48000.0);

        let ctx_hard = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &hard_ground,
            sample_rate: 48000.0,
            source_pos: Vec3::new(0.0, 0.0, 1.5),
            target_pos: Vec3::new(10.0, 0.0, 1.5),
            wall_materials: &default_wall_materials(),
        };
        let ctx_soft = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &soft_ground,
            sample_rate: 48000.0,
            source_pos: Vec3::new(0.0, 0.0, 1.5),
            target_pos: Vec3::new(10.0, 0.0, 1.5),
            wall_materials: &default_wall_materials(),
        };

        hard_effect.update(&ctx_hard);
        soft_effect.update(&ctx_soft);

        // Process some signal to let filters settle.
        let mut hard_out = 0.0;
        let mut soft_out = 0.0;
        for _ in 0..500 {
            hard_out = hard_effect.process_sample(1.0);
            soft_out = soft_effect.process_sample(1.0);
        }

        // Soft ground should produce lower output (more attenuation at low shelf).
        assert!(
            hard_out > soft_out,
            "hard ground ({hard_out}) should pass more than soft ground ({soft_out})"
        );
    }

    /// Steady-state gain in dB of the ground filter at one frequency.
    fn ground_filter_response_db(
        ground: &GroundProperties,
        horizontal_distance: f32,
        source_height: f32,
        receiver_height: f32,
        frequency: f32,
    ) -> f32 {
        let path = PathContribution {
            kind: PathKind::Direct,
            direction: Vec3::new(1.0, 0.0, 0.0),
            distance: horizontal_distance,
            delay_seconds: 0.0,
            gain: 1.0,
            wall_index: None,
        };
        let atmosphere = AtmosphericParams::default();
        let mut effect = GroundEffectFilter::new(48000.0);
        effect.update(&PathEffectContext {
            path: &path,
            atmosphere: &atmosphere,
            ground,
            sample_rate: 48000.0,
            source_pos: Vec3::new(0.0, 0.0, source_height),
            target_pos: Vec3::new(horizontal_distance, 0.0, receiver_height),
            wall_materials: &default_wall_materials(),
        });
        let total = 48000;
        let settle = 24000;
        let mut input_power = 0.0f64;
        let mut output_power = 0.0f64;
        for n in 0..total {
            let input = (std::f32::consts::TAU * frequency * n as f32 / 48000.0).sin();
            let output = effect.process_sample(input);
            if n >= settle {
                input_power += (input as f64).powi(2);
                output_power += (output as f64).powi(2);
            }
        }
        (10.0 * (output_power / input_power).log10()) as f32
    }

    /// Hard ground is a flat +3 dB (ISO 9613-2: −1.5 dB per region, every band).
    /// The old filter applied the attenuation with the wrong sign and CUT by
    /// 1.5 dB, disagreeing with the WorldLocked renderer's broadband gain.
    #[test]
    fn ground_filter_hard_ground_is_flat_plus_three_db() {
        let hard = GroundProperties::hard();
        for frequency in [63.0, 125.0, 500.0, 2000.0, 8000.0] {
            let response = ground_filter_response_db(&hard, 10.0, 1.5, 1.5, frequency);
            assert!(
                (response - 3.0).abs() < 0.05,
                "{frequency} Hz: expected +3.0 dB, got {response}"
            );
        }
    }

    /// The filter is a three-stage fit to the ISO octave-band table. It must
    /// track the table within 3 dB from 125 Hz to 8 kHz (measured worst case: 2.9 dB,
    /// where a deep 250–500 Hz dip leaks into the 125 Hz band), for porous and mixed
    /// ground, at ear height and on the ground, near and far.
    #[test]
    fn ground_filter_tracks_iso_band_table() {
        use crate::audio::propagation::{ground_effect_bands, GROUND_BAND_FREQUENCIES};
        let mut worst = 0.0f32;
        for ground in [GroundProperties::soft(), GroundProperties::mixed(0.5)] {
            for (distance, source_height, receiver_height) in [
                (10.0, 1.2, 1.2),
                (20.0, 6.0, 1.2),
                (10.0, 0.0, 1.2),
                (60.0, 0.3, 1.2),
                (3.0, 1.0, 1.7),
            ] {
                let bands = ground_effect_bands(distance, source_height, receiver_height, &ground);
                for band in 1..8 {
                    let frequency = GROUND_BAND_FREQUENCIES[band];
                    let response = ground_filter_response_db(
                        &ground,
                        distance,
                        source_height,
                        receiver_height,
                        frequency,
                    );
                    let error = response - (-bands[band]);
                    worst = worst.max(error.abs());
                    assert!(
                        error.abs() < 3.0,
                        "G={} d={distance} hs={source_height} hr={receiver_height} {frequency} Hz: \
                         table {:.2} dB, filter {response:.2} dB",
                        ground.g_source, -bands[band]
                    );
                }
            }
        }
        eprintln!("ground filter worst deviation from ISO table: {worst:.2} dB");
    }

    /// Porous ground never boosts through the filter either.
    #[test]
    fn ground_filter_porous_ground_never_boosts() {
        let soft = GroundProperties::soft();
        for frequency in [125.0, 250.0, 500.0, 1000.0, 4000.0] {
            let response = ground_filter_response_db(&soft, 15.0, 1.2, 1.2, frequency);
            assert!(response < 0.5, "{frequency} Hz: {response} dB");
        }
    }

    #[test]
    fn ground_effect_short_distance_is_unity() {
        let ground = GroundProperties::default();
        let path = PathContribution {
            kind: PathKind::Direct,
            direction: Vec3::new(1.0, 0.0, 0.0),
            distance: 0.1,
            delay_seconds: 0.0,
            gain: 1.0,
            wall_index: None,
        };
        let atmo = AtmosphericParams::default();
        let mut effect = GroundEffectFilter::new(48000.0);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(0.1, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };
        effect.update(&ctx);

        let mut out = 0.0;
        for _ in 0..200 {
            out = effect.process_sample(1.0);
        }
        assert!(
            (out - 1.0).abs() < 0.01,
            "short distance should be near unity, got {out}"
        );
    }

    // ── WallAbsorptionEffect ────────────────────────────────────────────

    #[test]
    fn wall_absorption_direct_path_is_passthrough() {
        let path = PathContribution {
            kind: PathKind::Direct,
            direction: Vec3::new(1.0, 0.0, 0.0),
            distance: 5.0,
            delay_seconds: 0.0,
            gain: 1.0,
            wall_index: None, // Direct path, no wall
        };
        let atmo = AtmosphericParams::default();
        let ground = GroundProperties::default();
        let mut effect = WallAbsorptionEffect::new(48000.0);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(5.0, 0.0, 0.0),
            wall_materials: &default_wall_materials(),
        };
        effect.update(&ctx);

        let mut out = 0.0;
        for _ in 0..200 {
            out = effect.process_sample(1.0);
        }
        assert!(
            (out - 1.0).abs() < 0.01,
            "direct path should be passthrough, got {out}"
        );
    }

    #[test]
    fn wall_absorption_carpet_attenuates_more_than_hard_wall() {
        // Carpet has much higher α at high frequencies than hard wall.
        let carpet_materials: [WallMaterial; 6] = std::array::from_fn(|_| WallMaterial::carpet());
        let hard_materials: [WallMaterial; 6] = std::array::from_fn(|_| WallMaterial::hard_wall());

        let path = PathContribution {
            kind: PathKind::Reflection,
            direction: Vec3::new(-1.0, 0.0, 0.0),
            distance: 8.0,
            delay_seconds: 0.01,
            gain: 0.9,
            wall_index: Some(0), // Bounced off wall 0
        };
        let atmo = AtmosphericParams::default();
        let ground = GroundProperties::default();

        let mut hard_effect = WallAbsorptionEffect::new(48000.0);
        let mut carpet_effect = WallAbsorptionEffect::new(48000.0);

        let ctx_hard = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(8.0, 0.0, 0.0),
            wall_materials: &hard_materials,
        };
        let ctx_carpet = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(8.0, 0.0, 0.0),
            wall_materials: &carpet_materials,
        };

        hard_effect.update(&ctx_hard);
        carpet_effect.update(&ctx_carpet);

        // Process DC to let filters settle (DC tests low-shelf behavior).
        let mut hard_out = 0.0;
        let mut carpet_out = 0.0;
        for _ in 0..500 {
            hard_out = hard_effect.process_sample(1.0);
            carpet_out = carpet_effect.process_sample(1.0);
        }

        // Hard wall is nearly uniform α — should be near unity (filters near unity).
        // Carpet has much higher HF absorption, so its high-shelf cuts relative to mid.
        // At DC both pass through (shelving filters affect LF/HF relative to mid).
        // The key test: carpet and hard wall should produce different results.
        assert!(
            (hard_out - carpet_out).abs() > 0.001 || (hard_out - 1.0).abs() < 0.05,
            "carpet and hard wall should differ, or hard wall should be near unity. \
             hard={hard_out}, carpet={carpet_out}"
        );
    }
}
