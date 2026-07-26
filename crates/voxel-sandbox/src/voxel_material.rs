//! Terrain material that moves per-voxel brightness **jitter** off the
//! vertices and into the fragment shader.
//!
//! The MagicaVoxel look needs a small per-voxel brightness speckle. Baking it
//! into vertex colors blocks greedy meshing (merging N voxels into one quad
//! collapses the per-voxel values). Instead we recompute the speckle in the
//! fragment shader from the fragment's world position, so it survives any
//! future merge. Every voxel type's jitter is mean-1.0
//! (`center + span·roll`, `center = 1 − span/2`), so only the per-type
//! **amplitude** (`span/2`) has to travel — the mesher packs it into the
//! otherwise-unused vertex-color alpha (terrain is opaque; water is a separate
//! material). See `docs/voxel-engine-plan.md` Stage 2.
//!
//! This is an [`ExtendedMaterial`] on top of [`StandardMaterial`], so all of
//! Bevy's PBR lighting, shadows, and fog still apply — we only add the jitter
//! multiply after the standard material is evaluated.

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
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

impl MaterialExtension for VoxelExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/voxel_terrain.wgsl".into()
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
