# xima's Voxel Engine — Research Dossier

*Compiled by Pascal (2026-07-30) from the channel's latest 14 videos, xima's X
posts, GitHub (maierfelix / VoxelChain), and Hacker News history. The newer
engine is closed-source; VoxelChain repositories document an earlier generation
and are used only as historical evidence. This is the reference document for
voxel-rt design decisions — see `voxel-rt-plan.md`.*

## Main conclusion

The latest public engine is a browser-based, almost entirely GPU-resident voxel
engine combining shader software ray tracing, integer cellular-automata light
transport, GPU-driven indirect entity processing, on-demand entity voxelization,
GPU particles and collision, and procedural voxel-world generation. It is a
hybrid:

- **Ray traversal** handles directional visibility: scene rendering, ambient
  occlusion, glass, entity intersections and acoustic visibility.
- **Cellular automata** handle local, diffuse, iterative processes: GI
  propagation and eventually fluids/material simulation.
- **GPU work queues and indirect passes** handle dynamic entities, animation,
  spawning, particles, collision and memory management.
- **Procedural generation** produces terrain, caves, plants and trees directly
  in voxel form.

WebGPU as the current API is a strong inference (earlier gen was WebGL2 + CPU
WASM simulation; xima explicitly planned the GPU migration; his other public
work is WebGPU path tracing and Chromium WebGPU ray-tracing experiments) but no
primary source states it outright.

## The latest 14 videos — techniques per video (reverse chronological)

1. **New Caves in my Voxel Engine** — terrain/cave generator reworked; no
   algorithm disclosed.
2. **GPU-Driven Voxel Engine: Transparency and Underwater** — transparent-block
   rendering completed before integrating the CA fluid simulation; transparent
   materials use ray-traced reflection and refraction. The description names the
   target materials: **water, oil, clouds and honey** — so transparency is a
   per-material CLASS (its own extinction, tint and index of refraction), not a
   water special case, and water is merely its first row. Our `material.rs`
   `opacity` / `transmittance` columns are the equivalent seam; IOR is the field
   still missing from that table.
3. **GPU-Driven Voxel Engine: Random Exploration and New Features** — fully
   shader-based engine; inventory/items; ray-traced ambient occlusion; fully
   CA-based GI; environment-sensitive cave-horror sound; experimental
   infinite-world support; planned compressible CA fluid simulation (~2 years of
   work, pre-integration).
4. **Butterfly Benchmark** — ~65,000 butterflies animated, simulated and
   rendered on the GPU every frame (dynamic-object throughput test).
5. **Entity Manhunt** — GPU-driven entities continually spawn and pursue the
   player; navigation technique undisclosed.
6. **Exploring a Forest Meadow Biome** — improved procedural terrain, new
   vegetation, glowing plants; vegetation wind response controlled by **sky
   occlusion**.
7. **Massive Engine Stress Test** — a few billion voxels and tens of thousands
   of entities; **no LODs; pure GPU software ray tracing; running in a
   browser**. ("No LODs" ≠ no spatial acceleration; "billions of voxels" may be
   logical address space, not resident VRAM records.)
8. **Entity GPU Voxel Splatting [TEASER]** — Blockbench entity renderer;
   screen-space voxel splatting; analytic OBB/SDF intersection + local DDA;
   entities voxelized on demand; animation, bones, spawning, updates and memory
   management via indirect GPU passes. (NOT Gaussian splatting.)
9. **Solving Global Illumination with Cellular Automata** — pure-integer CA for
   light propagation, diffusion, bouncing; rays retained for
   reflection/refraction.
10. **Ray tracing Sound (on the GPU)** — path-tracing infrastructure reused for
    sound; occlusion + reverb; environmental sound tied to the voxel world.
11. **Adding Particles and Entity Collision** — cascaded GPU particle system,
    millions of particles, world + particle-particle interaction,
    player/entity collision in shaders.
12. **Adding procedural trees** — procedural trees; vines from leaves; glowing
    berries; improved auto exposure/colour; **sub-voxel emission** (light
    sources smaller than a block).
13. **Exploring a procedurally generated Forest** — GPU world generation and
    simulation; lighting at that time "fully path traced".
14. **Adding Global Illumination to my Voxel Engine** — earlier real-time
    path-traced GI experiments (predates the CA move).

## GI generations

1. **VoxelChain era (2022)**: ray tracing + stochastic cone tracing
   (cache-friendly, low-noise).
2. **Path-traced GI** (videos 13–14): physically convincing, but multi-traversal
   cost, Monte Carlo noise, temporal accumulation, ghosting on change.
3. **Integer CA GI** (video 9 onward): propagation + diffusion + bouncing in
   pure integers; rays kept for directional phenomena.

Conceptual update (reconstruction, not published implementation):

```
L_{t+1}(x) = E(x) + Q( Σ_{n∈N(x)} T(n→x) · A(x) · L_t(n) )
```

E = emission, N = neighbourhood, T = transparency/blocking/attenuation,
A = material reflectance, Q = integer quantisation/clamping.

Why CA GI fits a dynamic voxel engine: predictable local memory access, uniform
work per cell, temporally stable (no Monte Carlo noise), emissive voxels feed
naturally, edits affect only nearby propagation, packs into integers, fixed
iterations/frame, maps to compute + **ping-pong buffers** (xima explicitly
preferred double buffering in VoxelChain: "more precise and less chaotic" than
in-place).

Known compromises (= our test checklist): multi-frame propagation latency,
grid-direction anisotropy, over-diffusion / "glowing walls", light leaks around
thin geometry, weak long-distance transport, loss of high-frequency directional
information. The hybrid mitigates: CA for diffuse only, rays for directional.

## AO — named, never explained (our inference, 2026-07-30)

"Ray-traced ambient occlusion" is listed as a feature; **no** ray count,
distance, direction distribution, resolution or temporal strategy is disclosed,
and his hardware/resolution are unknown, so no ms comparison is possible.

Two inferences that matter for voxel-rt:

1. **His AO sits on top of CAGI, not in place of it.** A flood-fill light volume
   already darkens crevices (light does not propagate into corners), so his AO
   rays are plausibly cheap fine-detail garnish over GI that already does the
   heavy occlusion. Our E1 measured AO as the *only* occlusion mechanism (no GI
   yet) — so its +5.8–8.1 ms partly reflects it doing CAGI's job. Consequence:
   after E4, AO's remaining role is small-scale contact detail → favours the
   cheap analytic variant (technique bank T7) over rays, and argues for keeping
   E1b's SSAO contender on hold until CAGI's contribution is known.
2. **Sky-occlusion field hypothesis.** His vegetation wind is driven by sky
   occlusion, so a sky-visibility field exists for non-shading reasons. If it is
   per-voxel/per-cell, the same field could also attenuate ambient sky light —
   one field, two consumers, zero per-pixel rays. Our benched "bent-up" AO
   variant is exactly this proxy (cheap, clean, misses lateral contact — correct
   behaviour for sky visibility rather than a defect). Ties directly to backlog
   B2 (wind by sky occlusion), where the same field would drive audio wind too.

## Entity GPU voxel splatting (future reference)

Blockbench models = bone hierarchies of boxes → animated OBBs. Probable
sequence: GPU entity update (state/animation/bones) → indirect work generation
→ screen-space splatting (candidate pixels/tiles) → analytic OBB/SDF
intersection (ray into local space, entry/exit interval) → local DDA inside the
box's voxel field → world/entity depth merge. Avoids: CPU animation, draw-call
generation, mesh rebuilds, re-voxelizing animated models into the world.
GPU-side memory management (allocator undisclosed; plausibly free-index stacks,
atomic counters, append/consume queues, compaction).

## Procedural generation

Confirmed: procedural terrain, reworked caves, forest-meadow biome, procedural
trees, vines from leaves, conditional glowing berries, wind-by-sky-occlusion,
GPU world generation. NOT established: any specific noise (Perlin/Simplex/
Worley), domain warping, erosion, WFC, L-systems.

Historical VoxelChain generator (public source): multiscale 3D subdivision
inspired by diamond-square, but instead of averaging corners it uses randomised
CA lookup rules — neighbourhood templates for cube centres/faces/edges/
boundaries, four non-zero states, random rule tables gated by a lambda
threshold, seed plane, halve subdivision width per level:
`counts = histogram(neighbour_states); new_state = rule_table[counts]`.
Volumetric fractal-like coherent structure without smooth scalar noise.

## Sound (video 10)

Geometric acoustics, not wave simulation. Confirmed: occlusion + reverb via GPU
rays, environment-sensitive sound (cave-horror by darkness/enclosure). No public
evidence for: HRTF processing, wave-equation simulation, diffraction,
frequency-dependent materials, physically generated impulse responses.
Plausible reverb inputs: bounce counts before escape, path lengths, sky-escape
fraction, enclosedness. (Note: an earlier video description claimed "sound
spatialization uses hrtf" — treat spatialization details as unconfirmed either
way. Atrium is far deeper than anything evidenced here.)

## Transparency, fluids, materials

Transparent voxels: RT reflection + transmission continuation through the voxel
field, colour/fog accumulated along travelled distance (plausible). Compressible
CA fluid simulation: ~2 years of work, upcoming at video 3 — cells likely carry
mass/pressure/density, not a water bit. VoxelChain material CA: parallel
neighbour updates, double-buffered, programmable behaviours (circuits, falling
material, water) — Noita-adjacent but generalized.

## Voxel scale and the two-tier grid (screenshot-measured, 2026-07-31)

Measured off two gameplay frames, not from any statement by the author.

A zoomed single block face (an ice/water block) resolves into a clean **8 x 8**
grid of independently shaded cells. The inventory is a Minecraft-style hotbar
with stack counts, i.e. the *placement* unit is the familiar 1 m block. So:

- **Build/edit grid: 1 m.** What the player places and breaks.
- **Render/sim grid: ~0.125 m.** 8 subdivisions per block edge.

That makes his voxel the same size as ours (`voxel_core::world::VOXEL_SIZE` =
0.125), and — more usefully — his 1 m build block is *exactly* our 8-voxel
brick (`voxel_rt::brickmap::BRICK_SIZE` = 8). The two engines agree on both
lattice constants. Any perf difference is therefore NOT a voxel-scale
difference; see the "no ms comparison is possible" caveat above, which stands.

**Everything is a voxel; some are transparent.** Foliage is not billboards,
alpha-tested quads, or a separate mesh path — grass blades are ~1 voxel wide
and several blocks tall, occupying cells in the same lattice as the stone, and
water/leaves are cells that transmit. This is the architecture
`docs/transparent-voxels-plan.md` (T1-T3) is already aimed at, and it raises
that plan's payoff: transparency classes are what collapse foliage from a
special case into ordinary voxels.

**Variant instancing (inference, unconfirmed).** A small set of grass/plant
variants appears to be held once in memory and *referenced* per placement,
rather than each block owning unique voxel data. The visible repetition across
the grass field is consistent with a palette of prefab 8^3 templates.

### What this changes for voxel-rt

Our level-0 array is already a `u32` brick pointer per 1 m cell
(`brick_indices`, 125 x 32 x 125 = 500k entries) — the indirection that
instancing needs **already exists**. What we do not do is *share* level-1 data:
every occupied brick gets its own unique 576 bytes (16 occupancy words + 128
material words). A fully-solid stone brick underground costs the same 576 bytes
as a hand-carved one, and a thousand identical ones cost it a thousand times.

Brick deduplication — many pointers into one shared slot, copy-on-write on
`set_voxel` — is the direct consequence of this observation. NOT started, not
scheduled, and it needs a measurement first (what fraction of occupied bricks
are byte-identical in a generated world) before it earns a stage.

## Historical voxel format (VoxelChain, public)

32-bit cell: 8-bit material id, 8-bit state, 3-bit animation index, 5-bit
rotation; separate 4-bit power + 1-bit signal flow value; GZIP world files.
Not confirmed for the new engine, but shows the style: packed integer state,
material ids, separate simulation channels, GPU-friendly words.

## Likely frame pipeline

GPU worldgen/streaming → GPU simulation (materials, entities, bones, particles,
collision, spawning, memory/work queues) → CA lighting (emission injection,
integer propagation, diffusion, bounce iterations) → world rendering (camera
rays, software traversal, material lookup, transparent continuation) → entity
rendering (indirect selection, splatting, OBB/SDF, local DDA, depth merge) →
additional ray queries (AO, reflection, refraction, sound occlusion,
environment sampling) → presentation (HDR, auto exposure, colour post).

## Confidence tiers

**High confidence:** browser-based; fully shader-driven; GPU software ray
tracing; no LOD in stress test; GPU worldgen + simulation; integer CA GI;
RT-AO; RT glass reflection/refraction; GPU acoustic rays (occlusion + reverb);
GPU entity update/animation/bones; indirect GPU passes; Blockbench entities;
voxel splatting; OBB/SDF + DDA entity intersection; GPU particles + collision;
procedural terrain/trees/vegetation; sub-voxel emission.

**Strong reconstruction:** WebGPU; compute-oriented architecture; ping-pong GI
buffers; GPU work queues/free lists; per-entity local voxel grids; shared world
occupancy for rendering + collision; acoustic rays aggregated into environment
parameters; layered terrain→caves→biome→vegetation.

**Medium confidence (second-hand only, see the intel-drop section):** 2-level
DDA traversal; ray-direction skew for grass; directional propagating CA for
sun/sky; separate LPV-like macrovoxel CA for point lights; voxel-face blurring
filter as the AO/reflection denoiser; per-frame entity voxelization into a
player-centric microvoxel grid; diamond-square + CA (BWerness) terrain with
CA-grown trees; no open world yet; survival/underwater gameplay direction;
multiplayer planned.

**Unknown:** memory layout; cave/biome/tree math; CA GI stencil + attenuation +
bit depth; temporal filtering; entity AI; particle neighbour structure;
collision solver; acoustic material model; fluid equations; confirmed API;
hardware/frame-time breakdown.

**Do NOT attribute without evidence:** greedy meshing, marching cubes, surface
nets, mesh shaders, SVO/DAG, NanoVDB, hardware RTX, ReSTIR, NeRF, Gaussian
splatting, L-systems, Perlin/Simplex/Worley caves, erosion, WFC biomes,
navmeshes, wave-based acoustics.

## Author comment threads (primary-source quotes — dated, possibly stale)

Direct statements from xima in YouTube replies, kept because they are primary
source rather than inference. **All from the early-CAGI video, ~2 years before
this dossier**, so they describe an older generation than the screenshots in
this file's other sections. Nothing here is a recommendation; treat each as a
dated data point to re-check if it ever becomes relevant.

- **"For the shadows I use regular shadow mapping."** So at that time the crisp
  sub-voxel shadow edges were a *separate high-frequency term* composited over
  low-frequency CA GI — not produced by the CA. He was ray tracing primaries
  while still rasterizing a shadow map. Whether that is still true is unknown;
  it is the kind of thing an engine drops once traversal is cheap.
  *Our position, measured independently:* shadow rays through the shared DDA
  core (`trace_shadow_visibility`), no shadow map — exact at pixel precision,
  and cheaper for us because traversal is already paid for. Same
  low-frequency-GI + high-frequency-visibility split, different mechanism.
- **"The CAGI is only ran at voxel scale, not sub-voxel scale (at least for
  now)."** Note the hedge. Ours is coarser still: 0.5 m cells = 4 voxels
  (33 MB, ~1.2 ms; 0.25 m measured at 258 MB and 6× cost). Suggests he hit a
  comparable memory wall. Current Voxile may be multi-resolution or cascaded —
  unknown.
- **Rasterization rejected:** *"even with LODs, greedy meshing etc I quickly
  reached too many fundamental performance limits for the amount of detail I was
  planning for. Compared to raster, ray tracing also simplifies a lot of things
  — it doesn't really need techniques like frustum or occlusion culling, as it's
  naturally built inside the algorithm."* The culling argument is the part worth
  remembering: it is a structural property of traversal, not a tuning result.

### Direction-free injection (the one architectural item here)

A commenter assumed CA sun/sky lighting would need "a massive amount of CA
instances" for directional light. It does not, and this is version-independent:

**The CA carries direction-free RGB irradiance. Direction dies at the injection
boundary and lives only in the ray-traced terms.** A candidate air cell fires
one shadow ray to answer a boolean "lit / not lit", then deposits only the
resulting diffuse bounce into the grid. So **cost scales per cell, not per
light** — sun, sky and emissives all deposit into the same volume, and adding a
light type is an injection rule, not another instance. Our `cagi.wgsl` already
works exactly this way (albedo-tinted, Lambert-weighted by solid neighbours),
which is why this is recorded as a confirmation rather than a finding.

Corollary worth remembering when emissives come up: the sun is injected almost
everywhere above terrain, so transport distances stay 1–3 cells and anisotropy
stays invisible. A *local* emitter in an enclosed space is the opposite case and
would be the first real test of transport reach and stencil isotropy.

### Gaps noticed while comparing against the screenshots

Neither is a plan item; listed so they are not re-discovered from scratch.

- **Volumetric light shafts / god rays** appear in his newer footage and are not
  described anywhere in this dossier. Mechanism unknown; a view-ray scatter term
  is the obvious guess and would share machinery with fog.
- **"Ray tracing subsumes frustum + occlusion culling"** is not written down as
  a rationale anywhere in our own docs, though voxel-rt has the property.

## Second-hand intel drop (2026-07-30, provenance unstated)

Relayed to Pascal as a summary of the current engine, not quoted from a video or
post. Treat as **medium confidence**: more specific than anything published, but
unverifiable. Where it collides with the reconstructions above, it is noted.
It resolves — or claims to resolve — five of the "Unknown" items.

- **Traversal: "most likely just 2-level DDA by the last iteration."** Matches
  voxel-rt's own brickmap DDA (brick grid + intra-brick), and matches the "no
  LODs" stress-test claim: two levels is an acceleration structure, not a detail
  hierarchy. Notably *not* SVO/DAG. If true, our structure and his are the same
  family, which makes his frame-time claims a fair (if unmeasured) yardstick.
- **Grass animation by skewing the ray direction on entering a grass block.**
  A pure shading-time trick: no geometry moves, no per-frame voxel writes, no
  simulation state — the ray bends, so the blade *appears* to bend, and the cost
  is a few ALU ops inside the traversal loop. Combines with the sky-occlusion
  wind driver (video 6): occlusion sets amplitude, the skew applies it.
  **Directly portable to voxel-rt** and far cheaper than our mesh-era
  vertex-displacement grass; it also has no equivalent in a rasterizer, so it is
  one of the clearer "ray tracing buys you something structural" cases.
- **Sunlight and skylight: *directional propagating* cellular automata.** This
  sharpens (and partly contradicts) the direction-free-injection note above:
  the CA itself carries a propagation direction for sun/sky rather than only
  isotropic irradiance. Ours injects direction at the boundary and diffuses
  isotropically. Both reach "sky lights the open areas"; his should hold
  long-distance sun transport better (a directional sweep marches, a diffusion
  stencil decays), ours is cheaper per cell. Worth a bench once CAGI is in.
- **Individual lights: a separate CA "similar to light propagation volumes", at
  the *macrovoxel* level.** So point/emissive lights are a second, coarser CA —
  a different resolution and probably a different (SH-ish / directional) payload
  than sun/sky. Confirms the multi-resolution guess in the author-comments
  section, and answers the local-emitter question raised there: he does not push
  emissives through the same grid as the sun.
- **Reflections and AO: normal ray tracing, plus a *voxel-face blurring
  filter*.** The denoiser is the news. Filtering per voxel face — rather than
  per screen-space pixel neighbourhood — keeps the blur inside one flat, coplanar
  surface, so it cannot bleed across a silhouette or a corner and needs no
  depth/normal edge-stopping weights. It is the voxel-native equivalent of a
  surfel/texel-space denoiser, and it means his AO ray count per pixel can be
  very low. **This is the missing piece from the "AO — named, never explained"
  section**: cheap AO is achieved by denoising in voxel-face space, not by
  tracing few rays and living with noise. Relevant to E1's cost verdict — we
  measured un-denoised per-pixel AO.
- **Entities: voxelized *per frame* into a microvoxel grid centred on the
  player, then ray traced.** Adjusts the video-8 reconstruction above: rather
  than per-entity local voxel fields intersected via OBB/SDF, entities are
  rasterized/scattered into one shared player-centric microvoxel volume each
  frame, and the normal traversal then just hits them. That explains "no separate
  entity depth merge" and why 65k butterflies were affordable — cost scales with
  the volume, not the entity count. The two accounts may both be true across
  versions; the OBB/SDF path is the older teaser.
- **Terrain: diamond-square fractal + CA**, explicitly
  <https://bitbucket.org/BWerness/voxel-automata-terrain>. This **confirms** the
  VoxelChain-era generator described above is still the lineage: BWerness's
  voxel automata terrain is exactly "diamond-square subdivision where the
  averaging step is replaced by random CA rule tables". Trees and props are then
  grown by a further CA. Moves "diamond-square + CA lookup rules" from
  *historical evidence* to *current method*, and keeps Perlin/Simplex/Worley
  firmly on the do-not-attribute list.
- **Open world: does not exist yet.** So the "experimental infinite-world
  support" in video 3 is experimental in the literal sense. Our streaming work
  is not behind on this axis.
- **Gameplay: probably survival-like, likely significant underwater gameplay,
  mostly undecided. Multiplayer: yes, future.** Explains the ordering — fluids
  and transparency/underwater before world scale.

Supplied references (not yet read into this dossier):

- Voraldo paper — <https://jbaker.graphics/resources/voraldo_paper/Voraldo.pdf>
- WebGPU voxel path tracing thread —
  <https://www.reddit.com/r/proceduralgeneration/comments/15r6lqz/webgpu_voxel_path_tracing/>

### What this changes for voxel-rt

Nothing in the staged plan moves without approval; these are the candidate
deltas this drop creates.

1. **Ray-skew grass** (new technique-bank entry): shading-time animation inside
   the DDA loop, driven by the sky-occlusion field of backlog B2. Cheapest
   plausible foliage motion we have on the table.
2. **Voxel-face-space denoise** for AO/reflections: re-opens E1's verdict. If
   AO can be filtered per face, the analytic-vs-ray choice was decided against a
   handicapped ray variant.
3. **Directional sun/sky CA** vs our isotropic diffusion: a CAGI variant to
   bench once E4 lands, specifically for long-distance sun transport.
4. **Two-tier lighting CA** (fine sun/sky, macrovoxel emissives) instead of one
   grid for everything — matches our own 0.5 m memory wall finding.
5. **Player-centric microvoxel entity volume** instead of per-entity OBB/SDF —
   simpler, and the right shape for dynamic audio occluders too.

## Closest reproducible architecture (the blueprint)

GPU-resident sparse brick world with compact material/state words; software ray
traversal (no meshes); integer RGB radiance fields via ping-pong CA passes;
separate visibility rays for AO/glass/acoustics; GPU entity buffers with
indirect lifecycle; Blockbench bones on GPU; screen-space splats + OBB clip +
local DDA; GPU spatial grids for particles/collision; multiscale procedural
density + ecological decoration; HDR + auto exposure (sub-voxel emissives and
dark caves = huge luminance range); minimal CPU orchestration.

**The defining idea: the same discrete voxel world is shared by rendering, GI,
simulation, collision, entities and sound, while the GPU owns almost all
fine-grained work. The shared representation is what makes the combination
coherent.** (This is voxel-rt's thesis too — with atrium as the far deeper
audio half.)
