# Streaming / Big Worlds (Stage 9)

Convert the engine from **one fixed 125 m island** to a **chunk-based world
streamed around a moving camera** — generated on demand, meshed near the camera,
unloaded (LRU) far from it. Committed "go big" (user, 2026-07-25).

## Why this is a rewrite, not a feature
The current `VoxelWorld::generate` builds the *whole* 1000×1000×256 island at
once (RLE grid), and the orbit camera views it whole. Streaming only matters
when you *can't* see everything — so the world must become **unbounded** and
generated **per chunk on demand**. The terrain noise is already position-based
(fBm hills + river + lake); the ONLY thing making it a finite island is the
radial `land_radius` cutoff in `compute_heightmap`. Drop that → infinite terrain.

## Staged plan (each gated)

- [ ] **S1 — On-demand column height (`voxel-core`)** *(this slice)*
  Extract `terrain_column_height(world_x, world_z, seed, season) -> i32` from
  `compute_heightmap` WITHOUT the radial falloff: infinite fBm hills + river +
  lake, deterministic per (x,z). **Gate: tests** — determinism, tiles across
  chunk borders (a column's height is identical regardless of which chunk asks),
  in valid range. Keeps the existing island path working (it can call this then
  apply its radial mask).
- [ ] **S2 — On-demand chunk voxel gen** — `generate_chunk(chunk_x, chunk_z,
  seed, season) -> ChunkVoxels`: fill a chunk's columns from the height fn +
  surface materials (grass/dirt/sand/stone/water), self-contained, no full-grid
  dependency. Reuse the existing surface-material logic per column.
- [ ] **S3 — Streaming manager (`voxel-sandbox`)** — a `ChunkStreamer` resource:
  track the camera's chunk coord; each frame spawn missing chunks within a load
  radius (gen + mesh + spawn entity) and despawn chunks beyond an unload radius.
  Reuse `mesh::build_chunk_meshes` per chunk. Start synchronous; then async.
- [ ] **S4 — Free-fly camera** — the orbit camera can't explore infinity; add a
  fly camera (WASD + mouse-look, no collision) that moves through the world and
  drives streaming. Gate behind a mode so the diorama view still exists.
- [ ] **S5 — Async meshing + LRU + pooling** — mesh chunks on the task pool
  (no hitches), LRU cache of generated chunk data, mesh-buffer pooling
  (research-doc §4) to cut allocation churn.

- [x] **S6 — One renderer for both worlds** (`VoxelSource`) — S3's streamer had
  its own standalone mesher and a plain vertex-colour material, so the streamed
  world looked nothing like the island (no biome colours, jitter, AO, trees,
  grass, or water shading). Fixed by abstraction rather than a second renderer:
  - `voxel_core::voxel_source::VoxelSource` — what the mesher actually needs from
    a world: `unpack_chunk` (a dense voxel window) + the four per-column colour
    contexts (`dryness_at`, `cover_at`, `water_distance_at`, `tree_tone_at`) +
    `world_offset` (island centres on its footprint, infinite world does not).
    Implemented by both `VoxelWorld` and the new `StreamedSource`.
  - `mesh::build_chunk_meshes` now takes `&impl VoxelSource` plus a caller-owned
    `ChunkScratch` (`mesh::unpack_chunk_window`), so a caller that needs the same
    voxels twice — the streamer packs its shader occupancy from them — generates
    the window once. The island renders **pixel-identically** through this path.
  - `voxel_core::streamed_source::StreamedSource` — infinite terrain + trees +
    bushes + ground cover + water, every column a pure function of `(x, z, seed)`.
    Trees use jittered grid cells and grow in canonical order over a window's
    apron, so a canopy whose trunk sits in another chunk stamps identically —
    that is what makes borders seamless without the neighbour chunk resident.
  - Terrain shader AO generalised: the island binds one global occupancy buffer
    (world-indexed); a streamed chunk binds a per-chunk buffer plus a
    `chunk_origin` uniform (binding 106) that localises world coordinates. The
    island passes `(0,0)`, so its path is unchanged.
  - Streamed chunks mirror the island's spawn recipe: per-chunk
    `VoxelTerrainMaterial` (live-lit for free — `update_terrain_lighting` iterates
    every material asset), reflection layers on above-water terrain + canopy,
    `NotShadowCaster` on cover/underwater, `CanopyConfetti` on the leaves, and a
    flat water surface rendered with the island's `WaterMaterial` (its vertex
    shader *sets* Y from the `heights` buffer, so one flat entry replaces the
    fluid sim). Tagged `StreamedChunk`, deliberately **not** `WorldMesh` — the
    regenerate (R) system despawns `WorldMesh`, which would strand the streamer.

- [x] **S7 — Streaming is ENTITY-bound, not geometry-bound** (36 → 112 fps).
  First-person at a large view distance was unplayable. Measured, rather than
  guessed, with two probes that remove different things:
  - dropping ~6k tiny grass **entities** (≈0 vertices): **+32%**
  - dropping 2.3M **vertices** of leaf confetti (~170 entities): **+15%**

  So frame cost tracks entity + draw count, not triangles. Two fixes:
  1. **Distance detail tiers.** Grass and leaf confetti spawn only within their
     own radius of the camera and despawn again as it moves; the terrain
     silhouette (and the solid inner canopy, so distant trees keep their shape
     and shadow) stays resident out to the full view distance. Live levers:
     `RenderQuality::{grass_radius, confetti_radius}` (defaults 3 / 4), sliders
     in the P overlay. → 45 fps.
  2. **One shared terrain material** (the big one: ~2.5×). Per-fragment shader AO
     needs a per-chunk occupancy buffer, which forces a material per resident
     chunk and stops bevy batching chunk meshes. Streamed chunks now use
     `mesh::AmbientOcclusion::Baked`: AO is sampled at each merged quad's corners
     and multiplied into vertex colours, and vertex alpha carries the shader's
     **existing** "AO already baked" sentinel (the one cover geometry uses) — so
     no shader change, and one material serves every chunk. → 112 fps.
     Trade-off, stated honestly: AO interpolates across a merged quad instead of
     being exact per fragment, so an occluder in the middle of a large flat face
     is missed. Uniform cases (a wall base, a step) still read correctly because
     every face along them samples the same occlusion. The island keeps
     `PerFragment` and is verified byte-identical.

- [x] **S8 — Confetti shell, not volume.** The leaf confetti emitted all six
  faces of every leaf cube, including cubes buried deep inside a canopy — where
  the solid inner canopy mesh already fills the volume behind them. Skipping
  blocks whose six neighbours are all leaves cut canopy vertices **41%** on the
  island (1,773,144 → 1,052,808) and **26%** per streamed chunk, with **no
  visible change** (verified by screenshot; every other mesh byte-identical).
  Only *fully enclosed* blocks are dropped — the cubes are shrunken with gaps you
  can see through, so anything on the canopy surface keeps all six faces.
  Notably this did **not** move fps measurably, which is more evidence that
  neither world is vertex-bound.

- [x] **S9 — Season/regenerate in the streamed world** (bug, found by the user
  switching to autumn). `regenerate_system` called `spawn_world` unconditionally,
  so in streaming mode changing the season — or pressing **R** — built the entire
  fixed island and dropped it at the origin on top of the streamed terrain. And
  separately, streamed chunks ignored the season entirely: it was a hardcoded
  `STREAM_SEASON = 0.2` constant, so autumn could never have reached them.
  Now: the streamer owns `seed` + `season`, `ChunkStreamer::request_reload`
  drops every resident chunk (and in-flight tasks, which carry the old values) so
  they stream back in recoloured, and `regenerate_system` hands off to it and
  returns instead of respawning an island. Tests: season must change the streamed
  canopy's vertex colours, and reload must carry the new seed/season.

## Notes
- Determinism is the anchor: a column at world (x,z) must generate identically
  no matter which chunk/frame requests it, so chunk seams are invisible.
- **Measure which resource is actually scarce before optimising.** The instinct
  here was "too many triangles"; the truth was "too many entities and draw
  calls", and the two fixes that mattered (detail tiers, one shared material)
  both *added* vertices while removing entities/materials.
- **Shader/GPU rewrites of the trees would optimise the wrong resource.** Leaves
  are already batched into two meshes per chunk, so they cost vertices, not
  entities. Instancing them as entities would make things worse; instancing them
  without entities, or billboard impostors, means a custom render pipeline for a
  resource that is not scarce. The wins that fit the real bottleneck were the
  confetti distance tier and dropping invisible interior cubes.
- **Streaming's cost profile is the opposite of the island's.** The island
  precomputes whole-grid biome maps once, so `water_distance_at` & friends are
  O(1) array reads; `StreamedSource` recomputes them, and the mesher calls them
  roughly per face. Naively that made a chunk take ~44 s (a radius-24 water
  search, ~2400 noise evals, run tens of thousands of times). Per-column
  memoisation + an O(cells) distance transform are what make streaming viable —
  the generator, not the mesher, is the hot path.
- Keep the hand-crafted island as an optional world type (radial mask on top of
  the infinite base) — streaming is a second world mode, not a deletion.
- Chunk size: reuse `mesh::CHUNK_SIZE` (64) column chunks to match the mesher.
