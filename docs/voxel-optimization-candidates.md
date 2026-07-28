# Voxel engine — optimization candidates

Running list distilled from external sources (YouTube deep-dives on voxel
rendering), checked against what `crates/voxel-sandbox` + `crates/voxel-core`
actually do today. Nothing here is committed work — it is a menu.

Sources so far:
1. "4 simple optimizations" — mesh/vertex compression, instancing, multi-draw indirect (`40JzyaOYJeY`)
2. Zepha engine part 1 — face culling, greedy meshing, LOD, quad overdraw (`2v8r0oQd9Xc`)
3. Voxel Lords — octree LOD, generate-don't-load, sky/fog shaders (`WTp8qAegARs`)
4. Biome-first vs terrain-first worldgen (`oztbyI-1NII`) — architecture, not perf
5. Starboard grass & trees (`GOfttJQ-FGw`) — foliage looks, mostly art-side
6. Zepha part 2 — instancing vs indirect rendering, GPU buffer suballocation (`YS_t3FtnQXw`)
7. Sparse bindless texture arrays + GPU texture compression (`YTfdBSjitd8`) — **N/A for us**, see below

---

## Already done ✅

| Technique | Where |
|---|---|
| Chunked meshes, not per-voxel | 64×256×64 column chunks, `mesh.rs:123` |
| Hidden/interior face culling | neighbour checks, `mesh.rs:320` |
| **2D** greedy merging (both axes) | `greedy_merge_terrain`, `mesh.rs:519` — past what Zepha settled for (they do 1D positive-growth only) |
| Mesh groups so non-mergeable geometry doesn't poison the chunk | `Terrain` / `Cover` / `Canopy`, `mesh.rs:38-66` |
| Distance detail tiers (drop grass + canopy confetti far out) | `streaming.rs:560` |
| One shared material → bevy batches | Stage 9 S6 |
| Tight AABBs from real vertices for frustum culling | `mesh.rs:123-127` |
| World is a **pure function of position** (no disk, no region files) | `StreamedSource`, `streaming.rs:14` — this is the prerequisite Voxel Lords had to fight for, and we already have it |
| Horizon/sun-direction sky lerp + depth fog | `sky.rs` (`horizon_color`, `fog`), `fog_ring.rs` |
| Confetti canopy is `NotShadowCaster`; a cheap solid inner shell casts all tree shadows | `emit_canopy_solid`, `mesh.rs:877` + `streaming.rs:577` — exactly source 5's architecture, already built |
| Grass batched per chunk, spawned/dropped per chunk | `streaming.rs:592` |
| Per-clump random rotation / offset jitter | canopy `OFFSET`, `mesh.rs:780`; grass variants |
| Wind vertex bend on grass | `GrassExtension`, `voxel_material.rs:38` |

---

## Baseline (2026-07-28)

Reproduce headlessly — no window needed, so geometry changes are measurable
from the terminal:

```
cargo test -p voxel-sandbox --release island_geometry_baseline -- --nocapture
```

Whole island, seed 1, 289 chunks:

| group | meshes | vertices | triangles | MB |
|---|---:|---:|---:|---:|
| terrain | 121 | 1 382 676 | 691 338 | 63.60 |
| underwater | 121 | 1 337 928 | 668 964 | 61.54 |
| cover | 112 | 134 012 | 67 006 | 6.16 |
| canopy | 111 | 1 052 016 | 526 008 | 48.39 |
| canopy solid | 111 | 1 179 288 | 589 644 | 54.25 |
| water | 80 | 778 236 | 389 118 | 35.80 |
| **TOTAL** | **656** | **5 864 156** | **2 932 078** | **269.75** |

Three things this immediately shows:

1. **The "cheap" canopy shadow proxy is not cheap.** `emit_canopy_solid` is
   documented as the cheap caster behind the confetti shell, but at 590k
   triangles it is *larger* than the 526k-triangle shell it backs. Its comment
   claims ~0.3M. Worth attacking before anything else in the canopy.
2. **Half the terrain budget is underwater** (56–62 MB), rendered main-view
   only and mostly hidden beneath the surface.
3. **Water is 36 MB for a flat plane** — the `WATER_QUAD_MAX_COLUMNS = 4` cap
   that keeps wave displacement resolvable.

### After G + F′

| group | MB before | MB after | Δ |
|---|---:|---:|---:|
| terrain | 63.60 | 61.68 | −1.92 |
| underwater | 61.54 | 56.11 | −5.43 |
| cover | 6.16 | 6.31 | **+0.15** (the hole fix adds faces back) |
| **TOTAL** | **269.75** | **262.54** | **−7.21 (−2.7%)** |

Triangles 2 932 078 → 2 853 746 (−78 332). The terrain and underwater rows move
because waterline plants and lily pads are routed into those buckets, not into
`cover` — so their dropped undersides show up there.

### After greedy-merging the solid canopy shell

| group | MB before | MB after | Δ |
|---|---:|---:|---:|
| canopy solid | 54.25 | 29.08 | **−25.17 (−46%)** |
| **TOTAL** | **262.54** | **237.38** | **−25.16** |

Triangles 589 644 → 316 062 on that group. The shell's cubes are full-size and
grid-aligned, so its faces are coplanar and mergeable — unlike the confetti,
which is shrunk *and* per-voxel jittered so no two of its faces ever line up.

Two things worth keeping in mind for the next merge target:

- **Merging is lossless here and pinned by a test.** A uniform `span³` leaf
  block must merge to exactly 6 quads with the surface area unchanged, which
  catches both a dropped face and a double-covered one.
- **The naive slice sweep cost more CPU than it saved.** Sweeping every slice
  of a chunk six times looking for sparse leaves took island meshing from 6.5 s
  to 11.3 s. Collecting exposed faces in one pass and bucketing them by
  (direction, slice) — then merging only non-empty slices, over the bounding
  box of their faces — brought it back to 7.1 s for identical output. Any
  future merge over sparse voxel types wants the same shape.

Running total: **269.75 → 237.38 MB (−12%)**, 2 932 078 → 2 580 164 triangles
(−12%).

### After batching grass (candidate A)

This one is **not** a geometry win — the triangles are identical. It is an
*entity* win, which is what the streaming work said mattered:

| | before | after |
|---|---:|---:|
| grass entities (island) | **3 778** (one per clump) | **111** (one per chunk) |
| grass triangles | 181 344 | 181 344 |
| grass VRAM | 19.59 MB | 19.59 MB |

The census now includes grass, which it previously omitted entirely — the
streamer builds it, not the chunk mesher, so the group whose cost drove this
work was invisible in the baseline. Full picture with grass counted: **767
meshes, 2 761 508 triangles, 256.96 MB.**

**What batching cost.** A clump's identity used to live in its `Transform`, and
the wind shader read it from there. Merging clumps meant moving two things into
vertex data or the look would break:

- `UV.x` = **wind phase**, previously the transform's translation. Merged,
  every clump in a chunk would share the chunk origin and the whole patch would
  sway in lockstep. There is a test for exactly this.
- `UV.y` = **unscaled blade height**, previously `position.y` in object space.
  Merged, positions are world space, so `position.y` is terrain height plus
  blade height — meaningless as a bend factor. It stays *unscaled* on purpose:
  the old transform scaled the geometry but not the value passed to the wind
  function.

Both `grass.wgsl` and `grass_prepass.wgsl` had to change identically — their
wind functions are diffed byte-for-byte, because if the two passes disagree on
displaced depth the grass z-fights against its own prepass.

---

## Candidates, roughly by value/effort

### A. Instance the grass clumps — *high value, medium effort*
One entity per clump today (`streaming.rs:~595`). This is the exact
"entity-bound, not geometry-bound" trap Stage 9 already hit once. Options:
bake all clumps of a tone in a chunk into one mesh, or a real per-instance
buffer. Video 1's whole instancing section is this idea.

### B. Vertex compression — *high value, high effort*
Today: `pos f32x3 + normal f32x3 + color f32x4` = **40 B/vertex**, 4 verts +
6× u32 indices = **184 B per quad**. Packable to ~8 B/vertex:

- position → chunk-local `u16x3` (or bit-packed into one `u32`)
- normal → 3-bit face index, unpacked in the vertex shader
- baked AO / colour → `Unorm8x4`

**Our specific blocker:** `voxel_material.rs:30-33` documents that a custom
vertex stage must be mirrored by a custom *prepass* vertex stage or the two
passes disagree on depth and z-fight. So this is a two-shader change plus an
attribute-layout change — not a one-liner. Same trap the grass material
already had to solve.

### C. LOD by voxel downscaling — *high value, high effort*
We drop *detail objects* by distance but never coarsen the terrain itself.
Two payoffs:
- fewer triangles at range
- **quad overdraw**: GPUs shade in 2×2 pixel groups, so a sub-pixel face wastes
  up to 75% of its shading work. Distant fine detail is actively expensive.

Implementation for us is unusually cheap on the data side: because
`StreamedSource` is pure, a coarse chunk is just *sampling the generator at
stride N* — no disk reads, no region-file stitching, no read/write races. That
is Voxel Lords' "option B", and it is the only option we need.

Open problem (flagged by both video 2 and video 3): **seams between LOD
levels**. Neither author has solved it cleanly.

### D. Octree / variable-size chunks — *only if C lands*
Constant voxel count per chunk, varying physical size, so chunk count stays
bounded no matter how far we render. Natural companion to C. Big restructure
of `streaming.rs`'s flat `HashMap<(i32,i32), LoadedChunk>`.

Caveat from video 3 — far-away player edits vanish because distant terrain is
regenerated, not loaded. **Moot for us**: streaming mode has no persistent
edits.

### E. Greedy-merge the mergeable faces of `Cover` / `Canopy` — *medium/low effort*
Zepha's point: a block needn't be a cube to benefit — a grass block's *top*
face still merges even when its sides can't. We exclude these groups from
merging wholesale. Worth auditing which of their faces are actually
full-extent.

### F. ~~Doubled foliage quads~~ → **cull cover bottoms against solid** — *verified, low effort*
**Investigated and closed: we are NOT double-emitting.** `add_quad`
(`mesh.rs:79`) pushes 4 verts / 6 indices in one winding, and cover geometry is
*closed shrunken boxes*, not flat crossed quads — so back-face culling is
correct and there is no free 50% here. Video 2's mistake isn't one we made.

The investigation did turn up two real things, and **one change fixes both**:

- **A bug.** Cover culls only against same-group neighbours (`mesh.rs:305`).
  Flowers generated on top of a `TallGrass` cell (`streamed_source.rs:1038`,
  ~40% of flowers) therefore have their bottom face culled against a neighbour
  that is *never meshed* — `TallGrass` becomes an instanced clump only 0.5
  voxel tall. Result: an open hole into the flower's hollow interior.
- **~17% of cover geometry.** A cover voxel's bottom face is anchored at the
  cell base (`mesh.rs:347`), exactly coplanar with the terrain top below it, so
  whenever the voxel below is solid that quad is invisible from every angle.

Fix: cull the bottom face on `is_solid()` below, rather than on group identity.
Closes the hole *and* drops the face.

Two smaller notes from the same pass: a grass clump is 24 quads / 96 verts
(4 boxes × 6 faces), and `clump_transform` anchors at the *topmost* TallGrass
cell, so clumps on 2–3-tall stalks float 0.125–0.25 m above the ground.

### G. Foliage normal cheats — *low effort, pure visual win*
Source 5's two best tricks, both about lying to the lighting:

- **Grass: force normals to point up** instead of using true per-face cube
  normals. Our blade cubes push a real outward normal per face
  (`grass.rs:316`), so the side faces — most of what you see — barely catch the
  sun and read as dark clutter. Biasing the normal toward `+Y` makes a grass
  field read as one lit surface.
- **Canopy: point normals away from the clump centre**, so light never bounces
  around inside the leaves. Our confetti already uses per-cube outward normals,
  which is this trick at *cube* granularity; blending toward "away from the
  tree's trunk axis" would push it further.

Free, no new draw calls, no shader-pipeline surgery. Probably the best
effort-to-payoff item on the whole list.

### H. Grazing-angle fade on foliage — *low effort*
Source 5 fades polygons out as they approach perpendicular-to-camera, killing
the harsh edge-on lines. We have a *minification* fade on grass jitter already
(different problem: sub-pixel shimmer). A grazing-angle term is a separate,
complementary `dot(normal, view)` fade.

### I. Wind-driven colour modulation — *low effort*
We bend grass vertices with wind but don't modulate colour. Source 5 samples a
smooth tiling scroll texture for *both* bend and a subtle colour shift — cheap,
and it sells the motion far more than displacement alone. Our
`voxel_core::wind` gust field already provides the signal.

### J. Mid-tier grass LOD instead of a hard cutoff — *medium effort*
Today grass is binary: full detail inside `grass_radius`, gone outside
(`streaming.rs:592`). Sources 2 and 5 both swap to a cheaper model at range
instead. A single coarse tier would extend the apparent grass horizon for far
less than doubling the radius.

### K. Per-face-direction mesh split (6 meshes/chunk) — *questionable for us*
Skips the vertex shader entirely for face directions pointing away from the
camera. But it multiplies entity count, and we are entity-bound. Only worth it
paired with a real batching/indirect path.

---

## Deliberately not pursuing ⚠️

- **Hand-rolled multi-draw indirect / SSBO chunk positions / `gl_DrawID`.**
  Bevy 0.18 already does GPU preprocessing + indirect batching
  (`bevy_pbr-0.18.1/src/render/gpu_preprocess.rs`,
  `multi_draw_indirect_count`). Reimplementing this means reimplementing the
  render pipeline.
- **Hand-rolled buffer suballocation** (source 6's 64 MB buffers carved into
  4 KB regions with spatial locality). Bevy already slabs meshes into shared
  vertex/index buffers — `MeshAllocator` in
  `bevy_render-0.18.1/src/mesh/allocator.rs`, tunable via
  `MeshAllocatorSettings`. We get the mechanism free.
- **Triangle strips + 4-vertex base-quad instancing.** A raw-GL/Vulkan design.
  The bevy-shaped equivalent is candidate A.
- **Sparse / bindless texture arrays, texture atlasing, GPU texture
  compression (source 7).** Entirely moot: our voxel renderer has **no albedo
  textures at all**. Colour is per-vertex (solid colour per voxel + baked AO),
  and the only `Handle<Image>` in the whole crate is the water reflection
  render target (`water.rs:216`). We have no bind-slot pressure and no texture
  VRAM to compress. The one transferable idea from that source — bit-packing
  vertex attributes instead of storing four floats — is already candidate B.

**The useful reframing from sources 1 and 6:** we don't build the indirect
path, we *stay eligible* for the one bevy already runs — share one material,
avoid per-entity uniforms, and keep entity counts low so batches are fat. Every
per-clump grass entity (candidate A) is a batch bevy can't make as wide. Source
6's own conclusion is that indirect rendering only pays off if meshes that draw
together *live together* in memory; the bevy-shaped version of that concern is
`MeshAllocatorSettings` slab sizing, not a custom allocator.

---

## Architecture note (not an optimization): biome-first worldgen

Source 4 contrasts two philosophies:

- **Terrain-first** (Minecraft): generate shape, then classify which biome
  that shape *is*. Scales to many biomes, transitions are free, but you cannot
  change one biome's shape without editing the classifier.
- **Biome-first** (Cube World): place biome regions first, then let each biome
  run its own terrain shaper. Total per-biome control, high contrast, and
  deterministic live-tweaking — at the cost of needing explicit blending at
  borders, which means evaluating *every* contributing biome's noise per
  column.

**We are firmly terrain-first** — `world.rs:45-46` says outright that biomes
are derived from terrain and never authored (`dryness` etc. classify a column
after the fact). Switching would be a rewrite of `world.rs` + `streamed_source.rs`
worldgen, and would make generation *slower* per column, not faster.

Only worth revisiting if we want visually distinct, high-contrast biome
regions in the voxel world. Filed here because it came up in the same batch,
not because it's a perf lever.
