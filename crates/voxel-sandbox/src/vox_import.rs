//! MagicaVoxel `.vox` prop import.
//!
//! Loads a model through [`voxel_core::vox`] and meshes it with the same
//! culled-face + baked-AO + color-jitter treatment as the terrain, so hand-made
//! (or MagicaVoxel-procedural) props sit visually seamless in the world.
//!
//! The format parsing used to live here. It moved to `voxel-core` when voxel-rt
//! gained a `.vox` material importer: a second copy of the axis swap, the palette
//! decode and the palette-index arithmetic was exactly the duplication that work
//! set out to end. This module keeps the parts that are about *meshing a prop for
//! Bevy* and nothing about the file format.

use std::path::Path;

use bevy::prelude::*;

use crate::mesh::{ambient_occlusion_level, MeshBuffers, FACE_DIRECTIONS, QUAD_CORNERS};
use voxel_core::noise::{hash_3d, hash_to_unit};
use voxel_core::vox::VoxFile;
use voxel_core::world::VOXEL_SIZE;

pub struct VoxModel {
    size_x: i32,
    size_y: i32,
    size_z: i32,
    /// Dense grid of linear-RGBA colors; `None` = empty cell.
    cells: Vec<Option<[f32; 4]>>,
}

impl VoxModel {
    /// Build a model procedurally (code-generated props like the campfire
    /// stone ring) — engine axes, y up, `cells` indexed `(y * z + z) * x`.
    pub fn from_cells(size_x: i32, size_y: i32, size_z: i32, cells: Vec<Option<[f32; 4]>>) -> Self {
        assert_eq!(cells.len(), (size_x * size_y * size_z) as usize);
        Self {
            size_x,
            size_y,
            size_z,
            cells,
        }
    }

    fn index(&self, x: i32, y: i32, z: i32) -> usize {
        ((y * self.size_z + z) * self.size_x + x) as usize
    }

    fn color_at(&self, x: i32, y: i32, z: i32) -> Option<[f32; 4]> {
        if x < 0 || y < 0 || z < 0 || x >= self.size_x || y >= self.size_y || z >= self.size_z {
            return None;
        }
        self.cells[self.index(x, y, z)]
    }

    /// Footprint in meters (x, z) and height (y), for placement.
    pub fn dimensions_meters(&self) -> Vec3 {
        Vec3::new(
            self.size_x as f32 * VOXEL_SIZE,
            self.size_y as f32 * VOXEL_SIZE,
            self.size_z as f32 * VOXEL_SIZE,
        )
    }
}

/// Load every model in a `.vox` file (packs commonly hold several).
///
/// The FORMAT work — reading the chunks, swapping Z-up to Y-up, and resolving the
/// palette's two index spaces — moved to [`voxel_core::vox`], because a `.vox`
/// grid is world data and voxel-rt's material import needs the same three things.
/// What stays here is the one thing that is genuinely this renderer's: baking each
/// cell's palette colour into the linear RGBA the mesher wants.
///
/// The colour conversion deliberately still goes through Bevy's
/// `Color::srgba_u8().to_linear()` on the raw bytes rather than
/// [`voxel_core::vox::VoxPaletteEntry::linear_rgb`]. Both implement the same sRGB
/// transfer, but routing this crate's pixels through a second implementation to
/// save one line would be risking a look change for nothing.
pub fn load_vox_models(path: &Path) -> Result<Vec<VoxModel>, String> {
    let file = VoxFile::load(path)?;
    let models = file
        .models
        .iter()
        .map(|model| {
            let cells = model
                .cells
                .iter()
                .map(|cell| {
                    let entry = file.palette[(*cell)? as usize];
                    let linear =
                        Color::srgba_u8(entry.rgba[0], entry.rgba[1], entry.rgba[2], entry.rgba[3])
                            .to_linear();
                    Some([linear.red, linear.green, linear.blue, 1.0])
                })
                .collect();
            VoxModel {
                size_x: model.size_x,
                size_y: model.size_y,
                size_z: model.size_z,
                cells,
            }
        })
        .collect();
    Ok(models)
}

/// Meshes of one prop, split by material treatment.
pub struct PropMeshes {
    /// Regular voxels: terrain-style culled faces + baked AO + jitter.
    pub solid: Option<Mesh>,
    /// Hot (fire-colored) voxels: separate mesh for the animated, emissive
    /// [`crate::flame::FlameMaterial`]. No AO — fire is self-lit.
    pub flame: Option<Mesh>,
}

/// Fire colors (linear space): strongly red-dominant, warm ramp r > g > b.
fn is_flame_color(color: [f32; 4]) -> bool {
    color[0] > 0.45 && color[0] > color[1] * 1.05 && color[1] > color[2]
}

pub fn build_prop_meshes(model: &VoxModel, jitter_seed: u32) -> PropMeshes {
    PropMeshes {
        solid: mesh_subset(model, jitter_seed, false),
        flame: mesh_subset(model, jitter_seed, true),
    }
}

/// Mesh the flame or non-flame subset of a prop: culled faces against the
/// same subset, baked corner AO (solid subset only), per-voxel brightness
/// jitter. Centered on x/z with the model base at y = 0. `None` when the
/// subset is empty.
fn mesh_subset(model: &VoxModel, jitter_seed: u32, flame_subset: bool) -> Option<Mesh> {
    let cell_in_subset = |x: i32, y: i32, z: i32| -> Option<[f32; 4]> {
        model
            .color_at(x, y, z)
            .filter(|&color| is_flame_color(color) == flame_subset)
    };

    let mut buffers = MeshBuffers::default();
    let half_x = model.size_x as f32 / 2.0;
    let half_z = model.size_z as f32 / 2.0;

    for y in 0..model.size_y {
        for z in 0..model.size_z {
            for x in 0..model.size_x {
                let Some(base_color) = cell_in_subset(x, y, z) else {
                    continue;
                };
                let jitter =
                    0.90 + 0.18 * hash_to_unit(hash_3d(x, y, z, jitter_seed.wrapping_add(13)));
                let voxel_position = IVec3::new(x, y, z);

                for (normal, tangent_1, tangent_2) in FACE_DIRECTIONS {
                    let neighbor = voxel_position + normal;
                    if cell_in_subset(neighbor.x, neighbor.y, neighbor.z).is_some() {
                        continue;
                    }

                    let face_center = Vec3::new(
                        voxel_position.x as f32 + 0.5,
                        voxel_position.y as f32 + 0.5,
                        voxel_position.z as f32 + 0.5,
                    ) + normal.as_vec3() * 0.5;

                    let mut corners = [Vec3::ZERO; 4];
                    let mut corner_colors = [[0.0; 4]; 4];
                    let mut occlusion_levels = [3_u32; 4];

                    for (corner_index, &(along_1, along_2)) in QUAD_CORNERS.iter().enumerate() {
                        let corner_offset = (tangent_1.as_vec3() * along_1 as f32
                            + tangent_2.as_vec3() * along_2 as f32)
                            * 0.5;
                        let corner = face_center + corner_offset;
                        corners[corner_index] = Vec3::new(
                            (corner.x - half_x) * VOXEL_SIZE,
                            corner.y * VOXEL_SIZE,
                            (corner.z - half_z) * VOXEL_SIZE,
                        );

                        let occlusion_base = voxel_position + normal;
                        let side_1_solid = model
                            .color_at(
                                occlusion_base.x + tangent_1.x * along_1,
                                occlusion_base.y + tangent_1.y * along_1,
                                occlusion_base.z + tangent_1.z * along_1,
                            )
                            .is_some();
                        let side_2_solid = model
                            .color_at(
                                occlusion_base.x + tangent_2.x * along_2,
                                occlusion_base.y + tangent_2.y * along_2,
                                occlusion_base.z + tangent_2.z * along_2,
                            )
                            .is_some();
                        let corner_position =
                            occlusion_base + tangent_1 * along_1 + tangent_2 * along_2;
                        let corner_solid = model
                            .color_at(corner_position.x, corner_position.y, corner_position.z)
                            .is_some();
                        let occlusion_level = if flame_subset {
                            // Fire is self-lit: no ambient occlusion.
                            3
                        } else {
                            ambient_occlusion_level(side_1_solid, side_2_solid, corner_solid)
                        };
                        occlusion_levels[corner_index] = occlusion_level;

                        let brightness = (0.55 + 0.15 * occlusion_level as f32) * jitter;
                        corner_colors[corner_index] = [
                            base_color[0] * brightness,
                            base_color[1] * brightness,
                            base_color[2] * brightness,
                            base_color[3],
                        ];
                    }

                    let flip_diagonal = occlusion_levels[0] + occlusion_levels[2]
                        < occlusion_levels[1] + occlusion_levels[3];
                    buffers.add_quad(corners, normal.as_vec3(), corner_colors, flip_diagonal);
                }
            }
        }
    }

    if buffers.is_empty() {
        None
    } else {
        Some(buffers.into_mesh())
    }
}
