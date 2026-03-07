//! Concrete `PathResolver` implementations.
//!
//! - `DirectPathResolver`: always returns 1 direct line-of-sight path.

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
}
