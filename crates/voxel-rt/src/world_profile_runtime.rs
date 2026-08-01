//! Runtime adapter from compiled world profiles to the current brickmap.
//!
//! The authored model stays renderer-independent. This module is the concrete
//! consumer that turns physical `AddWorldVoxelLayer` projections into generated
//! voxels before GPU upload. Presentation, audio, and animation consumers can
//! subscribe to the same compiled profile without entering world generation.

use std::collections::BTreeMap;

use voxel_core::world::{
    Voxel, WorldVoxelCoord, WORLD_VOXELS_X, WORLD_VOXELS_Y, WORLD_VOXELS_Z, WORLD_VOXEL_SIZE_METERS,
};

use crate::brickmap::{Brickmap, ClearanceUpdate};
use crate::environment::{
    EnvironmentChannel, EnvironmentContext, GeneratedEnvironment, RuntimeEnvironmentState,
};
use crate::material::{material_blocks_movement, material_is_liquid, material_voxel};
use crate::world_profile::{CompiledWorldProfile, WorldProfileError};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GenerationApplication {
    pub sampled_columns: usize,
    pub changed_voxels: usize,
}

/// Apply the physical surface projection to a freshly built brickmap. A runtime
/// gate makes non-matching seasons/weather O(1); spatial sampling happens only
/// when at least one voxel-layer rule can be active.
pub fn apply_initial_generation_profile(
    brickmap: &mut Brickmap,
    profile: &CompiledWorldProfile,
    runtime: &RuntimeEnvironmentState,
    world_seed: u64,
) -> Result<GenerationApplication, WorldProfileError> {
    if !profile.has_active_voxel_layer_rules(runtime) {
        return Ok(GenerationApplication::default());
    }
    let mut result = GenerationApplication::default();
    for z in 0..WORLD_VOXELS_Z as i32 {
        for x in 0..WORLD_VOXELS_X as i32 {
            let Some(surface_y) = terrain_surface_y(brickmap, x, z) else {
                continue;
            };
            result.sampled_columns += 1;
            let left = terrain_surface_y(brickmap, x - 1, z).unwrap_or(surface_y);
            let right = terrain_surface_y(brickmap, x + 1, z).unwrap_or(surface_y);
            let back = terrain_surface_y(brickmap, x, z - 1).unwrap_or(surface_y);
            let front = terrain_surface_y(brickmap, x, z + 1).unwrap_or(surface_y);
            let normal = normalize([(left - right) as f32, 2.0, (back - front) as f32]);
            let slope = (1.0 - normal[1].clamp(0.0, 1.0)).clamp(0.0, 1.0);
            let generated = GeneratedEnvironment {
                world_seed,
                position: [
                    (x as f32 + 0.5) * WORLD_VOXEL_SIZE_METERS,
                    (surface_y as f32 + 1.0) * WORLD_VOXEL_SIZE_METERS,
                    (z as f32 + 0.5) * WORLD_VOXEL_SIZE_METERS,
                ],
                normal,
                fields: BTreeMap::from([
                    (
                        EnvironmentChannel::Elevation,
                        surface_y as f32 * WORLD_VOXEL_SIZE_METERS,
                    ),
                    (EnvironmentChannel::Depth, 0.0),
                    (EnvironmentChannel::Slope, slope),
                    (EnvironmentChannel::Temperature, 0.0),
                    (EnvironmentChannel::Humidity, 0.0),
                ]),
            };
            let environment = EnvironmentContext::compose(&generated, runtime, &BTreeMap::new())
                .expect("finite generated surface context");
            let resolved = profile.resolve(&environment)?;
            let mut offset = 1_i32;
            for layer in &resolved.generation.added_voxel_layers {
                let voxel = material_voxel(layer.material_slot);
                if voxel == Voxel::Air {
                    continue;
                }
                for _ in 0..layer.thickness_world_voxels {
                    let y = surface_y + offset;
                    let coordinate = WorldVoxelCoord::new(x, y, z);
                    if !coordinate.is_in_bounds() {
                        break;
                    }
                    let detail = coordinate.detail_origin();
                    let existing = brickmap.get(detail[0], detail[1], detail[2]);
                    if (existing == 0
                        || (!material_blocks_movement(existing) && !material_is_liquid(existing)))
                        && brickmap
                            .set_world_voxel(
                                coordinate,
                                voxel,
                                ClearanceUpdate::LocalBox { radius_cells: 0 },
                            )
                            .is_some()
                    {
                        result.changed_voxels += 1;
                    }
                    offset += 1;
                }
            }
        }
    }
    Ok(result)
}

fn terrain_surface_y(brickmap: &Brickmap, x: i32, z: i32) -> Option<i32> {
    let mut encountered_liquid = false;
    for y in (0..WORLD_VOXELS_Y as i32).rev() {
        let detail = WorldVoxelCoord::new(x, y, z).detail_origin();
        let material = brickmap.get(detail[0], detail[1], detail[2]);
        encountered_liquid |= material_is_liquid(material);
        if material_blocks_movement(material) {
            return (!encountered_liquid).then_some(y);
        }
    }
    None
}

fn normalize(value: [f32; 3]) -> [f32; 3] {
    let length = value
        .iter()
        .map(|component| component * component)
        .sum::<f32>()
        .sqrt();
    if length <= f32::EPSILON {
        [0.0, 1.0, 0.0]
    } else {
        value.map(|component| component / length)
    }
}
