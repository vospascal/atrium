//! Stage-9 streaming: an infinite world meshed around the camera, rendered by
//! the **same** greedy mesher and terrain material as the fixed island.
//!
//! Opt-in via `VOXEL_STREAMING=1` (see `main.rs`), which skips the fixed island
//! and instead streams chunks of [`StreamedSource`] around the camera: chunks
//! within a load radius are generated + meshed + spawned, chunks beyond an
//! unload radius are despawned. Generation + greedy meshing run on the async
//! compute pool; finished chunks are spawned on the main thread.
//!
//! Every chunk goes through [`mesh::build_chunk_meshes`] on a [`StreamedSource`]
//! — the identical code path the island uses — so streamed terrain gets the
//! island's colors, per-voxel jitter, corner ambient occlusion, trees, grass
//! cover, and water with no second renderer. Because [`StreamedSource`] is a
//! pure function of world position, face culling and AO across chunk borders are
//! seamless without the neighbour chunk being resident.

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::storage::ShaderStorageBuffer;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use std::collections::HashMap;
use std::sync::Arc;

use voxel_core::streamed_source::StreamedSource;
use voxel_core::voxel_source::{IslandSource, VoxelSource};
use voxel_core::world::{ChunkScratch, Voxel, VOXEL_SIZE, WATER_LEVEL, WORLD_SIZE_Y};

use crate::geometry_census::{GeometryCensus, GeometryKind};
use crate::grass;
use crate::mesh::{self, CHUNK_SIZE};
use crate::voxel_material::{self, VoxelTerrainMaterial};
use crate::water;
use crate::CanopyConfetti;

/// Extra margin beyond the live load radius before a chunk is despawned —
/// hysteresis, so chunks at the edge don't thrash in and out as the camera
/// drifts across a border. The load radius itself is a live perf lever
/// (`RenderQuality::stream_radius`), since resident geometry grows with its
/// square; the fog sea hides the far edge.
const UNLOAD_MARGIN: i32 = 2;
/// Widest water quad, in voxel columns. Bounds how coarsely the wave
/// displacement in `water_surface.wgsl` is sampled across the surface (a chunk is
/// 64 columns, so un-capped runs would be 8 m wide and could only tilt).
const WATER_QUAD_MAX_COLUMNS: i32 = 4;
/// Cap NEW async generate+mesh tasks spawned per frame so the pool isn't flooded
/// and first fill spreads out (the tasks themselves run off-thread).
const TASKS_PER_FRAME: usize = 6;

/// One finished chunk, produced off-thread: the island mesher's six sub-meshes
/// plus the per-chunk solid-occupancy buffer (for shader AO) and its window.
struct BuiltChunk {
    meshes: mesh::ChunkMeshes,
    /// Flat water surface for the chunk's sunken columns, rendered with the
    /// island's water shader (see [`build_water_surface_mesh`]).
    water_surface: Option<Mesh>,
    /// Every grass clump in the chunk, baked into ONE mesh off-thread. The
    /// mesher deliberately skips `TallGrass`, so a chunk places its own grass
    /// or the terrain reads bare (see [`grass::build_chunk_grass_mesh`]).
    grass_mesh: Option<Mesh>,
}

/// A resident chunk: its always-drawn entities plus the **near-detail** it only
/// keeps spawned while the camera is close.
///
/// Streaming is CPU/entity-bound, not geometry-bound — measured on this world,
/// dropping ~6k tiny grass entities gained twice as much frame time as dropping
/// 2.3M vertices of leaf confetti. So the two dense-but-small-scale details are
/// tiered by distance and spawned/despawned as the camera moves, while the
/// terrain silhouette stays resident out to the full view distance.
struct LoadedChunk {
    /// Terrain, cover, and water — resident for the chunk's whole life.
    base: Vec<Entity>,
    /// Leaf-confetti mesh, kept so the detail can come back when the camera
    /// returns without regenerating the chunk. The solid inner canopy stays
    /// resident, so distant trees keep their silhouette and shadow.
    canopy_mesh: Option<(Handle<Mesh>, GeometryCensus)>,
    canopy: Option<Entity>,
    /// The chunk's baked grass mesh, kept so the detail can come back when the
    /// camera returns, and its live entity while near.
    grass_mesh: Option<(Handle<Mesh>, GeometryCensus)>,
    grass: Option<Entity>,
}

/// Where a chunk's voxels come from. Both variants are [`VoxelSource`]s meshed
/// by the same greedy mesher into the same materials — the *only* difference
/// between "the island" and "the infinite world" is which one of these the
/// streamer holds. There is no island render path.
///
/// Cheap to clone (a unit or an `Arc` bump), because every queued chunk task
/// takes its own handle onto the compute pool.
#[derive(Clone)]
pub enum ChunkSource {
    /// The infinite procedural world. Stateless — a pure function of position
    /// and seed — so each chunk task builds its own [`StreamedSource`] rather
    /// than sharing one.
    Streamed,
    /// The fixed island, bounded and shared. Its dense grid is generated once
    /// up front and read by every chunk task through the `Arc`.
    Island(Arc<IslandSource>),
}

impl ChunkSource {
    /// Half-extent in chunks beyond which no chunk is ever generated, or `None`
    /// for the infinite world. The island's bound comes from its footprint; the
    /// streamed world's is the optional `VOXEL_STREAM_BOUNDS` "set world size".
    fn bounds(&self) -> Option<i32> {
        match self {
            ChunkSource::Streamed => std::env::var("VOXEL_STREAM_BOUNDS")
                .ok()
                .and_then(|value| value.parse().ok()),
            ChunkSource::Island(_) => Some(IslandSource::chunk_bounds(CHUNK_SIZE)),
        }
    }

    /// Where this source's chunks get their ambient occlusion — and the reason
    /// unifying the two worlds costs the island nothing visually.
    ///
    /// The island has a dense grid of the whole world, so it can hand the shader
    /// **one global occupancy bitset** and recompute exact AO per fragment, with
    /// every chunk still sharing a single material. The infinite world has no
    /// such grid: per-fragment AO there would need a buffer, and therefore a
    /// material, per resident chunk — which stops bevy batching and measured
    /// ~2.5× the frame time. So it bakes AO into vertex colours instead.
    ///
    /// One material either way; the mode follows from what the source can offer.
    fn ambient_occlusion(&self) -> mesh::AmbientOcclusion {
        match self {
            ChunkSource::Streamed => mesh::AmbientOcclusion::Baked,
            ChunkSource::Island(_) => mesh::AmbientOcclusion::PerFragment,
        }
    }

    /// The occupancy bitset backing per-fragment AO, in the shader's global
    /// indexing (`(z * WORLD_SIZE_X + x) * WORLD_SIZE_Y + y`). The streamed
    /// world bakes its AO, so it binds a one-word dummy: `AsBindGroup` has no
    /// optional storage binding, and the vertex-alpha sentinel means the
    /// shader's occupancy path is never taken.
    fn occupancy_bits(&self) -> Vec<u32> {
        match self {
            ChunkSource::Streamed => vec![0],
            ChunkSource::Island(island) => island.world().solid_occupancy_bits(),
        }
    }
}

/// Tracks streamed chunks around the camera. Generation + greedy meshing run on
/// the async compute pool (`pending`); finished chunks are spawned as entities
/// (`loaded`) on the main thread. A shared water material and an optional world
/// bound keep it cheap and, if set, finite.
#[derive(Resource)]
pub struct ChunkStreamer {
    /// Chunk coord → its resident entities + distance-tiered detail.
    loaded: HashMap<(i32, i32), LoadedChunk>,
    /// Chunks generating+meshing off-thread (chunk coord → task).
    pending: HashMap<(i32, i32), Task<BuiltChunk>>,
    /// Shared water material for every streamed chunk (created once) — the
    /// island's [`water::WaterMaterial`], so streamed water gets the same tint,
    /// reflection, and glint. Water needs no per-chunk data, so one handle
    /// serves all chunks; `water::update_water` animates it for free.
    water_material: Option<Handle<water::WaterMaterial>>,
    /// The ONE shared grass wind material. Tones live in the batched meshes'
    /// vertex colours, so a single material serves every chunk.
    grass_material: Option<Handle<crate::voxel_material::GrassMaterial>>,
    seed: u32,
    /// The ONE terrain material every streamed chunk shares. Sharing it is what
    /// lets bevy batch chunk meshes together — worth ~2.5× the frame time here —
    /// and it is only possible because streamed chunks bake their ambient
    /// occlusion into vertex colours (see [`mesh::AmbientOcclusion`]).
    terrain_material: Option<Handle<VoxelTerrainMaterial>>,
    /// Foliage season (0 = high summer, 1 = deep autumn) baked into the chunks
    /// currently resident — the same knob the island passes to the mesher.
    season: f32,
    /// Set by `regenerate_system` when the seed or season changed (the R key, or
    /// the panel's season slider). Every resident chunk is dropped and rebuilt
    /// with the new values; the island's rebuild path does not apply here.
    reload_requested: bool,
    /// Half-extent of the world in chunks (`None` = infinite). Chunks outside
    /// `[-bounds, bounds]` on either axis are never generated — a "set world
    /// size", also a hard cap on chunk count. Derived from the source.
    bounds: Option<i32>,
    /// What the chunks are made of: the island or the infinite world.
    source: ChunkSource,
}

impl ChunkStreamer {
    pub fn new(seed: u32, season: f32, source: ChunkSource) -> Self {
        Self {
            loaded: HashMap::new(),
            pending: HashMap::new(),
            water_material: None,
            grass_material: None,
            terrain_material: None,
            seed,
            season,
            reload_requested: false,
            bounds: source.bounds(),
            source,
        }
    }

    /// Is this the fixed island? Regenerating it means building a whole new
    /// grid (and everything derived from it), where the infinite world only
    /// needs a new seed.
    pub fn is_island(&self) -> bool {
        matches!(self.source, ChunkSource::Island(_))
    }

    /// Rebuild every resident chunk with a new seed / season. The streamed world's
    /// answer to the island's regenerate: there is no single world to respawn, so
    /// the chunks around the camera are dropped and stream back in.
    pub fn request_reload(&mut self, seed: u32, season: f32) {
        self.seed = seed;
        self.season = season;
        self.reload_requested = true;
    }
}

/// What the streamer did on the main thread this frame.
///
/// Chunk *generation* is off-thread, but three things are not: despawning far
/// chunks, spawning the ones that finished, and the detail-tier pass that adds
/// and removes grass and confetti entities. Those are the streaming costs that
/// can actually spike a frame, so they are measured separately — a spike with
/// `spawned` > 0 is chunk hand-off, a spike with `detail_churn` > 0 is the
/// entity tiering, and a spike with neither is not the streamer's fault.
#[derive(Resource, Default)]
pub struct StreamStats {
    pub loaded_chunks: usize,
    pub pending_chunks: usize,
    /// Chunks handed off to the main thread this frame.
    pub spawned: usize,
    /// Detail entities spawned + despawned this frame (grass clumps, confetti).
    pub detail_churn: usize,
    /// This frame's main-thread cost of `stream_chunks`, in milliseconds.
    pub last_ms: f32,
    /// Worst `last_ms` seen in the recent past; decays so an old spike doesn't
    /// pin the readout forever.
    pub peak_ms: f32,
}

impl StreamStats {
    /// Fold this frame's cost in, decaying the peak so it tracks the recent
    /// worst case rather than the all-time one.
    fn record(&mut self, milliseconds: f32) {
        self.last_ms = milliseconds;
        self.peak_ms = self.peak_ms.max(milliseconds) * 0.99;
    }
}

/// Marks a streamed chunk entity (so it can be despawned/queried in bulk).
#[derive(Component)]
pub struct StreamedChunk;

/// Where a chunk sits in the world. Chunk meshes are built CHUNK-LOCAL (see
/// `mesh::build_chunk_meshes`) so their positions can be quantised tightly, and
/// this transform is what puts them back in the world. It is per-instance data
/// bevy already uploads, so it costs no batching.
fn chunk_transform(chunk_x: i32, chunk_z: i32) -> Transform {
    Transform::from_xyz(
        (chunk_x * CHUNK_SIZE) as f32 * VOXEL_SIZE,
        0.0,
        (chunk_z * CHUNK_SIZE) as f32 * VOXEL_SIZE,
    )
}

/// Which chunk coordinate a render-space position sits in.
fn chunk_of(render_x: f32, render_z: f32) -> (i32, i32) {
    let voxel_x = (render_x / voxel_core::world::VOXEL_SIZE).floor() as i32;
    let voxel_z = (render_z / voxel_core::world::VOXEL_SIZE).floor() as i32;
    (
        voxel_x.div_euclid(CHUNK_SIZE),
        voxel_z.div_euclid(CHUNK_SIZE),
    )
}

/// Flat water surface for one chunk, or `None` where the chunk has no sunken
/// columns. A single horizontal quad layer at the water plane over every column
/// whose terrain sits below [`WATER_LEVEL`], row-merged along X so open water
/// costs a handful of quads instead of one per column.
///
/// Rendered with the island's [`water::WaterMaterial`] so streamed water gets the
/// same tint, sky reflection, and sun glint. That shader *sets* each vertex's Y
/// from its `heights` buffer entry (indexed by `UV.x`), so every vertex carries
/// corner id `0` and the buffer holds the single flat water height — the island's
/// fluid-sim displacement path, minus the sim. Positions are still emitted at the
/// real water height so bevy derives a correct culling AABB.
fn build_water_surface_mesh(scratch: &ChunkScratch, chunk_x: i32, chunk_z: i32) -> Option<Mesh> {
    let origin_x = chunk_x * CHUNK_SIZE;
    let origin_z = chunk_z * CHUNK_SIZE;
    let surface_y = water::water_surface_y();

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for local_z in 0..CHUNK_SIZE {
        let world_z = origin_z + local_z;
        let mut local_x = 0;
        while local_x < CHUNK_SIZE {
            // Wetness is read from the voxels themselves rather than from a
            // terrain-height function: both sources place real `Voxel::Water`
            // cells, so this works for the island and the infinite world alike,
            // and it costs a column scan of an already-unpacked window instead
            // of a second round of terrain generation.
            let is_wet = |offset: i32| {
                let column_x = origin_x + offset;
                (0..=WATER_LEVEL).any(|y| scratch.get(column_x, y, world_z) == Voxel::Water)
            };
            if !is_wet(local_x) {
                local_x += 1;
                continue;
            }
            // Extend the run while columns stay sunken, then emit one quad —
            // but cap its width. The water surface shader displaces VERTICES for
            // the swell, so a quad only samples the wave field at its corners: an
            // 8 m merged quad would just tilt instead of undulating. Capping at
            // `WATER_QUAD_MAX_COLUMNS` keeps ~0.5 m of wave resolution, which
            // costs vertices — the resource streaming has to spare.
            let run_start = local_x;
            while local_x < CHUNK_SIZE
                && local_x - run_start < WATER_QUAD_MAX_COLUMNS
                && is_wet(local_x)
            {
                local_x += 1;
            }
            // Chunk-local, like every other chunk mesh; the entity transform
            // places it.
            let x_min = run_start as f32 * VOXEL_SIZE;
            let x_max = local_x as f32 * VOXEL_SIZE;
            let z_min = local_z as f32 * VOXEL_SIZE;
            let z_max = (local_z + 1) as f32 * VOXEL_SIZE;

            let base = positions.len() as u32;
            // Counter-clockwise seen from above, so the surface faces up.
            for (x, z) in [
                (x_min, z_min),
                (x_max, z_min),
                (x_max, z_max),
                (x_min, z_max),
            ] {
                positions.push([x, surface_y, z]);
                normals.push([0.0, 1.0, 0.0]);
                uvs.push([0.0, 0.0]);
            }
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }

    if positions.is_empty() {
        return None;
    }
    Some(
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(indices)),
    )
}

/// Generate + mesh one chunk off-thread: the island mesher on a
/// [`StreamedSource`], the flat water surface, plus the per-chunk occupancy
/// buffer for shader AO.
///
/// The voxel window is generated ONCE and shared: the mesher meshes from it and
/// the occupancy bitset is packed from the same voxels. That keeps the shader's
/// ambient occlusion faithful to the island's — occupancy covers *every* solid
/// voxel, tree trunks and leaves included, not just the terrain surface — while
/// costing one generation pass per chunk instead of two.
fn build_chunk(
    chunk_x: i32,
    chunk_z: i32,
    seed: u32,
    season: f32,
    source: &ChunkSource,
) -> BuiltChunk {
    // The two arms differ only in which `VoxelSource` they hand to the shared
    // builder: the infinite world is stateless so each task makes its own, the
    // island is a shared grid read through the `Arc`.
    match source {
        ChunkSource::Streamed => build_chunk_from(
            &StreamedSource::new(seed),
            chunk_x,
            chunk_z,
            seed,
            season,
            source.ambient_occlusion(),
        ),
        ChunkSource::Island(island) => build_chunk_from(
            island.as_ref(),
            chunk_x,
            chunk_z,
            seed,
            season,
            source.ambient_occlusion(),
        ),
    }
}

/// The one chunk-building path, over any [`VoxelSource`].
fn build_chunk_from<S: VoxelSource>(
    source: &S,
    chunk_x: i32,
    chunk_z: i32,
    seed: u32,
    season: f32,
    ambient_occlusion: mesh::AmbientOcclusion,
) -> BuiltChunk {
    let scratch = mesh::unpack_chunk_window(source, chunk_x, chunk_z);
    let clumps = harvest_grass_clumps(source, &scratch, chunk_x, chunk_z);
    // Baked here, on the compute pool, rather than spawned as thousands of
    // per-clump entities on the main thread. Chunk-local, like the chunk
    // meshes — the entity `Transform` places it.
    let grass_mesh = grass::build_chunk_grass_mesh(&clumps, chunk_x, chunk_z, seed);
    let meshes = mesh::build_chunk_meshes(
        source,
        &scratch,
        seed,
        season,
        chunk_x,
        chunk_z,
        ambient_occlusion,
    );
    let water_surface = build_water_surface_mesh(&scratch, chunk_x, chunk_z);
    BuiltChunk {
        meshes,
        water_surface,
        grass_mesh,
    }
}

/// Find this chunk's instanced-grass clump sites: the topmost [`Voxel::TallGrass`]
/// cell of every clump column (see [`grass::is_clump_column`]), tagged with its
/// biome tone variant. Reads the already-generated voxel window, so it costs a
/// scan rather than another generation pass.
pub(crate) fn harvest_grass_clumps<S: VoxelSource>(
    source: &S,
    scratch: &ChunkScratch,
    chunk_x: i32,
    chunk_z: i32,
) -> Vec<(i32, i32, i32, usize)> {
    let origin_x = chunk_x * CHUNK_SIZE;
    let origin_z = chunk_z * CHUNK_SIZE;
    let mut clumps = Vec::new();
    for local_z in 0..CHUNK_SIZE {
        for local_x in 0..CHUNK_SIZE {
            let world_x = origin_x + local_x;
            let world_z = origin_z + local_z;
            if !grass::is_clump_column(world_x, world_z) {
                continue;
            }
            // Topmost tall-grass cell in the column, matching the island's scan.
            let mut grass_top: Option<i32> = None;
            for y in 0..WORLD_SIZE_Y as i32 {
                if scratch.get(world_x, y, world_z) == Voxel::TallGrass {
                    grass_top = Some(y);
                }
            }
            let Some(top_y) = grass_top else {
                continue;
            };
            let variant = grass::clump_variant(
                source.dryness_at(world_x, world_z),
                source.water_distance_at(world_x, world_z),
            );
            clumps.push((world_x, top_y, world_z, variant));
        }
    }
    clumps
}

/// Stream chunks around the camera: despawn far ones, spawn finished async
/// chunks, then queue the nearest missing chunks for async generate+mesh.
/// Generation never blocks the render thread.
#[allow(clippy::too_many_arguments)]
pub fn stream_chunks(
    mut commands: Commands,
    streamer: Option<ResMut<ChunkStreamer>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut terrain_materials: ResMut<Assets<VoxelTerrainMaterial>>,
    mut water_materials: ResMut<Assets<water::WaterMaterial>>,
    mut grass_materials: ResMut<Assets<crate::voxel_material::GrassMaterial>>,
    mut storage_buffers: ResMut<Assets<ShaderStorageBuffer>>,
    reflection_target: Res<water::ReflectionTarget>,
    quality: Res<crate::RenderQuality>,
    mut stats: ResMut<StreamStats>,
    camera: Query<&GlobalTransform, With<bevy::core_pipeline::prepass::DepthPrepass>>,
) {
    let stream_started = std::time::Instant::now();
    let debug = std::env::var("VOXEL_DEBUG_STREAM").is_ok();
    // Only active in the streaming world (the resource is inserted then).
    let Some(mut streamer) = streamer else {
        if debug {
            warn!("stream_chunks: ChunkStreamer resource absent");
        }
        return;
    };
    let camera = match camera.single() {
        Ok(camera) => camera,
        Err(error) => {
            if std::env::var("VOXEL_DEBUG_STREAM").is_ok() {
                warn!(
                    "stream_chunks: camera query failed ({error:?}) — {} DepthPrepass cameras",
                    camera.iter().count()
                );
            }
            return;
        }
    };
    let position = camera.translation();
    let (center_x, center_z) = chunk_of(position.x, position.z);
    let seed = streamer.seed;
    let season = streamer.season;
    let bounds = streamer.bounds;
    // A bounded world is small and finite, so keep **all** of it resident
    // instead of following the camera with a radius. Two reasons: the overhead
    // and orbit views see the whole island at once, and a radius smaller than
    // the footprint would clip it; and streaming a finite world means chunks
    // churn in and out as the camera moves, which is a frame-time spike for
    // geometry we were always going to need again. The infinite world has no
    // such option and follows the live `stream_radius` lever.
    // A finite world is small enough to hold entirely, so nothing about it is
    // decided by where the camera is: not which chunks exist, not which keep
    // their grass and confetti. That matters most in the orbit view, where the
    // camera sits well outside the island looking back at it.
    let finite = bounds.is_some();
    let load_radius = match bounds {
        Some(bound) => bound,
        None => quality.stream_radius as i32,
    };
    // Beyond the bound there is nothing to unload, so this only ever bites in
    // the infinite world.
    let unload_radius = load_radius + UNLOAD_MARGIN;

    // One shared water material, created on first run. Its `heights` buffer is a
    // single flat water height — the streamed world has no fluid sim, and the
    // water vertex shader reads every corner from entry 0 (see
    // `build_water_surface_mesh`).
    if streamer.water_material.is_none() {
        let heights = storage_buffers.add(ShaderStorageBuffer::from(
            [water::water_surface_y()].as_slice(),
        ));
        streamer.water_material = Some(water_materials.add(water::WaterMaterial {
            water: water::WaterUniform::default(),
            reflection: reflection_target.image.clone(),
            reflection_clip_from_world: Mat4::IDENTITY,
            heights,
        }));
    }
    let water_material = streamer.water_material.clone().unwrap();

    // The ONE shared terrain material — for both worlds. The island binds its
    // global occupancy bitset here and gets exact per-fragment AO; the infinite
    // world binds a dummy word and reads its AO from baked vertex colours.
    // Either way every chunk shares this one handle, which is what lets bevy
    // batch them.
    if streamer.terrain_material.is_none() {
        let occupancy =
            storage_buffers.add(ShaderStorageBuffer::from(streamer.source.occupancy_bits()));
        let extension = match &streamer.source {
            ChunkSource::Island(_) => voxel_material::voxel_extension(seed, occupancy),
            ChunkSource::Streamed => {
                voxel_material::streamed_voxel_extension(seed, occupancy, 0, 0, 1, 1)
            }
        };
        streamer.terrain_material = Some(terrain_materials.add(VoxelTerrainMaterial {
            base: StandardMaterial {
                base_color: Color::WHITE,
                perceptual_roughness: 0.95,
                ..default()
            },
            extension,
        }));
    }
    let terrain_material = streamer.terrain_material.clone().unwrap();

    // The one shared grass wind material, created on first run.
    if streamer.grass_material.is_none() {
        streamer.grass_material = Some(grass::build_grass_material(&mut grass_materials));
    }
    let grass_material = streamer.grass_material.clone().unwrap();

    // A reload (new seed or season) drops everything; the chunks around the
    // camera then stream straight back in with the new values.
    if streamer.reload_requested {
        streamer.reload_requested = false;
        for (_, chunk) in streamer.loaded.drain() {
            for entity in chunk
                .base
                .into_iter()
                .chain(chunk.canopy)
                .chain(chunk.grass)
            {
                commands.entity(entity).despawn();
            }
        }
        // In-flight tasks carry the OLD seed/season, so drop them too.
        streamer.pending.clear();
    }

    // Drop chunks (loaded entities + in-flight tasks) beyond the unload radius.
    // A finite world has nothing to drop: it is entirely resident by definition,
    // and unloading it by camera distance would tear the island apart the moment
    // the orbit camera pulls back from it.
    let far = |cx: i32, cz: i32| {
        !finite && ((cx - center_x).abs() > unload_radius || (cz - center_z).abs() > unload_radius)
    };
    let to_remove: Vec<(i32, i32)> = streamer
        .loaded
        .keys()
        .copied()
        .filter(|&(cx, cz)| far(cx, cz))
        .collect();
    for key in to_remove {
        if let Some(chunk) = streamer.loaded.remove(&key) {
            for entity in chunk
                .base
                .into_iter()
                .chain(chunk.canopy)
                .chain(chunk.grass)
            {
                commands.entity(entity).despawn();
            }
        }
    }
    streamer.pending.retain(|&(cx, cz), _| !far(cx, cz));

    // Collect async tasks that finished this frame, then spawn their entities.
    let mut completed: Vec<((i32, i32), BuiltChunk)> = Vec::new();
    for (&key, task) in streamer.pending.iter_mut() {
        if let Some(built) = block_on(future::poll_once(&mut *task)) {
            completed.push((key, built));
        }
    }
    stats.spawned = completed.len();
    for (key, built) in completed {
        streamer.pending.remove(&key);

        let BuiltChunk {
            meshes: chunk_meshes,
            water_surface,
            grass_mesh,
        } = built;

        // `VOXEL_DEBUG_STREAM=1` logs each spawned chunk's sub-mesh vertex counts
        // — a headless check that the island mesher produced terrain, trees
        // (canopy), grass (meadow cover), and water on the streamed source.
        if std::env::var("VOXEL_DEBUG_STREAM").is_ok() {
            let count = |mesh: &Option<Mesh>| mesh.as_ref().map_or(0, Mesh::count_vertices);
            info!(
                "streamed chunk {key:?}: terrain {} + meadow {} + underwater {} + canopy {} (+solid {}) + water {} verts",
                count(&chunk_meshes.terrain_above_water),
                count(&chunk_meshes.meadow_cover),
                count(&chunk_meshes.terrain_below_water),
                count(&chunk_meshes.canopy),
                count(&chunk_meshes.canopy_solid),
                count(&chunk_meshes.water),
            );
        }

        // Spawn one entity per non-empty sub-mesh, mirroring the island's
        // reflection/shadow recipe exactly (see `spawn_world` in `main.rs`):
        // above-water terrain and canopy are reflection-visible; the meadow
        // carpet and anything below the waterline are main-view only and cast no
        // shadows; the confetti is reflection-visible but not a shadow caster
        // (the solid inner canopy is the caster). Streamed chunks are tagged
        // `StreamedChunk` — deliberately NOT `WorldMesh`, whose regenerate (R)
        // system would despawn them behind the streamer's back.
        let mut entities: Vec<Entity> = Vec::new();
        if let Some(chunk_mesh) = chunk_meshes.terrain_above_water {
            let census = GeometryCensus::of(&chunk_mesh, GeometryKind::TerrainAbove);
            entities.push(
                commands
                    .spawn((
                        Mesh3d(meshes.add(chunk_mesh)),
                        MeshMaterial3d(terrain_material.clone()),
                        chunk_transform(key.0, key.1),
                        crate::water::reflective_layers(),
                        census,
                        StreamedChunk,
                    ))
                    .id(),
            );
        }
        for (main_view_mesh, kind) in [
            (chunk_meshes.meadow_cover, GeometryKind::MeadowCover),
            (chunk_meshes.terrain_below_water, GeometryKind::TerrainBelow),
        ] {
            let Some(main_view_mesh) = main_view_mesh else {
                continue;
            };
            let census = GeometryCensus::of(&main_view_mesh, kind);
            entities.push(
                commands
                    .spawn((
                        Mesh3d(meshes.add(main_view_mesh)),
                        MeshMaterial3d(terrain_material.clone()),
                        chunk_transform(key.0, key.1),
                        NotShadowCaster,
                        census,
                        StreamedChunk,
                    ))
                    .id(),
            );
        }
        // Leaf confetti is near-detail: the mesh is kept, and the detail pass
        // below spawns it only while the camera is close (the solid inner canopy
        // above stays resident, so distant trees keep their shape and shadow).
        let canopy_mesh = chunk_meshes.canopy.map(|mesh| {
            // Measured here, kept alongside the handle: the mesh itself is
            // uploaded and dropped, and the confetti entity comes and goes with
            // the detail tier.
            let census = GeometryCensus::of(&mesh, GeometryKind::Canopy);
            (meshes.add(mesh), census)
        });
        if let Some(chunk_mesh) = chunk_meshes.canopy_solid {
            let census = GeometryCensus::of(&chunk_mesh, GeometryKind::CanopySolid);
            entities.push(
                commands
                    .spawn((
                        Mesh3d(meshes.add(chunk_mesh)),
                        MeshMaterial3d(terrain_material.clone()),
                        chunk_transform(key.0, key.1),
                        crate::water::reflective_layers(),
                        census,
                        StreamedChunk,
                    ))
                    .id(),
            );
        }
        // The mesher's per-chunk water faces (`chunk_meshes.water`) are voxel
        // geometry with no UVs, which the water shader needs; the flat surface
        // built alongside it covers the same columns as a single clean layer.
        if let Some(surface_mesh) = water_surface {
            let census = GeometryCensus::of(&surface_mesh, GeometryKind::Water);
            entities.push(
                commands
                    .spawn((
                        Mesh3d(meshes.add(surface_mesh)),
                        MeshMaterial3d(water_material.clone()),
                        chunk_transform(key.0, key.1),
                        NotShadowCaster,
                        census,
                        StreamedChunk,
                    ))
                    .id(),
            );
        }
        streamer.loaded.insert(
            key,
            LoadedChunk {
                base: entities,
                canopy_mesh,
                canopy: None,
                grass_mesh: grass_mesh.map(|mesh| {
                    let census = GeometryCensus::of(&mesh, GeometryKind::GrassClump);
                    (meshes.add(mesh), census)
                }),
                grass: None,
            },
        );
    }

    // Detail tiers: spawn confetti + grass only for chunks near the camera, and
    // drop them again as it moves away. Each is now ONE entity per chunk, so
    // this pass costs a handful of spawns rather than thousands.
    let grass_radius = quality.grass_radius as i32;
    let confetti_radius = quality.confetti_radius as i32;
    let mut detail_churn = 0_usize;
    for (&(chunk_x, chunk_z), chunk) in streamer.loaded.iter_mut() {
        let distance = (chunk_x - center_x).abs().max((chunk_z - center_z).abs());

        let wants_canopy = (finite || distance <= confetti_radius) && chunk.canopy_mesh.is_some();
        match (wants_canopy, chunk.canopy) {
            (true, None) => {
                let (mesh, census) = chunk.canopy_mesh.clone().unwrap();
                detail_churn += 1;
                chunk.canopy = Some(
                    commands
                        .spawn((
                            Mesh3d(mesh),
                            MeshMaterial3d(terrain_material.clone()),
                            chunk_transform(chunk_x, chunk_z),
                            NotShadowCaster,
                            CanopyConfetti,
                            crate::water::reflective_layers(),
                            census,
                            StreamedChunk,
                        ))
                        .id(),
                );
            }
            (false, Some(entity)) => {
                commands.entity(entity).despawn();
                chunk.canopy = None;
                detail_churn += 1;
            }
            _ => {}
        }

        let wants_grass = (finite || distance <= grass_radius) && chunk.grass_mesh.is_some();
        match (wants_grass, chunk.grass) {
            (true, None) => {
                let (mesh, census) = chunk.grass_mesh.clone().unwrap();
                detail_churn += 1;
                chunk.grass = Some(
                    commands
                        .spawn((
                            Mesh3d(mesh),
                            MeshMaterial3d(grass_material.clone()),
                            chunk_transform(chunk_x, chunk_z),
                            NotShadowCaster,
                            grass::GrassClump,
                            census,
                            StreamedChunk,
                        ))
                        .id(),
                );
            }
            (false, Some(entity)) => {
                commands.entity(entity).despawn();
                chunk.grass = None;
                detail_churn += 1;
            }
            _ => {}
        }
    }

    // Queue the nearest missing chunks (within bounds) for async generation.
    let mut wanted: Vec<(i32, i32, i32)> = Vec::new();
    // Finite: sweep the world's own extent. Infinite: sweep a window around the
    // camera. Sweeping a camera window over a finite world would leave the
    // island half-built whenever the camera sat off to one side of it.
    let (sweep_x, sweep_z) = match bounds {
        Some(bound) => (-bound..=bound, -bound..=bound),
        None => (
            (center_x - load_radius)..=(center_x + load_radius),
            (center_z - load_radius)..=(center_z + load_radius),
        ),
    };
    for cz in sweep_z {
        for cx in sweep_x.clone() {
            if streamer.loaded.contains_key(&(cx, cz)) || streamer.pending.contains_key(&(cx, cz)) {
                continue;
            }
            let distance = (cx - center_x).pow(2) + (cz - center_z).pow(2);
            wanted.push((distance, cx, cz));
        }
    }
    wanted.sort_by_key(|&(distance, _, _)| distance);

    let pool = AsyncComputeTaskPool::get();
    for &(_, cx, cz) in wanted.iter().take(TASKS_PER_FRAME) {
        // Each task gets its own handle on the source (a unit, or an `Arc` bump
        // onto the island's shared grid).
        let source = streamer.source.clone();
        let task = pool.spawn(async move { build_chunk(cx, cz, seed, season, &source) });
        streamer.pending.insert((cx, cz), task);
    }

    stats.loaded_chunks = streamer.loaded.len();
    stats.pending_chunks = streamer.pending.len();
    stats.detail_churn = detail_churn;
    stats.record(stream_started.elapsed().as_secs_f32() * 1000.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds one streamed chunk end-to-end (generation + the island mesher) and
    /// reports the wall time. Streaming needs this well under a frame's worth of
    /// budget per chunk; run with `--nocapture` to read the timing.
    #[test]
    fn build_chunk_produces_terrain() {
        let started = std::time::Instant::now();
        let built = build_chunk(0, 0, 7, 0.2, &ChunkSource::Streamed);
        let elapsed = started.elapsed();

        let count = |mesh: &Option<Mesh>| mesh.as_ref().map_or(0, Mesh::count_vertices);
        println!(
            "build_chunk took {elapsed:?} — terrain {} + meadow {} + underwater {} + canopy {} (+solid {}) + water {} verts",
            count(&built.meshes.terrain_above_water),
            count(&built.meshes.meadow_cover),
            count(&built.meshes.terrain_below_water),
            count(&built.meshes.canopy),
            count(&built.meshes.canopy_solid),
            count(&built.meshes.water),
        );
        assert!(
            built.meshes.terrain_above_water.is_some()
                || built.meshes.terrain_below_water.is_some(),
            "streamed chunk should mesh some terrain"
        );
    }

    /// The shader reads ambient occlusion from this bitset, so its layout has to
    /// agree with `voxel_terrain.wgsl`'s `is_solid` indexing exactly.
    #[test]
    fn chunk_occupancy_matches_terrain_surface() {
        let seed = 7;
        let source = StreamedSource::new(seed);
        let scratch = mesh::unpack_chunk_window(&source, 0, 0);
        let (origin_x, origin_z, span_x, span_z) = scratch.window();
        let bits = scratch.solid_occupancy_bits();
        assert_eq!((span_x, span_z), (CHUNK_SIZE + 2, CHUNK_SIZE + 2));

        // Spot-check that the packed bits agree with the terrain surface, in the
        // exact layout the shader indexes (see `chunk_occupancy`).
        let column_height = WORLD_SIZE_Y as i32;
        let is_set = |local_x: i32, y: i32, local_z: i32| {
            let index = ((local_z * span_x + local_x) * column_height + y) as usize;
            (bits[index >> 5] >> (index & 31)) & 1 == 1
        };
        for &(local_x, local_z) in &[(0, 0), (1, 1), (17, 40), (span_x - 1, span_z - 1)] {
            let height = voxel_core::world::terrain_column_height(
                origin_x + local_x,
                origin_z + local_z,
                seed,
            );
            assert!(
                is_set(local_x, height, local_z),
                "surface voxel should be solid at ({local_x}, {local_z})"
            );
            assert!(
                !is_set(local_x, height + 1, local_z),
                "the voxel above the surface should be air at ({local_x}, {local_z})"
            );
            assert!(
                is_set(local_x, 0, local_z),
                "bedrock should be solid at ({local_x}, {local_z})"
            );
        }
    }
}

#[cfg(test)]
mod season_tests {
    use super::*;

    /// Season must actually reach the streamed chunks. It used to be a constant,
    /// so the panel's season slider changed nothing out here — and worse, the
    /// island's regenerate path ran instead and spawned the whole fixed island on
    /// top of the streamed world.
    #[test]
    fn season_changes_streamed_foliage() {
        let summer = build_chunk(0, 0, 7, 0.0, &ChunkSource::Streamed);
        let autumn = build_chunk(0, 0, 7, 1.0, &ChunkSource::Streamed);

        let colors = |chunk: &BuiltChunk| {
            chunk.meshes.canopy.as_ref().and_then(|mesh| {
                mesh.attribute(crate::voxel_material::ATTRIBUTE_VOXEL_COLOR)
                    .cloned()
            })
        };
        let summer_colors = colors(&summer).expect("summer canopy");
        let autumn_colors = colors(&autumn).expect("autumn canopy");
        assert_ne!(
            format!("{summer_colors:?}"),
            format!("{autumn_colors:?}"),
            "deep autumn should recolour the streamed canopy"
        );
    }

    #[test]
    fn reload_updates_seed_and_season() {
        let mut streamer = ChunkStreamer::new(7, 0.0, ChunkSource::Streamed);
        streamer.request_reload(9, 0.75);
        assert_eq!(streamer.seed, 9);
        assert_eq!(streamer.season, 0.75);
        assert!(streamer.reload_requested);
    }
}
