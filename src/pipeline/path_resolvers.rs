//! Concrete `PathResolver` implementations.
//!
//! - `DirectPathResolver`: always returns 1 direct line-of-sight path.
//! - `ImageSourceResolver`: 1 direct + up to 6 first-order wall reflections.

use crate::pipeline::environment::WALL_COUNT;
use atrium_core::types::Vec3;

use crate::pipeline::path::{PathContribution, PathKind, PathResolver, PathSet, ResolveContext};

// ─────────────────────────────────────────────────────────────────────────────
// DirectPathResolver
// ─────────────────────────────────────────────────────────────────────────────

/// Returns a single direct (line-of-sight) path from source to target.
///
/// Direction points from target toward source (the apparent arrival direction).
/// Gain is always 1.0, delay is always 0.
pub struct DirectPathResolver;

impl PathResolver for DirectPathResolver {
    fn resolve(&self, ctx: &ResolveContext<'_>, out: &mut PathSet) {
        let diff = ctx.source_pos - ctx.target_pos;
        let distance = diff.length();
        let direction = if distance > 1e-10 {
            diff * (1.0 / distance)
        } else {
            Vec3::new(1.0, 0.0, 0.0)
        };

        out.push(PathContribution {
            kind: PathKind::Direct,
            direction,
            distance,
            delay_seconds: 0.0,
            gain: 1.0,
            wall_index: None,
        });
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ImageSourceResolver
// ─────────────────────────────────────────────────────────────────────────────

/// Returns 1 direct path + up to 6 first-order reflections (image-source method).
///
/// For a rectangular room with 6 walls, each reflection is computed by mirroring
/// the source position across the wall plane. Each reflection path carries:
/// - **direction**: unit vector from target toward the image source (for panning)
/// - **distance**: total path length from source to wall to target
/// - **delay**: propagation delay relative to the direct path
/// - **gain**: per-wall broadband amplitude reflection gain (distance applied by renderer)
///
/// Reflections are skipped when the image distance is shorter than the direct
/// path (source between wall and target) or when the delay would be zero.
pub struct ImageSourceResolver;

impl PathResolver for ImageSourceResolver {
    fn resolve(&self, ctx: &ResolveContext<'_>, out: &mut PathSet) {
        let diff = ctx.source_pos - ctx.target_pos;
        let direct_dist = diff.length();
        let direct_dir = if direct_dist > 1e-10 {
            diff * (1.0 / direct_dist)
        } else {
            Vec3::new(1.0, 0.0, 0.0)
        };

        // Direct path (always first)
        out.push(PathContribution {
            kind: PathKind::Direct,
            direction: direct_dir,
            distance: direct_dist,
            delay_seconds: 0.0,
            gain: 1.0,
            wall_index: None,
        });

        // One reflection per face, where that face has one to give.
        for wall_idx in 0..WALL_COUNT {
            let Some(reflection) =
                ctx.environment
                    .reflection_off(wall_idx, ctx.source_pos, ctx.target_pos)
            else {
                continue;
            };
            let image_dist = reflection.distance;
            let delay_seconds = reflection.delay_seconds;
            let image_diff = reflection.image - ctx.target_pos;

            let direction = image_diff * (1.0 / image_dist);
            // Per-wall broadband amplitude reflection gain, from the face's material.
            // Distance attenuation is applied by the renderer using the same
            // distance model as the direct path, keeping the gain staging consistent.
            let gain = ctx.environment.wall_reflection_gain(wall_idx);

            out.push(PathContribution {
                kind: PathKind::Reflection,
                direction,
                distance: image_dist,
                delay_seconds,
                gain,
                wall_index: Some(wall_idx as u8),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::atmosphere::AtmosphericParams;
    use crate::pipeline::environment::Environment;
    use crate::pipeline::wall_material::WallMaterial;

    /// Standard speed of sound at 20°C for test assertions.
    const TEST_SPEED: f32 = 343.42; // speed_of_sound(20.0) = 331.3 + 0.606*20

    #[test]
    fn direct_resolver_produces_one_path() {
        let resolver = DirectPathResolver;
        let environment =
            Environment::anechoic(Vec3::new(-10.0, -10.0, -10.0), Vec3::new(10.0, 10.0, 10.0));
        let ctx = ResolveContext {
            source_pos: Vec3::new(3.0, 0.0, 0.0),
            target_pos: Vec3::new(0.0, 0.0, 0.0),
            environment: &environment,
        };
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        assert_eq!(paths.len(), 1);
        let p = &paths.as_slice()[0];
        assert_eq!(p.kind, PathKind::Direct);
        assert!((p.distance - 3.0).abs() < 1e-6);
        assert!((p.delay_seconds).abs() < 1e-10);
        assert!((p.gain - 1.0).abs() < 1e-6);
    }

    #[test]
    fn direct_resolver_direction_points_from_target_to_source() {
        let resolver = DirectPathResolver;
        let environment = Environment::anechoic(Vec3::ZERO, Vec3::ZERO);
        let ctx = ResolveContext {
            source_pos: Vec3::new(0.0, 5.0, 0.0),
            target_pos: Vec3::new(0.0, 0.0, 0.0),
            environment: &environment,
        };
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        let dir = paths.as_slice()[0].direction;
        // Source is +Y from target, so direction should be (0, 1, 0)
        assert!((dir.x).abs() < 1e-6);
        assert!((dir.y - 1.0).abs() < 1e-6);
        assert!((dir.z).abs() < 1e-6);
    }

    #[test]
    fn direct_resolver_coincident_positions() {
        let resolver = DirectPathResolver;
        let environment = Environment::anechoic(Vec3::ZERO, Vec3::ZERO);
        let ctx = ResolveContext {
            source_pos: Vec3::new(1.0, 2.0, 3.0),
            target_pos: Vec3::new(1.0, 2.0, 3.0),
            environment: &environment,
        };
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        let p = &paths.as_slice()[0];
        assert!(p.distance < 1e-6);
        // Fallback direction should be valid (not NaN)
        assert!(p.direction.x.is_finite());
    }

    #[test]
    fn direct_resolver_3d_distance() {
        let resolver = DirectPathResolver;
        let environment = Environment::anechoic(Vec3::ZERO, Vec3::ZERO);
        let ctx = ResolveContext {
            source_pos: Vec3::new(3.0, 4.0, 0.0),
            target_pos: Vec3::new(0.0, 0.0, 0.0),
            environment: &environment,
        };
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        assert!((paths.as_slice()[0].distance - 5.0).abs() < 1e-6);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // ImageSourceResolver tests
    // ─────────────────────────────────────────────────────────────────────────

    const TEST_ATMOSPHERE: AtmosphericParams = AtmosphericParams {
        temperature_c: 20.0,
        humidity_pct: 50.0,
        pressure_kpa: 101.325,
    };

    /// The [-5, 5]³ room used by most tests below: every face has the given
    /// energy reflectivity, and the air is `TEST_ATMOSPHERE`.
    fn make_room(reflectivity: f32) -> Environment {
        let mut environment = Environment::uniform(
            Vec3::new(-5.0, -5.0, -5.0),
            Vec3::new(5.0, 5.0, 5.0),
            WallMaterial::with_reflectivity(reflectivity),
        );
        environment.atmosphere = TEST_ATMOSPHERE;
        environment
    }

    fn make_room_ctx(environment: &Environment, source: Vec3, target: Vec3) -> ResolveContext<'_> {
        ResolveContext {
            source_pos: source,
            target_pos: target,
            environment,
        }
    }

    #[test]
    fn image_source_produces_direct_plus_reflections() {
        let resolver = ImageSourceResolver;
        let environment = make_room(0.9);
        let ctx = make_room_ctx(
            &environment,
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
        );
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        // Direct path is always first
        assert!(paths.len() >= 2);
        assert_eq!(paths.as_slice()[0].kind, PathKind::Direct);

        // All non-direct paths are reflections
        for p in &paths.as_slice()[1..] {
            assert_eq!(p.kind, PathKind::Reflection);
        }
    }

    #[test]
    fn image_source_direct_path_matches_direct_resolver() {
        let image_resolver = ImageSourceResolver;
        let direct_resolver = DirectPathResolver;
        let environment = make_room(0.9);
        let ctx = make_room_ctx(
            &environment,
            Vec3::new(3.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
        );

        let mut image_paths = PathSet::new();
        image_resolver.resolve(&ctx, &mut image_paths);

        let mut direct_paths = PathSet::new();
        direct_resolver.resolve(&ctx, &mut direct_paths);

        let ip = &image_paths.as_slice()[0];
        let dp = &direct_paths.as_slice()[0];

        assert!((ip.distance - dp.distance).abs() < 1e-6);
        assert!((ip.direction.x - dp.direction.x).abs() < 1e-6);
        assert!((ip.direction.y - dp.direction.y).abs() < 1e-6);
        assert!((ip.direction.z - dp.direction.z).abs() < 1e-6);
        assert!((ip.gain - 1.0).abs() < 1e-6);
        assert!(ip.delay_seconds.abs() < 1e-10);
    }

    #[test]
    fn image_source_reflections_are_farther_than_direct() {
        let resolver = ImageSourceResolver;
        let environment = make_room(0.9);
        let ctx = make_room_ctx(
            &environment,
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
        );
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        let direct_dist = paths.as_slice()[0].distance;
        for p in &paths.as_slice()[1..] {
            assert!(
                p.distance > direct_dist,
                "reflection distance {} should exceed direct {}",
                p.distance,
                direct_dist
            );
        }
    }

    #[test]
    fn image_source_reflections_have_positive_delay() {
        let resolver = ImageSourceResolver;
        let environment = make_room(0.9);
        let ctx = make_room_ctx(
            &environment,
            Vec3::new(2.0, 1.0, 0.5),
            Vec3::new(0.0, 0.0, 0.0),
        );
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        for p in &paths.as_slice()[1..] {
            assert!(
                p.delay_seconds > 0.0,
                "reflection delay should be positive, got {}",
                p.delay_seconds
            );
        }
    }

    #[test]
    fn image_source_delay_equals_distance_difference_over_speed() {
        let resolver = ImageSourceResolver;
        let environment = make_room(0.9);
        let ctx = make_room_ctx(
            &environment,
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
        );
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        let direct_dist = paths.as_slice()[0].distance;
        for p in &paths.as_slice()[1..] {
            let expected_delay = (p.distance - direct_dist) / TEST_SPEED;
            assert!(
                (p.delay_seconds - expected_delay).abs() < 1e-6,
                "delay {} should equal (dist - direct) / c = {}",
                p.delay_seconds,
                expected_delay
            );
        }
    }

    #[test]
    fn image_source_gain_uses_reflectivity_only() {
        let reflectivity = 0.8;
        let resolver = ImageSourceResolver;
        let environment = make_room(reflectivity);
        let source = Vec3::new(2.0, 0.0, 0.0);
        let target = Vec3::new(0.0, 0.0, 0.0);
        let ctx = make_room_ctx(&environment, source, target);
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        // wall_reflectivity is energy-domain, so amplitude = √reflectivity
        let expected = reflectivity.sqrt();
        for p in &paths.as_slice()[1..] {
            assert!(
                (p.gain - expected).abs() < 1e-6,
                "gain {} should equal √refl = {}",
                p.gain,
                expected
            );
        }
    }

    #[test]
    fn image_source_reflectivity_scales_gain() {
        let source = Vec3::new(2.0, 0.0, 0.0);
        let target = Vec3::new(0.0, 0.0, 0.0);
        let environment_high = make_room(0.9);
        let environment_low = make_room(0.4);
        let ctx_high = make_room_ctx(&environment_high, source, target);
        let ctx_low = make_room_ctx(&environment_low, source, target);

        let resolver = ImageSourceResolver;

        let mut paths_high = PathSet::new();
        let mut paths_low = PathSet::new();
        resolver.resolve(&ctx_high, &mut paths_high);
        resolver.resolve(&ctx_low, &mut paths_low);

        assert_eq!(paths_high.len(), paths_low.len());

        // Higher reflectivity → higher gain (√0.9 > √0.4)
        for (high, low) in paths_high.as_slice()[1..]
            .iter()
            .zip(paths_low.as_slice()[1..].iter())
        {
            assert!(
                high.gain > low.gain,
                "higher reflectivity should give higher gain: {} vs {}",
                high.gain,
                low.gain
            );
        }

        // Direct path unaffected
        assert_eq!(paths_high.as_slice()[0].gain, 1.0);
        assert_eq!(paths_low.as_slice()[0].gain, 1.0);
    }

    #[test]
    fn image_source_symmetric_room_centered_source_produces_6_reflections() {
        // Source and target at center — all 6 walls equidistant, all reflections valid
        let resolver = ImageSourceResolver;
        let environment = Environment::uniform(
            Vec3::new(-10.0, -10.0, -10.0),
            Vec3::new(10.0, 10.0, 10.0),
            WallMaterial::with_reflectivity(0.9),
        );
        let ctx = ResolveContext {
            source_pos: Vec3::new(1.0, 0.0, 0.0),
            target_pos: Vec3::new(0.0, 0.0, 0.0),
            environment: &environment,
        };
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        // 1 direct + 6 reflections
        assert_eq!(paths.len(), 7);
    }

    #[test]
    fn image_source_skips_reflections_closer_than_direct() {
        // Source very close to +X wall — its -X image is farther than source,
        // but its +X image is very close to target
        let resolver = ImageSourceResolver;
        let environment = Environment::uniform(
            Vec3::new(-5.0, -5.0, -5.0),
            Vec3::new(5.0, 5.0, 5.0),
            WallMaterial::with_reflectivity(0.9),
        );
        let ctx = ResolveContext {
            source_pos: Vec3::new(4.9, 0.0, 0.0),
            target_pos: Vec3::new(0.0, 0.0, 0.0),
            environment: &environment,
        };
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        // +X wall image: mirror of (4.9,0,0) across x=5 => (5.1,0,0)
        // distance to origin = 5.1, direct = 4.9. 5.1 > 4.9, so this IS included.
        // All reflections should still be farther than direct.
        let direct_dist = paths.as_slice()[0].distance;
        for p in &paths.as_slice()[1..] {
            assert!(p.distance >= direct_dist);
        }
    }

    #[test]
    fn image_source_direction_points_toward_image() {
        let resolver = ImageSourceResolver;
        let environment = make_room(0.9);
        // Source at (2,0,0), target at origin, room [-5,5]^3
        // -X wall image: (2*(-5) - 2, 0, 0) = (-12, 0, 0)
        // Direction from target toward image: (-1, 0, 0)
        let ctx = make_room_ctx(
            &environment,
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
        );
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        // Find the reflection whose direction is approximately (-1, 0, 0)
        let neg_x_reflection = paths.as_slice()[1..]
            .iter()
            .find(|p| p.direction.x < -0.9 && p.direction.y.abs() < 0.1);
        assert!(
            neg_x_reflection.is_some(),
            "should have a reflection from -X wall direction"
        );
    }

    #[test]
    fn image_source_coincident_positions_still_works() {
        let resolver = ImageSourceResolver;
        let environment = Environment::uniform(
            Vec3::new(-5.0, -5.0, -5.0),
            Vec3::new(5.0, 5.0, 5.0),
            WallMaterial::with_reflectivity(0.9),
        );
        let ctx = ResolveContext {
            source_pos: Vec3::new(0.0, 0.0, 0.0),
            target_pos: Vec3::new(0.0, 0.0, 0.0),
            environment: &environment,
        };
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        // Direct path should exist (with fallback direction)
        assert!(!paths.is_empty());
        assert_eq!(paths.as_slice()[0].kind, PathKind::Direct);
        assert!(paths.as_slice()[0].direction.x.is_finite());

        // Reflections should have valid values (no NaN/Inf)
        for p in paths.as_slice() {
            assert!(p.gain.is_finite());
            assert!(p.distance.is_finite());
            assert!(p.delay_seconds.is_finite());
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Per-wall material tests (Phase 3A)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn carpeted_wall_reflection_has_lower_gain_than_hard_walls() {
        // One carpeted floor (wall index 4 = -Z) + 5 hard walls.
        // Carpet absorbs more → lower broadband reflection gain for that wall.
        let mut materials = std::array::from_fn(|_| WallMaterial::hard_wall());
        materials[4] = WallMaterial::carpet(); // floor = -Z wall

        let resolver = ImageSourceResolver;
        let mut environment = make_room(0.0);
        environment.wall_materials = materials;

        let source = Vec3::new(1.0, 1.0, 2.0);
        let target = Vec3::new(0.0, 0.0, 0.0);
        let ctx = make_room_ctx(&environment, source, target);
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        // Collect reflection gains (skip direct path at index 0).
        let gains: Vec<f32> = paths.as_slice()[1..].iter().map(|p| p.gain).collect();
        assert!(gains.len() >= 6, "should have 6 reflections");

        // Hard wall gain = √(1 - broadband_α_hard) > carpet gain = √(1 - broadband_α_carpet)
        let hard_gain = WallMaterial::hard_wall().broadband_reflection_gain();
        let carpet_gain = WallMaterial::carpet().broadband_reflection_gain();
        assert!(
            carpet_gain < hard_gain,
            "carpet gain ({carpet_gain}) should be lower than hard wall ({hard_gain})"
        );

        // At least one reflection should have the lower (carpet) gain.
        let has_carpet = gains.iter().any(|&g| (g - carpet_gain).abs() < 1e-6);
        let has_hard = gains.iter().any(|&g| (g - hard_gain).abs() < 1e-6);
        assert!(
            has_carpet,
            "should have a reflection with carpet gain {carpet_gain}, got {gains:?}"
        );
        assert!(
            has_hard,
            "should have a reflection with hard wall gain {hard_gain}, got {gains:?}"
        );
    }

    #[test]
    fn wall_gains_come_from_each_faces_material() {
        // Each reflection's gain is its own face's broadband_reflection_gain:
        // √(1 - avg(α[1..5])).
        let materials = [
            WallMaterial::hard_wall(), // 0: -X
            WallMaterial::hard_wall(), // 1: +X
            WallMaterial::carpet(),    // 2: -Y
            WallMaterial::hard_wall(), // 3: +Y
            WallMaterial::carpet(),    // 4: -Z (floor)
            WallMaterial::hard_wall(), // 5: +Z (ceiling)
        ];
        let resolver = ImageSourceResolver;
        let mut environment = make_room(0.0);
        environment.wall_materials = materials;

        // Resolve paths and check that wall 2 (-Y) and wall 4 (-Z) have carpet gain
        let ctx = make_room_ctx(
            &environment,
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(0.0, 0.0, 0.0),
        );
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        let carpet_gain = WallMaterial::carpet().broadband_reflection_gain();
        let hard_gain = WallMaterial::hard_wall().broadband_reflection_gain();

        // Count how many reflections have each gain level
        let carpet_count = paths.as_slice()[1..]
            .iter()
            .filter(|p| (p.gain - carpet_gain).abs() < 1e-6)
            .count();
        let hard_count = paths.as_slice()[1..]
            .iter()
            .filter(|p| (p.gain - hard_gain).abs() < 1e-6)
            .count();

        assert_eq!(carpet_count, 2, "should have 2 carpeted reflections");
        assert_eq!(hard_count, 4, "should have 4 hard wall reflections");
    }

    #[test]
    fn broadband_reflectivity_values_are_physically_correct() {
        // Hard wall: mid-band α ≈ [0.02, 0.03, 0.04, 0.05, 0.05] → avg ≈ 0.038
        // reflectivity = 1 - 0.038 = 0.962, gain = √0.962 ≈ 0.981
        let hard = WallMaterial::hard_wall();
        let hard_alpha_avg = (0.02 + 0.03 + 0.04 + 0.05 + 0.05) / 5.0;
        let expected_hard = (1.0 - hard_alpha_avg as f32).sqrt();
        assert!(
            (hard.broadband_reflection_gain() - expected_hard).abs() < 1e-6,
            "hard wall gain {}, expected {}",
            hard.broadband_reflection_gain(),
            expected_hard
        );

        // Carpet: mid-band α = [0.04, 0.08, 0.20, 0.35, 0.40] → avg = 0.214
        // reflectivity = 1 - 0.214 = 0.786, gain = √0.786 ≈ 0.887
        let carpet = WallMaterial::carpet();
        let carpet_alpha_avg = (0.04 + 0.08 + 0.20 + 0.35 + 0.40) / 5.0;
        let expected_carpet = (1.0 - carpet_alpha_avg as f32).sqrt();
        assert!(
            (carpet.broadband_reflection_gain() - expected_carpet).abs() < 1e-6,
            "carpet gain {}, expected {}",
            carpet.broadband_reflection_gain(),
            expected_carpet
        );

        // Carpet should be significantly more absorptive
        assert!(
            hard.broadband_reflection_gain() - carpet.broadband_reflection_gain() > 0.05,
            "hard wall and carpet should have meaningfully different gains"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Cross-module consistency tests
    // ─────────────────────────────────────────────────────────────────────────

    /// Verify that ImageSourceResolver and ReflectionCore compute identical
    /// reflection delays for the same geometry, confirming that both use
    /// speed_of_sound consistently (regression test for the dual-constant bug).
    #[test]
    fn reflection_delay_consistent_across_resolver_and_core() {
        use crate::pipeline::stages::reflections::ReflectionCore;

        let atmosphere = AtmosphericParams::default(); // 20°C
        let c = atmosphere.speed_of_sound(); // 331.3 + 0.606*20 = 343.42

        // Source at origin, listener at (10, 0, 0) in a 20m cube.
        let source = Vec3::new(0.0, 0.0, 0.0);
        let listener = Vec3::new(10.0, 0.0, 0.0);
        let environment_min = Vec3::new(-10.0, -10.0, -10.0);
        let environment_max = Vec3::new(10.0, 10.0, 10.0);
        let sample_rate = 48000.0;

        // --- ImageSourceResolver delays ---
        let resolver = ImageSourceResolver;
        let mut environment = Environment::uniform(
            environment_min,
            environment_max,
            WallMaterial::with_reflectivity(0.9),
        );
        environment.atmosphere = atmosphere;
        let ctx = ResolveContext {
            source_pos: source,
            target_pos: listener,
            environment: &environment,
        };
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        let resolver_delays: Vec<f32> = paths
            .as_slice()
            .iter()
            .filter(|p| p.kind == PathKind::Reflection)
            .map(|p| p.delay_seconds)
            .collect();
        assert!(!resolver_delays.is_empty(), "should have reflections");

        // --- ReflectionCore delays (in samples) ---
        let mut core = ReflectionCore::new(4096);
        core.update(&environment, source, listener, sample_rate, c, 1.0);

        // ReflectionCore and ImageSourceResolver both compute:
        //   delay = (image_dist - direct_dist) / speed_of_sound
        // Verify a specific wall: -X wall image is at (-0, 0, 0) → (x=-20, 0, 0)
        // Wait, image = (2*environment_min.x - source.x, source.y, source.z) = (-20, 0, 0)
        // image_dist = distance((-20,0,0), (10,0,0)) = 30
        // direct_dist = 10
        // delay = (30 - 10) / 343.42 = 20 / 343.42
        let expected_delay_neg_x = 20.0 / c;

        // Check that the resolver produced this delay
        let has_matching = resolver_delays
            .iter()
            .any(|&d| (d - expected_delay_neg_x).abs() < 1e-5);
        assert!(
            has_matching,
            "resolver should produce delay {expected_delay_neg_x:.6}s for -X wall, got {resolver_delays:?}"
        );

        // The 10m direct path should arrive at 10/c seconds — verify the formula
        let direct_time = 10.0 / c;
        assert!(
            (direct_time - 0.02912).abs() < 0.001,
            "10m at 20°C should be ~29.1ms, got {:.4}ms",
            direct_time * 1000.0
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Hand-derived image-source geometry (audit 2026-11)
    // ─────────────────────────────────────────────────────────────────────────

    fn reflection_for_wall(paths: &PathSet, wall_index: u8) -> Option<PathContribution> {
        paths
            .as_slice()
            .iter()
            .find(|path| path.wall_index == Some(wall_index))
            .copied()
    }

    /// Source (1,0,0), listener at the origin, room [-5,5]³. Mirroring the
    /// source across each wall plane x = w gives x' = 2w − x:
    ///   −X (x=−5): image (−11,0,0), path 11 m, arrives from (−1,0,0)
    ///   +X (x=+5): image (  9,0,0), path  9 m, arrives from (+1,0,0)
    ///   −Y (y=−5): image (1,−10,0), path √101 = 10.0499 m, from (0.0995,−0.9950,0)
    ///   +Y, −Z, +Z: the same √101 m with the sign of the mirrored axis.
    /// Delay is relative to the 1 m direct path: (L − 1) / 343.42 m/s.
    #[test]
    fn image_source_positions_lengths_and_directions_match_hand_derivation() {
        let resolver = ImageSourceResolver;
        let environment = make_room(1.0);
        let ctx = make_room_ctx(&environment, Vec3::new(1.0, 0.0, 0.0), Vec3::ZERO);
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        assert_eq!(paths.len(), 7, "1 direct + 6 first-order reflections");

        let side_length = 101.0_f32.sqrt(); // 10.0499 m
        let side_axial = 10.0 / side_length; // 0.99504
        let side_lateral = 1.0 / side_length; // 0.09950
        let expected: [(u8, f32, Vec3); 6] = [
            (0, 11.0, Vec3::new(-1.0, 0.0, 0.0)),
            (1, 9.0, Vec3::new(1.0, 0.0, 0.0)),
            (2, side_length, Vec3::new(side_lateral, -side_axial, 0.0)),
            (3, side_length, Vec3::new(side_lateral, side_axial, 0.0)),
            (4, side_length, Vec3::new(side_lateral, 0.0, -side_axial)),
            (5, side_length, Vec3::new(side_lateral, 0.0, side_axial)),
        ];
        for (wall_index, length, direction) in expected {
            let path = reflection_for_wall(&paths, wall_index)
                .unwrap_or_else(|| panic!("missing reflection for wall {wall_index}"));
            assert_eq!(path.kind, PathKind::Reflection);
            assert!(
                (path.distance - length).abs() < 1e-4,
                "wall {wall_index}: path length {} != {length}",
                path.distance
            );
            assert!(
                (path.direction - direction).length() < 1e-4,
                "wall {wall_index}: direction {:?} != {direction:?}",
                path.direction
            );
            let expected_delay = (length - 1.0) / TEST_SPEED;
            assert!(
                (path.delay_seconds - expected_delay).abs() < 1e-6,
                "wall {wall_index}: delay {} != {expected_delay}",
                path.delay_seconds
            );
        }
    }

    /// +X reflection from the geometry above: 8 m extra path at 343.42 m/s
    /// = 23.2951 ms → 1027.31 samples @ 44.1 kHz, 1118.16 @ 48 kHz,
    /// 2236.33 @ 96 kHz.
    #[test]
    fn reflection_delay_in_samples_at_common_sample_rates() {
        let resolver = ImageSourceResolver;
        let environment = make_room(1.0);
        let ctx = make_room_ctx(&environment, Vec3::new(1.0, 0.0, 0.0), Vec3::ZERO);
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);
        let positive_x = reflection_for_wall(&paths, 1).expect("+X reflection");

        assert!((positive_x.delay_seconds * 1000.0 - 23.2951).abs() < 1e-3);
        for (sample_rate, expected_samples) in [
            (44_100.0_f32, 1027.313_f32),
            (48_000.0, 1118.164),
            (96_000.0, 2236.327),
        ] {
            let delay_samples = positive_x.delay_seconds * sample_rate;
            assert!(
                (delay_samples - expected_samples).abs() < 0.02,
                "{sample_rate} Hz: {delay_samples} samples, expected {expected_samples}"
            );
        }
    }

    /// Mirroring is an isometric involution, so |M(S) − L| = |S − M(L)|:
    /// swapping source and listener must leave every per-wall path length and
    /// delay unchanged.
    #[test]
    fn image_source_paths_are_reciprocal() {
        let resolver = ImageSourceResolver;
        let environment = make_room(0.9);
        let point_a = Vec3::new(1.7, -2.3, 0.4);
        let point_b = Vec3::new(-3.1, 1.2, -1.9);

        let mut forward = PathSet::new();
        resolver.resolve(&make_room_ctx(&environment, point_a, point_b), &mut forward);
        let mut reverse = PathSet::new();
        resolver.resolve(&make_room_ctx(&environment, point_b, point_a), &mut reverse);

        assert_eq!(forward.len(), reverse.len());
        for wall_index in 0..6u8 {
            let forward_path = reflection_for_wall(&forward, wall_index).expect("forward");
            let reverse_path = reflection_for_wall(&reverse, wall_index).expect("reverse");
            assert!(
                (forward_path.distance - reverse_path.distance).abs() < 1e-4,
                "wall {wall_index}: {} vs {}",
                forward_path.distance,
                reverse_path.distance
            );
            assert!((forward_path.delay_seconds - reverse_path.delay_seconds).abs() < 1e-7);
        }
    }

    /// For any source and listener strictly inside the room, the wall plane is
    /// the perpendicular bisector of source and image, with the listener on the
    /// source's side — so every reflection is at least as long as the direct
    /// path, all 6 survive, and every direction is a unit vector.
    #[test]
    fn interior_points_always_give_six_longer_reflections() {
        let resolver = ImageSourceResolver;
        let environment = make_room(0.9);
        let coordinates = [-4.5_f32, -2.0, 0.3, 3.7];
        for &source_x in &coordinates {
            for &source_y in &coordinates {
                for &listener_z in &coordinates {
                    let source = Vec3::new(source_x, source_y, 1.1);
                    let listener = Vec3::new(-source_y * 0.5, 2.2, listener_z);
                    let mut paths = PathSet::new();
                    resolver.resolve(&make_room_ctx(&environment, source, listener), &mut paths);
                    assert_eq!(paths.len(), 7, "source {source:?} listener {listener:?}");
                    let direct_length = paths.as_slice()[0].distance;
                    for path in &paths.as_slice()[1..] {
                        assert!(path.distance >= direct_length);
                        assert!((path.direction.length() - 1.0).abs() < 1e-5);
                    }
                }
            }
        }
    }

    /// Source exactly on the +X wall: its +X image coincides with the source,
    /// so that reflection has zero extra delay and is folded into the direct
    /// path (skipped). The other 5 remain.
    #[test]
    fn source_on_wall_skips_the_coincident_reflection() {
        let resolver = ImageSourceResolver;
        let environment = make_room(0.9);
        let ctx = make_room_ctx(
            &environment,
            Vec3::new(5.0, 0.0, 0.0),
            Vec3::new(-1.0, 0.5, 0.0),
        );
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);
        assert_eq!(paths.len(), 6);
        assert!(reflection_for_wall(&paths, 1).is_none());
    }

    /// Listener on the −X wall plane: |image − listener| equals the direct
    /// distance, so the −X reflection carries no extra delay and is skipped.
    #[test]
    fn listener_on_wall_plane_skips_that_reflection() {
        let resolver = ImageSourceResolver;
        let environment = make_room(0.9);
        let ctx = make_room_ctx(
            &environment,
            Vec3::new(2.0, 1.0, 0.0),
            Vec3::new(-5.0, 0.0, 0.0),
        );
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);
        assert!(reflection_for_wall(&paths, 0).is_none());
        assert_eq!(paths.len(), 6);
    }

    /// Source outside the room beyond +X (x = 7, wall at 5): the +X "image" is
    /// at x = 3, closer to the listener than the source — rejected by the
    /// closer-than-direct guard instead of producing a negative delay.
    #[test]
    fn source_outside_room_rejects_the_inside_image() {
        let resolver = ImageSourceResolver;
        let environment = make_room(0.9);
        let ctx = make_room_ctx(&environment, Vec3::new(7.0, 0.0, 0.0), Vec3::ZERO);
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);
        assert!(reflection_for_wall(&paths, 1).is_none());
        for path in paths.as_slice() {
            assert!(path.delay_seconds >= 0.0);
        }
    }

    /// Wall index, image geometry and material must agree: with a distinct
    /// material on the floor (−Z, index 4), the reflection arriving from below
    /// carries the carpet gain √(1 − 0.214) = 0.88657 and every other wall the
    /// hard-wall gain √(1 − 0.038) = 0.98082.
    #[test]
    fn floor_reflection_arrives_from_below_with_floor_material_gain() {
        let mut materials: [WallMaterial; 6] = std::array::from_fn(|_| WallMaterial::hard_wall());
        materials[4] = WallMaterial::carpet();
        let resolver = ImageSourceResolver;
        let mut environment = make_room(0.0);
        environment.wall_materials = materials;
        let ctx = make_room_ctx(&environment, Vec3::new(1.0, 0.0, 0.0), Vec3::ZERO);
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        for path in &paths.as_slice()[1..] {
            let from_below = path.direction.z < -0.9;
            assert_eq!(from_below, path.wall_index == Some(4));
            let expected_gain = if from_below { 0.886_57 } else { 0.980_82 };
            assert!(
                (path.gain - expected_gain).abs() < 1e-4,
                "wall {:?}: gain {} != {expected_gain}",
                path.wall_index,
                path.gain
            );
        }
    }
}
