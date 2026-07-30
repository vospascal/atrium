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
   materials use ray-traced reflection and refraction.
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

**Unknown:** world acceleration structure; memory layout; traversal variant;
cave/biome/tree math; CA GI stencil + attenuation + bit depth; temporal
filtering; entity AI; particle neighbour structure; collision solver; acoustic
material model; fluid equations; confirmed API; hardware/frame-time breakdown.

**Do NOT attribute without evidence:** greedy meshing, marching cubes, surface
nets, mesh shaders, SVO/DAG, NanoVDB, hardware RTX, ReSTIR, NeRF, Gaussian
splatting, L-systems, Perlin/Simplex/Worley caves, erosion, WFC biomes,
navmeshes, wave-based acoustics.

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
