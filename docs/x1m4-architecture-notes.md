# x1m4's voxel engine — architecture, reconstructed from Discord history

**Who:** Felix Maier (`@_x1m4`, later handle `xima`, GitHub `maierfelix`). Freelance graphics
dev — "implementing custom light sims for design and game companies" (2026-07-11).
Browser-based, GPU-driven voxel game engine in WebGPU.

**Source:** Discord export in `temp/` — 600 messages, 6 channels, 2023-12-23 → 2026-08-03,
210 of them his. Guild `1003288330391273492`.

**Confidence marking:** ▸ = he said it directly (quoted, dated). ○ = inferred from what he
said plus how the pieces have to fit. Nothing here is from his source — the engine is closed
and he has published no paper or blog post:

> ▸ *"yeah I was thinking about a blog post, but soon people started replicating it based on
> the information I've shared so I thought it's just fine — and after all I'm developing a
> game and have no plans to run a blog on top of that"* (2026-08-03)

So Discord scrapes like this one **are** the primary documentation. That's the whole reason
this file exists.

---

## 1. The one decision everything else follows from

Every subsystem is expressed as an operation on **one shared 3D grid that lives on the GPU**.
Not "the renderer uses a grid" — the renderer, the lighting, the audio, the AI pathfinding,
the fluid sim and the entity animation all read and write the *same* voxel volume, on-device,
and the CPU is a consumer of queues rather than an owner of state.

He states the reasoning explicitly, and notice that it's *convenience*, not performance:

> ▸ *"Since the whole game is running on the GPU, I found it convenient to do the sound
> management and the ray tracing on the GPU as well. Each frame the CPU receives a sound
> queue from the GPU to process, and applies effects like reverb"* (2024-01-04)

> ▸ *"Since the engine already uses GPU path tracing for the lighting, I found it convenient
> to also ray trace the sound."* (2024-01-06)

That is the architectural trick worth stealing. Once the grid is the substrate, each new
feature is "another pass over the grid" instead of a new subsystem with its own data
structure, its own upload path and its own acceleration structure. It's why *one person*
has light, sound, fluids, AI, entities and weather in one engine.

The cost he pays is that everything must be phrased as indirect GPU work:

> ▸ *"Since this is a GPU-driven voxel engine, everything had to be implemented through
> indirect GPU passes (Splatting, animations, bone tra[nsforms]…)"* (2024-03-22)

> ▸ *"entities were actually a lot more tricky to implement than initially anticipated. So
> far the entities use 11 indirect GPU compute passes for both logic and rendering."*
> (2024-03-27)

○ Eleven indirect passes for *entities alone* is the honest price tag. There is no CPU-side
scene graph to fall back on; if the GPU can't express it, it doesn't exist.

---

## 2. Rendering: DDA all the way down, no LODs

> ▸ *"the majors are voxel meshing (rasterization) and voxel dda (ray tracing)"* — his advice
> to a beginner on how to start, 2026-08-03. He picked the second.

> ▸ *"Rendering a few billion voxels and tens of thousands of entities. **No LODs are used.**
> This is pure GPU software ray tracing, running inside a browser!"* (2024-03-23)

- **Software ray tracing, not hardware RT.** WebGPU has no ray pipeline, so it's compute
  shaders marching a grid. This is also what makes it portable to any decent GPU.
- **DDA is the traversal primitive.** Confirmed directly when you asked him
  (2026-08-03: *"are you also using this [DDA interactive]?"* → ▸ *"yes"*).
- ○ **No LODs is a deliberate uniformity trade.** One traversal path, one shading path, no
  LOD seams, no popping, no dual representation to keep in sync. He pays for it in bandwidth
  and buys back correctness and a much smaller codebase. Note this is the *opposite* call
  from your streaming work, where distance detail tiers were what got you 36→112fps — but
  yours was entity-bound, not geometry-bound, which is a different bottleneck.
- **Sub-voxel resolution is a knob, not a rewrite.** ▸ *"testing higher sub-voxel resolutions
  (16x in that screenshot)"* (2026-06-23) — because the representation is a grid, resolution
  is a parameter.

Support/post passes he's named:

| Pass | Detail | Date |
|---|---|---|
| Ray traced reflections | *"with stable temporal reprojection!"*, roughness 0/20/80% shown | 2026-07-30 |
| Ray traced refraction | for water | 2024-03-20 |
| RTAO | ▸ *"ray traced ambient occlusion"* | 2024-08-10 |
| Volumetric fog | marched through the CAGI volume (see §3) | 2024-08-16 |
| Firefly filter | ▸ *"caused by precision errors in my ray tracer, but now get eliminated by my firefly filter"* | 2024-08-12 |
| Auto exposure | histogram luminance; known-broken then fixed (see below) | 2024-03-30 |
| Color grading | *"humble attempt at refining my color grading"* | 2024-08-13 |

The auto-exposure exchange is worth reading in full because it shows how he debugs — he
names the defect, the correct fix, and that he hasn't done it yet:

> ▸ *"my auto exposure isn't tile based, so sometimes small bright spots can cause the sensor
> to get influenced too much"* … *"yeah but you want to average in tiles instead — it's
> harder to implement than regular full screen averaging — didn't get around implementing it
> yet"* … *"iirc there was an article about auto exposure, it used tile based averaging and
> then median to combine the values together"* → [alextardif.com/HistogramLuminance.html](https://alextardif.com/HistogramLuminance.html)
> (2024-03-30/31)

Also a data point you can benchmark against: ▸ *"usually at 1080p my screen passes take at
least around 0.01ms — but the more random the texture sampling the worse for sure"*
(2024-03-30).

And two SRGB bugs in three weeks (2024-03-04, 2024-03-25) — ▸ *"yikes had another SRGB bug"*.
Even at this level, colour-space handling stays a recurring papercut.

---

## 3. CAGI — Cellular-Automata Global Illumination

This is his signature invention and the thing you asked him about. He confirms authorship:

> ▸ *"yes I've created cagi a few years ago"* (2026-08-03)

The definitional statement is the video description, 2024-03-20 — *"Voxel Engine Dev:
Solving Global Illumination with Cellular Automata"*:

> ▸ *"Showcase of a new light simulation method that I came up with recently. It uses **pure
> integer cellular automata** to solve light **propagation, diffusion and bouncing**. Also
> added ray traced reflections and refractions for rendering water."*

Then, five months later, GI is no longer hybrid — CA has eaten the whole light solution:

> ▸ *"a now **fully cellular automata based** light solution for GI"* (2024-08-10)

> ▸ *"Infinite light bouncing (almost) for free!"* (2024-08-16)

### What "pure integer CA" buys him

○ Reading those three claims together with the rest of the engine, the shape of it is:

- **The radiance field is the grid.** Light is per-voxel state, not a probe grid, not an
  irradiance cache, not surfels. No separate acceleration structure, no probe placement
  problem, no probe-spacing detail loss — which is exactly the weakness the Split Radiance
  Cascades paper he commented on is trying to fix (2026-07-23, `arXiv:2607.20384`).
- **Integer state = cheap and stable.** Fixed-point/integer light levels make the update
  rule exact and idempotent-ish, so it can't drift or NaN, and it packs small enough that a
  whole-world light volume is affordable. This is the Minecraft flood-fill insight taken
  seriously and extended to *bouncing*, not just propagation.
- **Cost is amortised over frames, not paid per frame.** A CA converges by iteration — you
  do a bounded number of steps per frame and the solution chases the world. Hence "infinite
  bouncing almost for free": bounce *n* is just another iteration of the same rule, so extra
  bounces cost nothing extra per frame, they only cost latency-to-converge. That's the single
  cleverest property of the method.
- **Convergence latency is the visible cost**, and he says so: ▸ *"The first few seconds in
  the world take some time until the procedural world generation is finished and **until the
  lighting is propagated**"* (2025-07-16).

### CAGI is reused as a general volume

The part people miss. Once you have a converged per-voxel radiance grid, it isn't only for
surface shading — it's a *volumetric lighting lookup*:

> ▸ *"Underwater volumetric fog adds an interesting atmosphere! Achieved by **ray marching
> through the CAGI volume**."* (2024-08-26)

○ And the same volume is what the audio system reads for ambience (§7) and what vegetation
reads for wind response (§5). One solve, four consumers.

### Practical tip he gave for CA/probe-volume banding

> ▸ *"usually you can get rid of these by adding some slight **jittering when sampling** from
> your light volume/probes"* (2026-07-24, on visible banding in an RC implementation)

**Gap:** the actual update rule — neighbourhood, integer packing, per-channel handling,
how many iterations/frame, how emissive injection and occlusion work — is *not* in this
export. He says where it is: ▸ *"I have explained it multiple times on the voxelgamedev
server, if you dig through the message history there then you should find it"*, keyword
▸ *"cagi"* (2026-08-03). See §10.

---

## 4. Entities: voxelised on the fly, simulated as fields

### Rendering — voxel splatting

He tried the obvious thing first and rejected it:

> ▸ *"before I voxelized them into seperate volumes each, but soon realized **it's bad on a
> lot of levels**. now they use a quite optimized voxel splat voxelizer that does everything
> on the fly"* (2024-03-31)

The technique, in his words:

> ▸ *"It uses **voxel splatting** to project entities on the screen and a mix of **analytic
> OBB SDF intersection and DDA ray tracing** to voxelize the entities on-the-fly."*
> (2024-03-22)

> ▸ *"yeah with the individual model **bone boxes** … this part is done with analytic obb
> intersection and then **voxel dda is used through that region** for the pixelart style"*
> (2024-03-31)

○ So: skeleton → oriented boxes per bone → analytic OBB test finds the region a ray can hit
→ DDA inside that region snaps the result to the voxel lattice, which is what makes entities
*look* like they're made of the same voxels as the world without ever being written into it.
Entities stay geometry-free and instance-cheap; no per-entity volume allocation, no
re-voxelisation on animation.

Scale achieved: ▸ *"65000 butterfly objects running on the GPU that each frame get
individually rendered, animated and simulated"* (2024-03-30); *"100k animated entities"*
(2024-02-23); *"tens of thousands of entities"* in the stress test.

He predicted the payoff two years before taking it:

> ▸ *"in future I plan on exploring better procedural animations as the voxelizer uses SDFs
> already for the intersection — could allow for some really cool stuff"* (2024-03-31)

…and in 2026 he cashed it in, dropping the asset pipeline entirely:

> ▸ *"entities are fully procedural now (no blockbench anymore) — much easier and faster to
> iterate with"* (2026-07-30)

> ▸ *"the system is actually quite similar to how my blockbench model importer worked, but
> now **everything is expressed in the shaders** (except the model's nodes, which are defined
> in json), animations are still written as code by **modulating the SDFs**, and really the
> only difference is that **there are no longer any textures involved**"* (2026-07-31)

> ▸ *"and the best part is that they're defined and animated through SDFs"* (2026-07-31)

○ This is the deepest lesson in the whole export: because he'd chosen SDFs as the
*intersection* primitive for performance reasons in 2024, animation-by-SDF-modulation and
fully procedural creatures became available for free in 2026. The right low-level primitive
kept paying out years later.

Then IK (2025-11-26), skeletal animation (2026-08-01), ragdoll (▸ *"I started to learn about
ragdoll physics for exactly this since yesterday"*, 2026-08-02) and dismemberment
(2026-08-03) all land on that same SDF substrate within weeks.

### Logic — fields, not per-agent queries

Both hard entity problems are solved by writing a *field* into the grid and letting agents
read it locally. This is the same move as CAGI, applied to AI.

**Collisions / crowding → force field:**

> ▸ *"Entity cramming and collisions solved with a **GPU force field** which is very efficient
> to update and evaluate and can probably be extended to handling entity attacks and damage
> too!"* (2024-06-07)

**Pathfinding → flow field:**

> ▸ *"Using **flow fields** on the GPU for entity path finding: Blue tint is the flow field
> itself and the Green tint is the **flow field gradient** that each entity calculates to get
> a walking direction. Tight corners and stair[s]…"* (2024-06-10)

By 2025 the flow field has become genuinely sophisticated — it encodes *traversability*, not
just distance:

> ▸ *"the flow field also **encodes jumping and falling cases** and generally avoids paths
> that can't be traveled by walking entities, such as if **an entity is too large** to cross
> a path, if the **fall distance would be too high** or if it **wouldn't be able to jump** to
> the target location"* (2025-11-25)

> ▸ *"walk path finding now also supports **jumping over gaps**!"* (2025-11-25)

○ Why this is the right call on a GPU: N agents doing individual A* is divergent, serial and
cache-hostile. One field solve is a uniform stencil over the grid — perfectly GPU-shaped —
and then every agent's "decision" is a gradient sample, i.e. a few texture reads. Cost is
O(grid) once instead of O(agents × path length), so it gets *cheaper per agent* as the crowd
grows. Same reason CAGI beats per-pixel path tracing for diffuse light.

Entity spawning is also grid-derived: ▸ *"Added entity spawning (based on occupied space,
light level, block material, per entity type global count)"* (2025-11-19) — note **light
level**, which is a CAGI read.

---

## 5. World generation: cellular automata, in passes

When you asked how biome modelling works, he gave the standard answer first, then said what
he actually does — and it isn't noise:

> ▸ *"usually it's stacking and blending noise layers together — that's usually the
> foundation of all these methods"* (2026-08-01)

> ▸ *"otherwise, what I'm using myself for terrain generation is **cellular automata**"*

> ▸ *"no you can use **pure cellular automata**, you just need some basic noise RNG for
> randomness — randomness for like spawning grass randomly etc."*

> ▸ *"it's done in **multiple passes where each pass has a specific task**, like the first
> task being the basic terrain shape with caves etc. and the second pass adding vegetation
> etc."*

> ▸ *"that'd be part of the terrain pass or a second pass that e.g. **spawns biome cells into
> the terrain, which then get propagated around randomly** — at least that's like one of a
> thousand ways of how this could be tackled"*

○ So worldgen is *the same machinery as the lighting*: seed cells, apply a local rule, let it
propagate. Noise is demoted to an RNG source rather than being the generator. Biomes are
grown, not sampled — which is why he gets ▸ *"really crazy patterns are evolving now"*
(2024-01-18) rather than the smooth blobs noise-blending gives you.

Related, and the same idea taken to its limit: **strata-voxel**, his public toy —
▸ *"This little tool lets you explore fractal worlds generated using **chaos theory**"*
([maierfelix.github.io/strata-voxel](https://maierfelix.github.io/strata-voxel/), 2026-02-16).
That one *is* open, and is the closest thing to readable source you'll get from him.

Vegetation is procedural and wind-reactive, and again keyed off the light volume:

> ▸ *"added back vegetation getting influenced and waved by the wind (**based on sky
> occlusion**)"* (2024-03-24)

○ Sky occlusion is already in the grid as a byproduct of the light solve, so "how exposed is
this leaf" is free. Compare your own `voxel_core::wind` — you drive grass/water from a
hierarchical gust model ported from the audio synth; he drives it from the lighting term.
Both avoid a dedicated wind volume.

Timeline of worldgen: procedural foliage generator (2024-01), terrain rework → *"crazy
patterns"* (2024-01), forest meadow biome (2024-03), simplex-noise test (2024-06), infinite
world prototype (2024-06), cave/terrain generator rework (2024-12), underwater proc-gen
(2025-10/11).

---

## 6. Fluids: three complete rewrites, driven by measurement

This is the most instructive engineering-process thread in the export, because you can watch
him abandon two working systems.

**Phase 1 — cellular automata fluids** (2024-08 → 2026). Consistent with everything else in
the engine; ▸ *"Before integrating the cellular automata fluid simulation into my voxel
engine, I'm tackling the challenge of rendering transparent blocks"* (2024-08-28).

**Phase 2 — SPH** (2026-07-07):

> ▸ *"been moving from ca to sph"* … *"allows to simulate a much wider range of materials"*
> … *"performance is similar to my ca stuff, so I can't wait to get the ecosystem cycle
> implemented and then move it over to my voxel engine again"*

**Phase 3 — PBMPM + reintegration tracking** (2026-07-30). And here's the money quote on
method selection:

> ▸ *"I'm currently experimenting with a mix of electronic art's **pbmpm** combined with
> **reintegration tracking** and it's quite stable so far"*

> ▸ *"tried numerous methods and they all were either **too slow, lost/generated mass,
> exploded or bubbled/flicked** — and it's the first method that has none of these problems"*

The non-negotiable constraint, and why:

> ▸ *"the reintegration tracking was basically the final bit towards a **grid-based particle
> data structure**, since I wanted to **strictly stay grid-based** like my CA fluid sim
> because it's **so much faster to deal with** than particle-based sims"*

> ▸ *"the reintegration tracking, or at least how I'm doing it, is basically storing multiple
> particles (**currently 4 particles per cell**) directly within cells instead of having them
> in a list"*

> ▸ *"fully lagrangian"* — when Mytino pushed back that you can't move particles fully
> Lagrangian without a list, the thread ended unresolved (2026-07-30). ○ Treat "fully
> Lagrangian" as his framing, not a settled classification; 4-particles-per-cell with
> reintegration is a hybrid by most definitions.

**Idle-region culling** — he went looking for Noita's trick:

> ▸ *"have you ever experimented with **culling idle regions** from updates in your fluid
> sims like Noita does with **dirty chunk updates**?"* (2026-07-30)

> ▸ *"synthetic fluid dampening is getting better which is essential for **dirty chunks** (the
> red squares)"* (2026-08-01)

○ The subtlety: dirty-chunk culling requires fluid to actually come to rest, and numerical
sims jitter forever. So the *damping* is not a visual choice, it's what makes the
optimisation possible. Sleep thresholds and damping are one design problem, not two.

Materials on top: ▸ *"first attempt at **material reactions**"* (2025-12-05); ▸ *"experimenting
with **temperature**, so it will be possible to refine and convert materials this way"*
(2025-12-17); scope fenced deliberately — ▸ *"I'm only planning for fluids and some basic
chemical reactions between them such as water ↔ oil, oil ↔ fire, fire ↔ smoke etc."*, and
explicitly **not** elastic/plastic MPM (2026-07-30). Plus foam (2026-07-28) and fire
(2026-07-31).

Water rendering has been the standing perf villain: ▸ *"The rendering speed is currently
heavily slowed down by my **lazy water rendering implementation** as it uses quite expensive
ray marching to make the water have realistic depth"* (2025-07-16). Also a `surface net` used
to keep water out of submerged bases, with a floated design idea of pressure + pump airlocks
(2025-11-14).

---

## 7. Audio — the part that matters most for atrium

He built GPU-ray-traced sound **in January 2024**, before most of the renderer features, and
it's the clearest precedent for your north star. The architecture:

> ▸ *"Since the whole game is running on the GPU, I found it convenient to do the **sound
> management and the ray tracing on the GPU** as well. **Each frame the CPU receives a sound
> queue from the GPU to process, and applies effects like reverb.**"* (2024-01-04)

○ That single sentence is the design. **Rays and geometry queries stay on the GPU; DSP stays
on the CPU; the interface between them is a per-frame queue of sound events with baked
parameters.** GPU→CPU readback of a small event list is affordable; GPU-side convolution and
per-source filter state would not be. This is very close to your `rtrb` command-queue seam,
just with the *acoustic solve* moved to the other side of it.

What the traced result drives:

> ▸ *"Various effects such as **reverb and occlusion** are supported, and also multiple
> ambient effects like **wind and vegetation producing noise when struck by wind**."*
> (2024-01-06)

And the cross-domain reuse — ambience derived from the *light* solve:

> ▸ *"better cave ambient sound, now determined based on **ray traced luminance, stone blocks
> and sky occlusion**"* (2024-05-31)

> ▸ *"The cave horror noise played in the background uses ray tracing too and is based on
> **how dark/sca[ry]** [the location is]"* (2024-08-10)

> ▸ *"The cave ambient noise is evaluated using sound ray tracing"* (2024-05-31)

○ Read that carefully: he is not tracing *audio* to decide the ambient bed — he's reading
CAGI's luminance and sky-occlusion terms, plus a material count, and mapping "dark, enclosed,
stony" → "cave horror bed, loud". The light field is doing double duty as a cheap
*acoustic-enclosure estimator*. You already compute directional occlusion and per-wall
materials in atrium; the trick worth stealing is using **one already-converged spatial field
to key ambience selection and intensity**, rather than running a second solve.

Other audio details:

| Thing | Quote / date |
|---|---|
| Underwater | ▸ *"sounds are dampened with a **lowpass filter** when submerged"* (2024-08-23) |
| Pitch shifting | ▸ *"dynamic high quality pitch shifting through **granular analysis**"* (2024-06-11) |
| DSP library swap | rubberband → [signalsmith-stretch](https://github.com/Signalsmith-Audio/signalsmith-stretch), ▸ *"for pitch shifting (without stretching)"*; reason: ▸ *"rubberband's licensing and tooling is harder to work with"* (2026-06-17/18) |
| Synthesis is offline-baked | ▸ *"it will take about a minute since a lot of pre-processing is done like **sound synthesis** and shader compilation"* (2025-07-16) |
| Still being reworked | ▸ *"slightly reworked the sound ray tracing"* (2025-11-19) |
| Creature vocals | fox calls startled a family member's household; ▸ *"in case you got a dog then he'll probably like that sound"* (2026-07-31) |

○ Note the one place he's *less* sophisticated than you: reverb is applied CPU-side from
traced parameters, with no indication of a per-source FDN, multi-band air absorption, HRTF,
or measurement-mode validation. Your DSP chain is deeper. His advantage is that the
*geometry* side is free because it shares the renderer's grid and rays. That asymmetry is
exactly the case for atrium-inside-a-voxel-engine.

---

## 8. Platform, tooling, distribution

- **WebGPU, in a browser.** ▸ *"Remember that the whole engine runs entirely on your GPU →
  **YOU NEED A DECENT GPU AND CHROME!**"* (2025-07-16)
- **Cold start ~1 minute**, no loading bar: sound synthesis + shader compilation, then
  worldgen, then light propagation (2025-07-16).
- **Playable build:** `voxelchain.app/xima-sandbox/0.0.1-pre-alpha/example/index.html` —
  posted 2025-07-16, 167 reactions. Worth trying if it still resolves; that build predates
  the SDF-entity and PBMPM work.
- **Modding API is the app's own API.** ▸ *"almost done with the modding api"* (2026-06-23);
  ▸ *"first test scene loaded as a **live main menu background** (done entirely through the
  modding api)"* (2026-06-26). ○ Dogfooding the mod API for engine-internal UI is the same
  discipline as your hard platform↔gpu↔passes seams — it forces the API to be complete.
- **Hot reloading** (2026-06-18), **in-engine editor + UI** (2026-07-09), **orthographic
  camera** (2026-07-02).
- Entity node definitions in JSON; everything else in shaders (2026-07-31).

---

## 9. Timeline

| Date | Milestone |
|---|---|
| 2023-12 | First screenshot in the export |
| **2024-01** | **GPU ray traced sound**; video; voxel Sponza; terrain rework |
| 2024-02 | 100k animated entities; sky rendering |
| **2024-03** | **CAGI video** ("Solving GI with Cellular Automata"); **entity voxel splatting**; billion-voxel/no-LOD stress test; forest meadow biome; entity hunting (11 indirect passes); 65k butterflies |
| 2024-04 | Water rendering + reflections |
| 2024-05 | Inventory, items, HUD; cave ambience from traced luminance |
| **2024-06** | **GPU force field** collisions; **flow field** pathfinding; granular pitch shift; conveyors + mining; **first infinite world prototype** |
| 2024-07 | Portals |
| **2024-08** | Fully-CA GI + RTAO; firefly filter; volumetric fog; underwater (DDA-marched water, LPF audio, fog via CAGI volume); transparency video |
| 2024-10 → 11 | 2D prototypes; pure-SDF shapes; material conversion |
| 2024-12 | Cave/terrain generator rework |
| **2025-07** | **Public playable WebGPU build** |
| 2025-10 → 11 | Islands, fish, underwater proc-gen, submerged bases + surface net; entity spawning, world save/load; **flow field encodes jump/fall/size**; first IK |
| 2025-12 | Material reactions; temperature |
| 2026-02 | strata-voxel (chaos-theory fractal worlds) released publicly |
| **2026-06** | Hot reload; **modding API**; 16× sub-voxel; rubberband→signalsmith |
| **2026-07** | CA→**SPH**→**PBMPM + reintegration tracking**; dirty chunks; RT reflections w/ temporal reprojection; foam; fire; **entities fully procedural via SDFs**; editor/UI |
| 2026-08 | Skeleton animation; ragdoll; dismemberment; fluid damping for dirty chunks |

**Direction of travel:** rendering (2024 H1) → simulation & AI (2024 H2) → world systems &
persistence (2025) → tooling, modding, and a *second* fluid rewrite (2026). The stated
destination is ecosystems:

> ▸ *"since I'm working towards having **ecosystems with weather** etc. I was thinking about
> how species could be made part of these worlds, and one idea I had was to let the player
> **train species in something like a DNA chamber**, define some basic parameters, and then
> work towards species that stabilize towards some goal or behavior and then can be put into
> the live ecosystem"* (2026-07-31)

> ▸ *"there is one promising ML technique called **active inference**, which doesn't rely on
> defining any strict goal, but learns by exploring and **minimizing "surprise"**, which I
> generally find a neat idea of how intelligence works on a much more fundamental level"*
> (2026-07-31; he'd shared [pymdp](https://github.com/infer-actively/pymdp) on 2026-06-29)

---

## 10. What this export does *not* contain, and how to get it

**The CAGI update rule is missing.** He told you exactly where it is:

> ▸ *"I have explained it multiple times on the **voxelgamedev server**, if you dig through
> the message history there then you should find it"* → keyword ▸ *"cagi"* (2026-08-03)

Three concrete gaps in the current export:

1. **Every file is capped at exactly 100 messages** (`-page-1`). You have page 1 of 6
   channels; the rest of each channel is unexported. `#graphics_programming` alone starts
   2023-03 but your export only covers 2026-07-08 onward.
2. **A channel from the same guild is missing.** You linked
   `discord.com/channels/1003288330391273492/1006843475494445137/…` yourself (2026-08-03,
   *"i saw this and i was like wow!"*). Guild `1003288330391273492` is the guild this whole
   export came from, but channel `1006843475494445137` (created 2022-08-10) isn't in it.
   That's a concrete, known-good target.
3. **The VoxelGameDev server isn't here at all** — and per his own pointer, that's where the
   CAGI explanations live.

**So yes, please export more.** In priority order:

- **The VoxelGameDev server**, searched for `cagi` — his own recommendation, highest value by
  far. Also worth searching there: `cellular automata`, `x1m4`/`xima`, `light propagation`.
- **Channel `1006843475494445137`** in guild `1003288330391273492`, all pages.
- **Remaining pages of `#graphics_programming` and `#chat`** in this guild — that's where the
  ReversedCausality-style deep technical Q&A happens (the voxel-splatting explanation came
  out of exactly that kind of thread).
- If the exporter supports it: **filter by author `210574545709563906`** across the whole
  guild rather than per-channel paging. That would collapse thousands of messages into just
  his, and is how you'd catch the CAGI explanations wherever they landed.

Also unanswered in the export: you asked ▸ *"just wonder in some of your examples how many
fps you're getting and what video card you're using?"* (2026-08-01) — he never replied. Worth
re-asking; it's the one number that would let you calibrate your ledger against his.

---

## 11. Five things to take from this

1. **Pick one substrate and force every subsystem onto it.** His grid serves rendering,
   lighting, audio, AI, fluids and worldgen. Not code reuse — *data structure* reuse. That's
   what makes the scope survivable solo.
2. **Solve fields, not queries.** Light, pathfinding, and crowding are all "write a field
   over the grid, sample it locally." Cost becomes O(grid) instead of O(agents), so it gets
   relatively cheaper as the scene grows. This is the same insight three times.
3. **Amortise convergence across frames instead of converging per frame.** CAGI's extra
   bounces are free because they're extra iterations, not extra work per frame. The cost
   shows up as latency-to-converge, which is far easier to hide than a per-frame budget.
4. **Reuse a converged field in a different domain.** Cave ambience from light luminance +
   sky occlusion; wind from sky occlusion; volumetric fog from the CAGI volume. Directly
   applicable to atrium: derive ambience keying and enclosure estimates from a field you're
   already solving.
5. **Choose the primitive that pays out later.** SDFs were chosen in 2024 for cheap OBB
   intersection; that choice made fully procedural, code-animated creatures possible in 2026
   with no rewrite. And symmetrically — he rewrote fluids twice and states his rejection
   criteria numerically (too slow / lost mass / exploded / flickered). Both halves of that
   are your variant-registry discipline.
