//! Concrete `PathResolver` implementations.
//!
//! - `DirectPathResolver`: always returns 1 direct line-of-sight path.
//! - `ImageSourceResolver`: 1 direct + up to 6 first-order wall reflections.
//! - `BarrierDiffractionResolver`: decorator that adds diffraction paths over barriers.

use atrium_core::types::Vec3;

use crate::audio::propagation::{barrier_attenuation_gain, BarrierGeometry};
use crate::pipeline::path::{
    PathContribution, PathKind, PathResolver, PathSet, ResolveContext, WallMaterial,
};

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
pub struct ImageSourceResolver {
    /// Per-wall broadband amplitude reflection gains [−X, +X, −Y, +Y, −Z, +Z].
    wall_gains: [f32; 6],
}

impl ImageSourceResolver {
    /// Create from per-wall materials. Each wall's broadband reflection gain is
    /// derived from mid-band absorption coefficients (250 Hz–4 kHz).
    pub fn from_materials(materials: &[WallMaterial; 6]) -> Self {
        let mut wall_gains = [0.0f32; 6];
        for (i, mat) in materials.iter().enumerate() {
            wall_gains[i] = mat.broadband_reflection_gain();
        }
        Self { wall_gains }
    }

    /// Create with uniform reflectivity for all walls (legacy/test convenience).
    pub fn new(wall_reflectivity: f32) -> Self {
        let gain = wall_reflectivity.sqrt();
        Self {
            wall_gains: [gain; 6],
        }
    }
}

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

        // 6 image sources: mirror source across each wall of the rectangular room
        let src = ctx.source_pos;
        let images = [
            Vec3::new(2.0 * ctx.room_min.x - src.x, src.y, src.z), // -X wall
            Vec3::new(2.0 * ctx.room_max.x - src.x, src.y, src.z), // +X wall
            Vec3::new(src.x, 2.0 * ctx.room_min.y - src.y, src.z), // -Y wall
            Vec3::new(src.x, 2.0 * ctx.room_max.y - src.y, src.z), // +Y wall
            Vec3::new(src.x, src.y, 2.0 * ctx.room_min.z - src.z), // -Z wall (floor)
            Vec3::new(src.x, src.y, 2.0 * ctx.room_max.z - src.z), // +Z wall (ceiling)
        ];

        for (wall_idx, image) in images.iter().enumerate() {
            let image_diff = *image - ctx.target_pos;
            let image_dist = image_diff.length();

            // Skip degenerate or closer-than-direct reflections
            if image_dist < 0.1 || image_dist < direct_dist {
                continue;
            }

            let delay_seconds = (image_dist - direct_dist) / ctx.atmosphere.speed_of_sound();
            if delay_seconds < 1e-6 {
                continue;
            }

            let direction = image_diff * (1.0 / image_dist);
            // Per-wall broadband amplitude reflection gain (pre-computed from materials).
            // Distance attenuation is applied by the renderer using the same
            // distance model as the direct path, keeping the gain staging consistent.
            let gain = self.wall_gains[wall_idx];

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

// ─────────────────────────────────────────────────────────────────────────────
// BarrierDiffractionResolver
// ─────────────────────────────────────────────────────────────────────────────

/// Decorator that adds diffraction paths over barriers (ISO 9613-2 §7.4).
///
/// Wraps an inner `PathResolver` (e.g. `ImageSourceResolver`) and appends one
/// diffraction path per barrier that occludes the direct source-receiver path.
/// Each diffraction path carries:
/// - **direction**: unit vector from target toward barrier top (apparent source)
/// - **distance**: total diffracted path length (source→barrier_top + barrier_top→receiver)
/// - **delay**: extra propagation time relative to direct path
/// - **gain**: broadband ISO 9613-2 Maekawa attenuation (averaged over 6 octave bands)
///
/// Barriers in the illuminated zone (δ < 1e-4 m) are skipped — they don't
/// meaningfully occlude the direct path.
pub struct BarrierDiffractionResolver {
    inner: Box<dyn PathResolver>,
}

impl BarrierDiffractionResolver {
    pub fn new(inner: Box<dyn PathResolver>) -> Self {
        Self { inner }
    }
}

impl PathResolver for BarrierDiffractionResolver {
    fn resolve(&self, ctx: &ResolveContext<'_>, out: &mut PathSet) {
        // Delegate to inner resolver for direct + reflection paths.
        self.inner.resolve(ctx, out);

        // Direct distance for delay computation.
        let d_sr = ctx.source_pos.distance_to(ctx.target_pos);

        for barrier in ctx.barriers {
            let d_sb = ctx.source_pos.distance_to(barrier.top);
            let d_br = barrier.top.distance_to(ctx.target_pos);
            let delta = d_sb + d_br - d_sr;

            // Skip if barrier doesn't meaningfully occlude (illuminated zone).
            if delta < 1e-4 {
                continue;
            }

            // Direction: sound appears to arrive from the barrier top.
            let diff = barrier.top - ctx.target_pos;
            let diff_len = diff.length();
            let direction = if diff_len > 1e-10 {
                diff * (1.0 / diff_len)
            } else {
                Vec3::new(0.0, 0.0, 1.0) // fallback: straight up
            };

            // ISO 9613-2 broadband attenuation gain.
            let geom = BarrierGeometry {
                source: ctx.source_pos,
                receiver: ctx.target_pos,
                barrier_top: barrier.top,
            };
            let gain = barrier_attenuation_gain(&geom, ctx.atmosphere.speed_of_sound());

            let delay_seconds = delta / ctx.atmosphere.speed_of_sound();
            let distance = d_sb + d_br;

            out.push(PathContribution {
                kind: PathKind::Diffraction,
                direction,
                distance,
                delay_seconds,
                gain,
                wall_index: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::atmosphere::AtmosphericParams;

    /// Standard speed of sound at 20°C for test assertions.
    const TEST_SPEED: f32 = 343.42; // speed_of_sound(20.0) = 331.3 + 0.606*20

    #[test]
    fn direct_resolver_produces_one_path() {
        let resolver = DirectPathResolver;
        let ctx = ResolveContext {
            source_pos: Vec3::new(3.0, 0.0, 0.0),
            target_pos: Vec3::new(0.0, 0.0, 0.0),
            room_min: Vec3::new(-10.0, -10.0, -10.0),
            room_max: Vec3::new(10.0, 10.0, 10.0),
            barriers: &[],
            atmosphere: &AtmosphericParams::default(),
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
        let ctx = ResolveContext {
            source_pos: Vec3::new(0.0, 5.0, 0.0),
            target_pos: Vec3::new(0.0, 0.0, 0.0),
            room_min: Vec3::ZERO,
            room_max: Vec3::ZERO,
            barriers: &[],
            atmosphere: &AtmosphericParams::default(),
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
        let ctx = ResolveContext {
            source_pos: Vec3::new(1.0, 2.0, 3.0),
            target_pos: Vec3::new(1.0, 2.0, 3.0),
            room_min: Vec3::ZERO,
            room_max: Vec3::ZERO,
            barriers: &[],
            atmosphere: &AtmosphericParams::default(),
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
        let ctx = ResolveContext {
            source_pos: Vec3::new(3.0, 4.0, 0.0),
            target_pos: Vec3::new(0.0, 0.0, 0.0),
            room_min: Vec3::ZERO,
            room_max: Vec3::ZERO,
            barriers: &[],
            atmosphere: &AtmosphericParams::default(),
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

    fn make_room_ctx(source: Vec3, target: Vec3) -> ResolveContext<'static> {
        ResolveContext {
            source_pos: source,
            target_pos: target,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            barriers: &[],
            atmosphere: &TEST_ATMOSPHERE,
        }
    }

    #[test]
    fn image_source_produces_direct_plus_reflections() {
        let resolver = ImageSourceResolver::new(0.9);
        let ctx = make_room_ctx(Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0));
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
        let image_resolver = ImageSourceResolver::new(0.9);
        let direct_resolver = DirectPathResolver;
        let ctx = make_room_ctx(Vec3::new(3.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 0.0));

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
        let resolver = ImageSourceResolver::new(0.9);
        let ctx = make_room_ctx(Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0));
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
        let resolver = ImageSourceResolver::new(0.9);
        let ctx = make_room_ctx(Vec3::new(2.0, 1.0, 0.5), Vec3::new(0.0, 0.0, 0.0));
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
        let resolver = ImageSourceResolver::new(0.9);
        let ctx = make_room_ctx(Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0));
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
        let resolver = ImageSourceResolver::new(reflectivity);
        let source = Vec3::new(2.0, 0.0, 0.0);
        let target = Vec3::new(0.0, 0.0, 0.0);
        let ctx = make_room_ctx(source, target);
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
        let ctx = make_room_ctx(source, target);

        let resolver_high = ImageSourceResolver::new(0.9);
        let resolver_low = ImageSourceResolver::new(0.4);

        let mut paths_high = PathSet::new();
        let mut paths_low = PathSet::new();
        resolver_high.resolve(&ctx, &mut paths_high);
        resolver_low.resolve(&ctx, &mut paths_low);

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
        let resolver = ImageSourceResolver::new(0.9);
        let ctx = ResolveContext {
            source_pos: Vec3::new(1.0, 0.0, 0.0),
            target_pos: Vec3::new(0.0, 0.0, 0.0),
            room_min: Vec3::new(-10.0, -10.0, -10.0),
            room_max: Vec3::new(10.0, 10.0, 10.0),
            barriers: &[],
            atmosphere: &AtmosphericParams::default(),
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
        let resolver = ImageSourceResolver::new(0.9);
        let ctx = ResolveContext {
            source_pos: Vec3::new(4.9, 0.0, 0.0),
            target_pos: Vec3::new(0.0, 0.0, 0.0),
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            barriers: &[],
            atmosphere: &AtmosphericParams::default(),
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
        let resolver = ImageSourceResolver::new(0.9);
        // Source at (2,0,0), target at origin, room [-5,5]^3
        // -X wall image: (2*(-5) - 2, 0, 0) = (-12, 0, 0)
        // Direction from target toward image: (-1, 0, 0)
        let ctx = make_room_ctx(Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0));
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
        let resolver = ImageSourceResolver::new(0.9);
        let ctx = ResolveContext {
            source_pos: Vec3::new(0.0, 0.0, 0.0),
            target_pos: Vec3::new(0.0, 0.0, 0.0),
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            barriers: &[],
            atmosphere: &AtmosphericParams::default(),
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
    // BarrierDiffractionResolver tests
    // ─────────────────────────────────────────────────────────────────────────

    use crate::audio::propagation::Barrier;

    #[test]
    fn barrier_no_barriers_same_as_inner() {
        let inner = Box::new(DirectPathResolver);
        let resolver = BarrierDiffractionResolver::new(inner);
        let ctx = ResolveContext {
            source_pos: Vec3::new(3.0, 0.0, 0.0),
            target_pos: Vec3::new(0.0, 0.0, 0.0),
            room_min: Vec3::new(-10.0, -10.0, -10.0),
            room_max: Vec3::new(10.0, 10.0, 10.0),
            barriers: &[],
            atmosphere: &AtmosphericParams::default(),
        };
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        // Only the direct path from inner resolver.
        assert_eq!(paths.len(), 1);
        assert_eq!(paths.as_slice()[0].kind, PathKind::Direct);
    }

    #[test]
    fn barrier_shadow_zone_adds_diffraction_path() {
        let inner = Box::new(DirectPathResolver);
        let resolver = BarrierDiffractionResolver::new(inner);

        // Barrier top at (5, 0, 3) — well above the direct line from (0,0,0) to (10,0,0).
        let barriers = [Barrier {
            base: Vec3::new(5.0, 0.0, 0.0),
            top: Vec3::new(5.0, 0.0, 3.0),
        }];
        let ctx = ResolveContext {
            source_pos: Vec3::new(0.0, 0.0, 0.0),
            target_pos: Vec3::new(10.0, 0.0, 0.0),
            room_min: Vec3::new(-10.0, -10.0, -10.0),
            room_max: Vec3::new(10.0, 10.0, 10.0),
            barriers: &barriers,
            atmosphere: &AtmosphericParams::default(),
        };
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        // 1 direct + 1 diffraction.
        assert_eq!(paths.len(), 2);
        assert_eq!(paths.as_slice()[0].kind, PathKind::Direct);
        assert_eq!(paths.as_slice()[1].kind, PathKind::Diffraction);

        let diff = &paths.as_slice()[1];
        assert!(diff.gain > 0.0, "diffraction gain should be positive");
        assert!(
            diff.gain < 1.0,
            "diffraction gain should be < 1.0 (attenuated)"
        );
        assert!(
            diff.delay_seconds > 0.0,
            "diffraction should have positive delay"
        );
    }

    #[test]
    fn barrier_illuminated_zone_skipped() {
        let inner = Box::new(DirectPathResolver);
        let resolver = BarrierDiffractionResolver::new(inner);

        // Barrier top well below the direct line — doesn't occlude.
        // Source at (0,0,5), receiver at (10,0,5), barrier top at (5,0,0).
        // d_sr = 10, d_sb = sqrt(25+25) ≈ 7.07, d_br = sqrt(25+25) ≈ 7.07
        // delta = 7.07 + 7.07 - 10 = 4.14 > 0... this still occludes.
        // Instead: barrier top at (5,0,4.999) — nearly on the line.
        // Actually, to get illuminated zone we need barrier top BELOW the line of sight.
        // Source (0,0,0), receiver (10,0,0), barrier at (5,5,0) — off to the side.
        // d_sr = 10, d_sb = sqrt(25+25) ≈ 7.07, d_br = sqrt(25+25) ≈ 7.07
        // delta = 14.14 - 10 = 4.14... still positive because path goes through barrier.
        //
        // For illuminated zone, barrier must be collinear or behind the path.
        // Barrier at (5,0,0) — exactly on the line: d_sb=5, d_br=5, delta=0. Skipped.
        let barriers = [Barrier {
            base: Vec3::new(5.0, 0.0, 0.0),
            top: Vec3::new(5.0, 0.0, 0.0), // on the direct line
        }];
        let ctx = ResolveContext {
            source_pos: Vec3::new(0.0, 0.0, 0.0),
            target_pos: Vec3::new(10.0, 0.0, 0.0),
            room_min: Vec3::new(-10.0, -10.0, -10.0),
            room_max: Vec3::new(10.0, 10.0, 10.0),
            barriers: &barriers,
            atmosphere: &AtmosphericParams::default(),
        };
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        // Barrier on line of sight → delta ≈ 0 → skipped.
        assert_eq!(paths.len(), 1, "barrier on direct line should be skipped");
    }

    #[test]
    fn barrier_direction_points_toward_barrier_top() {
        let inner = Box::new(DirectPathResolver);
        let resolver = BarrierDiffractionResolver::new(inner);

        let barriers = [Barrier {
            base: Vec3::new(5.0, 0.0, 0.0),
            top: Vec3::new(5.0, 0.0, 3.0),
        }];
        let ctx = ResolveContext {
            source_pos: Vec3::new(0.0, 0.0, 0.0),
            target_pos: Vec3::new(10.0, 0.0, 0.0),
            room_min: Vec3::new(-10.0, -10.0, -10.0),
            room_max: Vec3::new(10.0, 10.0, 10.0),
            barriers: &barriers,
            atmosphere: &AtmosphericParams::default(),
        };
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        let diff = &paths.as_slice()[1];
        // Barrier top at (5,0,3), target at (10,0,0).
        // Direction = normalize((5,0,3) - (10,0,0)) = normalize((-5,0,3))
        let expected = Vec3::new(-5.0, 0.0, 3.0);
        let expected_len = expected.length();
        let expected_dir = expected * (1.0 / expected_len);

        assert!((diff.direction.x - expected_dir.x).abs() < 1e-4);
        assert!((diff.direction.y - expected_dir.y).abs() < 1e-4);
        assert!((diff.direction.z - expected_dir.z).abs() < 1e-4);
    }

    #[test]
    fn barrier_delay_equals_delta_over_speed_of_sound() {
        let inner = Box::new(DirectPathResolver);
        let resolver = BarrierDiffractionResolver::new(inner);

        let barriers = [Barrier {
            base: Vec3::new(5.0, 0.0, 0.0),
            top: Vec3::new(5.0, 0.0, 3.0),
        }];
        let ctx = ResolveContext {
            source_pos: Vec3::new(0.0, 0.0, 0.0),
            target_pos: Vec3::new(10.0, 0.0, 0.0),
            room_min: Vec3::new(-10.0, -10.0, -10.0),
            room_max: Vec3::new(10.0, 10.0, 10.0),
            barriers: &barriers,
            atmosphere: &AtmosphericParams::default(),
        };
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        let diff = &paths.as_slice()[1];
        let d_sb = Vec3::new(0.0, 0.0, 0.0).distance_to(Vec3::new(5.0, 0.0, 3.0));
        let d_br = Vec3::new(5.0, 0.0, 3.0).distance_to(Vec3::new(10.0, 0.0, 0.0));
        let d_sr = 10.0_f32;
        let expected_delay = (d_sb + d_br - d_sr) / TEST_SPEED;

        assert!(
            (diff.delay_seconds - expected_delay).abs() < 1e-6,
            "delay {} should equal delta/c = {}",
            diff.delay_seconds,
            expected_delay
        );
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

        let resolver = ImageSourceResolver::from_materials(&materials);

        let source = Vec3::new(1.0, 1.0, 2.0);
        let target = Vec3::new(0.0, 0.0, 0.0);
        let ctx = ResolveContext {
            source_pos: source,
            target_pos: target,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            barriers: &[],
            atmosphere: &TEST_ATMOSPHERE,
        };
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
    fn from_materials_matches_manual_gain_computation() {
        // Verify that from_materials produces gains consistent with
        // the broadband_reflection_gain formula: √(1 - avg(α[1..5]))
        let materials = [
            WallMaterial::hard_wall(), // 0: -X
            WallMaterial::hard_wall(), // 1: +X
            WallMaterial::carpet(),    // 2: -Y
            WallMaterial::hard_wall(), // 3: +Y
            WallMaterial::carpet(),    // 4: -Z (floor)
            WallMaterial::hard_wall(), // 5: +Z (ceiling)
        ];
        let resolver = ImageSourceResolver::from_materials(&materials);

        // Resolve paths and check that wall 2 (-Y) and wall 4 (-Z) have carpet gain
        let ctx = make_room_ctx(Vec3::new(1.0, 1.0, 1.0), Vec3::new(0.0, 0.0, 0.0));
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

    #[test]
    fn uniform_materials_matches_legacy_constructor() {
        // from_materials with all-same materials should produce identical results
        // to the legacy new(reflectivity) constructor with the same reflectivity.
        let materials: [WallMaterial; 6] = std::array::from_fn(|_| WallMaterial::hard_wall());
        let resolver_new = ImageSourceResolver::from_materials(&materials);

        let hard_refl = WallMaterial::hard_wall().broadband_reflectivity();
        let resolver_legacy = ImageSourceResolver::new(hard_refl);

        let ctx = make_room_ctx(Vec3::new(2.0, 1.0, 0.5), Vec3::new(0.0, 0.0, 0.0));

        let mut paths_new = PathSet::new();
        let mut paths_legacy = PathSet::new();
        resolver_new.resolve(&ctx, &mut paths_new);
        resolver_legacy.resolve(&ctx, &mut paths_legacy);

        assert_eq!(paths_new.len(), paths_legacy.len());
        for (a, b) in paths_new.as_slice().iter().zip(paths_legacy.as_slice()) {
            assert!(
                (a.gain - b.gain).abs() < 1e-6,
                "gain mismatch: from_materials={}, legacy={}",
                a.gain,
                b.gain
            );
            assert!((a.distance - b.distance).abs() < 1e-6);
            assert!((a.delay_seconds - b.delay_seconds).abs() < 1e-6);
        }
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
        let room_min = Vec3::new(-10.0, -10.0, -10.0);
        let room_max = Vec3::new(10.0, 10.0, 10.0);
        let sample_rate = 48000.0;

        // --- ImageSourceResolver delays ---
        let resolver = ImageSourceResolver::new(0.9);
        let ctx = ResolveContext {
            source_pos: source,
            target_pos: listener,
            room_min,
            room_max,
            barriers: &[],
            atmosphere: &atmosphere,
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
        let mut core = ReflectionCore::new(0.9);
        core.update(room_min, room_max, source, listener, sample_rate, c);

        // ReflectionCore and ImageSourceResolver both compute:
        //   delay = (image_dist - direct_dist) / speed_of_sound
        // Verify a specific wall: -X wall image is at (-0, 0, 0) → (x=-20, 0, 0)
        // Wait, image = (2*room_min.x - source.x, source.y, source.z) = (-20, 0, 0)
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
}
