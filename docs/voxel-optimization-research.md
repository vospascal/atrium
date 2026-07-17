# Voxel Engine Optimization — Research & Plan

Analysis of the techniques from the "25× faster voxel engine" thread (binary
greedy meshing, RLE chunks, bit-array meshing, fixed-block storage) mapped to
**our** engine's actual bottlenecks. Written 2026-07-17, after the planar-
reflection perf pass.

## Where we are today (measured)

| Metric | Value |
|---|---|
| World | 1000 × 256 × 1000 = **256 M cells**, flat `Vec<Voxel>` = **256 MB RAM** |
| Generation | 180 ms (noise + decorate + trees) — *not* a bottleneck |
| Meshing | 1.5 s single-threaded, full-world remesh on any change |
| Meshes | 4 giant entities: terrain-above 3.9 M verts, meadow 1.1 M, underwater 4.1 M, water 0.76 M ≈ **10 M verts** |
| GPU buffers | ~400 MB vertex + ~60 MB index |
| Frame cost | main pass + reflection pass + 4 shadow cascades × 2 views |
| FPS | ~66 first-person / ~71 orbit (M3 Max, debug + opt-level 2) |

What we already do right: culled-face meshing (only air-facing faces are
emitted — this IS "don't render the backside of things", the interior is never
meshed), per-face mesh buckets (above-water / meadow / underwater), shadow
diet (`NotShadowCaster` on meadow + underwater), half-res no-MSAA reflection.

The structural problem: **everything is one mesh**. No frustum culling
(the GPU transforms all ~10 M vertices for every view, every frame), no
partial remesh (any voxel change = 1.5 s full rebuild), no parallelism.

## Technique-by-technique assessment

### 1. Chunking (the prerequisite for everything)
Split the render mesh into chunks (e.g. 32³ voxels = 4 m³). Bevy then gives
per-chunk AABB **frustum culling for free** — main view, reflection view, and
every shadow cascade all stop paying for what they can't see. First-person
gains the most (most chunks are behind you). Enables `rayon`-parallel meshing
(~10 cores → meshing well under 200 ms) and **localized remesh** — the
prerequisite for in-app terrain editing and the fluid-dynamics water we want.
The voxel data itself can stay one flat array at first; chunking only the
*mesh* output is the low-risk first step.
**Verdict: do first. Biggest architectural unlock, no visual change.**

### 2. Binary greedy meshing (bit-array based)
Pack each chunk's occupancy into `u32`/`u64` bit-columns; find merge runs with
bit ops; emit one big quad per same-appearance rectangle. Typical 5–15×
vertex reduction on voxel terrain — our 10 M verts likely become <1.5 M.
This is the single biggest GPU win available.

**Our specific blocker (documented before): per-voxel appearance.** Our look
is per-vertex `palette × biome gradient × hash jitter × baked corner AO` —
every voxel is unique, so naive greedy can't merge anything. Two-step answer:

- **Step A — merge-safe greedy (no look change):** merge only faces whose
  merge key is identical: same voxel-type bucket, same tree-tone bucket, and
  *fully-open AO* (all corners level 3). Flat meadow tops, sand flats, and
  water surface — exactly where the millions of vertices are — are AO-uniform
  and merge perfectly; edges/corners stay unmerged and keep their baked AO.
  Low-frequency gradients (dryness/lushness) interpolate linearly across a
  ≤4 m quad with no visible error.
- **Step B — shader-driven voxel material (bigger, later):** custom terrain
  material computes jitter (`hash(floor(world_pos))`), biome tint, and corner
  AO in the fragment shader (AO from a per-chunk occupancy 3D texture, R8,
  32 KB/chunk). Then *everything* merges regardless of AO, and season/biome
  changes become a uniform update instead of a 1.5 s remesh. This is also how
  we'd get a **live season slider** with zero rebuild.

**Verdict: Step A soon after chunking; Step B is its own arc.**

### 3. RLE voxel storage (32×256×32 column chunks)
Our world is heightmap-like: air above, a few material bands, stone core —
per-column RLE would take 256 MB → **~5–15 MB**, fits caches, loads/saves
instantly (relevant for scene save/load later). The thread's key trick
applies to us directly: never random-access the RLE — unpack a chunk (+1 cell
apron for AO/culling neighbors) into a dense bit-array/scratch buffer, mesh
from that, throw it away. Our mesher already walks in exactly the order that
makes this cache-friendly.
The con (slow random writes) hits terrain *editing*; mitigation is the
standard one: edit chunks keep a small dense overlay until re-compacted.
**Verdict: worth it when we either (a) want bigger/streamed worlds, or
(b) build scene save/load. Not urgent for the fixed 125 m island.**

### 4. Fixed memory block / circular buffer (the BlockManager post)
Solves allocation churn + wraparound indexing for *streaming* worlds with a
moving view window. Our island is fixed and fits in one allocation already —
we get the same benefit from our single flat `Vec` + precomputed stride
offsets (which `fill_column`/mesher effectively use). The transferable idea:
**pool chunk meshes** (reuse vertex buffers on remesh instead of reallocating)
once chunked editing exists.
**Verdict: adopt the pooling idea inside the chunk system; skip the circular
buffer unless we ever stream beyond one island.**

### 5. Noise up-sampling / caching
Their generation was noise-bound; ours is 180 ms total (fBm already coarse,
maps precomputed per column). **Verdict: skip.**

### 6. LOD ("less detail far away") + occlusion culling
After chunking + greedy, the orbit view still sees every chunk (diorama!), so
LOD is the lever *if orbit needs more*: distant chunks meshed at 2× voxel
size (¼ the faces) — visually safe at tilt-shift distances. Occlusion culling
helps first-person in valleys; bevy has no built-in occlusion — skip until
proven needed.
**Verdict: optional Phase 4; only if numbers demand it.**

### Bonus quick win (independent): slimmer vertices
Positions `Float32x3` + normals `Float32x3` + colors `Float32x4` = 40 B/vert.
Colors as `Unorm8x4` (16→4 B) and normals as a 1-byte face-index (6 axis
normals, expanded in a tiny shader or via `Snorm8x4`) ≈ 40 B → ~20 B: halves
GPU bandwidth and the 400 MB buffer without touching topology.
**Verdict: cheap, do alongside chunking.**

## Mixed voxel density (art direction — user's autumn reference)

Separate idea from performance LOD: use voxel *size* expressively, like the
autumn-lake reference — chunky blocks for cliffs/rock, confetti-fine voxels
for tree canopies, thin stalks for grass. Maps to us as:

- **Chunky cliffs (trivial):** quantize the heightmap in rock zones to 2–4
  voxel steps, so cliffs terrace in big blocks. Generator-side only.
- **Thin grass blades:** shrink TallGrass footprint (x/z scale ~0.5) the way
  height is already scaled — cover never self-culls, so no holes. Cheap.
- **Confetti canopies:** render *surface* leaf voxels as shrunken cubes with
  random offsets (interior stays solid and culled, like a MagicaVoxel
  render). Needs a canopy mesh group that emits all faces for surface cells —
  face count rises, so best landed together with (or after) greedy meshing.
- Alternative for trees: grow them as separate half-voxel-size prop meshes
  (`VoxModel::from_cells` pipeline) — finer detail AND movable/placeable
  like props; aligns with the scene-maker direction.

## Recommended phases

1. **Chunked meshing** — 32³ render chunks, rayon-parallel, mesh pooling,
   slim vertex formats. Frustum culling everywhere; localized remesh; no
   visual change. *(Unlocks: editing, fluid sim, everything below.)*
2. **Merge-safe binary greedy** — bit-array mesher per chunk, merge on
   (type, tone-bucket, AO-open) keys. Expect >5× vertex cut, look preserved.
3. **Shader voxel material** — jitter/tint/AO in fragment shader from chunk
   occupancy textures. Full greedy everywhere + live season/biome sliders.
4. **RLE storage** — with scene save/load; or earlier if RAM matters.
5. **LOD** — only if orbit-view numbers still hurt.

Rough expectation after 1+2: vertex load per pass drops ~10×; the reflection
and shadow passes become nearly free; FP fps should be comfortably >120 even
in debug builds, and full-world remesh (season change, R) well under 300 ms.
