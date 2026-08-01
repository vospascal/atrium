//! Runtime adapter from compiled world profiles to the current brickmap.
//!
//! The authored model stays renderer-independent. This module is the concrete
//! consumer that turns physical `AddVoxelLayer` projections into generated
//! voxels before GPU upload. Presentation, audio, and animation consumers can
//! subscribe to the same compiled profile without entering world generation.

use std::collections::BTreeMap;

use voxel_core::world::{Voxel, VOXEL_SIZE, WORLD_SIZE_X, WORLD_SIZE_Z};

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
    for z in 0..WORLD_SIZE_Z as i32 {
        for x in 0..WORLD_SIZE_X as i32 {
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
                    (x as f32 + 0.5) * VOXEL_SIZE,
                    (surface_y as f32 + 1.0) * VOXEL_SIZE,
                    (z as f32 + 0.5) * VOXEL_SIZE,
                ],
                normal,
                fields: BTreeMap::from([
                    (EnvironmentChannel::Elevation, surface_y as f32 * VOXEL_SIZE),
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
                for _ in 0..layer.thickness_voxels {
                    let y = surface_y + offset;
                    let existing = brickmap.get(x, y, z);
                    if (existing == 0
                        || (!material_blocks_movement(existing) && !material_is_liquid(existing)))
                        && brickmap
                            .set_voxel(
                                x,
                                y,
                                z,
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
    let top = brickmap.top_occupied_y(x, z)?;
    let mut encountered_liquid = false;
    for y in (0..=top).rev() {
        let material = brickmap.get(x, y, z);
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
