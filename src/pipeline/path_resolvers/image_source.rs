//! The direct path plus first-order wall reflections (image-source method).

use atrium_core::types::Vec3;

use crate::pipeline::environment::WALL_COUNT;
use crate::pipeline::path::{PathContribution, PathKind, PathResolver, PathSet, ResolveContext};

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
