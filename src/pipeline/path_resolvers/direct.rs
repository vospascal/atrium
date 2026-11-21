//! The direct line-of-sight path.

use atrium_core::types::Vec3;

use crate::pipeline::path::{PathContribution, PathKind, PathResolver, PathSet, ResolveContext};

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
