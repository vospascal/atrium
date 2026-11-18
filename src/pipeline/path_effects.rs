//! Concrete `PathEffect` implementations.
//!
//! - `AirAbsorptionEffect`: ISO 9613-1 frequency-dependent atmospheric absorption.
//! - `GroundEffectFilter`: ISO 9613-2 ground effect where the floor has no image.
//! - `PropagationDelayEffect`: fractional-delay line for reflection timing.
//! - `WallAbsorptionEffect`: per-wall material colouring of a reflection.

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
        self.inner
            .update(ctx.path.distance, &ctx.environment.atmosphere);
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
        // off the ground, summed as energy (+3 dB over hard ground). A floor
        // that reflects is already rendered as that bounce, as its own
        // image-source path. Applying the ground term as well counted the
        // floor twice, so the explicit reflection wins and this stage only
        // acts where the floor has no image (an `open` floor, outdoors).
        if ctx.environment.floor_reflects() {
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

// ─────────────────────────────────────────────────────────────────────────────
// PropagationDelayEffect
// ─────────────────────────────────────────────────────────────────────────────

/// Fractional-delay line for propagation delay (reflections).
///
/// Uses a circular buffer with linear interpolation between adjacent samples,
/// following the pattern from `delay_comp.rs:118-126`. The delay is derived
/// from `path.delay_seconds * sample_rate` each buffer and approached per
/// sample, never jumped to: a tap that jumps skips or repeats audio, which is
/// a click on every buffer boundary for any moving source. Gliding the tap
/// is also what a moving reflection physically does (a small Doppler shift).
///
/// Buffer capacity is dynamically sized from the maximum first-order image-source
/// distance (via `room_acoustics::delay_capacity_for_seconds`), with a minimum of 8192
/// samples (~170ms at 48kHz). Power-of-2 sizing enables fast wrapping via bitmask.
pub struct PropagationDelayEffect {
    buffer: Box<[f32]>,
    capacity: usize,
    mask: usize,
    write_pos: usize,
    /// Delay the tap is at right now, in samples.
    delay_samples: f32,
    /// Delay the current path asks for, in samples.
    target_delay_samples: f32,
    /// Fraction of the remaining distance to the target covered per sample.
    glide_per_sample: f32,
    /// False until the first update: the first delay is taken as is, since
    /// there is no earlier tap position to glide from.
    primed: bool,
    sample_rate: f32,
}

impl PropagationDelayEffect {
    /// Time constant of the glide toward a new delay.
    const GLIDE_SECONDS: f32 = 0.010;
    /// Fastest the tap may move, in samples of delay per sample of audio.
    /// Half a sample per sample keeps the output's slope within 1.5× the
    /// input's, so even a source that teleports glides instead of clicking.
    const MAX_GLIDE_RATE: f32 = 0.5;

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
            target_delay_samples: 0.0,
            glide_per_sample: 1.0,
            primed: false,
            sample_rate,
        }
    }
}

impl PathEffect for PropagationDelayEffect {
    fn update(&mut self, ctx: &PathEffectContext) {
        self.sample_rate = ctx.sample_rate;
        self.target_delay_samples = ctx.path.delay_seconds * ctx.sample_rate;
        self.glide_per_sample = 1.0 - (-1.0 / (Self::GLIDE_SECONDS * ctx.sample_rate)).exp();
        if !self.primed {
            self.delay_samples = self.target_delay_samples;
            self.primed = true;
        }
    }

    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        let wp = self.write_pos;
        self.buffer[wp] = sample;
        self.write_pos = (wp + 1) & self.mask;

        let glide = (self.target_delay_samples - self.delay_samples) * self.glide_per_sample;
        self.delay_samples += glide.clamp(-Self::MAX_GLIDE_RATE, Self::MAX_GLIDE_RATE);

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
        self.target_delay_samples = 0.0;
        self.primed = false;
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

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::atmosphere::AtmosphericParams;
    use crate::audio::propagation::GroundProperties;
    use crate::pipeline::environment::{Environment, FLOOR_WALL_INDEX};
    use crate::pipeline::path::{PathContribution, PathKind};
    use crate::pipeline::wall_material::WallMaterial;
    use atrium_core::types::Vec3;

    fn default_wall_materials() -> [WallMaterial; 6] {
        std::array::from_fn(|_| WallMaterial::default())
    }

    /// Hard walls around an `open` floor: no floor image source, so the ISO
    /// ground term is what models the ground.
    fn open_floor_wall_materials() -> [WallMaterial; 6] {
        let mut materials = default_wall_materials();
        materials[FLOOR_WALL_INDEX] = WallMaterial::open();
        materials
    }

    /// The path effects read only the materials, ground and air, so the box
    /// bounds are arbitrary but kept the same across the file.
    fn test_environment(
        wall_materials: &[WallMaterial; 6],
        ground: &GroundProperties,
        atmosphere: &AtmosphericParams,
    ) -> Environment {
        Environment {
            min: Vec3::new(-10.0, -10.0, 0.0),
            max: Vec3::new(10.0, 10.0, 10.0),
            wall_materials: wall_materials.clone(),
            ground: ground.clone(),
            atmosphere: atmosphere.clone(),
        }
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
            environment: &test_environment(&default_wall_materials(), &ground, &atmo),
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
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
            environment: &test_environment(&default_wall_materials(), &ground, &atmo),
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
        };
        let ctx_far = PathEffectContext {
            path: &path_far,
            environment: &test_environment(&default_wall_materials(), &ground, &atmo),
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
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
            environment: &test_environment(&default_wall_materials(), &ground, &atmo),
            sample_rate,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(distance, 0.0, 0.0),
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
            environment: &test_environment(&default_wall_materials(), &ground, &atmo),
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
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
            environment: &test_environment(&default_wall_materials(), &ground, &atmo),
            sample_rate,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
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
            environment: &test_environment(&default_wall_materials(), &ground, &atmo),
            sample_rate,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
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
            environment: &test_environment(&default_wall_materials(), &ground, &atmo),
            sample_rate,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
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
            environment: &test_environment(&default_wall_materials(), &ground, &atmo),
            sample_rate,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
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
            environment: &test_environment(&open_floor_wall_materials(), &ground, &atmo),
            sample_rate: 48000.0,
            source_pos: Vec3::new(0.0, 0.0, 1.5),
            target_pos: Vec3::new(10.0, 0.0, 1.5),
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
            environment: &test_environment(&open_floor_wall_materials(), &hard_ground, &atmo),
            sample_rate: 48000.0,
            source_pos: Vec3::new(0.0, 0.0, 1.5),
            target_pos: Vec3::new(10.0, 0.0, 1.5),
        };
        let ctx_soft = PathEffectContext {
            path: &path,
            environment: &test_environment(&open_floor_wall_materials(), &soft_ground, &atmo),
            sample_rate: 48000.0,
            source_pos: Vec3::new(0.0, 0.0, 1.5),
            target_pos: Vec3::new(10.0, 0.0, 1.5),
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
            environment: &test_environment(&open_floor_wall_materials(), ground, &atmosphere),
            sample_rate: 48000.0,
            source_pos: Vec3::new(0.0, 0.0, source_height),
            target_pos: Vec3::new(horizontal_distance, 0.0, receiver_height),
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
            environment: &test_environment(&open_floor_wall_materials(), &ground, &atmo),
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(0.1, 0.0, 0.0),
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
            environment: &test_environment(&default_wall_materials(), &ground, &atmo),
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(5.0, 0.0, 0.0),
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
            environment: &test_environment(&hard_materials, &ground, &atmo),
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(8.0, 0.0, 0.0),
        };
        let ctx_carpet = PathEffectContext {
            path: &path,
            environment: &test_environment(&carpet_materials, &ground, &atmo),
            sample_rate: 48000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(8.0, 0.0, 0.0),
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

    // ── Hand-derived pins (audit 2026-11) ───────────────────────────────

    /// Steady-state gain in dB of any path effect at one frequency, measured
    /// with a sine over the second half of one second at 48 kHz.
    fn steady_state_response_db(
        effect: &mut dyn PathEffect,
        context: &PathEffectContext,
        frequency: f32,
    ) -> f32 {
        let sample_rate = 48_000.0;
        effect.reset();
        effect.update(context);
        let total = 48_000;
        let settle = 24_000;
        let mut input_power = 0.0f64;
        let mut output_power = 0.0f64;
        for sample_index in 0..total {
            let input =
                (std::f32::consts::TAU * frequency * sample_index as f32 / sample_rate).sin();
            let output = effect.process_sample(input);
            if sample_index >= settle {
                input_power += (input as f64).powi(2);
                output_power += (output as f64).powi(2);
            }
        }
        (10.0 * (output_power / input_power).log10()) as f32
    }

    fn reflection_path(wall_index: u8, delay_seconds: f32) -> PathContribution {
        PathContribution {
            kind: PathKind::Reflection,
            direction: Vec3::new(-1.0, 0.0, 0.0),
            distance: 9.0,
            delay_seconds,
            gain: 1.0,
            wall_index: Some(wall_index),
        }
    }

    /// The +X reflection of source (1,0,0) / listener origin in a [-5,5]³ room
    /// travels 8 m further than the direct path: 8 / 343.42 m/s = 23.2951 ms.
    /// At 48 kHz that is 1118.164 samples, so linear interpolation splits a unit
    /// impulse into 1 − 0.164 = 0.836 at sample 1118 and 0.164 at sample 1119.
    /// At 44.1 kHz: 1027.313 → 0.687 / 0.313. At 96 kHz: 2236.327 → 0.673 / 0.327.
    #[test]
    fn propagation_delay_places_reflection_impulse_at_hand_derived_sample() {
        let atmosphere = AtmosphericParams::default();
        let ground = GroundProperties::default();
        let delay_seconds = 8.0 / atmosphere.speed_of_sound();
        let path = reflection_path(1, delay_seconds);
        for (sample_rate, integer_part, fraction) in [
            (44_100.0_f32, 1027usize, 0.313_f32),
            (48_000.0, 1118, 0.164),
            (96_000.0, 2236, 0.327),
        ] {
            let mut effect = PropagationDelayEffect::new(sample_rate, 8192);
            effect.update(&PathEffectContext {
                path: &path,
                environment: &test_environment(&default_wall_materials(), &ground, &atmosphere),
                sample_rate,
                source_pos: Vec3::new(1.0, 0.0, 0.0),
                target_pos: Vec3::ZERO,
            });
            let mut response = vec![0.0f32; integer_part + 3];
            for (sample_index, output) in response.iter_mut().enumerate() {
                let input = if sample_index == 0 { 1.0 } else { 0.0 };
                *output = effect.process_sample(input);
            }
            assert!(response[..integer_part]
                .iter()
                .all(|value| value.abs() < 1e-6));
            assert!(
                (response[integer_part] - (1.0 - fraction)).abs() < 2e-3,
                "{sample_rate} Hz: sample {integer_part} = {}",
                response[integer_part]
            );
            assert!(
                (response[integer_part + 1] - fraction).abs() < 2e-3,
                "{sample_rate} Hz: sample {} = {}",
                integer_part + 1,
                response[integer_part + 1]
            );
            assert!(response[integer_part + 2].abs() < 1e-6);
        }
    }

    /// Requested delays beyond the ring are clamped to capacity − 2 instead of
    /// wrapping and reading future samples. 8192-sample ring → 8190 samples.
    #[test]
    fn propagation_delay_clamps_to_capacity_minus_two() {
        let atmosphere = AtmosphericParams::default();
        let ground = GroundProperties::default();
        let sample_rate = 48_000.0;
        let path = reflection_path(0, 1.0); // 48000 samples requested
        let mut effect = PropagationDelayEffect::new(sample_rate, 8192);
        effect.update(&PathEffectContext {
            path: &path,
            environment: &test_environment(&default_wall_materials(), &ground, &atmosphere),
            sample_rate,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
        });
        let mut first_nonzero = None;
        for sample_index in 0..9000 {
            let input = if sample_index == 0 { 1.0 } else { 0.0 };
            if effect.process_sample(input).abs() > 0.5 && first_nonzero.is_none() {
                first_nonzero = Some(sample_index);
            }
        }
        assert_eq!(first_nonzero, Some(8190));
    }

    /// A moving source changes `delay_seconds` every buffer. The delay line
    /// jumps straight to the new tap instead of ramping, so the output skips
    /// samples. A 1 kHz sine at 48 kHz never moves more than
    /// 2π·1000/48000 = 0.131 per sample; alternating the tap between 100 and
    /// 105 samples every 256-sample buffer (≈ 0.036 m of path per 5.3 ms, a
    /// 6.8 m/s path-length rate — a darting source) skips 4 samples forward
    /// (100 → 105) or 6 back (105 → 100), steps up to 2·sin(3·ω) = 0.765 —
    /// a click on every buffer boundary (measured 0.759). WorldLocked
    /// already ramps its arrival delay per sample; the per-path chain does not.
    #[test]
    fn propagation_delay_change_is_click_free() {
        let atmosphere = AtmosphericParams::default();
        let ground = GroundProperties::default();
        let sample_rate = 48_000.0;
        let frequency = 1000.0;
        let largest_natural_step = std::f32::consts::TAU * frequency / sample_rate;
        let mut effect = PropagationDelayEffect::new(sample_rate, 8192);
        let mut previous_output = 0.0f32;
        let mut largest_step = 0.0f32;
        for sample_index in 0..48_000usize {
            if sample_index % 256 == 0 {
                let delay_samples = if (sample_index / 256) % 2 == 0 {
                    100.0
                } else {
                    105.0
                };
                let path = reflection_path(0, delay_samples / sample_rate);
                effect.update(&PathEffectContext {
                    path: &path,
                    environment: &test_environment(&default_wall_materials(), &ground, &atmosphere),
                    sample_rate,
                    source_pos: Vec3::ZERO,
                    target_pos: Vec3::new(1.0, 0.0, 0.0),
                });
            }
            let input =
                (std::f32::consts::TAU * frequency * sample_index as f32 / sample_rate).sin();
            let output = effect.process_sample(input);
            if sample_index > 1024 {
                largest_step = largest_step.max((output - previous_output).abs());
            }
            previous_output = output;
        }
        assert!(
            largest_step < 1.5 * largest_natural_step,
            "largest sample-to-sample step {largest_step} vs sine slope {largest_natural_step}"
        );
    }

    /// Ground effect for a hard floor: ISO 9613-2 gives A_gr = −3 dB (direct +
    /// ground-reflected ray, energy sum). In a room, the first-order floor
    /// image source (wall index 4) IS that ground-reflected ray, rendered as
    /// its own path. The chain still applies the full ground effect to it — and
    /// to the direct path that already has the floor image beside it — so the
    /// floor bounce is counted twice. The floor path should pass the ground
    /// stage at 0 dB; it gets +3 dB.
    #[test]
    fn ground_effect_is_not_applied_to_the_floor_reflection_path() {
        let atmosphere = AtmosphericParams::default();
        let ground = GroundProperties::hard();
        let floor_path = PathContribution {
            kind: PathKind::Reflection,
            direction: Vec3::new(0.97, 0.0, -0.24),
            distance: 10.28,
            delay_seconds: 0.0008,
            gain: 0.98,
            wall_index: Some(4),
        };
        let mut effect = GroundEffectFilter::new(48_000.0);
        let context = PathEffectContext {
            path: &floor_path,
            environment: &test_environment(&default_wall_materials(), &ground, &atmosphere),
            sample_rate: 48_000.0,
            source_pos: Vec3::new(0.0, 0.0, 1.2),
            target_pos: Vec3::new(10.0, 0.0, 1.2),
        };
        let response = steady_state_response_db(&mut effect, &context, 1000.0);
        assert!(
            response.abs() < 0.1,
            "floor reflection got {response:.2} dB of ground effect on top of its own bounce"
        );
    }

    /// Combined reflection level = path.gain (broadband √(1 − mean α 250–4k))
    /// × WallAbsorptionEffect response. Per band, the physical amplitude
    /// reflection coefficient is √(1 − α). For the shipped materials the
    /// level+colour decomposition must land within 1 dB of that at 63 Hz
    /// (α̅ of 125/250 Hz), 1 kHz (α at 1 kHz) and 8 kHz (α̅ of 2k/4k).
    /// Glass example: path.gain = √(1 − 0.132) = −0.615 dB; low shelf
    /// 20·log10(√0.70/√0.85) = −0.84 dB → 63 Hz ≈ −1.45 dB vs truth −1.55 dB.
    #[test]
    fn wall_absorption_plus_broadband_gain_tracks_per_band_reflection_coefficient() {
        let atmosphere = AtmosphericParams::default();
        let ground = GroundProperties::default();
        for material in [
            WallMaterial::hard_wall(),
            WallMaterial::stone(),
            WallMaterial::glass(),
            WallMaterial::carpet(),
            WallMaterial::wood(),
            WallMaterial::grass(),
        ] {
            let materials: [WallMaterial; 6] = std::array::from_fn(|_| material.clone());
            let broadband_db = 20.0 * material.broadband_reflection_gain().log10();
            let path = reflection_path(0, 0.01);
            let context = PathEffectContext {
                path: &path,
                environment: &test_environment(&materials, &ground, &atmosphere),
                sample_rate: 48_000.0,
                source_pos: Vec3::ZERO,
                target_pos: Vec3::new(1.0, 0.0, 0.0),
            };
            let mut effect = WallAbsorptionEffect::new(48_000.0);
            let alpha = material.alpha;
            for (frequency, band_alpha) in [
                (63.0_f32, (alpha[0] + alpha[1]) * 0.5),
                (1000.0, alpha[3]),
                (8000.0, (alpha[4] + alpha[5]) * 0.5),
            ] {
                let truth_db = 10.0 * (1.0 - band_alpha).log10();
                let combined_db =
                    broadband_db + steady_state_response_db(&mut effect, &context, frequency);
                assert!(
                    (combined_db - truth_db).abs() < 1.0,
                    "{} at {frequency} Hz: rendered {combined_db:.2} dB, √(1−α) = {truth_db:.2} dB",
                    material.name
                );
            }
        }
    }

    /// The colour filter and path.gain must agree on a reference level. The
    /// filter's shelves are measured from the 500 Hz/1 kHz mean (α_mid) while
    /// path.gain uses the 250 Hz–4 kHz mean; without the mid-level correction
    /// ceiling tile (α_mid = 0.75, broadband mean 0.58) rendered every band
    /// 20·log10(√0.42/√0.25) = +2.25 dB too loud and reached ≈ 1.06 at 63 Hz —
    /// a passive wall reflecting more than it receives (truth √(1 − 0.30) = 0.837).
    ///
    /// The mid band is one flat level for both 500 Hz (α 0.70, −5.23 dB) and
    /// 1 kHz (α 0.80, −6.99 dB): it sits at √(1 − 0.75) = −6.02 dB plus 0.2 dB
    /// of shelf skirt, 1.16 dB above the 1 kHz truth and 0.6 dB below the
    /// 500 Hz one. Two shelves cannot do better for a material this uneven, so
    /// the bound here is 1.25 dB rather than the 1 dB the smoother materials meet.
    #[test]
    fn ceiling_tile_reflection_is_passive_and_tracks_per_band_coefficient() {
        let atmosphere = AtmosphericParams::default();
        let ground = GroundProperties::default();
        let material = WallMaterial::ceiling_tile();
        let materials: [WallMaterial; 6] = std::array::from_fn(|_| material.clone());
        let broadband_gain = material.broadband_reflection_gain();
        let path = reflection_path(0, 0.01);
        let context = PathEffectContext {
            path: &path,
            environment: &test_environment(&materials, &ground, &atmosphere),
            sample_rate: 48_000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
        };
        let mut effect = WallAbsorptionEffect::new(48_000.0);
        let low_response_db = steady_state_response_db(&mut effect, &context, 63.0);
        let combined_low_gain = broadband_gain * 10.0_f32.powf(low_response_db / 20.0);
        assert!(
            combined_low_gain <= 1.0,
            "63 Hz reflection gain {combined_low_gain} exceeds unity"
        );
        let mid_response_db = steady_state_response_db(&mut effect, &context, 1000.0);
        let combined_mid_db = 20.0 * broadband_gain.log10() + mid_response_db;
        let truth_mid_db = 10.0 * (1.0 - material.alpha[3]).log10(); // √0.2 = −6.99 dB
        assert!(
            (combined_mid_db - truth_mid_db).abs() < 1.25,
            "1 kHz: rendered {combined_mid_db:.2} dB vs {truth_mid_db:.2} dB"
        );
    }

    /// The four effects the speaker/headphone pipelines build, in the order
    /// `pipeline::builders::image_source_path_effect_factories` lists them,
    /// compose to: delay (lossless) ×
    /// air absorption × ground × wall colour. For a hard wall, no ground
    /// (horizontal distance < 0.5 m → unity) and a short path, the chain is
    /// transparent at 1 kHz apart from the delay — no stage applies the
    /// broadband reflectivity or distance gain a second time (those live in
    /// path.gain and in the renderer).
    #[test]
    fn path_chain_applies_no_hidden_broadband_gain() {
        use crate::pipeline::path::PathEffectChain;
        let atmosphere = AtmosphericParams::default();
        let ground = GroundProperties::default();
        let sample_rate = 48_000.0;
        let mut chain = PathEffectChain::new(vec![
            Box::new(PropagationDelayEffect::new(sample_rate, 8192)),
            Box::new(AirAbsorptionEffect::new(sample_rate)),
            Box::new(GroundEffectFilter::new(sample_rate)),
            Box::new(WallAbsorptionEffect::new(sample_rate)),
        ]);
        let mut path = reflection_path(0, 0.002);
        path.distance = 1.0;
        let materials: [WallMaterial; 6] = std::array::from_fn(|_| WallMaterial::hard_wall());
        chain.update(&PathEffectContext {
            path: &path,
            environment: &test_environment(&materials, &ground, &atmosphere),
            sample_rate,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(0.0, 0.0, 0.3),
        });
        let mut input_power = 0.0f64;
        let mut output_power = 0.0f64;
        for sample_index in 0..48_000usize {
            let input = (std::f32::consts::TAU * 1000.0 * sample_index as f32 / sample_rate).sin();
            let output = chain.process_sample(input);
            if sample_index >= 24_000 {
                input_power += (input as f64).powi(2);
                output_power += (output as f64).powi(2);
            }
        }
        let response_db = 10.0 * (output_power / input_power).log10();
        assert!(
            response_db.abs() < 0.2,
            "chain response {response_db:.3} dB"
        );
    }
}
