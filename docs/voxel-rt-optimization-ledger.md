# voxel-rt — Optimization Ledger

**The scoreboard.** One row per optimization idea that has ever been on the table
for this engine — from `voxel-sandbox`'s mesh era, from the Shadertoy technique
bank, from the xima dossier, from devlog sources — with a status and, where it
was measured, the number that decided it.

Status vocabulary:

- ✅ **USED** — shipped and on by default in voxel-rt today.
- 🎚️ **LEVER** — implemented, measured, shipped **off**; kept runnable because an
  M3 Max loss can be a Quest win (Pascal's variant/lever hygiene rule).
- 🔜 **OPEN** — not built; real candidate, tagged with its experiment slot.
- ❌ **DEAD** — cannot apply here, or measured and closed. Reason given.

Numbers are M3 Max @ 2560×1440 from the headless harness
(`docs/voxel-rt-bench.md`). Companion docs: `voxel-rt-plan.md` (the ladder),
`voxel-rt-technique-bank.md` (Shadertoy menu, T-numbers),
`voxel-optimization-candidates.md` (the sandbox/mesh-era list this ledger
triages).

### The one principle that predicts these verdicts

**Direct light is high-frequency and needs accurate visibility; indirect light is
smooth gradation, which masks visibility error.** Published result — Yu, Cox, Kim,
Ritschel, Grosch, Dachsbacher & Kautz, *Perceptual influence of approximate
visibility in indirect illumination*, ACM TAP 6(4):24, 2009; it is the stated
motivation behind Imperfect Shadow Maps and is cited by VGI (2.18) for exactly
this purpose.

Worth writing down because it retroactively explains two verdicts we reached
*empirically and separately*: the per-brick distance field was unusable for
penumbrae (2.11) while the same order of geometric coarseness is invisible in the
0.5 m GI volume (2.13). That looked like two unrelated results. It is one
principle — the lattice landed in the **direct** term, where nothing masks it.

Operationally: coarseness is cheap to spend on indirect terms and expensive to
spend on direct ones. Check which side a proposed approximation lands on *before*
building it, rather than discovering it per lever.

---

## 1. Traversal & world data — the DDA hot loop

| # | Technique | Status | Evidence / note |
|---|---|---|---|
| 1.1 | Two-level sparse brickmap (8³ bricks, empty bricks skipped) | ✅ USED | S1. 71,941 occupied bricks, seed-1 island |
| 1.2 | **Chebyshev distance-field skip** (bindings 9/10) | ✅ USED | S2 headline win, **17–27% under baseline**. Its byte doubles as the occupancy test |
| 1.3 | Global-max sky-out for upward rays | ✅ USED | Cheap, exact |
| 1.4 | Per-XZ column max-brick-Y | ✅ USED | Feeds 1.3; E4 reuses it as the sky test |
| 1.5 | Compile-time const folding of traversal levers | ✅ USED | E1c: the folding *is* the S2 win; measured, kept as consts |
| 1.6 | Column fast-forward / descend fast-forward | 🎚️ LEVER | **+9–17% if re-enabled** — superseded by 1.2 in all directions. Retry on Quest |
| 1.7 | Specialized any-hit shadow loop | 🎚️ LEVER | Lost **1–3%** to plain `trace()` in three separate rounds |
| 1.8 | Brick bit-grid | 🎚️ LEVER | Redundant next to the distance byte; its data is read by 2.6. Retry where caches are small |
| 1.9 | **Brick-grid mip pyramid** (distant rays step coarser) | 🔜 OPEN | The one *real* ray-tracer form of sandbox candidate C. Distinct from 2.7, which lost. No slot yet |
| 1.10 | Frustum culling | ❌ DEAD | Per-pixel rays are perfect culling by construction — nothing to cull |
| 1.11 | **Workgroup-cooperative brick traversal** (shared-memory prefetch) | 🔜 OPEN | Untested, no number. Adjacent pixels' rays walk near-identical brick sequences, so a workgroup could fetch each brick once into shared memory instead of once per lane. This is the ray-tracer form of sandbox's "group work by coherence" (5.10's motivation, not its implementation). Bench before believing |

## 2. Lighting, shading & occlusion

| # | Technique | Status | Evidence / note |
|---|---|---|---|
| 2.1 | Sun shadow ray, integer-reconstructed origins (no acne) | ✅ USED | S2 |
| 2.2 | Hemisphere ambient, linear-space light + Reinhard | ✅ USED | S2 |
| 2.3 | **Analytic corner AO** (8 occupancy bits, bilinear over face-local UV) | ✅ USED | E1b winner, T7. **+0.25–0.31 ms** vs RT-AO's +4.2–8.2, 82% of its coverage, *noiseless*. Direct port of sandbox's baked-AO lesson |
| 2.4 | Hard shadows | ✅ USED | Also suits the art direction — crisp voxel shadows are the Voxile look |
| 2.5 | Ray-traced AO (2 rays / 8 voxels / cosine / falloff) | 🎚️ LEVER | E1 verdict. Beautiful tier only, kept for its *reach*; +4.2–8.2 ms |
| 2.6 | AO brick-neighbourhood early-out | 🎚️ LEVER | **Fires 0% of the time** on terrain — byte-identical output |
| 2.7 | AO distance fade / LOD | 🎚️ LEVER | **0.6–2.9%** at ground level; AO cost is dominated by near pixels. Potato ships it at 15→30 m |
| 2.8 | Sun-aware ray budget | 🎚️ LEVER | **≤7.5%**, and it puts the 1-ray crosshatch on exactly the bright flat ground that shows it |
| 2.9 | Analytic AO, 3×3×3 / 26-neighbour form | 🎚️ LEVER | 5× the cost, broad over-darkening (68–82% coverage), per-voxel flat facets |
| 2.10 | Half-res AO | ❌ DEAD | Not separable without a G-buffer + bilateral pass → became 2.12 |
| 2.11 | **Soft shadows from the distance field** (T1) | ❌ DEAD (revivable) | Free (+0.10–0.35 ms) but the per-*brick* field stamps a 1 m lattice + sun streaks at **every** penumbra scale (k = 4/16/64/115 swept). Both refinements measured, artifact survives both. Confirmed broken in-app. Revival order: trilinear clearance interpolation first, then voxel-level clearance (≈37 MB) |
| 2.12 | SSAO + depth/normal G-buffer (T8) | 🔜 OPEN | Backlog **B12**, deferred out of E1b. Revisit after E4 once CAGI's own occlusion is measured |
| 2.13 | **CAGI** integer light volume, ping-pong CA | ✅ USED | **E4 shipped.** RGB 10:10:10 in one u32, 0.5 m cells, **33 MB**, 6-neighbour integer diffusion. **+0.40–0.51 ms** sampling + **0.92–1.52 ms** CA per frame, against 2.25–3.55 ms *per ray* for a per-pixel gather. Contract honoured: `indirect = CAGI_sample * AO`. CPU cross-check: 0 mismatches, bit-identical re-floods |
| 2.13a | CAGI rule: 6-neighbour diffusion vs max-decrement flood | ✅ USED (diffusion) | E4 A/B: **identical cost** (both 6 loads, pass is bandwidth-bound), 66% of the frame differs at mean 8.8/255 — the flood reads flatter/brighter in shade. Free look win |
| 2.13b | CAGI rule: 26-neighbour diffusion (isotropy fix) | 🎚️ LEVER | **2.1–2.7× the cost for a mean 0.5/255** on sky-lit terrain: transport distances are 1–3 cells, too short for front shape to show. Reach for it at **E5** (point lights) |
| 2.13c | CAGI sky test from the per-column max-brick-Y buffer (1.4) | ✅ USED | **Free** — no ray at all; the exact upward trace costs **+33–53%** of the CA pass and disagrees on 33% of the frame at mean 2.1/255 (1 m column quantization near trunks). Kept as a lever for dense canopy |
| 2.13d | CAGI sun-source cache (bit 30 caches the shadow-ray RESULT) | ✅ USED | **−10 to −19%** of the CA pass at **byte-identical output**. Caching the cell's *value* instead was measured as a defect (26% of the frame, mean 0.6/255, max 38 too dark) and reverted |
| 2.13e | CAGI trilinear sampling with solid-tap rejection | ✅ USED | **+0.28–0.35 ms** over nearest, which otherwise stamps flat 0.5 m patches over 36% of the frame (max delta 76). Nearest kept as the Quest lever |
| 2.13f | CAGI at 0.25 m cells | ❌ DEAD | **258 MB** and 5.8–7.9 ms per frame — 6× the shipped tier for a mean 7.8/255 change. 1 m cells (4.3 MB, 5.8× cheaper) are the Quest rung instead |
| 2.14 | Fake bounce light — opposite-sun × albedo tint (T4) | 🔜 OPEN | The cheap-GI tier *below* CAGI, ~free. One registry row. Now has a target to beat: E4's real volume costs 1.4–2.0 ms per frame all-in at the Balanced tier, and Potato ships with GI **off**, which is exactly the slot T4 would fill |
| 2.15 | Bent normals | 🔜 OPEN | Backlog B9. Kept as a cheap Quest off-lever from E1 |
| 2.16 | Temporal accumulation / denoisers | ❌ DEAD | Noiselessness is the engine's stated identity — non-goal |
| 2.17 | **Directional miss radiance** (VGI I3D'11 §5.1 / Fig. 7-C) | ✅ USED (Beautiful only) | **E1d.** An escaping AO ray samples the hemisphere lobes in ITS OWN direction, so ambient becomes a visibility-weighted environment integral, not a flat constant × a scalar. **+0.18–0.41%** (noise) on the rays it reuses. Coverage 72.5% at max delta 116 vs baseline RT-AO's 34.1% at 55. Needs `AO_MODE = rays`, so Beautiful is the only tier it can ship on. **Catch:** ambient becomes Monte Carlo, so the 2-ray crosshatch lands in ambient *colour* → grain in dark foreground; wants 4 rays (+6.8 ms) or 2.12 |
| 2.17a | Miss radiance sampled from the raw sky function | ❌ DEAD | Luminance-normalized so the level matched exactly, and it still **turned shadowed grass teal and rock purple**: the sky constants are emitted radiance through inverse Reinhard, normalized zenith ≈ (0.19, 0.73, 6.03), unusable as a tint. Sampling `ambient_light` instead needs no calibration constant. Do not retry without a chroma-desaturation knob |
| 2.18 | VGI's per-pixel hemisphere gather + RSM back-projection | ❌ DEAD | The *architecture* of the same paper, rejected on E1's number: 2.25–3.55 ms per marginal full-res short ray. Their own figures confirm it — 20 dirs/fragment cost 13.6 ms at ¼×¼ + 7.7 ms upsample, **123 ms at full res** (GTX 295). This is the quantitative case for 2.13 (CAGI). Salvaged from it: 2.17, and the ε back-projection idea parked for **E5** (a lantern's RSM is tiny → zero-ray emissive fill) |

## 3. Memory

| # | Technique | Status | Evidence / note |
|---|---|---|---|
| 3.1 | Bit-packed occupancy (16 u32 / brick) + byte materials (128 u32 / brick) | ✅ USED | 4.6 MB masks + 36.8 MB materials ≈ **41.4 MB** at level 1. This IS sandbox's vertex-packing lesson, applied to voxel buffers instead of vertices: occupancy at 1 bit/voxel, skip distance at 1 byte/brick |
| 3.5 | **Per-brick local material palette** | 🔜 OPEN | Materials are the dominant consumer — 36.8 MB, **89% of the total** — at 1 byte/voxel for only **24 materials** (5 bits of payload in an 8-bit slot). Most bricks hold 2–4 distinct materials, so a 2-bit local index + small per-brick table should cut it ~3–4×. Quest-shaped (E9), where 41 MB stops being "nothing" |
| 3.2 | Vertical clamp of the CAGI volume to occupied height + sky margin | ✅ USED | E4 — everything above is open sky by definition; allocating it is paying to store a constant. **44 of 64 cell rows = −31%** on every volume buffer (33 MB instead of 48) |
| 3.3 | Interior-voxel culling | 🔜 OPEN (deferred) | **Memory only, zero traversal speed** — rays terminate on the shell. ~20–30 MB of 41.4 MB back. Blocked because the brickmap doubles as the *acoustic* occupancy structure: audio occlusion through a hill must still see the hill. Revisit at Quest (E9) |
| 3.4 | Voxel-level clearance field | 🔜 OPEN | ≈37 MB. Would unblock 2.11. **E2 did not add it**: the brick-level field is what the edit path now repairs incrementally, and a voxel-level one would multiply that work by 512 per edit as well as costing the memory. Re-open at E3/E9 with the incremental-update cost in the estimate |
| 3.6 | **Edit headroom in the level-1 arrays** | ✅ USED | E2, see 4.7c. 2.4 MB per side to keep brick materialization a word patch |

## 4. Threading, authority & generation — the GPU-first question

| # | Technique | Status | Evidence / note |
|---|---|---|---|
| 4.1 | World is a **pure function of position** (no disk, no region files) | ✅ USED | Inherited from sandbox's `StreamedSource`. The precondition that makes 4.4 possible at all |
| 4.2 | **World thread** (builds/edits off-frame, owned deltas out) | ✅ USED | **E2 winner.** The frame thread's edit cost is 0.000 ms idle / 0.065 ms median / 0.123 ms max at 4 edits per frame. The argument is the *worst* frame, not the median: a full clearance rebuild is a **33.3 ms hitch inline vs 1.4 ms + 8 frames of latency threaded**. Inline stays an off-lever (better latency, Quest single-core tier) |
| 4.2a | `Arc<Brickmap>` snapshot swap as the publish mechanism | ❌ DEAD (revivable) | **4.9 ms deep copy of 46.4 MB per published edit** against a **14-byte** delta. Replaced by one brickmap behind an `RwLock` + owned `WorldDelta`s: the render thread never locks, readers hold the lock for the microseconds of a ray. Revive only where a *stable* view is needed (save / network frame) |
| 4.2b | Rayon inside the world thread | 🔜 OPEN (unneeded so far) | Every multi-ms CPU job (62 ms build, 50 ms attribute rebuild, 31 ms full clearance rebuild) became *off-frame* rather than *fast*, which was the requirement. Reach for it only if a future job is latency-critical |
| 4.3 | GPU-authoritative bricks + CPU occupancy-only mirror via delta readback | ❌ DEAD | **E2 variant C, rejected on latency, not bandwidth: a GPU→CPU readback costs 1.29 ms round trip REGARDLESS OF SIZE** (64 B and 43.8 MB both), so "read back only the delta" buys nothing, and non-blocking mapping converts the cost into **7–10 submit/poll cycles of staleness**. E8's resolver needs the mirror exact, so C pays for two copies plus synchronization where B pays for one that is authoritative. GPU-authoritative *derived* data with only GPU consumers (E3, E4) is untouched by this verdict |
| 4.4 | GPU world generation in compute | 🔜 OPEN | **E3.** A/B/C/D: CPU baseline vs WGSL column stack vs VoxelChain subdivision+CA vs cave variants |
| 4.5 | Analytic-derivative value noise + FBM, small-triple rotations (T5) | 🔜 OPEN | E3 — ideal formulation for a compute generator, no sin/cos, no finite differencing |
| 4.6 | Band-pass terrain at synthesis time (T6) | 🔜 OPEN | E3 — omit the FBM octaves in the vegetation scale range |
| 4.7 | `Brickmap::set_voxel` edit API | ✅ USED | E2. One call repairs occupancy bits, material bytes, brick alloc/free, the clearance field, both height maxima and E4's cell attributes, and reports the touched word ranges. **0.3 µs and 14 bytes for a typical edit**; 2.8 µs / 1689 B when a brick materializes. Unit-tested against brute-force recomputes of every derived structure, including "every changed word is in the delta" |
| 4.7a | **Incremental chebyshev clearance update** (the add/remove asymmetry) | ✅ USED | E2. Adding solid only shrinks clearance → exact chebyshev shell walk with an exact early-out (~106 cells per new brick). Removing it can grow clearance arbitrarily far → **bounded local recompute, radius 8, 258 µs**, provably never an overestimate and never low by more than the freed brick's own new clearance *independent of the radius* (= exact for any edit into terrain). Full rebuild: **31.5 ms + 500 KB**, kept as the correctness reference off-lever |
| 4.7b | Delta uploads (`COPY_DST` word patches instead of whole buffers) | ✅ USED | E2. **14 B/edit** for a wall, 1689 B when a brick materializes, 592 B of that being the zeroed slot words. Ranges are coalesced with a 64-word gap tolerance so a dirty box is a handful of `write_buffer` calls |
| 4.7c | **Brick-slot free list + edit headroom** | ✅ USED | E2. 4096 spare slots = **2.4 MB per side (5.2% of the brickmap)**, so materializing a brick patches words instead of reallocating 46 MB. Fixed-size slots ⇒ **no fragmentation to manage**; freed slots are reused LIFO before headroom, so dig-and-rebuild consumes none |
| 4.7d | Incremental CAGI cell-attribute update | ✅ USED | E2. A cell never straddles a brick and its attribute depends only on its own voxels, so an edit invalidates exactly ONE cell: E4's **48 ms** full attribute build collapses to ≤ 512 bit reads and a 4-byte upload. Pinned against the full build cell by cell |
| 4.7e | Off-frame CAGI attribute rebuild on a GI resolution switch | ✅ USED | E2 fixed E4's noted **~50 ms frame hitch**: the volume is allocated with zeroed attributes (valid — every cell reads empty), the world thread builds the real ones, and the flood starts when they land |
| 4.10 | **CPU voxel DDA for picking + audio** (`voxel_dda`) | ✅ USED | E2, and the direct seed of E8's `VoxelDdaResolver`: `&Brickmap` in, meters in/out, hit = voxel + face voxel + integer normal + material. Accelerated by the same chebyshev field as the shader → **0.94 µs per occlusion ray, 0.96 µs per reflection cast** (4096 rays in 3.9 ms). Tested against a fine-step brute-force walk |
| 4.11 | **CPU swept-box character collision** (`character.rs`, E2b) | ✅ USED | Second consumer of the same seam as 4.10, and the same verdict shape: `&Brickmap` in, world meters out, no renderer type. **0.62–0.96 µs per movement step, 4.04 µs through a 1 s hitch, 6.17 µs to enter walk mode** = 0.01–0.05% of an 8 ms frame, so no fixed timestep, no thread and no amortization is needed. Per-axis (X/Y/Z) sweeps test **every voxel layer the leading face crosses**, which is what makes the anti-tunneling guarantee hold independently of the substep clamp |
| 4.11a | Early-out in the body's box test | ✅ USED (and it inverts the intuition) | `any_blocking_voxel` returns on the first blocking voxel, so **open air is the expensive case and dense terrain the cheap one** — "sprint into a rise" measures *cheaper* than plain sprinting because the auto-step's extra sweeps hit geometry that answers immediately. Sizing consequence for B4/B6: a query's cost tracks the EMPTY volume it scans, not the occupied one |
| 4.11b | Heightfield terrain-following instead of a swept box (the sandbox controller) | ❌ DEAD here | voxel-sandbox's controller is a baked per-column heightfield + a 2D trunk mask: no body volume, unbounded step-up, no ceilings, no walls. It cannot see an *edited* world (E2's whole point), it cannot represent an overhang or a cave, and a heightfield of a 1000x1000 world is another array to repair per edit. The swept box reads the authoritative brickmap directly and costs under a microsecond, so the cheaper structure buys nothing |
| 4.8 | **GPU physics queries** (floating-voxel fall = connected-component flood) | 🔜 OPEN | Backlog **B6**. Structurally identical to CAGI's flood — same ping-pong volume, same neighbour kernel. Sizing caution: a ~63 ms GPU query is still ~8 frames at our 8 ms budget, so it must be async/chunked, never synchronous-per-frame |
| 4.9 | Async readback | ✅ USED (timers) / ❌ DEAD for per-frame data | GPU pass timers ship today. E2 priced the data case: **1.29 ms per round trip, size-independent**, and 7–10 submit/poll cycles of staleness if the CPU refuses to block. A sound-queue readback must therefore be amortized over many frames or dropped |

## 5. Sandbox mesh-era techniques — mostly dead by construction

Most of these optimize **triangle submission**: draw calls, vertex bandwidth,
the GPU vertex stage. voxel-rt has no meshes and no triangles (an explicit plan
non-goal), and its cost is ray steps + memory latency. They are not "rejected" —
there is no analogue to port. They remain live and correct in `voxel-sandbox`.

**Audit correction, 2026-07-30** (prompted by Pascal asking *why* each one is
dead — the right question to ask of a blanket verdict): two rows here were
misfiled. **Chunked meshes** and **vertex packing** are not dead, they are
*shipped under different names* (1.1 and 3.1) — the mesh implementation is
inapplicable but the technique landed. Re-deriving the rest per-item also
surfaced two open candidates that a blanket "no triangles" answer had hidden:
**1.11** (workgroup-cooperative traversal) and **3.5** (per-brick local material
palette). Blanket verdicts hide analogues; each row now carries its own reason.

| # | Technique | Status |
|---|---|---|
| 5.1 | 2D greedy merging (both axes) | ❌ DEAD — DDA derives the hit face from the crossed axis in O(1); a 1-voxel wall and a 1000-voxel wall cost the same. The *idea* (exploit contiguity) survives twice: merging **empty** space = the distance field (1.2), merging **solid** space = interior culling (3.3) |
| 5.2 | Hidden / interior **face** culling | ❌ DEAD — the ray terminates on first hit, so interior faces are never *visited*. Free by construction, no mechanism needed. Memory analogue is 3.3 |
| 5.3 | Mesh groups (Terrain / Cover / Canopy) | ❌ DEAD — exists to stop non-mergeable geometry poisoning a chunk's merge and to vary material/shadow flags. Nothing to poison; a material byte → palette lookup differentiates per voxel at zero structural cost |
| 5.4 | Chunked meshes, not per-voxel | ✅ **USED as 1.1** *(corrected 2026-07-30 — was misfiled here)*. The lesson is "amortize per-unit overhead over a spatial block"; the brickmap is exactly that. Only the mesh implementation is inapplicable |
| 5.5 | Vertex compression / packing (40 B → ~8 B per vertex) | ✅ **USED as 3.1** *(corrected 2026-07-30 — was misfiled here)*. "Pack per-element data as tight as it goes" is applied to the voxel buffers. Still has headroom → 3.5 |
| 5.6 | 16-bit index buffers | ❌ DEAD — no indices. *(Was already reverted in sandbox: mixed index formats split the batch set key)* |
| 5.7 | Tight AABBs for frustum culling | ❌ DEAD — see 1.10 |
| 5.8 | One shared material → bevy batching | ❌ DEAD — no bevy, no materials, one compute pipeline |
| 5.9 | Grass clump instancing | ❌ DEAD in this form — the voxel-rt analogue is domain-tiled SDF instancing (6.6) |
| 5.10 | Per-face-direction mesh split (6 meshes/chunk) | ❌ DEAD — no vertex shader to skip. Its *motivation* (group work by coherence) is alive as 1.11 |
| 5.11 | Octree / variable-size chunks | ❌ DEAD in this form — the analogue is 1.9 |
| 5.12 | Multi-draw indirect / SSBO chunk positions / `gl_DrawID` | ❌ DEAD — no draw calls at all |
| 5.13 | Hand-rolled buffer suballocation | ❌ DEAD — we own our buffers directly; no allocator to beat |
| 5.14 | Texture atlasing / bindless arrays / GPU texture compression | ❌ DEAD — no albedo textures anywhere; colour is a palette lookup |
| 5.15 | `NotShadowCaster` confetti + solid inner shadow shell | ❌ DEAD — no shadow-caster concept; shadows are rays |

**The one genuinely transferable lesson from this block** is not a technique but
a reframing: *anything that varies per unit fragments the fast path.* In sandbox
that was the batch set key; here it is pipeline permutations — which is why E1c
precompiles the 3 distinct preset pipelines (≈4.0 ms at startup, 67 µs to
re-prewarm) instead of branching at runtime.

## 6. Look pass — cheap beauty (mostly E7)

| # | Technique | Status |
|---|---|---|
| 6.1 | Three-channel exponential atmosphere (T2) | 🔜 OPEN — E7. Biggest "expensive render" impression per ms in the whole IQ video |
| 6.2 | Foliage normal blending toward terrain normal (T3) | 🔜 OPEN — E7 / B10. Fixes canopy confetti; **may port *back* to sandbox** |
| 6.3 | Sun glare, vignette, smoothstep contrast, per-channel gamma, rim highlights | 🔜 OPEN — E7 grab bag, all near-free |
| 6.4 | Rounded voxel edges + silhouette AA | 🔜 OPEN — E7 |
| 6.5 | Volumetric clouds by density accumulation; low-pass the *gradient* not the shape; cloud shadows via one extra plane intersection | 🔜 OPEN — E7 / B-slot |
| 6.6 | Domain-tiled SDF instancing (`floor(p/tile)` + hash) | 🔜 OPEN — B5 / B10 |
| 6.7 | Triplanar mapping | 🔜 OPEN — only if we ever want surface detail on voxel faces |
| 6.8 | HDR accumulation + auto exposure | 🔜 OPEN — **E7, and first in that experiment**: emissives + dark caves = huge luminance range |
| 6.9 | Fresnel reflect / Snell refract on water, absorption by path length | 🔜 OPEN — E6 |
| 6.10 | Isometric projection | ❌ DEAD — we target VR perspective |
| 6.11 | Full SDF-raymarched terrain as the *renderer* | ❌ DEAD — we are voxel-authoritative. SDF/noise math is welcome as a *generator* (4.5) |

## 7. Process & measurement — the lessons that transferred wholesale

| # | Lesson | Status |
|---|---|---|
| 7.1 | Dump numbers, never eyeball the overlay at an unknown window size | ✅ USED — inherited from sandbox's `geometry_census.rs` + `VOXEL_STATS=1`; now the plan's benchmark rule (±2% = noise) |
| 7.2 | Perf levers as a live panel | ✅ USED — sandbox's `RenderQuality` P-overlay → E1c's **registry-driven** Quality panel, upgraded so the measured verdict is the hover text |
| 7.3 | One lever registry drives bench columns + overlay rows + pinning tests | ✅ USED — E1c, extended by E4 to **30 levers / 5 subsystems** (a settings field without a row still stops the tests compiling) |
| 7.4 | Presets as a sparse override table, not if/else | ✅ USED — E4 added GI per tier by touching only the lines that differ. Frame totals (shading + CA): Potato 2.5–3.9 / Quest 3.6–5.5 / Balanced 6.0–8.8 / Beautiful 10.8–18.1 ms |
| 7.9 | **Report distributions, not medians, when the question is hitches** | ✅ USED — E2's storm section prints median / p99 / **max** per frame, which is the only reason the 33 ms inline hitch is visible at all (its median is 0.14 ms). Medians answer "is it fast"; maxima answer "does it stutter" |
| 7.10 | **Measure the alternative you are about to reject** | ✅ USED — E2 measured the plan's own snapshot-swap sketch (4.9 ms/edit) and the GPU-authority readback (1.29 ms, size-independent) instead of arguing about them. Both verdicts are now numbers a future reader can re-check |
| 7.8 | **CPU reference implementation cross-checked against the GPU on real data** | ✅ USED — E4: `cagi::propagate_reference` predicts every propagating cell of all three rules, 0 mismatches over 181,928 cells, plus a bit-identical re-flood check. The way to prove "deterministic and noiseless" instead of asserting it |
| 7.5 | Pixel-diff gates guard correctness alongside timing | ✅ USED — Stage 2 gate reads 19/0 |
| 7.6 | "The cost isn't where the shape suggests — measure the boundary" | ✅ USED — from sandbox's 44s→67ms generator trap and *streaming is entity-bound, not geometry-bound* (36→112 fps). Sizing input for E2/B8 |
| 7.7 | Keep measured losers runnable, never delete or inline-clutter | ✅ USED — the 🎚️ rows above are the whole point |

---

## Scoreboard

- ✅ **USED: 44** — traversal core, analytic AO, memory packing, the E4 CAGI
  volume and its five sub-decisions, **E2's whole edit pipeline (the world thread,
  `set_voxel` and its five sub-decisions, the CPU DDA)**, **E2b's swept-box
  character collision and its early-out finding**, and the measurement/lever
  discipline. (Two more are *also* used but live in section 5 under their
  mesh-era names, 5.4 and 5.5.)
- 🎚️ **LEVER: 10** — all measured, all with numbers, all re-run on Quest at E9;
  E4 added the 26-neighbour stencil (worth 2.7× only once a point light exists)
  and nearest volume sampling. E2's inline-authority and full-clearance-rebuild
  levers live in the registry with the same discipline.
- 🔜 **OPEN: 22** — dominated by E3 (GPU generation) and E7 (the look pass, which
  is cheap and almost entirely unbuilt).
- ❌ **DEAD: 24** — 13 mesh-era techniques with no ray-tracer analogue, plus 11
  measured or scope closures (E4 closed 0.25 m cells; **E2 closed GPU authority,
  snapshot swapping and per-frame data readback**; **E2b closed the sandbox's
  heightfield terrain-follower** — it cannot see an edited world).

**Biggest open value, in order:** 4.4 (GPU world generation — E2 settled the
authority question *around* it: generation may run on the GPU, but its output has
to reach a CPU mirror that stays authoritative, and the readback price is now
known), then the E7 look pass, which is the cheapest beauty-per-millisecond block
on the page and has not been touched, then 2.12/2.14 — both re-priced by E4: SSAO
has to beat a light volume that already owns the medium-scale band, and T4 fake
bounce has a concrete slot (the Potato tier, which ships with GI off).
