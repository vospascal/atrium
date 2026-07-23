# Voxel Engine — Implementation Plan

Forward plan for growing `crates/voxel-sandbox` into a real voxel engine,
walking the technique catalog one stage at a time. Written 2026-07-22.

**Companion docs:**
- [`voxel-techniques-catalog.md`](voxel-techniques-catalog.md) — the raw
  19-video technique catalog (source material). Video numbers below (`V3`,
  `V8`, …) reference it.
- [`voxel-optimization-research.md`](voxel-optimization-research.md) — measured
  bottlenecks of the *current* sandbox + a 5-phase mesh-perf plan. Stages 1–5
  below **are** that plan; this doc wraps it in the bigger picture.

## Decisions (2026-07-22 planning session)
- **Staged**: pick techniques off the list and implement one at a time.
- **Paradigm**: **mesh-based** (extend the current sandbox). The ray-marched /
  Teardown family stays a *research branch* (see bottom), not the main line.
- **Engine**: **Bevy now**, architected so the engine-agnostic core can later
  move to another host (Unity or a custom engine). See recommendation below.
- **VR (Quest 3)**: **deferred** — desktop first. But keep "VR-readiness"
  constraints alive from the start (headless core, stereo-friendly frame
  budget) so it isn't a rewrite later.
- **Spatial audio**: the atrium engine's HRTF binaural output is a natural fit
  for eventual VR; wire it as its own stage once the world is interactive.

## Engine recommendation: Bevy + a pure-Rust core

**Don't build a custom engine.** You're already productive on Bevy 0.18
(`bevy-ui`, `behavior`, `voxel-sandbox`), and Bevy gives PBR, ECS, XR crates
(`bevy_mod_openxr` / `bevy_mod_xr`), and audio for free. A custom engine is
only justified by heavy GPU ray-marching — which we've explicitly made a side
branch.

Get the "move to another engine later" property from **modularity, not from
rolling your own engine**. This mirrors the project's existing two-box /
behavior-ECS-headless philosophy (engine-agnostic logic, thin host adapter).

### Target crate layout
```
crates/
  voxel-core/     ← NEW. Pure Rust, NO bevy dependency.
                    Voxel data structures, chunk storage (dense→RLE),
                    generation, greedy mesher (emits plain vertex/index
                    buffers + AABBs), LOD selection, edit ops.
                    Unit-testable headless, like atrium-core / behavior.
  voxel-bevy/     ← Thin adapter: consumes voxel-core buffers, spawns Bevy
                    meshes/entities, materials, camera, input, frustum plumbing.
  voxel-sandbox/  ← App: scenes, tweak panel, wiring. (current crate, slimmed)
```
Moving to Unity/other later = reimplement the thin host layer (or FFI into
`voxel-core`); the expensive, subtle parts (meshing, storage, generation) are
portable Rust with no engine ties. **Rule: any technique that is pure data/CPU
math goes in `voxel-core`; anything touching Bevy types stays in `voxel-bevy`.**

## Staged roadmap

Each stage is independently shippable and (mostly) visually invisible until
noted. Verify with the measured baseline in `voxel-optimization-research.md`
(~66 FP fps, 1.5 s full remesh, ~10 M verts, 400 MB buffers on M3 Max).

### Stage 0 — Modularity refactor *(prerequisite, no behavior change)* — ✅ DONE 2026-07-22
Extracted `crates/voxel-core` (pure Rust: `noise`, `world` [RLE storage +
generation + biome classifier], `terrain_import`; deps `glam`+`serde`+
`serde_json`, **no Bevy**). The one Bevy coupling (`bevy::math::IVec3`) became
`glam::IVec3` — the identical type Bevy 0.18 re-exports, so the seam is
friction-free. `voxel-sandbox` now depends on `voxel-core`; the mesher and
`.vox` mesh-building stay on the adapter side (they legitimately emit
`bevy::Mesh`). Behavior-preserving: the 12 tests now split 11 (core) + 1
(mesh), all green; clippy + fmt clean. `voxel-core` compiles standalone.
*Not yet done (deferred to Stage 1): extracting the mesher's plain-buffer
computation from its `bevy::Mesh` assembly — only needed when a second backend
or greedy meshing lands.*

### Stage 1 — Chunked meshing  *(V3, V14; research-doc Phase 1)*
32³ render chunks, `rayon`-parallel meshing, mesh pooling, slim vertex formats
(40 B → ~20 B via `Unorm8x4` color + 1-byte face-index normal). Bevy per-chunk
AABB **frustum culling** for main + reflection + shadow views. Localized
remesh. **Biggest architectural unlock**; enables editing, fluids, LOD.

> **Reorder (approved 2026-07-22): Stage 2 and Stage 3 were swapped.** Studying
> the mesher showed greedy meshing cannot preserve the look while per-voxel
> jitter is baked into vertex colors (merging N voxels into one quad destroys
> the per-voxel speckle on flat regions). So the shader material must come
> first. This matches the user's original priority (greedy last).

### Stage 2 — Shader-driven voxel material — ✅ DONE 2026-07-23  *(V3 backface split, V16 materials)*
Landed as `voxel_material.rs` (`VoxelTerrainMaterial = ExtendedMaterial<StandardMaterial,
VoxelExtension>`) + `assets/shaders/voxel_terrain.wgsl`. Per-voxel jitter now
recomputed per-fragment from world position (hash matches `voxel_core::noise`
exactly; sample offset half a voxel inward along the normal so all 6 faces of a
voxel share one value). Mesher bakes un-jittered color + packs per-type
amplitude in vertex alpha; props stay on a plain StandardMaterial. Build +
clippy + fmt + test green; user confirmed the look is preserved in-app.
Move the per-voxel hash **jitter** off the vertices into a fragment shader that
recomputes it from world position, so a later greedy merge preserves it.
Vehicle: `ExtendedMaterial<StandardMaterial, VoxelExtension>` — keeps all of
StandardMaterial's PBR/shadows/fog, just adds a jitter multiply. Key insight:
every voxel-type's jitter is already **mean-1.0** (`center + span·roll` with
`center = 1 − span/2`), so only the per-type **amplitude** (`span/2`) needs to
travel — carried in the otherwise-unused vertex-color **alpha** (terrain is
opaque). AO stays per-corner baked; low-frequency gradients (dryness / season /
tone) and underside-bounce stay baked (they interpolate fine across a quad).
Later extension (its own step): move tint/AO too → full greedy + a **live
season/biome slider** (uniform update, no 1.6 s rebake).

### Stage 3 — Greedy meshing — ✅ DONE 2026-07-23  *(V3, V9)*
Per face direction, per slice, greedy-merge exposed flat terrain faces into
maximal rectangles (grow width then height), keyed by voxel type + water-plane
bucket; corner colors sampled at the rectangle's 4 corner voxels so gradients
interpolate. Cover/water stay 1×1.
- **Step A (merge-safe, AO-open only):** −26% verts, look preserved.
- **Step B (full greedy):** moved corner AO into the fragment shader too — a
  packed 1-bit/voxel global occupancy buffer (`solid_occupancy_bits`) uploaded
  as a `ShaderStorageBuffer`; the shader recomputes the 4-corner AO per
  fragment (bilinear, matching the baked formula) so AO no longer gates
  merging. Cover keeps baked AO via an alpha-sentinel (+10) the shader detects.
  **Result: −47% verts overall (terrain −58%, 2.4×), look preserved**, user
  confirmed a large frame-time drop in-app. Meshing ~865 ms, gen ~726 ms.
- Deferred: vertex bit-packing / slim formats (further GPU-bandwidth win, its
  own step); a subtle mesh-time cost (6× volume re-iteration) is acceptable
  since it's one-time and the app is GPU-bound.

### Stage 4 — RLE column storage — ✅ DONE 2026-07-17  *(V6, V18; Phase 4)*
(Landed before this planning session — `VoxelWorld` is per-column RLE, 256 MB →
17 MB, unpack-to-scratch when meshing.)
Per-column RLE (`32×256×32`): 256 MB → ~5–15 MB. Never random-access — unpack
a chunk (+1 apron) to a dense scratch buffer, mesh, discard (mesher already
walks in this order). Dense overlay for edited chunks until re-compacted.
Unlocks fast scene save/load and bigger/streamed worlds. Add **palette**
per-chunk file encoding (V18) for save files.

### Stage 5 — LOD  *(V3, V14)*
**ROI caveat (2026-07-23):** LOD simplifies terrain far from the camera, but
this is a 125 m diorama viewed in full — orbit sees the whole island at ~equal
distance (LOD would coarsen the beauty shot), and first-person's far edge is
only ~125 m off. So LOD does little for the current scene; its payoff is
**large/streamed worlds and an eventual Quest standalone build**. Higher-ROI
perf levers for *this* scene: slim vertex formats (bandwidth) + trimming the
fullscreen raymarch/reflection costs. Keep LOD for when worlds grow.

Distant chunks meshed at coarser voxel size (¼ faces per level). Approach:
- **Coarse meshing:** `build_chunk_meshes` gains an `lod` level; at lod>0 it
  samples the world at stride `2^lod` (representative center sample per coarse
  cell) and emits faces at voxel size `VOXEL_SIZE·2^lod`, greedy-merged.
- **AO on LOD chunks:** skip the per-fragment shader AO (distant + fog hides
  it) via the same alpha sentinel cover uses — avoids full-res AO on coarse
  geometry.
- **Selection:** bake LOD0 (near) + LOD1 (far) meshes per chunk; spawn both
  with complementary **`VisibilityRange`** (already used for fireflies) so
  Bevy dither-crossfades between them by camera distance — no custom
  selection system, and the dither hides the pop.
- **Cracks:** try fog+distance+dither first; add downward **skirts** on chunk
  edges only if seams show.
- Further perf lever (own step): **slim vertex formats** (Unorm8x4 color +
  packed normal, needs a custom vertex shader) to halve GPU vertex bandwidth.

> Stages 1–5 are the mesh-perf core. Expected after 1+2: ~10× fewer verts/pass,
> reflection+shadow nearly free, FP fps >120 in debug, full remesh <300 ms.

### Stage 6 — Materials & art direction  *(V16; research-doc "mixed density")* — ✅ mixed-density done
3D textures spanning multiple voxels + gradient-lookup + procedural (formula)
materials. Mixed voxel density (chunky cliffs, confetti canopies, thin grass)
from the existing art-direction notes. Rasterized particle system for
wind-blown grass/leaves (V16: instanced, doesn't cast shadows — accepted).
- ✅ Done 2026-07-23: thin grass blades, chunky cliffs, confetti canopies
  (with cheap solid shadow-proxy), sun-softening curve + slider.
- Remaining: 3D/gradient textures, procedural materials — and the wind-blown
  grass, which folds into the foliage-instancing arc below.

### Stage 6b — Foliage instancing → collision arc  *(V3 backface split, V5 instancing, V16)*
Rework grass and trees from **baked per-voxel geometry** (static, no
per-object identity → no collision, ~1.2M grass verts in the chunk mesh) into
**instanced discrete objects**: a small palette of variant meshes stamped many
times from a per-instance buffer. Cheaper, and it unlocks LOD, wind, and
collision. (User direction 2026-07-23. Baked cross-quad blades were tried and
reverted — a dead end; the blade only wins as single-sided + cull-off, which
belongs here.)

Ordered steps (collision + wading deferred to the end / Stage 8):
1. **Instanced grass base** — custom instanced render pipeline (Bevy's
   `shader_instancing` / `custom_shader_instancing` pattern: one entity, an
   instance buffer, a `RenderCommand` that draws the base mesh instanced). Base
   mesh = a **single-sided cross-quad blade** with a **cull-off material**
   (8 verts). Instance buffer built from the grass cells: position + per-
   instance height / lean / tint / sway-phase. Replaces the baked meadow-cover
   grass → big vert + memory cut. Precedent in-repo: fireflies / precipitation
   (bounded pseudo-instancing via shared mesh + per-instance vertex attrs).
2. **Variants** — ~5 pre-made blade/tuft meshes; each instance hash-picks one
   → a more natural, less uniform meadow.
3. **LOD / render distance** — **view-aware**: full blades near, cheaper/fewer
   far, none beyond X in first-person; orbit keeps all (the diorama). Filter
   the instance set by camera distance (per-chunk instance groups).
4. **Tree instancing** — same pattern for trees: X pre-made tree meshes
   (VoxModel-based) instanced at the generator's tree positions with variation.
   Removes trees from the terrain mesh → discrete objects.
5. **Tree collision** *(Stage 8)* — discrete tree instances get colliders
   (cylinder/capsule for the trunk) so the first-person walker can't pass
   through them (currently does).
6. **Water wading** *(Stage 8)* — `GroundHeights` / first-person terrain-follow
   currently treats the water *surface* as standable ("walk on water like
   Jesus"); make the walker wade INTO water (sink to a wade depth / the bed),
   not stand on top.

**Wind sway** (the Stage-6 wind-blown-grass item) folds into steps 1–2 as
per-instance vertex-shader animation.

### Stage 7 — Lighting upgrades  *(V1, V2, V15)*
Start with what Bevy gives (PBR + cascaded shadows, already in use). Then
selectively adopt: baked/greedy AO from occupancy (already partly there),
optional **screen-space or voxel-cone GI** as a research spike. The full
stochastic-GI + temporal-accumulation approach (V1/V2/V15) is really a
ray-marched feature — note it, don't force it into the mesh pipeline.

### Stage 8 — Interactivity: editing, physics, cellular automata  *(V2, V10)*
Localized edits (chunking makes this cheap). Falling-sand / water via
**cellular automata** with **ping-pong buffers + atomic CAS** for correctness
(V10) — can run CPU-side in `voxel-core` first, GPU-compute later. Destruction
via **flood-fill** attachment checks (V2). Physics: integrate a rigid-body lib
(Bevy has `avian`/`rapier`) rather than the custom voxel narrow-phase (V2) at
first.

### Stage 9 — Streaming / big worlds  *(V13)*
If worlds outgrow one island: chunk streaming with an LRU cache. Start CPU-side
(simple, in `voxel-core`); the GPU-managed LRU (V13) is an optimization for
later. Mesh-buffer **pooling** (research-doc §4) lands here.

### Stage 10 — Spatial-audio integration  *(atrium synergy)*
Wire the atrium HRTF/binaural engine to the voxel world: sources placed in
voxel space, listener = camera. This is where the two projects meet. Reuse the
`behavior` ECS (IntensityLfo/OrbitMotion) to drive ambient sources (wind/water)
positionally. Desktop headphones first — this *is* the VR audio path.

### Stage 11 — VR on Quest 3  *(deferred capstone)*
Only after desktop is solid. `bevy_mod_openxr` for stereo rendering + head
pose; feed head pose to the atrium listener for head-tracked binaural audio.
**Delivery decision deferred** — likely PCVR streaming (render on a
Windows/Linux box; note macOS OpenXR is limited) vs. a stripped standalone
Android build leaning hard on Stage 5 LOD. Revisit when we get here.

## Performance status (2026-07-23)

Current state after the mesh-perf arc + fragment trims — the engine is in good
shape; the levers below are **deferred, to pull only if FPS ever needs it.**

**Landed & confirmed:**
- Greedy meshing with shader-side AO — **−47% vertices** (terrain −58%), cuts
  main + reflection + shadow passes. Big confirmed frame-time drop.
- Fog sea render cost **~⅓** (raymarch steps 24→12, `fbm` octaves 3→2) — look
  identical (density conserved via `sigma ∝ step_length`; dither hides
  sampling). Fog was the confirmed fill-bound hog.
- Sky cloud march **~30%** cheaper (steps 14→10 + dithered start); runs for
  both the main and reflection views, so it counts double.

**Deferred perf levers (ranked by remaining payoff for *this* 125 m diorama):**
1. **Half-res fog pass** — render the soft fog sea to a half-res target and
   upsample (~4× on top of the ⅓ already done). Biggest remaining win.
   Architectural: separate render target + composite. Fog is soft/low-freq so
   half-res is nearly invisible.
2. **Reflection per-camera trims** — give the reflection (wavy, half-res
   mirror) a cheap sky (gradient, no cloud march) and/or no shadows. Needs the
   sky/light shaders to know which camera they're rendering for. Medium effort,
   smaller payoff (reflection is already dynamic-res + off past 120 m).
3. **Slim vertex formats** — pack 40 B → ~20 B/vertex (Unorm8x4 color + packed
   normal). Needs a custom vertex shader reproducing Bevy's PBR vertex path
   *including* `VISIBILITY_RANGE_DITHER` (which LOD depends on). Look-safe but
   fiddly + uncertain gain on a fill-bound machine — do only if we become
   vertex-bound.
4. **LOD** (Stage 5) — low ROI here (diorama viewed in full); real value for
   large/streamed worlds and an eventual Quest standalone build.

Live `P`-overlay levers (MSAA / reflections / DoF / bloom) remain the way to
identify the top cost per machine before pulling any deferred lever.

## Ray-marched renderer: a toggleable second backend (build later)
The Teardown-family renderer (V1, V2, V8, V10, V12, V17, V19) — GPU DDA
(Amanatides & Woo), brick maps / octrees / SVO-DAG for empty-space skipping,
distance-field / depth-pre-pass acceleration, per-voxel GI.

**Design intent: this is a swappable render backend, not a rewrite.** Because
`voxel-core` is renderer-agnostic (it only stores/queries voxels), both the
mesh path and a ray-march path consume the *same* voxel data:
- **Mesh backend** — meshes chunks → Bevy entities (Stages 1–5). *Current.*
- **Ray-march backend** — uploads voxel data to a GPU 3D texture / brick map
  and traces per pixel (custom wgpu render node hosted in Bevy).

Put both behind a `VoxelRenderer` trait, selected by a runtime toggle or a
Cargo feature. **The toggle is trivial; building the second backend is the
real work** — it's its own GPU pipeline, acceleration structure, and lighting.

**Caveat — it's a separate visual world, not a layer.** The mesh path uses the
Bevy PBR / shadows / planar-reflection stack you've already built; the
ray-march path reimplements its own lighting (GI/shadows). "Toggle on" switches
*into* the ray-traced look; it doesn't add ray-traced effects on top of the
mesh scene.

**What it costs today: almost nothing** — just keep `voxel-core` free of
mesh-specific assumptions (good hygiene anyway). Buys true GI + fine per-voxel
destruction later; from-scratch wgpu pipeline, VR-hostile on mobile GPUs.
**Don't build until Stages 1–8 prove the mesh path.**

## VR-readiness constraints to preserve NOW (even while deferred)
- Keep `voxel-core` fully headless (no Bevy, no window) — VR just adds a host.
- Watch the frame budget: VR wants ~2× the pixels at 90 Hz. If desktop FP fps
  is comfortably >120 after Stage 2, there's headroom.
- Keep the atrium listener/source API camera-driven so swapping the camera for
  an HMD pose is a one-line change (Stage 10 sets this up).
