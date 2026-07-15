//! Culled-face voxel meshing with baked ambient occlusion.
//!
//! Produces three meshes per world: opaque terrain, clouds (separate
//! material so they can glow slightly and skip shadow casting), and
//! translucent water. Colors are per-vertex: material palette × biome
//! dryness gradient × per-voxel hash jitter × corner ambient occlusion —
//! the combination that gives the MagicaVoxel-render look.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::noise::{hash_3d, hash_to_unit};
use crate::world::{Voxel, VoxelWorld, VOXEL_SIZE, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z};

/// (face normal, tangent_1, tangent_2) with `tangent_1 × tangent_2 = normal`,
/// so corners emitted counter-clockwise face outward.
pub(crate) const FACE_DIRECTIONS: [(IVec3, IVec3, IVec3); 6] = [
    (IVec3::X, IVec3::Y, IVec3::Z),
    (IVec3::NEG_X, IVec3::Z, IVec3::Y),
    (IVec3::Y, IVec3::Z, IVec3::X),
    (IVec3::NEG_Y, IVec3::X, IVec3::Z),
    (IVec3::Z, IVec3::X, IVec3::Y),
    (IVec3::NEG_Z, IVec3::Y, IVec3::X),
];

/// Quad corners in tangent space, counter-clockwise: (-,-) (+,-) (+,+) (-,+).
pub(crate) const QUAD_CORNERS: [(i32, i32); 4] = [(-1, -1), (1, -1), (1, 1), (-1, 1)];

#[derive(Clone, Copy, PartialEq, Eq)]
enum MeshGroup {
    Terrain,
    /// Squashed ground-cover voxels (grass tufts, flowers). A separate
    /// culling group: they render below full voxel height, so faces they
    /// touch can never be culled as hidden — neither theirs nor a
    /// full-height neighbor's, or see-through holes open up.
    Cover,
    Cloud,
    Water,
}

fn group_of(voxel: Voxel) -> Option<MeshGroup> {
    match voxel {
        Voxel::Air => None,
        Voxel::Water => Some(MeshGroup::Water),
        Voxel::Cloud => Some(MeshGroup::Cloud),
        Voxel::TallGrass | Voxel::FlowerPink | Voxel::FlowerWhite | Voxel::FlowerYellow => {
            Some(MeshGroup::Cover)
        }
        _ => Some(MeshGroup::Terrain),
    }
}

#[derive(Default)]
pub(crate) struct MeshBuffers {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl MeshBuffers {
    pub(crate) fn add_quad(
        &mut self,
        corners: [Vec3; 4],
        normal: Vec3,
        corner_colors: [[f32; 4]; 4],
        flip_diagonal: bool,
    ) {
        let base_index = self.positions.len() as u32;
        for corner_index in 0..4 {
            self.positions.push(corners[corner_index].to_array());
            self.normals.push(normal.to_array());
            self.colors.push(corner_colors[corner_index]);
        }
        // Two triangles; the diagonal choice follows ambient occlusion so
        // interpolation never smears a dark corner across the whole quad.
        let quad_indices: [u32; 6] = if flip_diagonal {
            [1, 2, 3, 1, 3, 0]
        } else {
            [0, 1, 2, 0, 2, 3]
        };
        self.indices
            .extend(quad_indices.iter().map(|&index| base_index + index));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    pub(crate) fn into_mesh(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
        .with_inserted_indices(Indices::U32(self.indices))
    }
}

pub struct WorldMeshes {
    pub terrain: Mesh,
    pub clouds: Mesh,
    pub water: Mesh,
}

pub fn build_meshes(world: &VoxelWorld, seed: u32) -> WorldMeshes {
    let mut terrain_buffers = MeshBuffers::default();
    let mut cloud_buffers = MeshBuffers::default();
    let mut water_buffers = MeshBuffers::default();

    let half_x = WORLD_SIZE_X as f32 / 2.0;
    let half_z = WORLD_SIZE_Z as f32 / 2.0;

    for y in 0..WORLD_SIZE_Y as i32 {
        for z in 0..WORLD_SIZE_Z as i32 {
            for x in 0..WORLD_SIZE_X as i32 {
                let voxel = world.get(x, y, z);
                let Some(voxel_group) = group_of(voxel) else {
                    continue;
                };
                let voxel_position = IVec3::new(x, y, z);
                let base_color = voxel_color(world, voxel, x, y, z, seed);
                let vertical_scale = visual_vertical_scale(voxel, x, y, z, seed);

                for (normal, tangent_1, tangent_2) in FACE_DIRECTIONS {
                    let neighbor_position = voxel_position + normal;
                    let neighbor = world.get(
                        neighbor_position.x,
                        neighbor_position.y,
                        neighbor_position.z,
                    );
                    if group_of(neighbor) == Some(voxel_group) {
                        continue;
                    }
                    // Water renders only its boundary against air; its faces
                    // against terrain are invisible from any angle.
                    if voxel_group == MeshGroup::Water && neighbor != Voxel::Air {
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
                        let mut corner = face_center + corner_offset;
                        // Ground-cover voxels render squashed (grass carpet,
                        // not knee-high blocks): compress the voxel's local
                        // y-extent while keeping its footprint.
                        corner.y = voxel_position.y as f32
                            + (corner.y - voxel_position.y as f32) * vertical_scale;
                        corners[corner_index] = Vec3::new(
                            (corner.x - half_x) * VOXEL_SIZE,
                            corner.y * VOXEL_SIZE,
                            (corner.z - half_z) * VOXEL_SIZE,
                        );

                        let occlusion_base = voxel_position + normal;
                        let side_1_solid = world
                            .get_offset(occlusion_base + tangent_1 * along_1)
                            .is_solid();
                        let side_2_solid = world
                            .get_offset(occlusion_base + tangent_2 * along_2)
                            .is_solid();
                        let corner_solid = world
                            .get_offset(occlusion_base + tangent_1 * along_1 + tangent_2 * along_2)
                            .is_solid();
                        let occlusion_level =
                            ambient_occlusion_level(side_1_solid, side_2_solid, corner_solid);
                        occlusion_levels[corner_index] = occlusion_level;

                        let brightness = 0.55 + 0.15 * occlusion_level as f32;
                        corner_colors[corner_index] = [
                            base_color[0] * brightness,
                            base_color[1] * brightness,
                            base_color[2] * brightness,
                            base_color[3],
                        ];
                    }

                    // Connect the diagonal across the brighter corner pair.
                    let flip_diagonal = occlusion_levels[0] + occlusion_levels[2]
                        < occlusion_levels[1] + occlusion_levels[3];

                    let buffers = match voxel_group {
                        MeshGroup::Terrain | MeshGroup::Cover => &mut terrain_buffers,
                        MeshGroup::Cloud => &mut cloud_buffers,
                        MeshGroup::Water => &mut water_buffers,
                    };
                    buffers.add_quad(corners, normal.as_vec3(), corner_colors, flip_diagonal);
                }
            }
        }
    }

    WorldMeshes {
        terrain: terrain_buffers.into_mesh(),
        clouds: cloud_buffers.into_mesh(),
        water: water_buffers.into_mesh(),
    }
}

/// Visual height of a voxel as a fraction of a full cube. Ground cover is
/// squashed — tufts vary per-cell so the meadow reads as an uneven carpet.
fn visual_vertical_scale(voxel: Voxel, x: i32, y: i32, z: i32, seed: u32) -> f32 {
    match voxel {
        Voxel::TallGrass => {
            0.30 + 0.40 * hash_to_unit(hash_3d(x, y ^ 0x51, z, seed.wrapping_add(9)))
        }
        Voxel::FlowerPink | Voxel::FlowerWhite | Voxel::FlowerYellow => 0.55,
        _ => 1.0,
    }
}

pub(crate) fn ambient_occlusion_level(
    side_1_solid: bool,
    side_2_solid: bool,
    corner_solid: bool,
) -> u32 {
    if side_1_solid && side_2_solid {
        0
    } else {
        3 - (side_1_solid as u32 + side_2_solid as u32 + corner_solid as u32)
    }
}

fn lerp_rgb(from: [f32; 3], to: [f32; 3], t: f32) -> [f32; 3] {
    [
        from[0] + (to[0] - from[0]) * t,
        from[1] + (to[1] - from[1]) * t,
        from[2] + (to[2] - from[2]) * t,
    ]
}

/// Base color of one voxel: palette entry, biome-dryness gradient for
/// vegetation, water depth tint, and per-voxel brightness jitter.
fn voxel_color(world: &VoxelWorld, voxel: Voxel, x: i32, y: i32, z: i32, seed: u32) -> [f32; 4] {
    let jitter_roll = hash_to_unit(hash_3d(x, y, z, seed.wrapping_add(3)));
    let dryness = world.dryness_at(x, z);

    let (srgb, alpha, jitter) = match voxel {
        Voxel::Grass => {
            // Ground reads as dirt between the grass clumps and olive-green
            // under them, before the biome dryness sweep.
            let patchiness = crate::noise::smoothstep(0.35, 0.70, world.cover_at(x, z));
            let ground = lerp_rgb([0.50, 0.42, 0.30], [0.41, 0.52, 0.29], patchiness);
            (
                lerp_rgb(ground, [0.72, 0.64, 0.38], dryness),
                1.0,
                0.92 + 0.16 * jitter_roll,
            )
        }
        Voxel::TallGrass => (
            lerp_rgb([0.28, 0.45, 0.23], [0.66, 0.60, 0.35], dryness),
            1.0,
            0.85 + 0.30 * jitter_roll,
        ),
        Voxel::Leaves => (
            lerp_rgb([0.36, 0.50, 0.27], [0.55, 0.55, 0.28], dryness),
            1.0,
            0.86 + 0.26 * jitter_roll,
        ),
        Voxel::LeavesDark => (
            lerp_rgb([0.25, 0.39, 0.21], [0.46, 0.47, 0.24], dryness),
            1.0,
            0.86 + 0.26 * jitter_roll,
        ),
        Voxel::Dirt => ([0.44, 0.32, 0.22], 1.0, 0.92 + 0.16 * jitter_roll),
        Voxel::Sand => ([0.86, 0.77, 0.55], 1.0, 0.94 + 0.12 * jitter_roll),
        Voxel::Stone => ([0.52, 0.52, 0.55], 1.0, 0.90 + 0.20 * jitter_roll),
        Voxel::Trunk => ([0.45, 0.31, 0.19], 1.0, 0.88 + 0.24 * jitter_roll),
        Voxel::FlowerPink => ([0.93, 0.55, 0.75], 1.0, 1.0),
        Voxel::FlowerWhite => ([0.96, 0.95, 0.90], 1.0, 1.0),
        Voxel::FlowerYellow => ([0.95, 0.83, 0.35], 1.0, 1.0),
        Voxel::Cloud => ([0.97, 0.97, 1.0], 1.0, 0.985 + 0.03 * jitter_roll),
        Voxel::Snow => ([0.92, 0.93, 0.96], 1.0, 0.96 + 0.07 * jitter_roll),
        Voxel::Water => {
            // Deeper water → darker blue and more opaque.
            let mut depth = 0;
            while depth < 8 && world.get(x, y - 1 - depth, z) == Voxel::Water {
                depth += 1;
            }
            let depth_amount = depth as f32 / 8.0;
            (
                lerp_rgb([0.30, 0.72, 0.82], [0.08, 0.32, 0.60], depth_amount),
                0.55 + 0.30 * depth_amount,
                1.0,
            )
        }
        Voxel::Air => unreachable!("air voxels are never meshed"),
    };

    let linear = Color::srgb(
        (srgb[0] * jitter).min(1.0),
        (srgb[1] * jitter).min(1.0),
        (srgb[2] * jitter).min(1.0),
    )
    .to_linear();
    [linear.red, linear.green, linear.blue, alpha]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meshes_are_consistent_and_nonempty() {
        let world = VoxelWorld::generate(1);
        let world_meshes = build_meshes(&world, 1);
        for (label, mesh) in [
            ("terrain", &world_meshes.terrain),
            ("clouds", &world_meshes.clouds),
            ("water", &world_meshes.water),
        ] {
            let vertex_count = mesh.count_vertices();
            assert!(vertex_count > 0, "{label} mesh is empty");
            assert_eq!(vertex_count % 4, 0, "{label} mesh has partial quads");
            let index_count = mesh.indices().expect("indices").len();
            assert_eq!(index_count % 6, 0, "{label} mesh has partial quad indices");
        }
    }
}
