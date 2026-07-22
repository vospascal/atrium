# Voxel Engine — Technique Catalog (from 19 YouTube devlogs)

Raw technique catalog distilled from 19 voxel-engine YouTube videos, logged
2026-07-22. This is the **source material** — a per-video record of every
technique, with the author's numbers and caveats.

**Companion docs:**
- [`voxel-optimization-research.md`](voxel-optimization-research.md) — applied
  analysis mapping a subset of these techniques to `crates/voxel-sandbox`'s
  *actual measured* bottlenecks (mesh-based engine, written 2026-07-17).
- [`voxel-engine-plan.md`](voxel-engine-plan.md) — forward plan (Bevy vs. own
  engine, Quest 3 VR, spatial-audio integration). *(to be written)*

## Project goals driving this catalog
- Build a voxel engine — either **on Bevy** or a **custom engine** (undecided).
- **Stretch goal: Quest 3 VR** — view the voxel world in headset, ideally with
  the atrium **spatial audio** engine feeding binaural sound. (Ambitious; may
  need PCVR-streaming rather than standalone-on-headset rendering.)

## The two paradigms (important for planning)
The 19 videos split into two rendering families that share almost nothing at
the pipeline level. Pick one before planning:
- **Rasterized mesh** (Minecraft/Zepha family): voxels → triangle meshes →
  GPU rasterization. Videos 3, 4, 5, 9, 14. **This is what `voxel-sandbox` is
  today.** Best fit for Bevy (Bevy is a mesh/PBR renderer) and for VR (stereo
  rasterization is cheap and well-supported).
- **Ray-marched / ray-traced** (Teardown family): cast a ray per pixel through
  a voxel grid, no meshing. Videos 1, 2, 8, 10, 12, 13, 15, 16, 17, 18, 19.
  Enables true GI, destruction, per-voxel detail — but stereo ray tracing at
  90 Hz on a Quest 3 (mobile Adreno GPU) is very hard; realistically PCVR-only.

See the "Cross-video overlap & contrast" section at the bottom for the
consolidated cross-references.

---

## Video 1 — Building the renderer

### Geometry / albedo
- Objects stored as voxel grids (grid of volumetric pixels).
- Triangle-rasterize 6 faces per grid; record triangle hit position, reconstruct position in grid space.
- Per rasterized pixel: **DDA (digital differential analyzer)** to step through grid cells; first non-zero byte → color-map lookup → pixel color.
- **Optimization — distance-ring binning**: sort objects into distance rings around the player; render near→far; farther bins skip the expensive DDA if the pixel buffer already has data (occluded). (~+200 FPS)
- **Optimization — MIP/LOD maps**: per-grid level-of-detail maps; step down to a more detailed map only when a level is marked; skips large empty regions. (~+300 FPS)

### Shadows
- **Global bit mask**: 1 bit per voxel → 1 byte = a 2×2×2 voxel block.
- Voxel-splat all local grids onto a single global grid.
- Per pixel → map to global coord → ray-march toward the sun. Hit = in shadow; miss = collect sky color.
- Reuses the same ray-march skipping optimization as the albedo pass.
- Simple **BVH reader** added to provide the sky.
- Destruction of static objects = rewrite the cell data to zero.

### Global illumination
- **Hemispheric stochastic sampling**: randomly scattered rays approximate infinite light bounces → soft shadows + ambient occlusion. Noisy.
- **Temporal accumulation**: average current frame's lighting with previous frames; builds up over time. Risk: smearing if lighting changes too fast.
- **Edge-aware bilateral blur**: blur the lighting to kill high-frequency noise; crisp albedo geometry preserves edges.

### Notes / open problems
- Author's impl: ~40 FPS at 1080p, no physics/gameplay — needs optimization.
- **Unsolved**: splatting dynamic objects onto the global bit mask while keeping static voxels, without rewriting the whole bit mask each frame.

## Video 2 — Full engine (rendering + physics + destruction)

### Rendering — two paths
- **Static rendering**: one large 3D voxel grid; cast a ray per pixel from the camera; ray hits a voxel → color that pixel.
- **Dynamic rendering**: same idea, but the ray's *start point* is reconstructed from a rasterized triangle of the object's bounding box.

### Memory
- Don't store per-cell colors (too much memory). Store an **8-bit index into a color map** per cell.
- **Hierarchical grid**: keep lower-resolution versions of each grid; step through low-res until a non-empty cell, then step down in detail → skip large empty regions.
- Color data only needed at highest resolution → lower-res levels use a **bit-mask representation** (1 bit/voxel) = ~8× memory reduction; also cuts memory throughput → faster traversal.

### Ray tracing / lighting
- Since we already cast rays, RT is cheaper here. After normal + depth pass, reconstruct the ray's hit position in global space.
- **Scattered rays** from hit location step through the scene; certain indices marked **emissive** → collect lighting when a ray hits them.
- **Reflective materials**: ray is perfectly reflected instead of scattered.
- **Dynamic object shadows**: traditional **shadow map** — render dynamic objects from the sun (orthographic), sample the map to test shadow.
- **Hard shadows (static)**: cast a ray toward the sun; hit = in shadow.

### Noise reduction
- Real-time budget → **half-resolution ray-trace pass** + low sample counts.
- **Ray-to-voxel averaging**: average all rays that hit a given voxel (exploits the voxelized look) → big noise cut, cost = more stylized.
- **Blur voxel lighting with physical neighbors** → smooth per-voxel lighting.
- **Ray-per-voxel-hit (not ray-per-pixel)**: the ray↔voxel mapping lets the shadow-map pass cast far fewer rays.
- **Temporal accumulation**: average the ray-trace pass across frames → stable lighting; cost = smearing / delayed updates.

### Physics (Jolt)
- Chose **Jolt Physics** (optimized, multi-threaded, joints + ragdolls built in).
- Override Jolt's **narrow-phase contact solver** with custom voxel contact generation: on overlap, iterate voxels, test if radius around a voxel overlaps a voxel radius of the other grid.
- Optimization from contact topology: a **face can only contact a corner**, an **edge only an edge or corner** → far fewer voxels to check.
- Filter contacts down to the **4 most extreme points** — solving extremes makes the rest redundant, saves solver time.

### Destruction
- Engine handles only small-object destruction (not large structures).
- On cut: **flood fill** to check if all voxels are still reachable / the object is still attached.

### Limitations flagged
- Dynamic objects are NOT in the global grid → bounced rays can't hit them → no emissive dynamic objects, and dynamic objects can't occlude light.
- Needs chunk streaming for infinite/large worlds; needs large-scale destruction (Teardown-style).

## Video 3 — Zepha (C++/Vulkan, JS-modded) — terrain meshing, part 1

**Different paradigm:** Videos 1–2 = ray-marched voxel grids (Teardown-style). Zepha = **rasterized polygon meshing** (Minecraft-style blocks → triangle meshes). Techniques here are about generating efficient *meshes*, not ray traversal.

### Chunking & draw-call economy
- Infinite block grid → divided into fixed-size **chunks**; only load/render chunks near the player.
- GPU is fast at rendering; the CPU issuing draw calls is the bottleneck. Many small meshes → CPU can't keep up, GPU idles.
- **One mesh per chunk** (not per block) → fewer draw calls, more GPU work per command. Cost: editing one block re-generates the whole chunk (cheap on modern HW vs. draw-call savings).

### Mesh geometry reduction
- **Hidden-face culling**: don't emit quads for block faces covered by neighboring blocks → leaves a fraction of a percent of original quads. (Video notes Minecraft stops here.)
- **Backface culling**: skip quads only visible from the back (uses winding/point order). Mesh-level on/off only.
  - Foliage (grass, flowers) needs both sides. Old fix = duplicate the quad facing the other way (terrible — ~half of surface-chunk geometry was grass, doubled).
  - **Proper fix**: mesh generator outputs **two meshes per chunk** — one backface-culled (normal blocks), one non-culled (foliage). Extra draw calls only on surface terrain → minimal cost, big geometry savings.

### Greedy meshing
- Merge coplanar block faces into larger quads to cover more space with less geometry.
- True **greedy meshing** = optimal quad layout, but too slow for realtime.
- Zepha uses a faster **linear-time approximation**: grow each quad in the positive directions until obstructed, then move on. Less optimal but huge win over naive.
- Non-cubic blocks still benefit: any face spanning a full block on ≥1 axis (e.g. grass-block top faces) can be greedy-meshed.

### Level of Detail (LOD)
- Fine-detail blocks (twigs, pebbles, foliage) can't be greedy-meshed → handle via LOD instead.
- Blocks generate **low-res variants** (auto or mod-configured), swapped in beyond a distance. Variants are **full cubes or invisible** → distant terrain becomes greedy-meshable; tiny detail blocks (tallgrass) can be dropped entirely.
- **Planned**: progressive downscaling — 2/4/8/16 blocks per cube until a whole chunk = one cube at extreme distance.
- **GPU quad-overdraw motivation** (cites a SimonDev video): GPUs shade in **2×2 pixel quads** regardless of face size. Sub-pixel distant faces → GPU paints 4 px for 1 px of coverage (75% wasted) → strong reason to simplify distant meshes.
- Open LOD challenge: **smoothing gaps/cracks between adjacent LOD levels** (unsolved, future video).

### Next (teased for Part 2)
- Even with LOD, still hit the draw-call ceiling at long view distances → need to "speak the GPU's language" (likely indirect/GPU-driven rendering, batching).

## Video 4 — Zepha — texture memory & vertex packing

Same engine as Video 3 (rasterized mesh, OpenGL). Topic: fitting hundreds of textures into VRAM past the usual **16–32 bound-texture limit**.

### The problem
- GPUs allow only ~16–32 textures **bound** (readable) at once.
- Prior workaround = **texture atlasing** (many textures in one big image), but: anisotropic filtering needs padding around each sub-texture (wastes space), and a max-size atlas only holds a couple dozen high-res textures.

### Sparse Bindless Texture Arrays (3 composable techniques)
- **Sparse Textures**: reserve all needed **virtual** memory up front, allocate **physical** memory only for the parts actually uploaded. Virtual memory is near-unlimited (TBs) and just points to physical. Stream texture chunks in/out; freeing physical memory resets the virtual pointers. Upload only the regions you currently need.
- **Texture Arrays**: a list of up to ~2048 textures behind a **single bind slot**. Constraint: all layers must share size/shape/format. Handle size variety with **multiple arrays at exponential sizes** (put each texture in the smallest array that fits). Large-size arrays are expensive to allocate at full length.
- **Sparse Texture Array** = combine the two: array = 1 bind slot for many textures; sparse = each layer only occupies physical memory when committed. So an array costs memory only for the textures actually held, not its full length.
- **Bindless Textures**: access a texture via its **64-bit handle passed as a uniform** — no bind slot at all. Removes the bind-slot ceiling entirely (bind slots otherwise get eaten by multiple arrays + effects like SSAO kernels/wind).
- **Full stack = Sparse Bindless Texture Arrays**: hundreds of textures loaded, zero bind slots, physical memory only for what's used.

### Texture Compression
- GPU texture compression ≈ JPEG-style but uses read-optimized algorithms (no render-time inflation). In OpenGL it's **opaque** — just flag the texture as compressed; read/write path is identical to normal textures.
- 3 widely-supported levels, all **lossy** (trade precision for 4×–6× memory savings). Poor fit for pixel art; good for high-res antialiased textures.
- **Caveat**: GPUs that support sparse textures are **not required** to support *sparse compressed* textures. If unsupported, fall back to bindless (no sparse/array).

### Vertex Optimization (texture reference packing)
- Naive texture reference per vertex = **4 floats (16 bytes)**: UV x/y + array index + layer.
- Zepha packs into **64 bits (8 bytes)** via bit-shifting: 21 bits each for x and y (integer UV), 11 bits array layer (0–2047, matches OpenGL 4.0 min), 11 bits texture index (up to 2048 referenceable). → **2× memory savings** per vertex ref; precision loss only for exceedingly large textures.

### Notes
- All of these except vertex packing are **OpenGL extensions** — widely supported on modern desktop GPUs, may be missing on mobile/consoles → feature-detect and fall back to basic textures.
- Author's disclaimer: not certain Returnal uses these specifically; techniques are prolific in AAA. Info is scattered — corrections in pinned comment.
- Transcript tail (~11:00+) is channel/personal, non-technical.

## Video 5 — Zepha — "Speaking the GPU's Language" (meshing part 2)

Same engine (C++/Vulkan). Topic: cut **draw-call overhead** without touching the meshes themselves. Builds on Video 3's geometry optimizations.

### GPU primer / why overhead matters
- GPU is a separate "mini-computer" (own VRAM, parallel processors, own instruction language) specialized for **rasterization** (2M+ pixels every 16ms at 1080p60).
- Big physical/logical separation from CPU → lots of complexity issuing commands.
- Rendering a mesh = upload mesh to GPU memory → create/bind a **graphics pipeline** (container of shaders) → issue a draw call every frame.
- **Overhead** = any GPU time spent waiting for or interpreting instructions instead of rasterizing. GPU runs on a different clock, often faster than the CPU can feed it.
  - Cautionary tale: re-sending mesh data from CPU every frame (vs. keeping it resident) dropped 300 FPS → 20. This is *why* meshes-resident-on-GPU exist (vs. old **immediate mode** where every tri was issued per-frame from CPU).
- Even resident meshes aren't enough now: **each draw call has fixed setup cost** (pipeline setup + memory binding) regardless of mesh size → another reason to pack more geometry per mesh / fewer calls.

### Instancing
- One draw call renders **N copies of the same mesh** via an **instance count** parameter. Decades old, foundational.
- Most efficient possible render path: no mesh reload, no pipeline reset, no cache eviction between copies.
- To differentiate copies: pass **per-instance data** (uniforms in OpenGL; push constants / SSBOs in Vulkan). Can't change data mid-draw — instead pass an **array** (e.g. positions) and index it with the **instance index** built-in (starts at 0, +1 per copy).
- Any arbitrary per-instance data works: position, rotation, scale, blend color, animation state.
- Very cheap on throughput (just an integer count on top of a base call). Vulkan made **all draw calls instanced** (pass 1 for a single mesh).
- Zepha example: 5000+ animated rabbit instances (each own animation state) in <1ms on an RTX 3060.
- **Limitation**: every copy references the *same* mesh data / same vertex count → poor for many *different* meshes. Some engines render terrain purely by instancing (group blocks by type per call) but that restricts block models and makes hidden-face removal hard. Zepha does NOT use it for terrain.

### Indirect Rendering (Multi-Draw Indirect)
- Render **many distinct meshes in one aggregate draw call**, as long as they live in the **same memory buffer**. "More advanced instancing" — bind textures/geometry/data once, dispatch one call.
- Key realization: **uploading a mesh** and **creating its buffer** are separate steps. Allocate big buffers yourself; write/reference independent segments freely.
- Zepha's scheme: **64 MB buffers** for chunk data, subdivided into **4 KB regions**; a chunk mesh occupies ≥1 region (first-fit), freed regions marked reusable. Per frame: iterate buffers, find in-view chunks, render each buffer's visible chunks as **one indirect call** → whole visible scene in **<~12 draw commands**.
- Hybrid of instancing perf + per-mesh flexibility. Slower than pure instancing but far above N separate draw calls (GPU reuses memory-binding + pipeline-state assumptions).
- **Memory-layout caveats**: colocate spatially-nearby chunks in the same buffer (else you're back to 1–2 meshes per indirect call). Don't over-allocate per region (wastes VRAM). Add placement **fuzziness** so an overfull region can spill into an adjacent buffer. "No free lunch" — design the system to earn the perf. Can hit tens of thousands of meshes/frame with little bandwidth bottleneck.

### Teased (part 3)
- Zepha meshes are tiny: >3000 chunk meshes per 64 MB buffer → avg <21 KB/mesh, each spanning a 32³ block area, yet regularly tens of thousands of vertices. The trick: pull out data repeated in every copy of a block, encode only **per-face changing info**. (Next video — likely aggressive vertex compression, complements the Video 4 packing.)
- Transcript tail (~14:10+) is channel/personal, non-technical.

## Video 6 — "Blazingly fast" voxel engine (Rust + Bevy) — data generation

**Stack note:** Written in **Rust + Bevy** — same stack as your projects. Topic here is **world data generation** (the code that generates terrain shape), a domain the earlier videos didn't cover. Author claims ~20–25× total speedup. Full list they name: extremity bound checking, bitwise manipulation, cache locality, noise upsampling, noise caching, run-length decoding, lookup tables.

### Extremity bound checking (their "most powerful" optimization)
- Chunk gen loops over every voxel (flat 32³ vector; compute XYZ from a single index rather than nested loops).
- A heightmap voxel is grass if `y < noise()*scale`, else air.
- **Idea**: if a voxel's Y is above the noise function's *max possible* output, it's guaranteed air — skip the noise call. If below the min, guaranteed solid. Skip noise entirely outside the known output bounds.
- Wins: 300µs → ~100µs (3×) in-bounds; a fully-out-of-bounds chunk → 245ns. Demo: 29s → 2s (15×) on identical shaping logic.
- **Cost**: bounds must be tracked manually and recomputed whenever shaping logic changes (domain warping, thresholds, gradients, multiple summed noise layers all change bounds). Accurate bounds (e.g. skipping a 2nd noise call conditionally on the 1st) hurt readability badly.
- **Their solution**: don't write shaping logic in Rust at all — **serialize it to a data file** (declare noise params + a list of "layers": source noise → mapping range → output target like height/density). Bounds are then **auto-computed** from the layer graph — you get the optimization for free, no manual bounds. Tradeoff: less expressive than hand-written code; complex shaping may be hard/impossible to express. (Speculates Hytale's fast node-based editor may do similar.) Verdict: not an early-stage optimization — either painful by hand or a huge "build-an-interpreter" architectural effort.

### Noise upsampling (called mandatory)
- Instead of sampling noise per voxel, sample every 2nd/4th/8th/16th voxel and **interpolate** the gaps.
- 32³ chunk at 4× upsampling = only 729 noise calls (~45× fewer). But interpolation isn't free, so gains aren't linear.
- 64³ chunk, 3D noise benchmarks: baseline 7ms → 2× = 2.4ms → 4× = 1.4ms → 8× = 1ms → 16× = 1ms (8×→16× barely helps because per-voxel interpolation cost dominates). **4× upsampling ≈ 500% gain with little visible quality loss.**

### Noise caching ("never ask the same question twice")
- 2D heightmap noise depends only on X,Z — don't recompute it 32 times down the Y axis. Hoist the call out of the Y loop → immediate 32× fewer noise evals.
- **Cross-biome caching**: biome blending at borders normally runs *both* biomes' full shaping logic. But biomes often share the same underlying noise params → **bake shared noise once at startup** and pass it into every biome's shaper. Implemented via a shared file of globally-defined noise params; the engine bakes them and hands them to each shaper. Only matters at biome borders.

### Run-Length Encoding as *runtime* data (their unusual bet)
- Store voxels as RLE (voxel type + run length), NOT a flat 32³ array. Chunk size **32×256×32** (very tall).
- Everyone RLE-compresses for save/network; almost nobody uses it as *runtime* data because random access + modification are slower (must traverse to find X,Y,Z; inserting a block splits a run, resizes, inserts).
- **Benefits they found**:
  - Save-to-disk = basically a memory dump (runtime format already == compressed format).
  - Huge render distance, tiny RAM: 36-chunk render distance = **400 MB** (vs ~15 GB uncompressed, or 7 GB at 8-bit/voxel).
  - Tall 32×256×32 chunks → far fewer chunk positions for the load/unload scanner → less chunk-management overhead.
  - Extends noise caching vertically: one heightmap / biome lookup reused across all 256 Y (8× more reuse than 32-tall chunks). Biome (from X,Z) fetched once, reused 256×.
  - **Synergy with extremity bounds**: a bound saying "below Y200 is all grass" writes as a single RLE entry `grass 200` — a huge write in O(1) vs a for-loop touching every cell.
  - **Fast surface detection**: RLE already encodes where a block type changes → find surface positions (trees/rocks/NPCs) and cave heights instantly without iterating the full volume.
- **Caveats (still experimental)**: surface features (trees) needing random access not yet reimplemented; heavy world-modification not stress-tested — cache efficiency may drop.

### Result
- New engine: 30,000 chunks (~8 billion voxels) loaded + meshed in 14.4s = **~500M voxels/sec**. Old engine: 20M voxels/sec → **~25× faster**. (Trees not yet implemented.)
- Also mentioned in passing (not detailed): bitwise manipulation, cache locality, lookup tables, EOL markers. Some tried techniques *hurt* performance (not enumerated).
- Teases a next video on **occlusion-based color ramping** (per-voxel-type color gradient picked from ambient occlusion — a look/beauty technique, ~10% perf cost).

## Video 7 — GI voxel engine on integrated graphics — "mechanical sympathy" micro-opts

Engine goal: cellular-automata-approximated global illumination + water physics + real-time shadows + reflections, running on a laptop with **no discrete GPU** (Intel Iris ≈ old MX330). Static scene is fine; chunk generation while moving tanks FPS. (Note: video doubles as a promo for the author's "Devana Box" code-scanning tool; techniques are generic and sound.) Framed as 3 categories of low-hanging fruit to **reduce total work** without changing algorithms.

### 1. Math optimizations (cheap ops vs expensive ops)
- Some ops cost far more cycles: addition / basic logic checks are ~free; **integer division, modulo, and vector math are slow**. At millions of calls/frame this dominates.
- **Div/mod → bitwise** on power-of-two dimensions: world dims are 64/32 etc., so coordinate calc's `/` and `%` become bit shifts / masks — same result, single cycle. (Standard Minecraft-style power-of-2 trick.)
- **Vector-lib → scalar math + hoisting**: replaced a vector-math library call with raw scalar math and hoisted the computation out of the loop → faster AND eliminates a data pack/unpack loop.

### 2. Redundancy elimination (no algorithm change)
- **Cache chunk lookup across ray-march steps**: shadow ray-march was re-fetching chunk data at every step even within the same chunk → only look up / cache on **chunk-boundary crossing**.
- **Sequential index += 1 instead of recompute**: a loop recomputing the next block's coordinate + metadata (RLE-packing-like) operated on data that was **already sequential** → just increment the index.
- **Sample noise per chunk, not per volume border**: chunks contain "volumes" (for the cellular-automata step) that each re-sampled noise at their borders → sample noise once for the chunk and cache; volumes read from it implicitly.
- **Hoist shared shader constructions to CPU / constant memory**: identical constructions across shaders (90% shared) → build once on CPU, upload to GPU constant memory, share across threads, apply small per-use tweaks.

### 3. Memory layout ("mechanical sympathy" for the cache)
- Bottleneck localized to **water** (static most of the time but a heavy element).
- **Array-of-Structures → Structure-of-Arrays (AoS → SoA)**: water processing pulled in unneeded metadata alongside needed fields. Split into separate arrays per process → the fields a calculation needs pack densely into cache → fewer main-memory trips (cache-line efficiency).
- **SoA unlocked SIMD**: with data laid out per-field, use **single-instruction-multiple-data** (matrix-mul-style parallelism) to do ~16 water calcs at once instead of one → removed the water bottleneck almost entirely.

### Result
- ~2× frame rate (goal met, ~120 FPS), zero algorithmic changes — pure "don't do unnecessary/expensive work" refactors, each found in minutes, implemented in ~5 min. Heavier restructures were suggested but deferred.

## Video 8 — Voxel ray traversal from scratch (Rust + Vulkan compute) — 32B voxels real-time

Philosophy: skip meshing entirely — render voxels **directly** by casting one ray per pixel on the GPU (no triangles). Reference implementation in Rust + Vulkan compute shader (GitHub in the video). This is the foundational math the Teardown-style videos assumed.

### Fundamentals
- **Voxel** = smallest building block of 3D space; can hold anything (here: 1 bit solid/empty, or an int material id).
- **Flattening function**: 3D coord → 1D index for an array of size `w*d*h` (stack Y layers along X, Z rows end-to-end). Alternatives improve locality/compression (covered at the end).
- Position → voxel coord = `floor` per component; **clamp/bounds-check** to avoid out-of-bounds from float error.
- **Ray** = origin O + non-zero direction D; points `P = O + t·D`. Direction need NOT be normalized. `t` = multiples of D from origin.

### Voxel ray traversal
- **Naive (bad)**: step forward in small `Δt` increments checking each point — can miss voxels the ray truly intersects; shrinking the step wastes hundreds of iterations per voxel and still misses. Rejected.
- **First voxel**: if origin inside grid → `floor(origin)`. If outside → compute grid entry with a **Ray-AABB intersection** via the **slab method**. Precompute **1/direction** in the ray struct (multiply is faster than divide). No AABB hit → early return (misses all voxels). Use the entry `t` → entry point → clamped voxel coord.
- **Next voxel**: treat the grid as stacks of axis-aligned planes; crossing a plane = new voxel. Direction sign picks which plane per axis (positive x-dir can't exit through the lesser x-plane, etc.) → one candidate plane per axis. Use **sign of 1/direction** (handles float ±0 quirks).
  - Solve `P.x = x0` for `t` per axis → `t_x, t_y, t_z`; step the axis with the **smallest t**, adding the direction's sign to that voxel component.
  - **Optimization**: each step increments that axis's t by `|1/direction|` (constant) → precompute and just increment `t` each iteration instead of recomputing.
- This is exactly the **Amanatides & Woo (1987) "A Fast Voxel Traversal Algorithm for Ray Tracing"**.

### Rendering / hit data
- Voxelize a mesh (Stanford Bunny) to test (voxelizer in repo, not covered).
- Compute shader casts a ray per pixel. Beyond hit/miss, cheaply derive: **which face** was hit, **where on the face**, and **distance from camera** → enough for shading, texturing, or secondary/RT rays.

### The 10× optimization — memory layout, NOT the algorithm
- Baseline: 2048³ grid, 1000×1000 screen → **~12 FPS**.
- **Z-order curve (Morton) flattening**: nearby-in-space → nearby-in-memory → **~102 FPS (~8.5×)**. Why: traversal always steps to a neighbor → higher cache-hit chance; nearby rays share cache lines → less memory read. (A more complex flatten function is *faster* purely from locality.)
- **Better: store voxels in a GPU 3D texture / 3D image** — hardware-optimized locality (more sophisticated than Z-order) → **~121 FPS = 10×**, same algorithm.
- Lesson: **rethinking data storage** (Z-order → 3D texture) got 10× with zero algorithm change.

### Teased (next video)
- **Sparse Voxel Octrees (SVO)** + improved traversal exploiting sparseness: compress data (more voxels/less memory) and **skip large empty regions in one iteration**. (Same empty-space-skipping idea as Videos 1–2's hierarchical grids/mip levels, done via octree.)

## Video 9 — Open Builder — vertex packing to cut mesh VRAM

Mesh-based engine ("Open Builder", OpenGL). Topic: shrink per-vertex data so chunk meshes use far less video memory. Concrete worked example of the vertex packing that Videos 4 & 5 mentioned.

### The problem
- Each block-face vertex stored: position (3 floats = 12 B) + texture info (3 floats = 12 B) + lighting id (1 float = 4 B) = **28 B/vertex**. 6 vertices/face = 168 B/face.
- Best case (flat 32×32-chunk world, fewest faces) → **176 MB VRAM**; real terrain with trees/flowers is far worse.

### Chosen approach — reduce bytes per vertex (not greedy meshing)
- Notes **greedy meshing** as the well-known alternative (1024 faces → 1 face) but skips it (says the source article is hard to follow) in favor of bit-packing using assumptions about the voxel world.
- **Position**: chunk is 32³ → max local coord 32 → 6 bits per component → **3 floats (12 B) → 18 bits**.
- **Texture**: UVs don't vary per block type → move them into the **vertex shader**; only store the **texture-array index** (assume <500 blocks → 9 bits) + **3 bits** to select which of the shader-defined UV corners → texture info **12 B → 12 bits** (he corrects 9→12 mid-video).
- **Lighting**: only 4 possible face-direction values in range 0.4–1.0 → multiply by 5 → integers up to 5 → **3 bits** (shader divides by 5 to restore) → **4 B → 3 bits**.
- Total: **28 B → 4 B per vertex**, packed into a single 4-byte int sent to the shader, unpacked GPU-side.

### Result
- **No measurable FPS change** (packing/unpacking is free relative to the win).
- Flat 32×32 best case: **176 MB → 25 MB** VRAM.

## Video 10 — GPU falling-sand voxel engine (Godot compute shaders, MIT open source)

Fully-dynamic "falling-sand"-style world: hundreds of millions of voxels updated many times/sec, entire sim on the **GPU via compute shaders**, direct volume rendering so edits show instantly. Godot ("GDU" = Godot) for the app/physics.

### Voxel representation
- Voxels = data points on a regular 3D grid (not inherently cubes); nearest-neighbor sampling gives the cubic look.
- Storage buffer, **32 bits/voxel**: 8 bits type + 16 bits color + 8 bits auxiliary. Each voxel edge = 1/8 unit.

### Rendering — ray marching + Amanatides & Woo
- Naive equidistant stepping either double-samples or steps over voxels (misses hits) → rejected.
- Uses **Amanatides & Woo** DDA (same as Video 8): distances to the 3 upcoming grid planes, step to nearest, integer step in the grid; precompute distances/deltas since the grid is uniform.
- **GPU tweak**: use a **mask instead of if-statements** to pick the axis → reduces **thread divergence** on the GPU.

### Voxel Bricks (hierarchy for interactivity)
- Large worlds slow the DDA. Chose the **brick map** (minimal overhead vs. octrees, since voxels change constantly).
- Group voxels into **8³ bricks**; if all-air, mark the brick empty. During traversal: if brick empty → skip straight to the next brick; else run the per-voxel DDA inside it. Big speedup from fewer steps.
- Future: only allocate voxel memory for occupied bricks, grow dynamically.

### Editing (all on GPU)
- Ray cast in a tiny compute shader (1 thread — wasteful but avoids moving voxel data to CPU); writes the hit position to a small buffer. A second shader places a **ball of voxels** of the selected material around it. No surface-extraction/data-structure overhead → edit count limited only by world bounds.

### Cellular automata simulation
- Borrows CA (à la Conway's Game of Life): update each cell from its neighbors.
- **Water**: gravity (move down if air below); else move sideways in a pseudo-random axis-aligned direction (hash of position + frame # as RNG) if unoccupied. Pure-random wanders → instead **remember the first successful horizontal direction** and keep going until hitting a wall, then re-pick (small chance either side for naturalness).
- **Lava**: same as water, plus a 2nd pass checking the **von Neumann neighborhood** for water → turns to stone.
- **Sand**: falls straight down or diagonally (no horizontal wandering) → piles into pyramids; pushes liquids up.

### GPU concurrency correctness
- **Ping-pong double buffering**: read from current-step buffer, write next-step buffer, swap each iteration → every thread sees consistent, untainted neighbor data.
- **Atomic compare-and-swap** to prevent two threads writing the same voxel (voxels vanishing): write only if the slot still equals the assumed value; return-value check tells you if the write succeeded. (Alternative = multi-pass, one direction per pass — rejected as too complex.) Atomics cost some perf but fine here.
- **Brick occupancy recompute** (world changes constantly): per-brick compute shader, block of exactly 32 threads, each reads 16 voxels → counts non-air → writes to its slot in **shared memory** → syncthreads → thread 0 sums and writes the brick. Fast: no atomics, shared memory > global memory.

### Collisions
- Reuse Godot's physics via a **convex collision mesh** built from a localized surface-extraction around the player: async-collect nearby voxels as 1 bit (solid/not) → CPU builds a mesh with a simple algorithm (one quad per solid/non-solid boundary face). Rebuilt only a few times/sec — enough for movement.

### Visual fidelity
- Terrain from **hashes + fractal noise** (finite world, generated in one go); voxel = grass if air 2 blocks above, else rock.
- **Blinn-Phong shading**: round the camera-ray hit to the voxel grid → per-voxel specular highlights.
- **Hard shadows**: secondary ray toward the sun, occluded = shadow. Simple skybox + sun from dot(sunDir, sampleDir).
- **Post**: render HDR (32-bit/channel float) → tone map to SDR; bloom via Godot's WorldEnvironment.
- Future ideas: GI, better water sim, transparency.

## Video 11 — "Voxo" MPM physics engine — optimizing for 16× larger scenes

Physics-first voxel engine (Material Point Method / MPM) — unified particle model simulating gas↔liquid↔solid. Focus: memory bandwidth + multithreading to simulate 16× bigger scenes in real time on a laptop. (Not a renderer video — this is about *simulation* scaling.)

### Context / bottleneck
- Multithreading now scales ~linearly with core count (earlier a 12-core AMD 5900X did *worse* than a 6-core 8700K until fixed).
- **Memory bandwidth** is the long-standing limiter. Unified particle model tracks a 3×3 **deformation gradient** per particle → 48 bytes/particle.

### Optimizations
- **Material-group specialization**: keep deformation gradients only for **elastic** materials; **liquids** render from just position + velocity — and velocity can be derived from the **previous frame's upload buffer**, so only position is sent per frame. Big bandwidth cut.
- **Structured particles**: in MPM, the gather (G2P) and scatter (P2G) grid steps can be merged to save a temp velocity-gradient buffer — but rigid-body **shape matching** must happen between them. New structured-particle system lets each structure do its own **G2P → shape-match → P2G** on its own particles. Structures hold multiple clusters (working on inter-cluster linkages next).
  - Bonus: structured particles **don't need sorting for cache coherence** — spawn them along a **space-filling curve** and only reorder on destruction; clustering already helps cache locality.
- **Sparse grid**: only occupied tiles stored → faster for V-cache CPUs (occupied tiles stay in cache) and, surprisingly, faster on *all* tested machines. Prerequisite for multi-grid.
- **Adaptive level of detail** for particles by distance-to-player and distance-to-surface (surface distance computed at tile resolution for speed).
- **Multi-grid / coarse-scale pressure**: high gravity constant made liquids compress; computing pressure on a coarser scale lets water build depth quickly so lower LODs can activate → deep water instead of shallow.
- **PCIe transfer cuts**: send sand's deformation gradient as **bytes instead of floats** (4× less bandwidth). Future: for structured objects, send only a transform (not every particle position) → less PCIe traffic + faster lighting-volume construction + accurate raycast crosshair picking.

### Rendering / misc
- Recently moved voxel rendering to the **Amanatides & Woo** algorithm "similar to Teardown."
- Added **Intel XeGTAO** (ground-truth ambient occlusion), block editing, imported a MagicaVoxel NYC model.
- Open question being left to simmer: handling particles moving **between structured/unstructured systems** and **between material groups** (phase changes).

## Video 12 — Voxel engine devlog #8 — distance-field accelerated DDA (6× perf)

Devlog on a voxel engine's renderer; ~6× perf in the test scene via a new traversal accel structure. (Has a mid-video Core sponsor read — non-technical.)

### Grid hierarchy + distance field (instead of octree)
- Switched from an **octree** to a **grid hierarchy** storing a **distance field** in the voxel grid: each voxel stores the **distance (in steps) to the nearest solid voxel** → the ray can jump that many voxels at once through empty space.
- Test scene is deliberately hostile (voxels scattered everywhere, little empty space to skip):
  - Plain DDA: ~18 FPS.
  - **1-bit distance field** (store just "1 step" vs "2 steps") → ~47 FPS (**3×**), just by stepping 2 voxels at a time in empty space.
  - **8-bit** field (up to 256 steps) would improve it hugely more.
- **Cost / tradeoff**: larger max distance → field computation gets dramatically more expensive (must compute distance per voxel over a larger range). Fix (planned): **split the field calc across multiple frames** on the GPU.

### Modified DDA
- Had to modify the DDA to take **multiple steps at once** against the distance field. Used **ShaderToy** to visualize/debug (ray steps 3 voxels, then 2, then 1 — each dot = a step location); runs per pixel.

### Benchmark vs old renderer (MagicaVoxel scene from devlog #6)
- Old renderer: 13 FPS (and hit max ray steps — yellow = overstep, red = high step count).
- New renderer, **no distance field**: 25 FPS (**2×** already).
- New + distance field max-step 4: 74 FPS (**>5×**).
- New + max-step 16: hits the 74 FPS cap.
- Next: reintegrate with the caching/streaming system for massive scenes.

## Video 13 — Voxel engine devlog #5 — GPU-managed LRU voxel cache / streaming

Same engine as Video 12 (earlier devlog — #5 vs #8). Topic: the caching/streaming system that lets the engine hold an ~infinite voxel world while keeping only visible voxels in GPU memory.

### The cache
- Goal: load/unload only the voxels needed to render the current view → effectively infinite voxels. Demoed running on as little as **1 MB** of GPU voxel storage.
- **LRU via move-to-front**, not sorting: voxels live in a 1-D array; front = recently accessed, back = stale. When a voxel is **rendered, move it to the front**; un-rendered voxels drift toward the back and get evicted. Avoids the cost of sorting the whole array.
- **Adding voxels**: overwrite the stale entries at the back and move them to the front → constant load/unload while keeping the cache "sorted" by recency.
- Entire algorithm runs in **compute shaders** → the **GPU manages its own cache**. Bonus locality: voxels accessed together bunch together in memory → faster (nearby-memory access).
- Added a **pause-the-cache** debug mode: freeze eviction and fly the camera to see what's actually loaded outside the view.

### Scale / limits
- Addressable voxel store: up to **64 GB** as one big tree (bounded by system RAM).
- Stress tests: 14 MB cache (most off-screen voxels unloaded); 1 MB cache still renders fine — fewer voxels resident per frame but still >1 trillion viewable while flying. Typical real use = a few GB (≈ your GPU's memory).
- **Bug war story**: indexing past the end of a GPU array overwrote all GPU memory → garbage → renderer drew the garbage. (Cautionary: no bounds safety net on the GPU.)
- Next: refactor + add a **CPU-side cache** tier so more voxels can be resident than fit on the GPU.

## Video 14 — Voxel engine rewrite (libGDX/Java) — frustum culling + LOD

Mesh-based voxel engine rewrite (libGDX). Rebuilt from a messy/slow version. Basics-level coverage of two standard optimizations.

### Foundation
- Quad → 32×32 chunk (all quads merged into one chunk mesh — noted as good for performance and larger render distance).

### Frustum culling
- With a wide chunk radius, only render chunks **in view**; skip off-screen chunks → big perf win.
- `isChunkVisible(camera)`: uses the camera frustum + a `frustumBounds(frustum, planes, chunk)` check (uses a GitHub frustum library, per an article he links).
- Chunk x/y/z are **shifted left 4 bits + 8** to get the chunk **center**, so the frustum-plane checks test against the chunk's center point.

### Level of Detail (LOD)
- Far chunks rendered with **fewer vertices/indices**. Can produce holes when the count is set very low, but far away it's unnoticeable and resolves as you approach.
- Implementation: compute **distance between chunk and player** → pick one of **3 LOD states**, each holding a precomputed **index count**; tell the GPU to render with that count → fewer indices sent for distant chunks.

## Video 15 — Voxel engine devlog — real-time lighting: hard/soft shadows + GI

Devlog overhauling the lighting system: hard shadows, soft shadows, and indirect global illumination, all real-time. Runs the forest scene at ~70 FPS 1080p on a laptop RTX 3050 Ti (≈ GTX 1060). Notable for a **per-visible-voxel dedup** scheme via a hashmap.

### Hard shadows
- Cast one **shadow ray toward the sun** per voxel; hits something before exiting → in shadow.
- Problem: a voxel is visible from many pixels → avoid duplicate shadow rays. **Solution — visible-voxel hashmap**: primary ray hit → get the voxel's unique **64-bit ID** → use as hashmap key storing an `isVisible` bool. First hit adds it to a **queue** and sets the bool via an **atomic** op (data-race safe) → each voxel queued once.
- Separate shader shades each queued voxel, casts the shadow ray, sets `isShadowed`. Final full-screen pass: each pixel looks up its hit voxel and darkens if shadowed.

### Soft shadows
- Sun is an area light; partial occlusion → partial shadow. Shoot **multiple rays per voxel toward random points on the sun**, measure occluded fraction.
- Reuse the hashmap: replace bools with integers `numVisible` / `numShadowed`. Per pixel hitting a voxel: atomically increment `numVisible`. Shadow shader now runs **per pixel**, shoots a sun ray with a small random offset, atomically increments `numShadowed` if occluded. Final = `numShadowed / numVisible`.
- **Stratified sampling to cut noise**: instead of fully-random jitter, ensure each ray covers a unique sub-region of the sun. Since `numVisible` tells you exactly how many rays a voxel gets, offset the i-th pixel's ray by the i-th point of a **Fibonacci sphere** of N points → drastically less noise AND fully **deterministic**.

### Global illumination (path tracing)
- Per visible point, cast a ray into the random hemisphere above it; recurse on hits until reaching a light; add light × accumulated energy-loss from bounces.
- Reuse the hashmap: add a `vec3 indirectLight`. Per-pixel shader path-traces (random directions, tracking hit voxels) until a ray escapes to sky → multiply sky color by the product of hit-voxel colors → **atomically add** to `indirectLight`. Final shader divides by `numVisible`.
- Runs at **half resolution, max 2 bounces** for perf → noisy.
- **Temporal accumulation** to denoise: keep **two hashmaps** (current + previous frame); average previous-frame samples into the current frame before the final lighting pass → reuses prior computation, noise mostly gone. Costs a few frames to converge (barely noticeable at 60 FPS).
- **Emissive voxels**: mark voxels as light-emitting; add their light to GI when a path hits one.
- **Dynamic scenes**: clamp how many samples accumulate over time → lighting stays responsive to edits.

### Validation
- Tested on the **Cornell box** (classic GI test scene) — accurate even at 2 bounces; light bleeding from colored walls onto objects. Smaller voxels + detailed objects → fairly realistic.

## Video 16 — Voxel engine devlog — material system + particles (Vulkan RT)

Likely same author/engine as Video 15 (devlog series, Vulkan ray tracer; soft shadows teased here, detailed in #15). Topic: materials and particles to "bring the engine to life." Goal = keep the 3D-pixel-art look (each voxel a single color) while adding surface detail.

### Materials — 3D textures
- Per-voxel textures (Minecraft-style) look bad for small voxels → instead use **3D textures that span multiple voxels**, each voxel still one flat color. (2D textures can't cleanly project onto a 3D volume.)
- You *could* fake it by giving each voxel a palette entry matching the texture, but that's tedious and defeats the material-grouping compression from an earlier devlog.
- New system: assign a **texture to a material**, applied during rendering; params for **voxels-per-repeat** and **filtering type**.
- **Gradient textures**: user supplies a 1-component 3D texture used as a **lookup into a 1D color-gradient** → cheap variety. Example: many spheres sampling the **same 3D noise texture** at different scales/gradients.
- 3D textures cost memory → the intent is **repeat within and reuse across volumes**.

### Procedural materials
- Can't pre-support every use case (scrolling texture, pulsating glow, math-based coloring) → let users **define voxel color with a math formula** for full freedom. Implementation is Vulkan-RT-specific (skipped in video). Example: a scrolling rainbow material in a few lines.

### Test scene
- Quick noise-based terrain + MagicaVoxel foliage: Perlin-noise texture with different gradients for grass/dirt; trees use a 2×2×2 checkerboard texture; flowers use the rainbow procedural material.
- Also implemented **soft shadows** here — fully real-time, **no temporal amortization or denoising** — but deferring the technical details to the next (lighting) devlog. (Note: contrasts with Video 15's temporally-accumulated GI approach.)

### Particle system (rasterized, not ray traced)
- Wanted wind-blown grass/leaves (inspired by his game "Teroy"). Built a **general particle system** for any particle type.
- Like procedural materials: user provides a **function computing particle position + color** plus any data buffers.
- **Rendered via rasterization / instanced rendering**, NOT ray tracing — RT of millions of tiny objects is too slow; instancing is very fast. (Tried several RT approaches first; all too slow.)
- **Tradeoff**: particles **can't cast shadows, appear in reflections, or affect scene lighting** — accepted for the perf win.

## Video 17 — Voxel engine devlog — 4 voxel data structures compared (Vulkan RT)

Same author/engine as Videos 15 & 16 (Vulkan RT voxel engine, earlier devlog). Core goal: let the programmer **mix and match data structures per volume** to fit the use case. Implements and benchmarks four, from simplest to most compact.

### 1. Flat 3D texture (baseline)
- Simplest: a 3D texture, one entry per voxel. Wasteful — stores every voxel even in large empty/homogeneous regions.
- Dead simple to create/modify/traverse → good for **small, detailed volumes**.
- Traversed with **Amanatides & Woo DDA**: 3 axis planes at the current voxel's bounds, ray-plane intersect each, step along the nearest, repeat. Cost: **one step per voxel** → huge step counts in large volumes (visualized: wider pixel = more steps).

### 2. Brick map (two-level hierarchical grid)
- Split into a top-level **brick map** + **8³ bricks** (each an 8³ 3D texture). Brick map = 3D array of **indices** into a brick array; **completely-empty bricks aren't stored** (saves 512 bytes each).
- Traverse: DDA the top level (skip empty regions wholesale); on a non-empty brick, DDA again at the fine scale; stop on hit or continue in the top grid.
- Skips large regions at once → far fewer steps. Imperfect: a brick with one filled voxel still forces stepping through the whole brick.
- Verdict: one of the **best all-round** structures — great compromise of memory / modifiability / traversal perf. (Was the structure used in his previous engine.)

### 3. Octree (variable-level hierarchy)
- Generalization of a brick map: recursively split into **8 octants**; a **homogeneous octant is not split** (a whole region = a few bytes) → big memory savings. Memory layout based on an **Nvidia paper** (ESVO).
- Traversal (can't use plain DDA — must exploit sparseness to jump regions), 3 steps per iteration:
  1. **Descend** to the lowest homogeneous octant, pushing visited nodes on a **stack** (if filled non-empty → hit, done).
  2. **Advance** the ray to a neighbor node.
  3. If now outside the parent, **ascend** via the stack until back in bounds (reach root still out of bounds → ray exited).
- Far fewer steps; best for volumes with **large empty space**. Traversal is much more complex → for small volumes the space-skipping isn't worth it.

### 4. Sparse Voxel DAG (directed acyclic graph)
- Generalization of an octree where **identical subregions share memory** (dedup nodes) → most compact of the four.
- Harder to build (needs a **hashmap** to find identical regions); traversal is basically identical to the octree (same space-skipping).
- Purely a memory-reduction tool.

### Comparison / takeaways
- Moving left→right (flat → brick → octree → DAG): **memory shrinks, build time grows**.
- Flat volume: most memory, trivial to build/modify. **Octree & DAG are NOT directly modifiable** — must fully/partially rebuild to reflect edits.
- Writing **conversion functions between structures** so voxel data can transform on the fly for max versatility.
- (This is the SVO the Video 8 author teased for their "next video," now implemented by a different creator, plus the DAG extension.)

## Video 18 — Voxel RT engine devlog — VRAM / compute / file-size optimizations

Devlog on a voxel ray-tracing engine (8³ chunks). Optimizes VRAM, lighting compute, and map file size; adds refraction. Notable for applying the same "only pay for filled voxels" idea across three subsystems.

### VRAM: bitmask + packed voxel buffer
- Already skips storing empty chunks, but a chunk with even 1 voxel stored color+normal+lighting for all **512** slots.
- Fix: chunk stores only a **bitmask** (1 bit/voxel = exists or not); actual voxel data goes in a **large shared buffer**, allocating only as many voxels as the chunk has, **rounded up to a power of two**. Much less empty space. (Caveat: one benchmark scene was inflated by a separate bug pre-optimization.)

### Lighting compute: iterate filled voxels, not whole chunks
- Lighting computed per-chunk in a compute shader that previously ran for **all 512 voxels** even if one was filled → wasted work.
- Fix: run the compute over the **tightly-packed voxel buffer** from the VRAM optimization instead of whole chunks → only filled voxels are processed. Big drop from 512/chunk.

### Map file size
- Was dumping raw memory to disk → huge, redundant files.
- **Run-length encoding** of voxel materials (skip storing runs of identical adjacent voxels) — biggest single win.
- **Reconstruct instead of store**: drop the chunk map from the file, rebuild it from other data.
- **Palette per chunk**: store a color palette + per-voxel indices instead of full colors — great when many same-colored voxels cluster.
- Result: sphere scene halved, demo scene to 1/10. (Acknowledges a general lib like zlib would do better, but wanted a custom solution.)

### Refraction
- Light bends passing into a new medium (glass) → in ray tracing it's just **changing the ray's direction**. Easy to add. Looks blocky due to per-voxel normals (author likes the look).

## Video 19 — Voxel RT engine weekend update — depth pre-pass ray acceleration

Casual weekend-update devlog. Mix of small features + one significant rendering optimization (detailed later in a planned "AMA" video).

### Depth pre-pass to shorten ray traversal (the main technique)
- New voxel-rendering acceleration: a **low-resolution depth pre-pass** that, with assumptions about voxel size, estimates how far the ray can safely skip before it could possibly hit anything.
- At full res, **offset the ray origin by that precomputed start-depth** so the ray trace *begins* further from the camera → it traverses far less of the scene.
- Debug views: raw depth output; and "distance the ray actually traveled" (without adding back the start depth) shows most pixels resolve after a short trace.
- Perf: with shadows off, ~170–180 FPS → **~300 FPS** just by enabling the start-depth offset. Big win from not re-traversing empty near-camera space.
- (This is conceptually similar to the distance-field/empty-space-skip family, but done as a separate coarse depth pass rather than stored per-voxel.)

### Smaller items
- Public build gets: block editing with any block type, placing a **3D model** (Blender's Suzanne) as voxels, adjustable place-speed, the **AO** from the private branch, and distance **fog/haze** (looks good on custom objects).
- Started toying with **audio synthesis** via **libsoundio** (~100-line sample): sine (440 Hz / A4), sawtooth, square, and combined waves — wants C++ sound effects in the game.

## Cross-video overlap & contrast
- **Videos 1–2 (ray-marched):** hierarchical/LOD grid to skip empty space, bit-mask (1 bit/voxel) low-res representation, 8-bit color-map indices, temporal accumulation, ray-marched sun shadows. Both flag the same limit: **dynamic objects not in the global grid**.
- **Video 3 (rasterized mesh):** the LOD idea recurs, but here it's about *reducing polygon count / draw calls*, not ray-traversal cost. Shared theme across all three: **kill work you can't see** (hidden faces, empty space, sub-pixel detail) and **batch aggressively** to feed the GPU.
- Note: transcript 3 tail (from ~08:50) is channel/personal update, not technique.
