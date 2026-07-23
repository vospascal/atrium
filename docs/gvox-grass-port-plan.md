# GPU-Particle Grass Port (gvox_engine → Bevy)

Faithful port of GabeRundlett's `gvox_engine` grass to our Bevy voxel-sandbox.
Reference source (commit `da8e492`):

- `src/voxels/particles/grass/grass.inl` — data structures
- `src/voxels/particles/grass/grass.glsl` — blade shape + wind
- `src/voxels/particles/cube.raster.glsl` — procedural cube rasterization

## What gvox actually does

- **A blade = a vertical stack of tiny voxel cubes.** `GrassStrand { origin,
  packed_voxel, flags }`; segment `i` sits at height `i * VOXEL_SIZE`. Reads as
  a thin 3D voxel column, not a flat billboard.
- **Wind sway** = 4-octave fractal *value-noise* sampled at `origin + time`
  → rotation `rot = noise * 43` → `vec2(sin, cos)`; each segment is pushed
  sideways **proportional to its height** (`offset = rot_offset * z * 0.66`),
  so blades bend more toward the tip. Also computed for the previous frame to
  feed motion-vectors.
- **Tip gradient**: `voxel.color *= i + 1` brightens color up the blade.
- **Procedural cube geometry**: cube corners generated from `gl_VertexIndex`
  bit-patterns (`0x1C, 0x46, 0x070`) — no mesh uploaded; only camera-facing
  faces emitted (per-axis extension chosen from camera-vs-center).
- **Architecture**: up to `1<<22` (4.2M) strands in a GPU compute *particle
  allocator*; a compute sim pass populates instance/vertex buffers + indirect
  draw args; instanced cube raster into a **deferred G-buffer** (packed voxel,
  normal, velocity, depth). Shadow pass discards color.

## Interpretation for Bevy (the honest target)

**Take the ideas, adapt to Bevy/WGSL — NOT a 1-to-1 port.** The `.glsl`/`.inl`
files are daxa/Vulkan-specific (task-graph, bindless, deferred G-buffer, a GPU
compute particle allocator, indirect draw). None of that is needed for the
*look*, and cloning it would be a renderer rewrite. We lift the visual concepts
and implement them in the pipeline we **already have**: instanced clump entities
(Bevy auto-instancing) drawn with our `voxel_terrain.wgsl` material.

Target:

> **Voxel-cube blades** (vertical cube stacks) that **sway with noise wind**
> (bending with height) and have a **bright tip gradient** — via our existing
> instanced-clump + shader path. No new render-engine machinery.

### The standing risk this plan exists to manage

The machine is **fill-bound** — 34k quad grass entities already doubled
frametime. Cube blades trade big double-sided overdrawing quads for many small
solid single-sided cubes (more triangles, but potentially *less* fill/overdraw).
Net perf is unknown until measured, so **every stage gate is a real run that
checks the `P`-overlay frametime.** Clump density (stride) stays our throttle.

## Stages (each ends with a real `cargo run` visual + frametime check)

- [ ] **G1 — Voxel-cube blade mesh.** Rebuild `build_clump_mesh` so a blade is
  a thin vertical **stack of small cubes** (single-sided, cull back) instead of
  fanned flat quads. Reuse the existing instanced-clump spawn + `VoxelTerrainMaterial`
  (switch it back to `cull_mode: Back`). **Gate:** reads as voxel-column grass;
  frametime vs current quad grass acceptable at some stride.

- [ ] **G2 — Noise wind-sway (vertex shader).** In `voxel_terrain.wgsl`, add a
  `time` uniform and displace grass vertices sideways by value-noise of world
  position + time, **scaled by height up the blade** (their `rot_offset * z *
  0.66`). Gate the displacement to grass only (reuse the AO-skip alpha
  sentinel). **Gate:** living, swaying meadow; no perf regression.

- [ ] **G3 — Tip gradient + biome tone.** Bake **dark root → bright tip** into
  the blade cube colors (`color *= i+1` analog), keeping the green→straw biome
  variance we already pick per clump. **Gate:** coloring matches the reference.

## Not needed (explicitly out of scope — "no 1-to-1 port")

GPU compute particle allocator, indirect draw, procedural-cube-from-
`vertex_index`, deferred G-buffer, motion-vectors, millions of blades. These are
daxa *architecture*, not *look*. If we ever want far more density than clump-
instancing sustains, revisit — but not now.

## Notes / decisions

- Blade = **per-segment cubes** so wind can bend per height (gvox's model).
- Keep the current quad grass until G1 proves out, then replace it directly
  (no compat shim).
- Supersedes the open "keep instanced quads vs revert to baked" question:
  voxel-cube blades are the new grass, so that question is moot.
