//! Terrain material that moves per-voxel brightness **jitter** off the
//! vertices and into the fragment shader.
//!
//! The MagicaVoxel look needs a small per-voxel brightness speckle. Baking it
//! into vertex colors blocks greedy meshing (merging N voxels into one quad
//! collapses the per-voxel values). Instead we recompute the speckle in the
//! fragment shader from the fragment's world position, so it survives any
//! future merge. Every voxel type's jitter is mean-1.0
//! (`center + span·roll`, `center = 1 − span/2`), so only the per-type
//! **amplitude** (`span/2`) has to travel — the mesher packs it into spare bits
//! of the vertex's face word, and the vertex shader re-presents it as vertex
//! alpha for the fragment stage. See `docs/voxel-engine-plan.md` Stage 2.
//!
//! This is an [`ExtendedMaterial`] on top of [`StandardMaterial`], so all of
//! Bevy's PBR lighting, shadows, and fog still apply — we only add the jitter
//! multiply after the standard material is evaluated.

use bevy::mesh::{MeshVertexAttribute, MeshVertexBufferLayoutRef, VertexFormat};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::pbr::{MaterialExtensionKey, MaterialExtensionPipeline};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::render::render_resource::{RenderPipelineDescriptor, SpecializedMeshPipelineError};
use bevy::render::storage::ShaderStorageBuffer;
use bevy::shader::ShaderRef;
use voxel_core::world::{VOXEL_SIZE, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z};

/// The terrain material: StandardMaterial PBR + the voxel jitter/AO extension.
pub type VoxelTerrainMaterial = ExtendedMaterial<StandardMaterial, VoxelExtension>;

/// The grass material: plain StandardMaterial PBR (vertex-color tone) plus a
/// **wind** extension that overrides *both* the main-pass and depth-prepass
/// vertex shaders with the same sway. It is a **separate type** from the
/// terrain material on purpose: a custom vertex stage must be matched by a
/// custom prepass vertex stage or the two passes disagree on depth and
/// z-fight, so we keep that machinery off the shared terrain material entirely.
pub type GrassMaterial = ExtendedMaterial<StandardMaterial, GrassExtension>;

/// Wind extension: swaps in the grass vertex + prepass-vertex shaders and
/// carries the wind time. Time rides on the material (not `globals`) because
/// `globals` is only bound in the main pass, not the depth prepass — and both
/// passes must read the *same* time so their displaced depths match.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, Default)]
pub struct GrassExtension {
    /// `x` = current time (s), `y` = previous-frame time (s, for motion
    /// vectors). Updated every frame by `update_grass_wind`.
    #[uniform(100)]
    pub time: Vec4,
    /// The live weather wind: `xy` = unit direction on the ground plane
    /// (x, z), `z` = strength `0..1` (wind speed against
    /// [`FULL_SWAY_WIND_SPEED`]). Rides on the material for the same reason the
    /// time does — the depth prepass has no `globals`, and both passes must read
    /// identical values or the displaced grass z-fights.
    #[uniform(101)]
    pub wind: Vec4,
}

/// Wind speed (m/s) at which grass sway saturates — full flutter rate, full
/// amplitude, fully leaned downwind. Above this the grass simply stays pinned.
pub const FULL_SWAY_WIND_SPEED: f32 = 20.0;

impl MaterialExtension for GrassExtension {
    fn vertex_shader() -> ShaderRef {
        "shaders/grass.wgsl".into()
    }
    // MUST match the main vertex shader's positions bit-for-bit (same wind, same
    // math) so the depth prepass agrees and grass doesn't z-fight.
    fn prepass_vertex_shader() -> ShaderRef {
        "shaders/grass_prepass.wgsl".into()
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct VoxelExtension {
    /// `x` = world seed reinterpreted as f32 bits (`bitcast<u32>` in WGSL),
    /// `y` = VOXEL_SIZE (m), `z` = half world-X (voxels), `w` = half world-Z.
    /// Lets the shader recover each fragment's voxel coordinate for both the
    /// jitter hash and the ambient-occlusion neighbor lookups.
    #[uniform(100)]
    pub params: Vec4,
    /// World voxel dimensions `(x, y, z, _)` — for indexing the occupancy bits.
    #[uniform(101)]
    pub dimensions: Vec4,
    /// Packed solid-occupancy bitset (1 bit/voxel). The shader reads it to
    /// recompute per-voxel ambient occlusion in the fragment stage, so greedy
    /// meshing can merge flat faces regardless of their AO.
    #[storage(102, read_only)]
    pub occupancy: Handle<ShaderStorageBuffer>,
    /// Hemisphere ambient (GI feel): rgb = sky ambient colour (up-facing),
    /// w = strength (0 = off). Updated per-frame from the sky.
    #[uniform(103)]
    pub ambient_sky: Vec4,
    /// rgb = ground-bounce ambient colour (down-facing), w = AO-strength boost.
    #[uniform(104)]
    pub ambient_ground: Vec4,
    /// Procedural environment reflection (IBL feel): rgb = sky reflection colour
    /// (zenith), w = intensity (0 = off). Fresnel-boosted so it reads as a sky
    /// sheen at grazing angles.
    #[uniform(105)]
    pub env_reflection: Vec4,
    /// Occupancy indexing origin in voxels `(origin_x, 0, origin_z, 0)`. The
    /// island leaves this `ZERO` (its occupancy buffer is global, indexed in
    /// world coords); a streamed chunk sets it to its per-chunk buffer origin so
    /// the shader can localize global voxel coordinates. See `voxel_terrain.wgsl`.
    #[uniform(106)]
    pub chunk_origin: Vec4,
}

/// Packed vertex position + face data: `x`, `y`, `z` as 16-bit fixed point in
/// CHUNK-LOCAL space, then a bitfield (see [`pack_face_word`]).
///
/// Voxel geometry doesn't need float positions. A chunk spans a known
/// 64 × 256 × 64 voxels, so once vertices are chunk-local (the entity transform
/// puts them back in the world) 16 bits per axis resolves them to about half a
/// millimetre — 1/250th of a voxel. That takes the vertex from 40 bytes to 12,
/// which matters most for *bandwidth*: every one of these vertices is read
/// again by the depth prepass, four shadow cascades and the reflection view.
pub const ATTRIBUTE_VOXEL_POSITION: MeshVertexAttribute =
    MeshVertexAttribute::new("Voxel_Position", 91_534_001, VertexFormat::Uint16x4);

/// Packed vertex colour: rgb as 8-bit unorm. Alpha is unused — the jitter
/// amplitude and the baked-AO flag it used to carry now live in the position
/// word's spare bits, where they get 12 bits instead of 8.
pub const ATTRIBUTE_VOXEL_COLOR: MeshVertexAttribute =
    MeshVertexAttribute::new("Voxel_Color", 91_534_002, VertexFormat::Unorm8x4);

/// Lowest chunk-local coordinate the packed position can represent, in metres.
/// A little below zero so cover geometry that pokes just outside its cell (and
/// the mesher's 1-voxel apron) still encodes.
pub const PACKED_POSITION_ORIGIN: f32 = -1.0;
/// Span the packed position covers, in metres. A chunk is 32 m tall
/// (`WORLD_SIZE_Y` voxels) and 8 m across, so this clears the tallest axis with
/// headroom at both ends.
pub const PACKED_POSITION_SPAN: f32 = 34.0;

/// Quantise one chunk-local coordinate (metres) to 16-bit fixed point.
pub fn pack_position_axis(value: f32) -> u16 {
    let normalized = (value - PACKED_POSITION_ORIGIN) / PACKED_POSITION_SPAN;
    (normalized.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16
}

/// Inverse of [`pack_position_axis`] — the same maths the vertex shaders do,
/// available to Rust so tests can check geometry without a GPU.
#[cfg_attr(not(test), allow(dead_code))]
pub fn unpack_position_axis(packed: u16) -> f32 {
    packed as f32 / u16::MAX as f32 * PACKED_POSITION_SPAN + PACKED_POSITION_ORIGIN
}

/// The fourth position component: face direction, the baked-AO flag, and the
/// per-voxel jitter amplitude, packed into one 16-bit word.
///
/// * bits 0–2 — face index into `mesh::FACE_DIRECTIONS`; the shader turns it
///   back into a unit normal, since voxel faces only ever point six ways.
/// * bit 3 — "ambient occlusion is already baked into the vertex colour", the
///   flag the terrain shader used to read as a `+10` sentinel on vertex alpha.
/// * bits 4–15 — jitter amplitude, 12-bit unorm.
pub fn pack_face_word(face_index: u16, ambient_occlusion_baked: bool, amplitude: f32) -> u16 {
    let quantized_amplitude = (amplitude.clamp(0.0, 1.0) * 4095.0).round() as u16;
    (face_index & 0x7) | (u16::from(ambient_occlusion_baked) << 3) | (quantized_amplitude << 4)
}

impl MaterialExtension for VoxelExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/voxel_terrain.wgsl".into()
    }

    fn vertex_shader() -> ShaderRef {
        "shaders/voxel_terrain_vertex.wgsl".into()
    }

    /// A custom vertex stage MUST be mirrored in the depth prepass or the two
    /// passes disagree on depth and the terrain z-fights against itself. Same
    /// rule the grass material follows.
    fn prepass_vertex_shader() -> ShaderRef {
        "shaders/voxel_terrain_prepass_vertex.wgsl".into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // Every mesh drawn with this material must share this layout. Bevy keys
        // mesh slabs by vertex stride and puts the slab in the batch set key, so
        // a second layout here would split the batch — the same trap that made
        // mixed-width index buffers a net loss.
        let vertex_layout = layout.0.get_layout(&[
            ATTRIBUTE_VOXEL_POSITION.at_shader_location(0),
            ATTRIBUTE_VOXEL_COLOR.at_shader_location(1),
        ])?;
        descriptor.vertex.buffers = vec![vertex_layout];

        // `VertexOutput.color` — and the fragment shader's `in.color` — sit
        // behind `#ifdef VERTEX_COLORS`, which bevy only defines when the mesh
        // carries the *built-in* colour attribute. Ours is a custom packed one,
        // so the def has to be declared by hand, for both stages.
        descriptor.vertex.shader_defs.push("VERTEX_COLORS".into());
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader_defs.push("VERTEX_COLORS".into());
        }
        Ok(())
    }
}

/// Build the extension for a given world seed and a handle to its
/// solid-occupancy bitset (upload the bits from
/// [`voxel_core::world::VoxelWorld::solid_occupancy_bits`] into an
/// `Assets<ShaderStorageBuffer>` and pass the handle here).
pub fn voxel_extension(seed: u32, occupancy: Handle<ShaderStorageBuffer>) -> VoxelExtension {
    VoxelExtension {
        params: Vec4::new(
            f32::from_bits(seed),
            VOXEL_SIZE,
            WORLD_SIZE_X as f32 / 2.0,
            WORLD_SIZE_Z as f32 / 2.0,
        ),
        dimensions: Vec4::new(
            WORLD_SIZE_X as f32,
            WORLD_SIZE_Y as f32,
            WORLD_SIZE_Z as f32,
            0.0,
        ),
        occupancy,
        // `update_terrain_lighting` fills these from the sky + LightingSettings
        // each frame. Ground .w = AO strength; default 1.0 = the baked AO look.
        ambient_sky: Vec4::ZERO,
        ambient_ground: Vec4::new(0.0, 0.0, 0.0, 1.0),
        env_reflection: Vec4::ZERO,
        // Island occupancy is global / world-indexed, so no origin shift.
        chunk_origin: Vec4::ZERO,
    }
}

/// Extension for one streamed chunk of the infinite world. Two differences from
/// the island's [`voxel_extension`]: the jitter is **un-centered** (offset `0`,
/// so the per-voxel hash keys on raw world-voxel coordinates and stays seamless
/// across chunks), and ambient occlusion reads a **per-chunk** occupancy buffer
/// localized by `chunk_origin` + a per-chunk `dimensions` span (the infinite
/// world has no single global occupancy buffer). `origin`/`span` come from the
/// chunk's [`voxel_core::world::ChunkScratch::window`].
pub fn streamed_voxel_extension(
    seed: u32,
    occupancy: Handle<ShaderStorageBuffer>,
    origin_x: i32,
    origin_z: i32,
    span_x: i32,
    span_z: i32,
) -> VoxelExtension {
    VoxelExtension {
        params: Vec4::new(f32::from_bits(seed), VOXEL_SIZE, 0.0, 0.0),
        dimensions: Vec4::new(span_x as f32, WORLD_SIZE_Y as f32, span_z as f32, 0.0),
        occupancy,
        ambient_sky: Vec4::ZERO,
        ambient_ground: Vec4::new(0.0, 0.0, 0.0, 1.0),
        env_reflection: Vec4::ZERO,
        chunk_origin: Vec4::new(origin_x as f32, 0.0, origin_z as f32, 0.0),
    }
}
