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

use crate::voxel_material;
use voxel_core::noise::{hash_3d, hash_to_unit};
use voxel_core::voxel_source::VoxelSource;
use voxel_core::world::{Voxel, PLATEAU_FLOOR, VOXEL_SIZE, WORLD_SIZE_Y};

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

/// Which axis (0 = x, 1 = y, 2 = z) an axis-aligned unit vector lies on. Both
/// greedy passes sweep slices along a face normal and index half-extents by
/// tangent axis, so they share this.
fn axis_of(vector: IVec3) -> usize {
    if vector.x != 0 {
        0
    } else if vector.y != 0 {
        1
    } else {
        2
    }
}

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

/// The axis-aligned unit vector closest to `direction`, falling back to up for
/// a degenerate input. Used to fit a smooth shading direction into the packed
/// vertex's 3-bit face index.
fn dominant_axis(direction: Vec3) -> Vec3 {
    if direction.length_squared() < 1e-6 {
        return Vec3::Y;
    }
    let magnitude = direction.abs();
    if magnitude.x >= magnitude.y && magnitude.x >= magnitude.z {
        Vec3::new(direction.x.signum(), 0.0, 0.0)
    } else if magnitude.y >= magnitude.z {
        Vec3::new(0.0, direction.y.signum(), 0.0)
    } else {
        Vec3::new(0.0, 0.0, direction.z.signum())
    }
}

/// Which entry of [`FACE_DIRECTIONS`] a unit normal is. Voxel faces only ever
/// point six ways, so the normal travels as a 3-bit index and the vertex shader
/// turns it back into a vector.
fn face_index_of(normal: Vec3) -> u16 {
    FACE_DIRECTIONS
        .iter()
        .position(|(face, _, _)| face.as_vec3() == normal)
        .unwrap_or_else(|| panic!("voxel faces must be axis-aligned, got {normal:?}")) as u16
}

/// Vertex buffers in the **packed** layout every terrain-material mesh shares:
/// 12 bytes a vertex instead of 40 (see
/// [`crate::voxel_material::ATTRIBUTE_VOXEL_POSITION`]).
#[derive(Default)]
pub(crate) struct MeshBuffers {
    /// Chunk-local position as 16-bit fixed point, plus the packed face word.
    packed_positions: Vec<[u16; 4]>,
    /// Vertex colour, rgb as 8-bit unorm (alpha unused).
    packed_colors: Vec<[u8; 4]>,
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
        let base_index = self.packed_positions.len() as u32;
        let face_index = face_index_of(normal);
        for corner_index in 0..4 {
            let corner = corners[corner_index];
            let color = corner_colors[corner_index];
            // Alpha arrives carrying the jitter amplitude, offset by
            // `BAKED_AO_SENTINEL` where ambient occlusion is already in the
            // colour. Split that back apart here: the flag and the amplitude
            // get their own bits, so the colour channels can be plain unorm.
            let ambient_occlusion_baked = color[3] >= 1.0;
            let amplitude = if ambient_occlusion_baked {
                color[3] - BAKED_AO_SENTINEL
            } else {
                color[3]
            };
            self.packed_positions.push([
                voxel_material::pack_position_axis(corner.x),
                voxel_material::pack_position_axis(corner.y),
                voxel_material::pack_position_axis(corner.z),
                voxel_material::pack_face_word(face_index, ambient_occlusion_baked, amplitude),
            ]);
            self.packed_colors.push([
                (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
                255,
            ]);
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
        self.packed_positions.is_empty()
    }

    pub(crate) fn into_mesh(self) -> Mesh {
        // NOTE: do not "optimise" the indices into 16-bit for small meshes.
        // It saves real VRAM (~16 MB here) and costs real frame time, because
        // bevy keys mesh slabs by element layout and puts `index_slab` in the
        // batch set key (`bevy_pbr`'s `material.rs`). A mix of 16- and 32-bit
        // index formats therefore splits one batch set into two, and batching
        // is worth far more to this scene than the bytes are. If every mesh
        // could be 16-bit it would be uniform and fine — but some chunks
        // exceed 65 536 vertices, so the formats would always be mixed.
        //
        // The same rule governs the packed attributes below: `vertex_slab` is
        // in that key too, so EVERY mesh drawn with the terrain material has to
        // use this exact layout.
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(
            voxel_material::ATTRIBUTE_VOXEL_POSITION,
            bevy::mesh::VertexAttributeValues::Uint16x4(self.packed_positions),
        )
        .with_inserted_attribute(
            voxel_material::ATTRIBUTE_VOXEL_COLOR,
            bevy::mesh::VertexAttributeValues::Unorm8x4(self.packed_colors),
        )
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

    // Vertices are emitted CHUNK-LOCAL — relative to this chunk's origin — and
    // the entity's `Transform` puts the chunk in the world. They used to be
    // world-space with identity transforms, which is simpler but caps how
    // tightly a position can be quantised: the infinite world's coordinates
    // grow without bound, so no fixed-point format can cover them. Chunk-local
    // positions span a known 64 × 256 × 64 voxels, which 16 bits covers to well
    // under a millimetre.
    //
    // Culling is unaffected: bevy derives the AABB from these vertices and
    // transforms it. Batching is unaffected too — a `Transform` is per-instance
    // data bevy already uploads, unlike a per-chunk material binding, which
    // would have split the batch.
    let half_x = (chunk_x * CHUNK_SIZE) as f32;
    let half_z = (chunk_z * CHUNK_SIZE) as f32;

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
                    // Same-group neighbours hide each other's shared face (two
                    // stacked reeds don't need the surface between them).
                    //
                    // Cover needs one exception: `TallGrass` is in the Cover
                    // group but is never meshed here — it becomes an instanced
                    // clump barely half a voxel tall — so culling against it
                    // leaves an open hole into the hollow cover box. Flowers
                    // generated on top of tall grass hit exactly that.
                    let neighbor_group = group_of(neighbor);
                    let neighbor_is_emitted =
                        voxel_group != MeshGroup::Cover || neighbor != Voxel::TallGrass;
                    if neighbor_group == Some(voxel_group) && neighbor_is_emitted {
                        continue;
                    }
                    // A cover voxel's box is anchored to the bottom of its cell
                    // (only its top is squashed down), so its underside is
                    // exactly coplanar with the terrain face below. When that
                    // neighbour is solid the quad cannot be seen from any angle.
                    if voxel_group == MeshGroup::Cover
                        && normal == IVec3::NEG_Y
                        && neighbor.is_solid()
                    {
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

                // Which way is "out of the canopy" here? Sum the directions to
                // neighbouring blocks that hold leaves and negate it: the result
                // points away from the local leaf mass.
                //
                // This is the difference between a tree that reads as a tree and
                // one that reads as noise. Shading each cube by its own six face
                // normals means every cube has a lit top and a dark underside no
                // matter where it sits, so the canopy dissolves into speckle with
                // no form. Shading them all by the direction out of the clump
                // instead gives the canopy one rounded light gradient — bright on
                // the sunward outside, dark in the hollows — which is what makes
                // it look like foliage.
                //
                // It is quantised to the dominant axis because a packed vertex
                // stores its normal as a 3-bit face index (see
                // `voxel_material::pack_face_word`). Six directions is coarse,
                // but the win is that a cube's faces now agree with each other
                // and with their neighbours, which is where the form comes from.
                let mut mass_direction = Vec3::ZERO;
                for step_z in -1..=1 {
                    for step_y in -1..=1 {
                        for step_x in -1..=1 {
                            if step_x == 0 && step_y == 0 && step_z == 0 {
                                continue;
                            }
                            let step = IVec3::new(step_x, step_y, step_z);
                            let probe = here + step * STRIDE;
                            if group_of(scratch.get(probe.x, probe.y, probe.z))
                                == Some(MeshGroup::Canopy)
                            {
                                mass_direction += step.as_vec3().normalize();
                            }
                        }
                    }
                }
                let outward = dominant_axis(-mass_direction);

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
                    // Corners came from the true face direction above, so
                    // winding and back-face culling are untouched; only the
                    // SHADING normal is swapped for the clump-outward one.
                    buffers.add_quad(corners, outward, [color; 4], false);
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
    // The shell's cubes are FULL SIZE (no shrink) and sit exactly on the voxel
    // grid, so adjacent surface faces are coplanar and touch — which is what
    // lets this pass greedy-merge, unlike the confetti (shrunk *and* per-voxel
    // jittered, so no two of its faces ever line up).
    //
    // Merging changes nothing about the silhouette: identical faces in
    // identical places, just fewer and larger quads covering them. That matters
    // because this shell was the heaviest mesh group in the scene — heavier
    // than the confetti it exists to back.
    //
    // Leaves are sparse, so rather than sweep every slice of the chunk six
    // times hunting for them, ONE pass collects the exposed faces and buckets
    // them by (direction, slice). Only slices that actually hold faces are then
    // merged, over the bounding box of those faces rather than the whole chunk
    // cross-section.
    type SliceFaces = std::collections::HashMap<i32, Vec<(i32, i32, [f32; 4])>>;
    let mut buckets: [SliceFaces; 6] = std::array::from_fn(|_| SliceFaces::new());

    for y in 0..WORLD_SIZE_Y as i32 {
        for z in z_range.clone() {
            for x in x_range.clone() {
                let voxel = scratch.get(x, y, z);
                if group_of(voxel) != Some(MeshGroup::Canopy) {
                    continue;
                }
                let mut color = voxel_color(world, scratch, voxel, x, y, z, seed, season);
                // Slightly darker so it reads as canopy interior through gaps
                // (alpha carries the AO-skip sentinel — leave it alone).
                color[0] *= 0.8;
                color[1] *= 0.8;
                color[2] *= 0.8;
                let position = IVec3::new(x, y, z);
                for (direction, (normal, _, _)) in FACE_DIRECTIONS.iter().enumerate() {
                    // Surface only: skip faces against other leaves.
                    let neighbor = position + *normal;
                    if group_of(scratch.get(neighbor.x, neighbor.y, neighbor.z))
                        == Some(MeshGroup::Canopy)
                    {
                        continue;
                    }
                    // Split the position into "which slice" + "where in it",
                    // matching the (u, v) convention the merge below uses.
                    let (slice, u, v) = match axis_of(*normal) {
                        0 => (x, y, z),
                        1 => (y, x, z),
                        _ => (z, x, y),
                    };
                    buckets[direction]
                        .entry(slice)
                        .or_default()
                        .push((u, v, color));
                }
            }
        }
    }

    for (direction, (normal, tangent_1, tangent_2)) in FACE_DIRECTIONS.iter().enumerate() {
        let na = axis_of(*normal);
        for (&slice, faces) in &buckets[direction] {
            // Merge within the faces' own bounding box, not the chunk's.
            let u_lo = faces.iter().map(|&(u, _, _)| u).min().expect("non-empty");
            let u_hi = faces.iter().map(|&(u, _, _)| u).max().expect("non-empty");
            let v_lo = faces.iter().map(|&(_, v, _)| v).min().expect("non-empty");
            let v_hi = faces.iter().map(|&(_, v, _)| v).max().expect("non-empty");
            let u_count = (u_hi - u_lo + 1) as usize;
            let v_count = (v_hi - v_lo + 1) as usize;

            // Exposed faces keyed by colour: two cells merge only if they'd
            // have been the same shade anyway.
            let mut mask: Vec<Option<[f32; 4]>> = vec![None; u_count * v_count];
            for &(u, v, color) in faces {
                mask[(v - v_lo) as usize * u_count + (u - u_lo) as usize] = Some(color);
            }
            let mut used = vec![false; u_count * v_count];

            for vi0 in 0..v_count {
                for ui0 in 0..u_count {
                    let start = vi0 * u_count + ui0;
                    if used[start] {
                        continue;
                    }
                    let Some(color) = mask[start] else {
                        continue;
                    };
                    // Grow along u, then extend that whole run along v.
                    let mut width = 1;
                    while ui0 + width < u_count {
                        let index = vi0 * u_count + ui0 + width;
                        if used[index] || mask[index] != Some(color) {
                            break;
                        }
                        width += 1;
                    }
                    let mut height = 1;
                    'grow: while vi0 + height < v_count {
                        for du in 0..width {
                            let index = (vi0 + height) * u_count + ui0 + du;
                            if used[index] || mask[index] != Some(color) {
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

                    let u0 = u_lo + ui0 as i32;
                    let v0 = v_lo + vi0 as i32;
                    let u1 = u0 + width as i32 - 1;
                    let v1 = v0 + height as i32 - 1;
                    let (min_x, max_x, min_y, max_y, min_z, max_z) = match na {
                        0 => (slice, slice, u0, u1, v0, v1),
                        1 => (u0, u1, slice, slice, v0, v1),
                        _ => (u0, u1, v0, v1, slice, slice),
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
                    let half_t1 = half_axis[axis_of(*tangent_1)];
                    let half_t2 = half_axis[axis_of(*tangent_2)];

                    let mut corners = [Vec3::ZERO; 4];
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
        // M1b emissive types. voxel-sandbox has no emission channel — these
        // are the unlit body colours, so a lantern here reads as brass and a
        // berry cluster as pale green. The GLOW is voxel-rt's (CAGI, E5).
        Voxel::GlowBlock => ([0.95, 0.93, 0.88], 1.0, 0.02),
        Voxel::GlowBerry => ([0.55, 0.95, 0.80], 1.0, 0.10),
        Voxel::Lava => ([0.95, 0.20, 0.015], 1.0, 0.02),
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

    /// Vertex positions, unpacked back to chunk-local metres. Positions are
    /// stored as 16-bit fixed point now (see `voxel_material`), so tests have
    /// to undo the same quantisation the vertex shader does.
    fn unpacked_positions(mesh: &Mesh) -> Vec<Vec3> {
        let packed = match mesh
            .attribute(voxel_material::ATTRIBUTE_VOXEL_POSITION)
            .expect("packed positions")
        {
            bevy::mesh::VertexAttributeValues::Uint16x4(values) => values,
            other => panic!("packed position should be Uint16x4, got {other:?}"),
        };
        packed
            .iter()
            .map(|value| {
                Vec3::new(
                    voxel_material::unpack_position_axis(value[0]),
                    voxel_material::unpack_position_axis(value[1]),
                    voxel_material::unpack_position_axis(value[2]),
                )
            })
            .collect()
    }

    /// A cover box's underside is coplanar with the terrain top beneath it, so
    /// when that neighbour is solid the face is invisible and must not be
    /// emitted. This is pure savings — roughly a sixth of an isolated cover
    /// voxel's faces.
    #[test]
    fn cover_on_solid_ground_drops_its_underside() {
        let faces = cover_faces_over(Voxel::Grass);
        assert!(
            !faces.contains(&IVec3::NEG_Y),
            "cover sitting on solid ground still emitted a bottom face: {faces:?}"
        );
        assert!(
            faces.contains(&IVec3::Y),
            "cover must still emit its top face: {faces:?}"
        );
    }

    /// The bug this cull rule replaced: `TallGrass` is in the Cover group but
    /// is never meshed (it becomes an instanced clump barely half a voxel
    /// tall), so culling against it opened a hole into the hollow flower box.
    #[test]
    fn cover_over_tall_grass_keeps_its_underside() {
        let faces = cover_faces_over(Voxel::TallGrass);
        assert!(
            faces.contains(&IVec3::NEG_Y),
            "cover above unmeshed tall grass must keep its underside closed, \
             or you can see into it: {faces:?}"
        );
    }

    /// Which face directions a flower emits when the voxel below it is
    /// `below`. Counts quads by normal in a one-cover-voxel world.
    fn cover_faces_over(below: Voxel) -> Vec<IVec3> {
        let flower_y: i32 = 40;
        let scratch = voxel_core::world::ChunkScratch::from_columns(0, 0, 3, 3, |_, _, column| {
            column[flower_y as usize] = Voxel::FlowerPink;
            column[flower_y as usize - 1] = below;
        });

        let mut faces = Vec::new();
        for (normal, _, _) in FACE_DIRECTIONS {
            let neighbor = scratch.get(1, flower_y + normal.y, 1);
            let neighbor_group = group_of(neighbor);
            let neighbor_is_emitted = neighbor != Voxel::TallGrass;
            if neighbor_group == Some(MeshGroup::Cover) && neighbor_is_emitted {
                continue;
            }
            if normal == IVec3::NEG_Y && neighbor.is_solid() {
                continue;
            }
            faces.push(normal);
        }
        faces
    }

    /// A uniform source with a single solid block of leaves, for exercising the
    /// canopy shell in isolation.
    struct LeafBlock {
        span: i32,
        base_y: i32,
    }

    impl VoxelSource for LeafBlock {
        fn unpack_chunk(
            &self,
            x_start: i32,
            x_end: i32,
            z_start: i32,
            z_end: i32,
        ) -> voxel_core::world::ChunkScratch {
            let (span, base_y) = (self.span, self.base_y);
            voxel_core::world::ChunkScratch::from_columns(
                x_start - 1,
                z_start - 1,
                x_end - x_start + 2,
                z_end - z_start + 2,
                |x, z, column| {
                    if (0..span).contains(&x) && (0..span).contains(&z) {
                        for y in base_y..base_y + span {
                            column[y as usize] = Voxel::Leaves;
                        }
                    }
                },
            )
        }
        fn dryness_at(&self, _: i32, _: i32) -> f32 {
            0.5
        }
        fn cover_at(&self, _: i32, _: i32) -> f32 {
            0.5
        }
        fn water_distance_at(&self, _: i32, _: i32) -> f32 {
            10.0
        }
        fn tree_tone_at(&self, _: i32, _: i32) -> f32 {
            0.5
        }
    }

    /// Greedy merging the solid canopy shell must be **lossless**: the same
    /// surface, in the same place, just covered by fewer and larger quads.
    ///
    /// A solid `span³` block of leaves has exactly `6 · span²` exposed voxel
    /// faces. Merged perfectly — the block's colour is uniform, so nothing
    /// blocks a merge — that is 6 quads of `span × span`, and the total area
    /// must be unchanged. Area is the invariant that catches both a dropped
    /// face and a double-covered one.
    #[test]
    fn merged_canopy_shell_covers_the_same_surface() {
        const SPAN: i32 = 3;
        let source = LeafBlock {
            span: SPAN,
            base_y: 40,
        };
        let scratch = source.unpack_chunk(0, SPAN, 0, SPAN);
        let mut buffers = MeshBuffers::default();
        emit_canopy_solid(
            &source,
            &scratch,
            1,
            0.0,
            0..SPAN,
            0..SPAN,
            0.0,
            0.0,
            &mut buffers,
        );
        let mesh = buffers.into_mesh();

        let quads = mesh.count_vertices() / 4;
        assert_eq!(
            quads, 6,
            "a uniform {SPAN}³ leaf block should merge to one quad per side, got {quads}"
        );

        let positions = unpacked_positions(&mesh);
        let mut area = 0.0_f32;
        for quad in positions.chunks_exact(4) {
            let edge_1 = quad[1] - quad[0];
            let edge_2 = quad[3] - quad[0];
            area += edge_1.cross(edge_2).length();
        }
        // 6 sides × SPAN² voxel faces, each VOXEL_SIZE² in world units.
        let expected = 6.0 * (SPAN * SPAN) as f32 * VOXEL_SIZE * VOXEL_SIZE;
        // Positions are 16-bit fixed point, so each corner can round by up to
        // ~0.26 mm and the area drifts a fraction of a percent. Check the
        // relative error: the point is that no face is dropped or doubled, not
        // that the quantiser is lossless.
        let relative_error = (area - expected).abs() / expected;
        assert!(
            relative_error < 0.005,
            "merged shell area {area} should match the unmerged surface {expected} \
             (off by {:.3}%)",
            relative_error * 100.0
        );
    }

    /// Chunk meshes are emitted CHUNK-LOCAL, so every vertex must fall inside
    /// its chunk's own footprint — and adding the chunk origin back must put it
    /// exactly where the old world-space vertices were. If this drifts, the
    /// world tears along chunk borders.
    #[test]
    fn chunk_meshes_are_local_to_their_chunk() {
        use std::sync::Arc;
        use voxel_core::voxel_source::IslandSource;
        use voxel_core::world::VoxelWorld;

        let source = IslandSource::new(Arc::new(VoxelWorld::generate(1, 0.0)));
        let span = CHUNK_SIZE as f32 * VOXEL_SIZE;
        // Cover geometry is shrunk and jittered a little past its cell, and the
        // apron reaches one voxel out, so allow a voxel of slack either side.
        let slack = VOXEL_SIZE * 2.0;

        let mut checked = 0;
        for (chunk_x, chunk_z) in [(0, 0), (-3, 2), (4, -1)] {
            let scratch = unpack_chunk_window(&source, chunk_x, chunk_z);
            let chunk = build_chunk_meshes(
                &source,
                &scratch,
                1,
                0.0,
                chunk_x,
                chunk_z,
                AmbientOcclusion::PerFragment,
            );
            for mesh in [
                &chunk.terrain_above_water,
                &chunk.terrain_below_water,
                &chunk.meadow_cover,
                &chunk.canopy,
                &chunk.canopy_solid,
            ]
            .into_iter()
            .flatten()
            {
                for position in unpacked_positions(mesh) {
                    assert!(
                        position.x >= -slack && position.x <= span + slack,
                        "chunk ({chunk_x},{chunk_z}) vertex x {} escapes its 0..{span} span",
                        position.x
                    );
                    assert!(
                        position.z >= -slack && position.z <= span + slack,
                        "chunk ({chunk_x},{chunk_z}) vertex z {} escapes its 0..{span} span",
                        position.z
                    );
                    checked += 1;
                }
            }
        }
        assert!(
            checked > 1000,
            "expected real geometry to check, saw {checked}"
        );
    }

    /// Headless geometry baseline for the whole island: what every mesh group
    /// costs in vertices, triangles and VRAM bytes.
    ///
    /// This is the yardstick the optimization candidates in
    /// `docs/voxel-optimization-candidates.md` are measured against. It runs
    /// without a window, so a change's effect on geometry is reproducible from
    /// the terminal — frame time still needs the real app, but geometry does
    /// not. Run it with:
    ///
    /// ```text
    /// cargo test -p voxel-sandbox --release island_geometry_baseline -- --nocapture
    /// ```
    #[test]
    fn island_geometry_baseline() {
        use crate::geometry_census::{GeometryCensus, GeometryKind, GeometryTotals};
        use std::sync::Arc;
        use voxel_core::voxel_source::IslandSource;
        use voxel_core::world::VoxelWorld;

        let started = std::time::Instant::now();
        let source = IslandSource::new(Arc::new(VoxelWorld::generate(1, 0.0)));
        let bounds = IslandSource::chunk_bounds(CHUNK_SIZE);
        let mut totals = GeometryTotals::default();
        let mut chunks = 0;
        let mut clump_total = 0_usize;

        let add = |totals: &mut GeometryTotals, mesh: &Option<Mesh>, kind: GeometryKind| {
            let Some(mesh) = mesh else {
                return;
            };
            let entry = GeometryCensus::of(mesh, kind);
            let row = &mut totals.rows[kind.index()];
            row.entities += 1;
            row.vertices += entry.vertices as u64;
            row.triangles += entry.triangles as u64;
            row.bytes += entry.bytes as u64;
        };

        for chunk_z in -bounds..=bounds {
            for chunk_x in -bounds..=bounds {
                let scratch = unpack_chunk_window(&source, chunk_x, chunk_z);
                let chunk = build_chunk_meshes(
                    &source,
                    &scratch,
                    1,
                    0.0,
                    chunk_x,
                    chunk_z,
                    AmbientOcclusion::PerFragment,
                );
                chunks += 1;
                add(
                    &mut totals,
                    &chunk.terrain_above_water,
                    GeometryKind::TerrainAbove,
                );
                add(
                    &mut totals,
                    &chunk.terrain_below_water,
                    GeometryKind::TerrainBelow,
                );
                add(&mut totals, &chunk.meadow_cover, GeometryKind::MeadowCover);
                add(&mut totals, &chunk.canopy, GeometryKind::Canopy);
                add(&mut totals, &chunk.canopy_solid, GeometryKind::CanopySolid);
                add(&mut totals, &chunk.water, GeometryKind::Water);

                // Grass is built by the streamer rather than the chunk mesher,
                // so measure it here too — otherwise the census silently omits
                // the group whose cost drove the batching work.
                let clumps =
                    crate::streaming::harvest_grass_clumps(&source, &scratch, chunk_x, chunk_z);
                let grass = crate::grass::build_chunk_grass_mesh(&clumps, chunk_x, chunk_z, 1);
                clump_total += clumps.len();
                add(&mut totals, &grass, GeometryKind::GrassClump);
            }
        }

        println!(
            "\nisland geometry baseline ({chunks} chunks, {clump_total} grass clumps, {:.1?})",
            started.elapsed()
        );
        println!(
            "  {:<13} {:>7} {:>12} {:>12} {:>9}",
            "group", "meshes", "vertices", "triangles", "MB"
        );
        for kind in GeometryKind::ALL {
            let row = totals.row(kind);
            if row.entities == 0 {
                continue;
            }
            println!(
                "  {:<13} {:>7} {:>12} {:>12} {:>9.2}",
                kind.label(),
                row.entities,
                row.vertices,
                row.triangles,
                row.bytes as f64 / 1.0e6,
            );
        }
        let overall = totals.overall();
        println!(
            "  {:<13} {:>7} {:>12} {:>12} {:>9.2}\n",
            "TOTAL",
            overall.entities,
            overall.vertices,
            overall.triangles,
            overall.bytes as f64 / 1.0e6,
        );
        assert!(overall.vertices > 0, "island produced no geometry");
    }

    /// Mesh the whole island through [`IslandSource`] — the same path the
    /// streamer takes, one chunk at a time — and check the totals still look
    /// like an island. This replaced a test over the old bulk island mesher,
    /// which no longer exists: there is one render path now.
    #[test]
    fn island_chunks_are_consistent_and_nonempty() {
        use std::sync::Arc;
        use voxel_core::voxel_source::IslandSource;
        use voxel_core::world::VoxelWorld;

        let source = IslandSource::new(Arc::new(VoxelWorld::generate(1, 0.0)));
        let bounds = IslandSource::chunk_bounds(CHUNK_SIZE);

        let mut nonempty_chunks = 0;
        let mut above_total = 0;
        let mut meadow_total = 0;
        let mut below_total = 0;
        let mut water_total = 0;

        for chunk_z in -bounds..=bounds {
            for chunk_x in -bounds..=bounds {
                let scratch = unpack_chunk_window(&source, chunk_x, chunk_z);
                let chunk = build_chunk_meshes(
                    &source,
                    &scratch,
                    1,
                    0.0,
                    chunk_x,
                    chunk_z,
                    AmbientOcclusion::PerFragment,
                );

                let mut any = false;
                for (label, mesh) in [
                    ("terrain above water", &chunk.terrain_above_water),
                    ("meadow cover", &chunk.meadow_cover),
                    ("terrain below water", &chunk.terrain_below_water),
                    ("water", &chunk.water),
                ] {
                    let Some(mesh) = mesh else {
                        continue;
                    };
                    any = true;
                    let vertex_count = mesh.count_vertices();
                    assert!(vertex_count > 0, "{label} mesh present but empty");
                    assert_eq!(vertex_count % 4, 0, "{label} mesh has partial quads");
                    let index_count = mesh.indices().expect("indices").len();
                    assert_eq!(index_count % 6, 0, "{label} mesh has partial quad indices");
                }
                if any {
                    nonempty_chunks += 1;
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
        }

        assert!(
            nonempty_chunks > 30,
            "expected the island to span many chunks, got {nonempty_chunks}"
        );
        assert!(above_total > 100_000, "above-water terrain too small");
        assert!(meadow_total > 10_000, "meadow cover too small");
        assert!(below_total > 100_000, "underwater terrain too small");
        assert!(water_total > 10_000, "water too small");
    }
}
