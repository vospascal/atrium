//! Concrete `PathResolver` implementations.
//!
//! - `DirectPathResolver`: always returns 1 direct line-of-sight path.
//! - `ImageSourceResolver`: 1 direct + up to 6 first-order wall reflections.

use atrium_core::types::Vec3;

use crate::audio::atmosphere::SPEED_OF_SOUND;
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
    fn resolve(&self, ctx: &ResolveContext, out: &mut PathSet) {
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
/// - **gain**: wall reflectivity scaled by inverse distance
///
/// Reflections are skipped when the image distance is shorter than the direct
/// path (source between wall and target) or when the delay would be zero.
pub struct ImageSourceResolver {
    pub wall_reflectivity: f32,
}

impl ImageSourceResolver {
    pub fn new(wall_reflectivity: f32) -> Self {
        Self { wall_reflectivity }
    }
}

impl PathResolver for ImageSourceResolver {
    fn resolve(&self, ctx: &ResolveContext, out: &mut PathSet) {
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

        for image in &images {
            let image_diff = *image - ctx.target_pos;
            let image_dist = image_diff.length();

            // Skip degenerate or closer-than-direct reflections
            if image_dist < 0.1 || image_dist < direct_dist {
                continue;
            }

            let delay_seconds = (image_dist - direct_dist) / SPEED_OF_SOUND;
            if delay_seconds < 1e-6 {
                continue;
            }

            let direction = image_diff * (1.0 / image_dist);
            let gain = (self.wall_reflectivity / image_dist).min(1.0);

            out.push(PathContribution {
                kind: PathKind::Reflection,
                direction,
                distance: image_dist,
                delay_seconds,
                gain,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_resolver_produces_one_path() {
        let resolver = DirectPathResolver;
        let ctx = ResolveContext {
            source_pos: Vec3::new(3.0, 0.0, 0.0),
            target_pos: Vec3::new(0.0, 0.0, 0.0),
            room_min: Vec3::new(-10.0, -10.0, -10.0),
            room_max: Vec3::new(10.0, 10.0, 10.0),
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
        };
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        assert!((paths.as_slice()[0].distance - 5.0).abs() < 1e-6);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // ImageSourceResolver tests
    // ─────────────────────────────────────────────────────────────────────────

    fn make_room_ctx(source: Vec3, target: Vec3) -> ResolveContext {
        ResolveContext {
            source_pos: source,
            target_pos: target,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
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
            let expected_delay = (p.distance - direct_dist) / SPEED_OF_SOUND;
            assert!(
                (p.delay_seconds - expected_delay).abs() < 1e-6,
                "delay {} should equal (dist - direct) / c = {}",
                p.delay_seconds,
                expected_delay
            );
        }
    }

    #[test]
    fn image_source_gain_uses_wall_reflectivity_over_distance() {
        let reflectivity = 0.8;
        let resolver = ImageSourceResolver::new(reflectivity);
        let ctx = make_room_ctx(Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0));
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        for p in &paths.as_slice()[1..] {
            let expected = (reflectivity / p.distance).min(1.0);
            assert!(
                (p.gain - expected).abs() < 1e-6,
                "gain {} should equal min(reflectivity/dist, 1.0) = {}",
                p.gain,
                expected
            );
        }
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
        };
        let mut paths = PathSet::new();
        resolver.resolve(&ctx, &mut paths);

        // Direct path should exist (with fallback direction)
        assert!(paths.len() >= 1);
        assert_eq!(paths.as_slice()[0].kind, PathKind::Direct);
        assert!(paths.as_slice()[0].direction.x.is_finite());

        // Reflections should have valid values (no NaN/Inf)
        for p in paths.as_slice() {
            assert!(p.gain.is_finite());
            assert!(p.distance.is_finite());
            assert!(p.delay_seconds.is_finite());
        }
    }
}
