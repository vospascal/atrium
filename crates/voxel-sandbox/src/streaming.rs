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

use voxel_core::streamed_source::StreamedSource;
use voxel_core::voxel_source::VoxelSource;
use voxel_core::world::{
    terrain_column_height, ChunkScratch, Voxel, VOXEL_SIZE, WATER_LEVEL, WORLD_SIZE_Y,
};

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
    /// Instanced-grass clump sites: `(world_x, top_y, world_z, tone_variant)`.
    /// The mesher deliberately skips `TallGrass` (the island spawns it as swaying
    /// instanced clumps), so streamed chunks must place their own or the terrain
    /// reads bare. Harvested off-thread from the same voxel window.
    grass_clumps: Vec<(i32, i32, i32, usize)>,
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
    canopy_mesh: Option<Handle<Mesh>>,
    canopy: Option<Entity>,
    /// Instanced-grass clump sites, and their live entities when near.
    grass_clumps: Vec<(i32, i32, i32, usize)>,
    grass: Vec<Entity>,
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
    /// Shared instanced-grass assets (tone palette + wind material), created
    /// once. Sharing one mesh+material handle per tone is what lets bevy batch
    /// every clump of a tone into a single instanced draw.
    grass_assets: Option<(
        Vec<Handle<Mesh>>,
        Handle<crate::voxel_material::GrassMaterial>,
    )>,
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
    /// size", also a hard cap on chunk count. From `VOXEL_STREAM_BOUNDS`.
    bounds: Option<i32>,
}

impl ChunkStreamer {
    pub fn new(seed: u32, season: f32) -> Self {
        let bounds = std::env::var("VOXEL_STREAM_BOUNDS")
            .ok()
            .and_then(|value| value.parse().ok());
        Self {
            loaded: HashMap::new(),
            pending: HashMap::new(),
            water_material: None,
            grass_assets: None,
            terrain_material: None,
            seed,
            season,
            reload_requested: false,
            bounds,
        }
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

/// Marks a streamed chunk entity (so it can be despawned/queried in bulk).
#[derive(Component)]
pub struct StreamedChunk;

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
fn build_water_surface_mesh(chunk_x: i32, chunk_z: i32, seed: u32) -> Option<Mesh> {
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
            let is_wet =
                |offset: i32| terrain_column_height(origin_x + offset, world_z, seed) < WATER_LEVEL;
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
            let x_min = (origin_x + run_start) as f32 * VOXEL_SIZE;
            let x_max = (origin_x + local_x) as f32 * VOXEL_SIZE;
            let z_min = world_z as f32 * VOXEL_SIZE;
            let z_max = (world_z + 1) as f32 * VOXEL_SIZE;

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
fn build_chunk(chunk_x: i32, chunk_z: i32, seed: u32, season: f32) -> BuiltChunk {
    let source = StreamedSource::new(seed);
    let scratch = mesh::unpack_chunk_window(&source, chunk_x, chunk_z);
    let grass_clumps = harvest_grass_clumps(&source, &scratch, chunk_x, chunk_z);
    let meshes = mesh::build_chunk_meshes(
        &source,
        &scratch,
        seed,
        season,
        chunk_x,
        chunk_z,
        // Baked AO, so every streamed chunk can share ONE material — see
        // `mesh::AmbientOcclusion` for why that matters so much here.
        mesh::AmbientOcclusion::Baked,
    );
    let water_surface = build_water_surface_mesh(chunk_x, chunk_z, seed);
    BuiltChunk {
        meshes,
        water_surface,
        grass_clumps,
    }
}

/// Find this chunk's instanced-grass clump sites: the topmost [`Voxel::TallGrass`]
/// cell of every clump column (see [`grass::is_clump_column`]), tagged with its
/// biome tone variant. Reads the already-generated voxel window, so it costs a
/// scan rather than another generation pass.
fn harvest_grass_clumps(
    source: &StreamedSource,
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
    camera: Query<&GlobalTransform, With<bevy::core_pipeline::prepass::DepthPrepass>>,
) {
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
    let load_radius = quality.stream_radius as i32;
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

    // The one shared terrain material. Its occupancy binding is a single dummy
    // element: streamed chunks bake AO into vertex colours, so the shader's
    // occupancy path is never taken (the vertex-alpha sentinel switches it off).
    if streamer.terrain_material.is_none() {
        let occupancy = storage_buffers.add(ShaderStorageBuffer::from([0u32].as_slice()));
        streamer.terrain_material = Some(terrain_materials.add(VoxelTerrainMaterial {
            base: StandardMaterial {
                base_color: Color::WHITE,
                perceptual_roughness: 0.95,
                ..default()
            },
            extension: voxel_material::streamed_voxel_extension(seed, occupancy, 0, 0, 1, 1),
        }));
    }
    let terrain_material = streamer.terrain_material.clone().unwrap();

    // Shared instanced-grass palette + wind material, created on first run.
    if streamer.grass_assets.is_none() {
        streamer.grass_assets = Some((
            grass::build_clump_meshes(&mut meshes),
            grass::build_grass_material(&mut grass_materials),
        ));
    }
    let (clump_meshes, grass_material) = streamer.grass_assets.clone().unwrap();

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
    let far = |cx: i32, cz: i32| {
        (cx - center_x).abs() > unload_radius || (cz - center_z).abs() > unload_radius
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
    for (key, built) in completed {
        streamer.pending.remove(&key);

        let BuiltChunk {
            meshes: chunk_meshes,
            water_surface,
            grass_clumps,
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
            entities.push(
                commands
                    .spawn((
                        Mesh3d(meshes.add(chunk_mesh)),
                        MeshMaterial3d(terrain_material.clone()),
                        crate::water::reflective_layers(),
                        StreamedChunk,
                    ))
                    .id(),
            );
        }
        for main_view_mesh in [chunk_meshes.meadow_cover, chunk_meshes.terrain_below_water]
            .into_iter()
            .flatten()
        {
            entities.push(
                commands
                    .spawn((
                        Mesh3d(meshes.add(main_view_mesh)),
                        MeshMaterial3d(terrain_material.clone()),
                        NotShadowCaster,
                        StreamedChunk,
                    ))
                    .id(),
            );
        }
        // Leaf confetti is near-detail: the mesh is kept, and the detail pass
        // below spawns it only while the camera is close (the solid inner canopy
        // above stays resident, so distant trees keep their shape and shadow).
        let canopy_mesh = chunk_meshes.canopy.map(|mesh| meshes.add(mesh));
        if let Some(chunk_mesh) = chunk_meshes.canopy_solid {
            entities.push(
                commands
                    .spawn((
                        Mesh3d(meshes.add(chunk_mesh)),
                        MeshMaterial3d(terrain_material.clone()),
                        crate::water::reflective_layers(),
                        StreamedChunk,
                    ))
                    .id(),
            );
        }
        // The mesher's per-chunk water faces (`chunk_meshes.water`) are voxel
        // geometry with no UVs, which the water shader needs; the flat surface
        // built alongside it covers the same columns as a single clean layer.
        if let Some(surface_mesh) = water_surface {
            entities.push(
                commands
                    .spawn((
                        Mesh3d(meshes.add(surface_mesh)),
                        MeshMaterial3d(water_material.clone()),
                        NotShadowCaster,
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
                grass_clumps,
                grass: Vec::new(),
            },
        );
    }

    // Detail tiers: spawn confetti + instanced grass only for chunks near the
    // camera, and drop them again as it moves away. This is the streaming-mode
    // perf lever that matters — the cost here is per-entity, not per-vertex.
    let grass_radius = quality.grass_radius as i32;
    let confetti_radius = quality.confetti_radius as i32;
    for (&(chunk_x, chunk_z), chunk) in streamer.loaded.iter_mut() {
        let distance = (chunk_x - center_x).abs().max((chunk_z - center_z).abs());

        let wants_canopy = distance <= confetti_radius && chunk.canopy_mesh.is_some();
        match (wants_canopy, chunk.canopy) {
            (true, None) => {
                let mesh = chunk.canopy_mesh.clone().unwrap();
                chunk.canopy = Some(
                    commands
                        .spawn((
                            Mesh3d(mesh),
                            MeshMaterial3d(terrain_material.clone()),
                            NotShadowCaster,
                            CanopyConfetti,
                            crate::water::reflective_layers(),
                            StreamedChunk,
                        ))
                        .id(),
                );
            }
            (false, Some(entity)) => {
                commands.entity(entity).despawn();
                chunk.canopy = None;
            }
            _ => {}
        }

        let wants_grass = distance <= grass_radius && !chunk.grass_clumps.is_empty();
        if wants_grass && chunk.grass.is_empty() {
            // Sharing one mesh+material handle per tone is what lets bevy batch
            // every clump of a tone into a single instanced draw.
            for &(world_x, top_y, world_z, variant) in &chunk.grass_clumps {
                chunk.grass.push(
                    commands
                        .spawn((
                            Mesh3d(clump_meshes[variant].clone()),
                            MeshMaterial3d(grass_material.clone()),
                            grass::clump_transform(world_x, top_y, world_z, 0.0, 0.0, seed),
                            NotShadowCaster,
                            grass::GrassClump,
                            StreamedChunk,
                        ))
                        .id(),
                );
            }
        } else if !wants_grass && !chunk.grass.is_empty() {
            for entity in chunk.grass.drain(..) {
                commands.entity(entity).despawn();
            }
        }
    }

    // Queue the nearest missing chunks (within bounds) for async generation.
    let mut wanted: Vec<(i32, i32, i32)> = Vec::new();
    for cz in (center_z - load_radius)..=(center_z + load_radius) {
        for cx in (center_x - load_radius)..=(center_x + load_radius) {
            if let Some(bound) = bounds {
                if cx < -bound || cx > bound || cz < -bound || cz > bound {
                    continue;
                }
            }
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
        let task = pool.spawn(async move { build_chunk(cx, cz, seed, season) });
        streamer.pending.insert((cx, cz), task);
    }
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
        let built = build_chunk(0, 0, 7, 0.2);
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
            let height = terrain_column_height(origin_x + local_x, origin_z + local_z, seed);
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
        let summer = build_chunk(0, 0, 7, 0.0);
        let autumn = build_chunk(0, 0, 7, 1.0);

        let colors = |chunk: &BuiltChunk| {
            chunk
                .meshes
                .canopy
                .as_ref()
                .and_then(|mesh| mesh.attribute(Mesh::ATTRIBUTE_COLOR).cloned())
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
        let mut streamer = ChunkStreamer::new(7, 0.0);
        streamer.request_reload(9, 0.75);
        assert_eq!(streamer.seed, 9);
        assert_eq!(streamer.season, 0.75);
        assert!(streamer.reload_requested);
    }
}
