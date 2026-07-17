//! Culled-face voxel meshing with baked ambient occlusion.
//!
//! Produces two meshes per world: opaque terrain and translucent water.
//! Colors are per-vertex: material palette × biome dryness gradient ×
//! per-voxel hash jitter × corner ambient occlusion (plus a baked bounce
//! that keeps the island's underside readable) — the combination that
//! gives the MagicaVoxel-render look.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::noise::{hash_3d, hash_to_unit};
use crate::world::{
    Voxel, VoxelWorld, PLATEAU_FLOOR, VOXEL_SIZE, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z,
};

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
    Water,
}

fn group_of(voxel: Voxel) -> Option<MeshGroup> {
    match voxel {
        Voxel::Air => None,
        Voxel::Water => Some(MeshGroup::Water),
        Voxel::TallGrass
        | Voxel::FlowerPink
        | Voxel::FlowerWhite
        | Voxel::FlowerYellow
        | Voxel::FlowerBlue
        | Voxel::WaterWeed
        | Voxel::LilyPad
        | Voxel::LilyBloom
        | Voxel::Reed
        | Voxel::CattailHead => Some(MeshGroup::Cover),
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

/// Render chunks are full-height columns (like the RLE-thread layout):
/// 64×256×64 voxels = an 8 m footprint. Column chunks match the terrain's
/// heightmap structure, keep the entity count low (~hundreds), and their
/// AABBs (from actual vertices) stay tight, so bevy's per-entity frustum
/// culling works for the main view, the reflection view, and every shadow
/// cascade.
pub const CHUNK_SIZE: i32 = 64;

/// The meshes of one render chunk, split by material treatment (see the
/// field docs). `None` where the chunk has no such faces.
pub struct ChunkMeshes {
    /// Terrain faces above the water plane, plus the waterline plants
    /// (reeds, cattails) — the part the planar-reflection camera renders.
    pub terrain_above_water: Option<Mesh>,
    /// Meadow carpet above the plane (grass tufts, flowers, lily pads).
    /// Main view only: their reflections are invisible in the wavy
    /// half-res mirror, and skipping millions of cover quads makes the
    /// reflection pass cheap.
    pub meadow_cover: Option<Mesh>,
    /// Terrain faces at or below the water plane (river/lake beds, bank
    /// walls under the surface). Excluded from reflections: the mirrored
    /// camera sits under the plane, and underwater geometry would occlude
    /// the very reflection it is trying to render.
    pub terrain_below_water: Option<Mesh>,
    pub water: Option<Mesh>,
}

impl ChunkMeshes {
    fn is_empty(&self) -> bool {
        self.terrain_above_water.is_none()
            && self.meadow_cover.is_none()
            && self.terrain_below_water.is_none()
            && self.water.is_none()
    }
}

/// Mesh every render chunk in parallel; empty chunks (open sky beyond the
/// rim) are dropped.
pub fn build_all_chunk_meshes(world: &VoxelWorld, seed: u32, season: f32) -> Vec<ChunkMeshes> {
    use rayon::prelude::*;

    let chunks_x = (WORLD_SIZE_X as i32 + CHUNK_SIZE - 1) / CHUNK_SIZE;
    let chunks_z = (WORLD_SIZE_Z as i32 + CHUNK_SIZE - 1) / CHUNK_SIZE;
    let coordinates: Vec<(i32, i32)> = (0..chunks_z)
        .flat_map(|chunk_z| (0..chunks_x).map(move |chunk_x| (chunk_x, chunk_z)))
        .collect();

    coordinates
        .par_iter()
        .map(|&(chunk_x, chunk_z)| build_chunk_meshes(world, seed, season, chunk_x, chunk_z))
        .filter(|chunk| !chunk.is_empty())
        .collect()
}

/// Mesh one 64×256×64 column chunk. Neighbor lookups go through the whole
/// world, so face culling and ambient occlusion are seamless across chunk
/// borders. Vertices stay in world space (identity transforms) — bevy
/// derives each mesh's culling AABB from them.
fn build_chunk_meshes(
    world: &VoxelWorld,
    seed: u32,
    season: f32,
    chunk_x: i32,
    chunk_z: i32,
) -> ChunkMeshes {
    let mut above_water_buffers = MeshBuffers::default();
    let mut meadow_cover_buffers = MeshBuffers::default();
    let mut below_water_buffers = MeshBuffers::default();
    let mut water_buffers = MeshBuffers::default();
    // Faces above this (voxel units) belong to the reflection-visible mesh.
    let water_plane_y = (crate::world::WATER_LEVEL + 1) as f32 + 0.01;

    let half_x = WORLD_SIZE_X as f32 / 2.0;
    let half_z = WORLD_SIZE_Z as f32 / 2.0;

    let x_range = (chunk_x * CHUNK_SIZE)..((chunk_x + 1) * CHUNK_SIZE).min(WORLD_SIZE_X as i32);
    let z_range = (chunk_z * CHUNK_SIZE)..((chunk_z + 1) * CHUNK_SIZE).min(WORLD_SIZE_Z as i32);

    // The RLE world is unpacked once per chunk (plus a 1-cell apron for
    // neighbor culling and corner AO) into a dense window — voxel reads
    // below are plain array lookups, never run walks.
    let scratch = world.unpack_chunk(x_range.start, x_range.end, z_range.start, z_range.end);

    for y in 0..WORLD_SIZE_Y as i32 {
        for z in z_range.clone() {
            for x in x_range.clone() {
                let voxel = scratch.get(x, y, z);
                let Some(voxel_group) = group_of(voxel) else {
                    continue;
                };
                let voxel_position = IVec3::new(x, y, z);
                let base_color = voxel_color(world, &scratch, voxel, x, y, z, seed, season);
                let vertical_scale = visual_vertical_scale(&scratch, voxel, x, y, z, seed);

                for (normal, tangent_1, tangent_2) in FACE_DIRECTIONS {
                    let neighbor_position = voxel_position + normal;
                    let neighbor = scratch.get(
                        neighbor_position.x,
                        neighbor_position.y,
                        neighbor_position.z,
                    );
                    if group_of(neighbor) == Some(voxel_group) {
                        continue;
                    }
                    // A blooming lily is a green pad with a white blossom
                    // dot: only its top face takes the flower color.
                    let face_base_color = if voxel == Voxel::LilyBloom && normal != IVec3::Y {
                        voxel_color(world, &scratch, Voxel::LilyPad, x, y, z, seed, season)
                    } else {
                        base_color
                    };
                    // Water renders only its boundary against air (or thin
                    // ground cover — lily pads sit right on the surface and
                    // must not punch holes in it); its faces against terrain
                    // are invisible from any angle.
                    if voxel_group == MeshGroup::Water
                        && !(neighbor == Voxel::Air || group_of(neighbor) == Some(MeshGroup::Cover))
                    {
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
                        let side_1_solid = scratch
                            .get_offset(occlusion_base + tangent_1 * along_1)
                            .is_solid();
                        let side_2_solid = scratch
                            .get_offset(occlusion_base + tangent_2 * along_2)
                            .is_solid();
                        let corner_solid = scratch
                            .get_offset(occlusion_base + tangent_1 * along_1 + tangent_2 * along_2)
                            .is_solid();
                        let occlusion_level =
                            ambient_occlusion_level(side_1_solid, side_2_solid, corner_solid);
                        occlusion_levels[corner_index] = occlusion_level;

                        let brightness = 0.55 + 0.15 * occlusion_level as f32;
                        corner_colors[corner_index] = [
                            face_base_color[0] * brightness,
                            face_base_color[1] * brightness,
                            face_base_color[2] * brightness,
                            face_base_color[3],
                        ];
                    }

                    // Connect the diagonal across the brighter corner pair.
                    let flip_diagonal = occlusion_levels[0] + occlusion_levels[2]
                        < occlusion_levels[1] + occlusion_levels[3];

                    let buffers = match voxel_group {
                        MeshGroup::Terrain | MeshGroup::Cover => {
                            if face_center.y <= water_plane_y {
                                &mut below_water_buffers
                            } else if voxel_group == MeshGroup::Terrain
                                || matches!(voxel, Voxel::Reed | Voxel::CattailHead)
                            {
                                &mut above_water_buffers
                            } else {
                                &mut meadow_cover_buffers
                            }
                        }
                        MeshGroup::Water => &mut water_buffers,
                    };
                    buffers.add_quad(corners, normal.as_vec3(), corner_colors, flip_diagonal);
                }
            }
        }
    }

    let into_optional_mesh = |buffers: MeshBuffers| {
        if buffers.is_empty() {
            None
        } else {
            Some(buffers.into_mesh())
        }
    };
    ChunkMeshes {
        terrain_above_water: into_optional_mesh(above_water_buffers),
        meadow_cover: into_optional_mesh(meadow_cover_buffers),
        terrain_below_water: into_optional_mesh(below_water_buffers),
        water: into_optional_mesh(water_buffers),
    }
}

/// Visual height of a voxel as a fraction of a full cube. Ground cover is
/// squashed — tufts vary per-cell so the meadow reads as an uneven carpet.
fn visual_vertical_scale(
    scratch: &crate::world::ChunkScratch,
    voxel: Voxel,
    x: i32,
    y: i32,
    z: i32,
    seed: u32,
) -> f32 {
    match voxel {
        Voxel::TallGrass => {
            // Stalks may stack (or carry a flower on top): those cells stay
            // full height so the stalk is continuous; only the tip tapers.
            if matches!(
                scratch.get(x, y + 1, z),
                Voxel::TallGrass
                    | Voxel::FlowerPink
                    | Voxel::FlowerWhite
                    | Voxel::FlowerYellow
                    | Voxel::FlowerBlue
            ) {
                1.0
            } else {
                0.30 + 0.40 * hash_to_unit(hash_3d(x, y ^ 0x51, z, seed.wrapping_add(9)))
            }
        }
        Voxel::FlowerPink | Voxel::FlowerWhite | Voxel::FlowerYellow | Voxel::FlowerBlue => 0.55,
        Voxel::WaterWeed => {
            0.35 + 0.55 * hash_to_unit(hash_3d(x, y ^ 0x67, z, seed.wrapping_add(11)))
        }
        Voxel::LilyPad => 0.10,
        Voxel::LilyBloom => 0.22,
        Voxel::CattailHead => 0.75,
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
/// vegetation, waterside-lushness and altitude gradients, seasonal foliage
/// (0 = summer, 1 = autumn, per-tree turning order from the tone map),
/// water depth tint, and per-voxel brightness jitter.
fn voxel_color(
    world: &VoxelWorld,
    scratch: &crate::world::ChunkScratch,
    voxel: Voxel,
    x: i32,
    y: i32,
    z: i32,
    seed: u32,
    season: f32,
) -> [f32; 4] {
    let jitter_roll = hash_to_unit(hash_3d(x, y, z, seed.wrapping_add(3)));
    let dryness = world.dryness_at(x, z);
    let altitude_meters = (y - crate::world::WATER_LEVEL) as f32 * VOXEL_SIZE;
    // Lush right at the water, paling as the land climbs.
    let lushness = crate::noise::smoothstep(9.0, 1.5, world.water_distance_at(x, z));
    let paling = crate::noise::smoothstep(3.5, 11.0, altitude_meters);

    let (srgb, alpha, jitter) = match voxel {
        Voxel::Grass => {
            // Ground reads as dirt between the grass clumps and olive-green
            // under them, before the biome dryness sweep.
            let patchiness = crate::noise::smoothstep(0.35, 0.70, world.cover_at(x, z));
            let mut ground = lerp_rgb([0.50, 0.42, 0.30], [0.41, 0.52, 0.29], patchiness);
            ground = lerp_rgb(ground, [0.33, 0.50, 0.25], lushness * 0.55);
            ground = lerp_rgb(ground, [0.57, 0.55, 0.33], paling * 0.55);
            ground = lerp_rgb(ground, [0.63, 0.52, 0.28], season * 0.55);
            (
                lerp_rgb(ground, [0.72, 0.64, 0.38], dryness),
                1.0,
                0.92 + 0.16 * jitter_roll,
            )
        }
        Voxel::TallGrass => {
            let mut blade = [0.28, 0.45, 0.23];
            blade = lerp_rgb(blade, [0.23, 0.46, 0.20], lushness * 0.45);
            blade = lerp_rgb(blade, [0.62, 0.50, 0.26], season * 0.60);
            (
                lerp_rgb(blade, [0.66, 0.60, 0.35], dryness),
                1.0,
                0.85 + 0.30 * jitter_roll,
            )
        }
        Voxel::Leaves | Voxel::LeavesDark => {
            // Oak / willow / bush canopy. Per-tree tone picks the summer
            // hue, the autumn target color, and how early the tree turns.
            let tone = world.tree_tone_at(x, z);
            let summer = lerp_rgb([0.30, 0.47, 0.22], [0.46, 0.54, 0.25], tone);
            let autumn = if tone < 0.33 {
                [0.72, 0.30, 0.10]
            } else if tone < 0.66 {
                [0.80, 0.47, 0.12]
            } else {
                [0.82, 0.62, 0.16]
            };
            let turn_start = 0.15 + tone * 0.45;
            let turn = crate::noise::smoothstep(turn_start, (turn_start + 0.30).min(1.0), season);
            let mut leaf = lerp_rgb(summer, autumn, turn);
            leaf = lerp_rgb(leaf, [0.55, 0.55, 0.28], dryness * 0.5);
            if voxel == Voxel::LeavesDark {
                leaf = [leaf[0] * 0.74, leaf[1] * 0.74, leaf[2] * 0.74];
            }
            (leaf, 1.0, 0.86 + 0.26 * jitter_roll)
        }
        Voxel::LeavesBirch => {
            let tone = world.tree_tone_at(x, z);
            let summer = lerp_rgb([0.47, 0.56, 0.26], [0.55, 0.60, 0.30], tone);
            // Birches turn early and go pure gold.
            let turn = crate::noise::smoothstep(tone * 0.35, tone * 0.35 + 0.25, season);
            (
                lerp_rgb(summer, [0.88, 0.66, 0.14], turn),
                1.0,
                0.86 + 0.26 * jitter_roll,
            )
        }
        Voxel::LeavesPine => {
            let tone = world.tree_tone_at(x, z);
            let needles = lerp_rgb([0.18, 0.32, 0.23], [0.24, 0.37, 0.25], tone);
            // Evergreens just dull a little as the year fades.
            (
                lerp_rgb(needles, [0.24, 0.33, 0.22], season * 0.35),
                1.0,
                0.88 + 0.22 * jitter_roll,
            )
        }
        Voxel::Dirt => ([0.44, 0.32, 0.22], 1.0, 0.92 + 0.16 * jitter_roll),
        Voxel::Sand => {
            // Wet sand below the waterline is darker, so the shallows stay
            // believable through the transparent water.
            if y <= crate::world::WATER_LEVEL {
                ([0.38, 0.33, 0.23], 1.0, 0.94 + 0.12 * jitter_roll)
            } else {
                ([0.86, 0.77, 0.55], 1.0, 0.94 + 0.12 * jitter_roll)
            }
        }
        Voxel::Sediment => ([0.17, 0.16, 0.11], 1.0, 0.90 + 0.20 * jitter_roll),
        Voxel::Stone => ([0.52, 0.52, 0.55], 1.0, 0.90 + 0.20 * jitter_roll),
        Voxel::Trunk => ([0.45, 0.31, 0.19], 1.0, 0.88 + 0.24 * jitter_roll),
        Voxel::TrunkBirch => {
            // White paper bark broken by dark horizontal flecks.
            if jitter_roll < 0.16 {
                ([0.20, 0.18, 0.16], 1.0, 0.90 + 0.20 * jitter_roll)
            } else {
                ([0.80, 0.78, 0.72], 1.0, 0.92 + 0.14 * jitter_roll)
            }
        }
        Voxel::FlowerPink => ([0.93, 0.55, 0.75], 1.0, 1.0),
        Voxel::FlowerWhite => ([0.96, 0.95, 0.90], 1.0, 1.0),
        Voxel::FlowerYellow => ([0.95, 0.83, 0.35], 1.0, 1.0),
        Voxel::FlowerBlue => ([0.45, 0.52, 0.92], 1.0, 1.0),
        Voxel::WaterWeed => ([0.15, 0.30, 0.19], 1.0, 0.80 + 0.40 * jitter_roll),
        Voxel::LilyPad => ([0.26, 0.50, 0.24], 1.0, 0.90 + 0.20 * jitter_roll),
        Voxel::LilyBloom => ([0.95, 0.92, 0.85], 1.0, 0.95 + 0.10 * jitter_roll),
        Voxel::CattailHead => ([0.32, 0.18, 0.08], 1.0, 0.88 + 0.24 * jitter_roll),
        Voxel::Reed => {
            let stalk = lerp_rgb([0.55, 0.56, 0.31], [0.63, 0.52, 0.26], season * 0.5);
            (stalk, 1.0, 0.85 + 0.30 * jitter_roll)
        }
        Voxel::Snow => ([0.92, 0.93, 0.96], 1.0, 0.96 + 0.07 * jitter_roll),
        Voxel::Water => {
            // Deeper water → darker blue and more opaque. (The water shader
            // recomputes absorption from real optical depth; this vertex
            // tint is its per-column fallback hint.)
            let mut depth = 0;
            while depth < 8 && scratch.get(x, y - 1 - depth, z) == Voxel::Water {
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

    // Baked bounce light: the island's sculpted underside faces away from
    // the sun and would render near-black on ambient alone. The cloud sea
    // below is a bright diffuse reflector, so brighten those voxels in the
    // vertex colors — same philosophy as the baked AO. Slightly warm, so
    // the belly reads as sunlit earth.
    let underside_bounce = if y < PLATEAU_FLOOR {
        [1.9, 1.75, 1.6]
    } else {
        [1.0, 1.0, 1.0]
    };

    let linear = Color::srgb(
        (srgb[0] * jitter * underside_bounce[0]).min(1.0),
        (srgb[1] * jitter * underside_bounce[1]).min(1.0),
        (srgb[2] * jitter * underside_bounce[2]).min(1.0),
    )
    .to_linear();
    [linear.red, linear.green, linear.blue, alpha]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_meshes_are_consistent_and_nonempty() {
        let world = VoxelWorld::generate(1, 0.0);
        let chunks = build_all_chunk_meshes(&world, 1, 0.0);
        assert!(
            chunks.len() > 30,
            "expected the island to span many chunks, got {}",
            chunks.len()
        );

        let mut above_total = 0;
        let mut meadow_total = 0;
        let mut below_total = 0;
        let mut water_total = 0;
        for chunk in &chunks {
            for (label, mesh) in [
                ("terrain above water", &chunk.terrain_above_water),
                ("meadow cover", &chunk.meadow_cover),
                ("terrain below water", &chunk.terrain_below_water),
                ("water", &chunk.water),
            ] {
                let Some(mesh) = mesh else {
                    continue;
                };
                let vertex_count = mesh.count_vertices();
                assert!(vertex_count > 0, "{label} mesh present but empty");
                assert_eq!(vertex_count % 4, 0, "{label} mesh has partial quads");
                let index_count = mesh.indices().expect("indices").len();
                assert_eq!(index_count % 6, 0, "{label} mesh has partial quad indices");
            }
            above_total += chunk
                .terrain_above_water
                .as_ref()
                .map_or(0, Mesh::count_vertices);
            meadow_total += chunk.meadow_cover.as_ref().map_or(0, Mesh::count_vertices);
            below_total += chunk
                .terrain_below_water
                .as_ref()
                .map_or(0, Mesh::count_vertices);
            water_total += chunk.water.as_ref().map_or(0, Mesh::count_vertices);
        }
        assert!(above_total > 100_000, "above-water terrain too small");
        assert!(meadow_total > 10_000, "meadow cover too small");
        assert!(below_total > 100_000, "underwater terrain too small");
        assert!(water_total > 10_000, "water too small");
    }
}
