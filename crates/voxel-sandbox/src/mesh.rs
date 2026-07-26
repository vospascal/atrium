//! Culled-face voxel meshing with baked ambient occlusion.
//!
//! Produces two meshes per world: opaque terrain and translucent water.
//! Colors are per-vertex: material palette × biome dryness gradient ×
//! corner ambient occlusion (plus a baked bounce that keeps the island's
//! underside readable). The per-voxel brightness *jitter* is applied in the
//! terrain fragment shader instead (see `voxel_material.rs`) so it survives
//! greedy meshing; the mesher passes its per-type amplitude in vertex alpha.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use voxel_core::noise::{hash_3d, hash_to_unit};
use voxel_core::voxel_source::VoxelSource;
use voxel_core::world::{
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
    /// Tree-leaf voxels. Rendered as shrunken, per-voxel-offset cubes (all
    /// six faces) so canopies read as fluffy MagicaVoxel-style clumps rather
    /// than solid blobs. Emitted in a dedicated pass, not greedy-merged.
    Canopy,
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
        Voxel::Leaves | Voxel::LeavesDark | Voxel::LeavesBirch | Voxel::LeavesPine => {
            Some(MeshGroup::Canopy)
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
    /// Tree-leaf confetti (shrunken, offset cubes). Above the plane, so it is
    /// reflection-visible like the above-water terrain.
    pub canopy: Option<Mesh>,
    /// Solid inner canopy behind the confetti: the (cheap) tree shadow caster
    /// and gap backing. Confetti covers it; only its silhouette matters.
    pub canopy_solid: Option<Mesh>,
    pub water: Option<Mesh>,
}

impl ChunkMeshes {
    fn is_empty(&self) -> bool {
        self.terrain_above_water.is_none()
            && self.meadow_cover.is_none()
            && self.terrain_below_water.is_none()
            && self.canopy.is_none()
            && self.canopy_solid.is_none()
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
        .map(|&(chunk_x, chunk_z)| {
            let scratch = unpack_chunk_window(world, chunk_x, chunk_z);
            build_chunk_meshes(
                world,
                &scratch,
                seed,
                season,
                chunk_x,
                chunk_z,
                AmbientOcclusion::PerFragment,
            )
        })
        .filter(|chunk| !chunk.is_empty())
        .collect()
}

/// Unpack the dense voxel window one chunk needs: its own columns plus the
/// 1-cell apron used for neighbor face culling and corner ambient occlusion.
///
/// Separate from [`build_chunk_meshes`] so a caller that needs the same voxels
/// for something else — the streamed world packs its shader occupancy bitset
/// from them — can unpack once and share, instead of generating the window twice.
pub fn unpack_chunk_window(
    world: &impl VoxelSource,
    chunk_x: i32,
    chunk_z: i32,
) -> voxel_core::world::ChunkScratch {
    world.unpack_chunk(
        chunk_x * CHUNK_SIZE,
        (chunk_x + 1) * CHUNK_SIZE,
        chunk_z * CHUNK_SIZE,
        (chunk_z + 1) * CHUNK_SIZE,
    )
}

/// Where a chunk's terrain ambient occlusion comes from.
///
/// The island uses [`AmbientOcclusion::PerFragment`]: the mesher merges flat
/// faces freely and the shader recomputes exact corner AO per fragment from a
/// solid-occupancy bitset. That bitset has to cover the chunk, so **each chunk
/// needs its own material** — fine for one world built once, but in the streamed
/// world it means a material per resident chunk, which stops bevy batching chunk
/// meshes together. Measured on this world, that cost ~2.5× the frame time.
///
/// So streamed chunks use [`AmbientOcclusion::Baked`]: AO is sampled at the
/// merged quad's four corners and multiplied into its vertex colours, and the
/// vertex alpha carries the shader's existing "AO already baked" sentinel (the
/// same one cover geometry uses). No occupancy buffer, so **every streamed chunk
/// shares one material**. The trade-off is honest: AO interpolates across a
/// merged quad instead of being exact per fragment, so an occluder in the middle
/// of a large flat face is missed. Uniform cases (a wall base, a step) still read
/// correctly because every face along them samples the same occlusion.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AmbientOcclusion {
    /// Shader recomputes AO from an occupancy bitset (needs a per-chunk material).
    PerFragment,
    /// AO baked into vertex colours (lets all chunks share one material).
    Baked,
}

/// The offset added to vertex alpha to tell the terrain shader "ambient occlusion
/// is already baked into these colours, don't apply your own". Matches the
/// sentinel `voxel_terrain.wgsl` already uses for cover geometry.
const BAKED_AO_SENTINEL: f32 = 10.0;

/// Mesh one 64×256×64 column chunk from its unpacked window (see
/// [`unpack_chunk_window`]). Neighbor lookups reach into the window's apron, so
/// face culling and ambient occlusion are seamless across chunk borders.
/// Vertices stay in world space (identity transforms) — bevy derives each mesh's
/// culling AABB from them.
pub fn build_chunk_meshes(
    world: &impl VoxelSource,
    scratch: &voxel_core::world::ChunkScratch,
    seed: u32,
    season: f32,
    chunk_x: i32,
    chunk_z: i32,
    ambient_occlusion: AmbientOcclusion,
) -> ChunkMeshes {
    let mut above_water_buffers = MeshBuffers::default();
    let mut meadow_cover_buffers = MeshBuffers::default();
    let mut below_water_buffers = MeshBuffers::default();
    let mut canopy_buffers = MeshBuffers::default();
    let mut canopy_solid_buffers = MeshBuffers::default();
    let mut water_buffers = MeshBuffers::default();
    // Faces above this (voxel units) belong to the reflection-visible mesh.
    let water_plane_y = (voxel_core::world::WATER_LEVEL + 1) as f32 + 0.01;

    // Vertex-centering offset: (half_x, half_z) for the island (so it straddles
    // the origin), (0, 0) for the infinite streamed world.
    let (half_x, half_z) = world.world_offset();

    // Raw chunk span in world voxels — no world-size clamp, so streamed chunks
    // at any coordinate mesh correctly. Each source clamps its own bounds in
    // `unpack_chunk` (the island returns air past its footprint). Voxel reads
    // below go to the caller's dense window, never to run walks.
    let x_range = (chunk_x * CHUNK_SIZE)..((chunk_x + 1) * CHUNK_SIZE);
    let z_range = (chunk_z * CHUNK_SIZE)..((chunk_z + 1) * CHUNK_SIZE);

    for y in 0..WORLD_SIZE_Y as i32 {
        for z in z_range.clone() {
            for x in x_range.clone() {
                let voxel = scratch.get(x, y, z);
                let Some(voxel_group) = group_of(voxel) else {
                    continue;
                };
                // Full-height terrain is emitted (merged) by the greedy pass,
                // and canopy leaves by the confetti pass; this per-voxel loop
                // only handles cover and water.
                if matches!(voxel_group, MeshGroup::Terrain | MeshGroup::Canopy) {
                    continue;
                }
                // Tall grass is spawned as instanced clumps (see `grass.rs`),
                // not baked here.
                if voxel == Voxel::TallGrass {
                    continue;
                }
                let voxel_position = IVec3::new(x, y, z);
                let base_color = voxel_color(world, scratch, voxel, x, y, z, seed, season);
                let vertical_scale = visual_vertical_scale(scratch, voxel, x, y, z, seed);
                let footprint_scale = visual_footprint_scale(voxel);

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
                        voxel_color(world, scratch, Voxel::LilyPad, x, y, z, seed, season)
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
                        // Ground-cover voxels render squashed AND narrowed:
                        // compress the local y-extent (grass carpet, not
                        // knee-high blocks) and shrink the x/z footprint toward
                        // the voxel center so tufts read as thin blades rather
                        // than fat cubes. Cover is its own culling group, so
                        // shrinking the footprint never opens holes in the
                        // terrain or neighboring cover.
                        corner.y = voxel_position.y as f32
                            + (corner.y - voxel_position.y as f32) * vertical_scale;
                        let center_x = voxel_position.x as f32 + 0.5;
                        let center_z = voxel_position.z as f32 + 0.5;
                        corner.x = center_x + (corner.x - center_x) * footprint_scale;
                        corner.z = center_z + (corner.z - center_z) * footprint_scale;
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
                        // Skipped above; the confetti pass emits these.
                        MeshGroup::Canopy => unreachable!("canopy is emitted separately"),
                    };
                    buffers.add_quad(corners, normal.as_vec3(), corner_colors, flip_diagonal);
                }
            }
        }
    }

    // Merge the flat, fully-open terrain faces skipped above into big quads.
    greedy_merge_terrain(
        world,
        scratch,
        seed,
        season,
        x_range.clone(),
        z_range.clone(),
        half_x,
        half_z,
        water_plane_y,
        ambient_occlusion,
        &mut above_water_buffers,
        &mut below_water_buffers,
    );

    // Emit tree-leaf voxels as fluffy confetti cubes, plus a cheap solid inner
    // canopy behind them (the shadow caster + gap backing).
    emit_canopy(
        world,
        scratch,
        seed,
        season,
        x_range.clone(),
        z_range.clone(),
        half_x,
        half_z,
        &mut canopy_buffers,
    );
    emit_canopy_solid(
        world,
        scratch,
        seed,
        season,
        x_range.clone(),
        z_range.clone(),
        half_x,
        half_z,
        &mut canopy_solid_buffers,
    );

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
        canopy: into_optional_mesh(canopy_buffers),
        canopy_solid: into_optional_mesh(canopy_solid_buffers),
        water: into_optional_mesh(water_buffers),
    }
}

/// A full-height terrain face that is safe to greedy-merge: exposed, and
/// fully ambient-occlusion-open (all four corners level 3) so there is no
/// baked shading to lose. Carries its un-jittered color (alpha = jitter
/// amplitude), voxel type (the merge key), and water-plane bucket.
struct MergeFace {
    color: [f32; 4],
    voxel: Voxel,
    below_water: bool,
}

#[allow(clippy::too_many_arguments)]
fn terrain_merge_face(
    world: &impl VoxelSource,
    scratch: &voxel_core::world::ChunkScratch,
    seed: u32,
    season: f32,
    x: i32,
    y: i32,
    z: i32,
    normal: IVec3,
    water_plane_y: f32,
) -> Option<MergeFace> {
    let voxel = scratch.get(x, y, z);
    if group_of(voxel) != Some(MeshGroup::Terrain) {
        return None;
    }
    let position = IVec3::new(x, y, z);
    let neighbor_position = position + normal;
    let neighbor = scratch.get(
        neighbor_position.x,
        neighbor_position.y,
        neighbor_position.z,
    );
    // A face exists only where the neighbor is not also terrain. Ambient
    // occlusion is no longer a merge gate — the shader recomputes it per
    // fragment — so every exposed flat terrain face is mergeable.
    if group_of(neighbor) == Some(MeshGroup::Terrain) {
        return None;
    }
    let face_center_y = y as f32 + 0.5 + normal.y as f32 * 0.5;
    Some(MergeFace {
        color: voxel_color(world, scratch, voxel, x, y, z, seed, season),
        voxel,
        below_water: face_center_y <= water_plane_y,
    })
}

/// Greedy-merge the flat, fully-open terrain faces (per face direction, per
/// slice) into maximal rectangles — one quad each instead of one per voxel.
/// Corner colors are sampled at the rectangle's four corner voxels so the
/// low-frequency biome/season gradients still interpolate across the quad;
/// the per-voxel brightness jitter is added later by the terrain shader.
#[allow(clippy::too_many_arguments)]
fn greedy_merge_terrain(
    world: &impl VoxelSource,
    scratch: &voxel_core::world::ChunkScratch,
    seed: u32,
    season: f32,
    x_range: std::ops::Range<i32>,
    z_range: std::ops::Range<i32>,
    half_x: f32,
    half_z: f32,
    water_plane_y: f32,
    ambient_occlusion: AmbientOcclusion,
    above_water: &mut MeshBuffers,
    below_water: &mut MeshBuffers,
) {
    #[derive(Clone, Copy)]
    struct Cell {
        color: [f32; 4],
        voxel: Voxel,
        below: bool,
    }

    let axis_of = |vector: IVec3| {
        if vector.x != 0 {
            0
        } else if vector.y != 0 {
            1
        } else {
            2
        }
    };

    for (normal, tangent_1, tangent_2) in FACE_DIRECTIONS {
        // `na` = normal axis; the slice sweeps along it, and the in-slice grid
        // spans the other two axes (u, v).
        let na = axis_of(normal);
        let (slice_lo, slice_hi, u_lo, u_hi, v_lo, v_hi) = match na {
            0 => (
                x_range.start,
                x_range.end,
                0,
                WORLD_SIZE_Y as i32,
                z_range.start,
                z_range.end,
            ), // u = y, v = z
            1 => (
                0,
                WORLD_SIZE_Y as i32,
                x_range.start,
                x_range.end,
                z_range.start,
                z_range.end,
            ), // u = x, v = z
            _ => (
                z_range.start,
                z_range.end,
                x_range.start,
                x_range.end,
                0,
                WORLD_SIZE_Y as i32,
            ), // u = x, v = y
        };
        let u_count = (u_hi - u_lo) as usize;
        let v_count = (v_hi - v_lo) as usize;
        let to_world = |s: i32, u: i32, v: i32| match na {
            0 => (s, u, v),
            1 => (u, s, v),
            _ => (u, v, s),
        };

        for s in slice_lo..slice_hi {
            let mut mask: Vec<Option<Cell>> = vec![None; u_count * v_count];
            for vi in 0..v_count {
                for ui in 0..u_count {
                    let (x, y, z) = to_world(s, u_lo + ui as i32, v_lo + vi as i32);
                    if let Some(face) = terrain_merge_face(
                        world,
                        scratch,
                        seed,
                        season,
                        x,
                        y,
                        z,
                        normal,
                        water_plane_y,
                    ) {
                        mask[vi * u_count + ui] = Some(Cell {
                            color: face.color,
                            voxel: face.voxel,
                            below: face.below_water,
                        });
                    }
                }
            }

            let mut used = vec![false; u_count * v_count];
            for vi0 in 0..v_count {
                for ui0 in 0..u_count {
                    let start = vi0 * u_count + ui0;
                    if used[start] {
                        continue;
                    }
                    let Some(cell) = mask[start] else {
                        continue;
                    };
                    // Grow the run along u, then the block along v. Checks are
                    // inlined (not a closure) so the immutable reads release
                    // before `used` is written below.
                    let mut width = 1;
                    while ui0 + width < u_count {
                        let index = vi0 * u_count + ui0 + width;
                        let matches = !used[index]
                            && matches!(mask[index], Some(other)
                                if other.voxel == cell.voxel && other.below == cell.below);
                        if !matches {
                            break;
                        }
                        width += 1;
                    }
                    let mut height = 1;
                    'grow: while vi0 + height < v_count {
                        for du in 0..width {
                            let index = (vi0 + height) * u_count + ui0 + du;
                            let matches = !used[index]
                                && matches!(mask[index], Some(other)
                                    if other.voxel == cell.voxel && other.below == cell.below);
                            if !matches {
                                break 'grow;
                            }
                        }
                        height += 1;
                    }
                    for dv in 0..height {
                        for du in 0..width {
                            used[(vi0 + dv) * u_count + ui0 + du] = true;
                        }
                    }

                    // Rectangle bounds in world voxel coordinates.
                    let u0 = u_lo + ui0 as i32;
                    let v0 = v_lo + vi0 as i32;
                    let u1 = u0 + width as i32 - 1;
                    let v1 = v0 + height as i32 - 1;
                    let (min_x, max_x, min_y, max_y, min_z, max_z) = match na {
                        0 => (s, s, u0, u1, v0, v1),
                        1 => (u0, u1, s, s, v0, v1),
                        _ => (u0, u1, v0, v1, s, s),
                    };
                    let centroid = Vec3::new(
                        (min_x + max_x) as f32 / 2.0 + 0.5,
                        (min_y + max_y) as f32 / 2.0 + 0.5,
                        (min_z + max_z) as f32 / 2.0 + 0.5,
                    );
                    let half_axis = [
                        (max_x - min_x + 1) as f32 / 2.0,
                        (max_y - min_y + 1) as f32 / 2.0,
                        (max_z - min_z + 1) as f32 / 2.0,
                    ];
                    let half_t1 = half_axis[axis_of(tangent_1)];
                    let half_t2 = half_axis[axis_of(tangent_2)];

                    let mut corners = [Vec3::ZERO; 4];
                    let mut corner_colors = [[0.0f32; 4]; 4];
                    for (corner_index, &(along_1, along_2)) in QUAD_CORNERS.iter().enumerate() {
                        let point = centroid
                            + normal.as_vec3() * 0.5
                            + tangent_1.as_vec3() * (along_1 as f32 * half_t1)
                            + tangent_2.as_vec3() * (along_2 as f32 * half_t2);
                        corners[corner_index] = Vec3::new(
                            (point.x - half_x) * VOXEL_SIZE,
                            point.y * VOXEL_SIZE,
                            (point.z - half_z) * VOXEL_SIZE,
                        );
                        // Sample the color at the corner voxel so gradients
                        // interpolate across the merged quad.
                        let corner_cell = centroid
                            + tangent_1.as_vec3() * (along_1 as f32 * (half_t1 - 0.5))
                            + tangent_2.as_vec3() * (along_2 as f32 * (half_t2 - 0.5));
                        let (cell_x, cell_y, cell_z) = (
                            corner_cell.x.floor() as i32,
                            corner_cell.y.floor() as i32,
                            corner_cell.z.floor() as i32,
                        );
                        let (corner_ui, corner_vi) = match na {
                            0 => (cell_y - u_lo, cell_z - v_lo),
                            1 => (cell_x - u_lo, cell_z - v_lo),
                            _ => (cell_x - u_lo, cell_y - v_lo),
                        };
                        let corner_mask_index = corner_vi as usize * u_count + corner_ui as usize;
                        let mut corner_color = mask
                            .get(corner_mask_index)
                            .and_then(|maybe| maybe.map(|c| c.color))
                            .unwrap_or(cell.color);

                        if ambient_occlusion == AmbientOcclusion::Baked {
                            // Same neighbourhood and brightness curve the shader
                            // (and the per-voxel cover path) use, sampled at this
                            // corner's own voxel — see `AmbientOcclusion`.
                            let occlusion_base = IVec3::new(cell_x, cell_y, cell_z) + normal;
                            let side_1 = scratch
                                .get_offset(occlusion_base + tangent_1 * along_1)
                                .is_solid();
                            let side_2 = scratch
                                .get_offset(occlusion_base + tangent_2 * along_2)
                                .is_solid();
                            let diagonal = scratch
                                .get_offset(
                                    occlusion_base + tangent_1 * along_1 + tangent_2 * along_2,
                                )
                                .is_solid();
                            let brightness = 0.55
                                + 0.15 * ambient_occlusion_level(side_1, side_2, diagonal) as f32;
                            corner_color = [
                                corner_color[0] * brightness,
                                corner_color[1] * brightness,
                                corner_color[2] * brightness,
                                // Tell the shader the AO is already applied.
                                corner_color[3] + BAKED_AO_SENTINEL,
                            ];
                        }
                        corner_colors[corner_index] = corner_color;
                    }

                    let buffers = if cell.below {
                        &mut *below_water
                    } else {
                        &mut *above_water
                    };
                    buffers.add_quad(corners, normal.as_vec3(), corner_colors, false);
                }
            }
        }
    }
}

/// Emit tree-leaf voxels as fluffy confetti: each *surface* leaf voxel becomes
/// a shrunken cube (all six faces) nudged by a per-voxel offset, so canopies
/// read as clusters of chunky leaf blocks rather than solid blobs. Not
/// greedy-merged; ambient occlusion is skipped (the shrunk/offset geometry
/// doesn't map to voxel cells — the cover alpha sentinel tells the shader to
/// skip it), so the PBR normals do the shading. Interior leaves (fully
/// enclosed by leaves) are skipped — they'd never show through the shell.
#[allow(clippy::too_many_arguments)]
fn emit_canopy(
    world: &impl VoxelSource,
    scratch: &voxel_core::world::ChunkScratch,
    seed: u32,
    season: f32,
    x_range: std::ops::Range<i32>,
    z_range: std::ops::Range<i32>,
    half_x: f32,
    half_z: f32,
    buffers: &mut MeshBuffers,
) {
    /// Leaves are grouped into cubes this many voxels wide — chunky confetti
    /// (~1/STRIDE³ the cube count of per-voxel), still fluffy but far cheaper.
    const STRIDE: i32 = 2;
    /// Cube edge as a fraction of a block (gaps between cubes = fluff).
    const SHRINK: f32 = 0.82;
    /// Max per-axis positional jitter (voxels), for irregular clumping.
    const OFFSET: f32 = 0.25;

    let block = STRIDE as f32;
    let half_edge = 0.5 * block * SHRINK;
    // Blocks are aligned to the global grid (chunk starts are multiples of the
    // chunk size, itself a multiple of STRIDE), so no block straddles a chunk
    // border — no seams or double-emitted cubes.
    for block_y in (0..WORLD_SIZE_Y as i32).step_by(STRIDE as usize) {
        for block_z in (z_range.start..z_range.end).step_by(STRIDE as usize) {
            for block_x in (x_range.start..x_range.end).step_by(STRIDE as usize) {
                // Find any leaf voxel in this block; it names the cube's color.
                let mut representative: Option<(i32, i32, i32)> = None;
                'scan: for dy in 0..STRIDE {
                    for dz in 0..STRIDE {
                        for dx in 0..STRIDE {
                            let (cx, cy, cz) = (block_x + dx, block_y + dy, block_z + dz);
                            if group_of(scratch.get(cx, cy, cz)) == Some(MeshGroup::Canopy) {
                                representative = Some((cx, cy, cz));
                                break 'scan;
                            }
                        }
                    }
                }
                let Some((rx, ry, rz)) = representative else {
                    continue;
                };

                // Skip cubes buried inside the canopy. The confetti is a fluffy
                // SHELL: the solid inner canopy mesh already fills the volume
                // behind it, so a cube whose six neighbouring blocks are all
                // leaves contributes nothing but vertices. (Only fully enclosed
                // blocks are dropped — the shrunken cubes leave gaps you can see
                // through, so any block on the surface keeps all six faces.)
                let block_has_leaves = |block: IVec3| {
                    (0..STRIDE).any(|dy| {
                        (0..STRIDE).any(|dz| {
                            (0..STRIDE).any(|dx| {
                                group_of(scratch.get(block.x + dx, block.y + dy, block.z + dz))
                                    == Some(MeshGroup::Canopy)
                            })
                        })
                    })
                };
                let here = IVec3::new(block_x, block_y, block_z);
                let enclosed = FACE_DIRECTIONS
                    .iter()
                    .all(|(normal, _, _)| block_has_leaves(here + *normal * STRIDE));
                if enclosed {
                    continue;
                }

                let color = voxel_color(
                    world,
                    scratch,
                    scratch.get(rx, ry, rz),
                    rx,
                    ry,
                    rz,
                    seed,
                    season,
                );
                let offset = Vec3::new(
                    (hash_to_unit(hash_3d(block_x, block_y, block_z, seed.wrapping_add(21))) - 0.5)
                        * 2.0
                        * OFFSET,
                    (hash_to_unit(hash_3d(block_x, block_y, block_z, seed.wrapping_add(22))) - 0.5)
                        * 2.0
                        * OFFSET,
                    (hash_to_unit(hash_3d(block_x, block_y, block_z, seed.wrapping_add(23))) - 0.5)
                        * 2.0
                        * OFFSET,
                );
                let center = Vec3::new(
                    block_x as f32 + block / 2.0,
                    block_y as f32 + block / 2.0,
                    block_z as f32 + block / 2.0,
                ) + offset;

                for (normal, tangent_1, tangent_2) in FACE_DIRECTIONS {
                    let face_center = center + normal.as_vec3() * half_edge;
                    let mut corners = [Vec3::ZERO; 4];
                    for (corner_index, &(along_1, along_2)) in QUAD_CORNERS.iter().enumerate() {
                        let corner = face_center
                            + (tangent_1.as_vec3() * along_1 as f32
                                + tangent_2.as_vec3() * along_2 as f32)
                                * half_edge;
                        corners[corner_index] = Vec3::new(
                            (corner.x - half_x) * VOXEL_SIZE,
                            corner.y * VOXEL_SIZE,
                            (corner.z - half_z) * VOXEL_SIZE,
                        );
                    }
                    buffers.add_quad(corners, normal.as_vec3(), [color; 4], false);
                }
            }
        }
    }
}

/// A cheap solid inner canopy: surface-culled, slightly-shrunk full-size leaf
/// cubes, a touch darker (canopy interior). It sits *behind* the confetti
/// shell — the confetti covers it — so its job is twofold: cast the trees'
/// shadows without the confetti's ~1.8M-cube shadow cost (this mesh is the
/// only canopy shadow caster), and back the shell so gaps can never see
/// through to the sky. Same cheap surface-face count the old merged leaves had.
#[allow(clippy::too_many_arguments)]
fn emit_canopy_solid(
    world: &impl VoxelSource,
    scratch: &voxel_core::world::ChunkScratch,
    seed: u32,
    season: f32,
    x_range: std::ops::Range<i32>,
    z_range: std::ops::Range<i32>,
    half_x: f32,
    half_z: f32,
    buffers: &mut MeshBuffers,
) {
    // Full-size (1.0) so adjacent surface cubes touch into a gap-free
    // silhouette — the shadow reads as one solid (soft) tree shadow instead of
    // a stippled grid. The confetti shell (bigger, offset) still covers it.
    const SHRINK: f32 = 1.0;
    let half_edge = 0.5 * SHRINK;

    for y in 0..WORLD_SIZE_Y as i32 {
        for z in z_range.clone() {
            for x in x_range.clone() {
                let voxel = scratch.get(x, y, z);
                if group_of(voxel) != Some(MeshGroup::Canopy) {
                    continue;
                }
                let position = IVec3::new(x, y, z);
                let mut color = voxel_color(world, scratch, voxel, x, y, z, seed, season);
                // Slightly darker so it reads as canopy interior through gaps
                // (alpha carries the AO-skip sentinel — leave it alone).
                color[0] *= 0.8;
                color[1] *= 0.8;
                color[2] *= 0.8;
                let center = Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5);
                for (normal, tangent_1, tangent_2) in FACE_DIRECTIONS {
                    // Surface only: skip faces against other leaves.
                    let neighbor = position + normal;
                    if group_of(scratch.get(neighbor.x, neighbor.y, neighbor.z))
                        == Some(MeshGroup::Canopy)
                    {
                        continue;
                    }
                    let face_center = center + normal.as_vec3() * half_edge;
                    let mut corners = [Vec3::ZERO; 4];
                    for (corner_index, &(along_1, along_2)) in QUAD_CORNERS.iter().enumerate() {
                        let corner = face_center
                            + (tangent_1.as_vec3() * along_1 as f32
                                + tangent_2.as_vec3() * along_2 as f32)
                                * half_edge;
                        corners[corner_index] = Vec3::new(
                            (corner.x - half_x) * VOXEL_SIZE,
                            corner.y * VOXEL_SIZE,
                            (corner.z - half_z) * VOXEL_SIZE,
                        );
                    }
                    buffers.add_quad(corners, normal.as_vec3(), [color; 4], false);
                }
            }
        }
    }
}

/// Horizontal footprint of a cover voxel as a fraction of a full cube, shrunk
/// toward the cell center. Blades/flowers/reeds read as slender stalks rather
/// than fat cubes; flat pads keep their full width. Terrain is always 1.0.
fn visual_footprint_scale(voxel: Voxel) -> f32 {
    match voxel {
        Voxel::TallGrass => 0.5,
        Voxel::FlowerPink | Voxel::FlowerWhite | Voxel::FlowerYellow | Voxel::FlowerBlue => 0.55,
        Voxel::WaterWeed => 0.55,
        Voxel::Reed | Voxel::CattailHead => 0.6,
        // Lily pads are flat and wide — keep their footprint.
        _ => 1.0,
    }
}

/// Visual height of a voxel as a fraction of a full cube. Ground cover is
/// squashed — tufts vary per-cell so the meadow reads as an uneven carpet.
fn visual_vertical_scale(
    scratch: &voxel_core::world::ChunkScratch,
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
/// water depth tint. The RGB is *un-jittered* (the fragment shader adds the
/// per-voxel brightness speckle); the returned alpha carries either the
/// jitter amplitude (terrain) or the water transparency (water voxels).
#[allow(clippy::too_many_arguments)]
fn voxel_color(
    world: &impl VoxelSource,
    scratch: &voxel_core::world::ChunkScratch,
    voxel: Voxel,
    x: i32,
    y: i32,
    z: i32,
    seed: u32,
    season: f32,
) -> [f32; 4] {
    let jitter_roll = hash_to_unit(hash_3d(x, y, z, seed.wrapping_add(3)));
    let dryness = world.dryness_at(x, z);
    let altitude_meters = (y - voxel_core::world::WATER_LEVEL) as f32 * VOXEL_SIZE;
    // Lush right at the water, paling as the land climbs.
    let lushness = voxel_core::noise::smoothstep(9.0, 1.5, world.water_distance_at(x, z));
    let paling = voxel_core::noise::smoothstep(3.5, 11.0, altitude_meters);

    // Per-voxel brightness jitter is now applied in the terrain fragment
    // shader (so it survives greedy meshing). Each arm returns the jitter
    // *amplitude* `a` (= span/2) instead of a baked multiplier; every type is
    // mean-1.0 (`center + span·roll`, `center = 1 − span/2`), so the shader
    // reconstructs `1 + a·(2·roll − 1)`. The amplitude rides in the returned
    // alpha for terrain; water keeps its transparency in alpha (its own
    // shader), so its amplitude is unused. Birch bark still selects its fleck
    // *color* from the hash here (a color choice, not brightness).
    let (srgb, alpha, amplitude) = match voxel {
        Voxel::Grass => {
            // Ground reads as dirt between the grass clumps and olive-green
            // under them, before the biome dryness sweep.
            let patchiness = voxel_core::noise::smoothstep(0.35, 0.70, world.cover_at(x, z));
            let mut ground = lerp_rgb([0.50, 0.42, 0.30], [0.41, 0.52, 0.29], patchiness);
            ground = lerp_rgb(ground, [0.33, 0.50, 0.25], lushness * 0.55);
            ground = lerp_rgb(ground, [0.57, 0.55, 0.33], paling * 0.55);
            ground = lerp_rgb(ground, [0.63, 0.52, 0.28], season * 0.55);
            (lerp_rgb(ground, [0.72, 0.64, 0.38], dryness), 1.0, 0.08)
        }
        Voxel::TallGrass => {
            let mut blade = [0.28, 0.45, 0.23];
            blade = lerp_rgb(blade, [0.23, 0.46, 0.20], lushness * 0.45);
            blade = lerp_rgb(blade, [0.62, 0.50, 0.26], season * 0.60);
            (lerp_rgb(blade, [0.66, 0.60, 0.35], dryness), 1.0, 0.15)
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
            let turn =
                voxel_core::noise::smoothstep(turn_start, (turn_start + 0.30).min(1.0), season);
            let mut leaf = lerp_rgb(summer, autumn, turn);
            leaf = lerp_rgb(leaf, [0.55, 0.55, 0.28], dryness * 0.5);
            if voxel == Voxel::LeavesDark {
                leaf = [leaf[0] * 0.74, leaf[1] * 0.74, leaf[2] * 0.74];
            }
            (leaf, 1.0, 0.13)
        }
        Voxel::LeavesBirch => {
            let tone = world.tree_tone_at(x, z);
            let summer = lerp_rgb([0.47, 0.56, 0.26], [0.55, 0.60, 0.30], tone);
            // Birches turn early and go pure gold.
            let turn = voxel_core::noise::smoothstep(tone * 0.35, tone * 0.35 + 0.25, season);
            (lerp_rgb(summer, [0.88, 0.66, 0.14], turn), 1.0, 0.13)
        }
        Voxel::LeavesPine => {
            let tone = world.tree_tone_at(x, z);
            let needles = lerp_rgb([0.18, 0.32, 0.23], [0.24, 0.37, 0.25], tone);
            // Evergreens just dull a little as the year fades.
            (
                lerp_rgb(needles, [0.24, 0.33, 0.22], season * 0.35),
                1.0,
                0.11,
            )
        }
        Voxel::Dirt => ([0.44, 0.32, 0.22], 1.0, 0.08),
        Voxel::Sand => {
            // Wet sand below the waterline is darker, so the shallows stay
            // believable through the transparent water.
            if y <= voxel_core::world::WATER_LEVEL {
                ([0.38, 0.33, 0.23], 1.0, 0.06)
            } else {
                ([0.86, 0.77, 0.55], 1.0, 0.06)
            }
        }
        Voxel::Sediment => ([0.17, 0.16, 0.11], 1.0, 0.10),
        Voxel::Stone => ([0.52, 0.52, 0.55], 1.0, 0.10),
        Voxel::Trunk => ([0.45, 0.31, 0.19], 1.0, 0.12),
        Voxel::TrunkBirch => {
            // White paper bark broken by dark horizontal flecks.
            if jitter_roll < 0.16 {
                ([0.20, 0.18, 0.16], 1.0, 0.10)
            } else {
                ([0.80, 0.78, 0.72], 1.0, 0.07)
            }
        }
        Voxel::FlowerPink => ([0.93, 0.55, 0.75], 1.0, 0.0),
        Voxel::FlowerWhite => ([0.96, 0.95, 0.90], 1.0, 0.0),
        Voxel::FlowerYellow => ([0.95, 0.83, 0.35], 1.0, 0.0),
        Voxel::FlowerBlue => ([0.45, 0.52, 0.92], 1.0, 0.0),
        Voxel::WaterWeed => ([0.15, 0.30, 0.19], 1.0, 0.20),
        Voxel::LilyPad => ([0.26, 0.50, 0.24], 1.0, 0.10),
        Voxel::LilyBloom => ([0.95, 0.92, 0.85], 1.0, 0.05),
        Voxel::CattailHead => ([0.32, 0.18, 0.08], 1.0, 0.12),
        Voxel::Reed => {
            let stalk = lerp_rgb([0.55, 0.56, 0.31], [0.63, 0.52, 0.26], season * 0.5);
            (stalk, 1.0, 0.15)
        }
        Voxel::Snow => ([0.92, 0.93, 0.96], 1.0, 0.035),
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
                0.0,
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
        (srgb[0] * underside_bounce[0]).min(1.0),
        (srgb[1] * underside_bounce[1]).min(1.0),
        (srgb[2] * underside_bounce[2]).min(1.0),
    )
    .to_linear();
    // Alpha channel, by group: water carries the transparency its own shader
    // expects; cover carries `amplitude + 10` as a sentinel so the terrain
    // shader skips AO for it (cover keeps its baked AO and squashed geometry);
    // terrain carries the bare jitter amplitude and gets shader AO.
    let out_alpha = match group_of(voxel) {
        Some(MeshGroup::Water) => alpha,
        Some(MeshGroup::Cover) | Some(MeshGroup::Canopy) => amplitude + 10.0,
        _ => amplitude,
    };
    [linear.red, linear.green, linear.blue, out_alpha]
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
