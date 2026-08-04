# x1m4's engine — the #graphics-programming channel, 2023-03 → 2026-08

**Source:** `temp/folder 1` — a single-channel Discord export. Channel `1089129949228695633`
in guild `1003288330391273492` (Graphics Programming). 360 message files, **7 001 unique
messages, 1 854 of them x1m4's**, spanning **2023-03-25 10:16 UTC → 2026-08-03 10:17 UTC**
across 189 active days. All timestamps below are UTC and taken verbatim from the export.

**Who:** `xima` / `x1m4` / `@_x1m4`, user id `210574545709563906`. Browser-based, GPU-driven
voxel engine in WebGPU/WGSL, single developer, closed source, funded by contract work
("*that's why I do contract work, so I can mostly freely manage my own time*" — 2024-07-01).

**Relation to the other doc:** [x1m4-architecture-notes.md](x1m4-architecture-notes.md) was
built from a *different, smaller* export (600 messages, 6 channels, 210 his). This file is
built **only** from this channel and is deliberately standalone. Where the two disagree on a
number, this channel is usually earlier in time — see §11.

**Confidence marking:** ▸ = he said it, quoted and dated. ○ = my inference from what he said.

---

## 0. The one-paragraph summary

He spent **2023 building a conventional ray-traced GI stack** (SH world-space radiance cache
+ screen-space cache + cascades + shadow maps + SVGF-style denoising), got it looking very
good, then **concluded in early 2024 that 1 sample-per-pixel is structurally insufficient**
and threw the whole lighting approach away. What replaced it is **CAGI** — an integer
cellular-automata light propagation volume, anisotropic, deterministic, dirty-culled — which
is now the engine's only GI, with ray tracing demoted to AO and reflections. From there the
engine became a **GPU-resident simulation** where lighting, fluids, terrain gen, entities,
items, particles, pathfinding and **sound** are all passes over one grid. 2026 is spent on a
**WGSL mod system** and solving GPU thread divergence for entity behaviour.

---

## 1. Engine lineage and the game

| When | State |
|---|---|
| pre-2023 | `voxelchain` — earlier engine, LPV-based lighting, WebGL. Demo still on his site. |
| 2023-03 → 2023-12 | Ray-traced GI engine: SH world+screen radiance caches, cascades, shadow maps, ReSTIR started Dec 2023. |
| ~early 2023 | Deep-sea game idea forms. "*the deep sea idea is actually already about a year old*" (2024-02-15). |
| 2024-02 | **CAGI replaces ray-traced GI.** |
| 2024-02 → 2024-07 | Entity system: teardown-box raster → voxel splatting → analytic OBB + DDA. |
| 2024-09 | "*the engine is in a really good shape and I don't see too many hurdles ahead before being able to release a public demo*" (2024-09-24). |
| 2024-10 → 2025 | Sparse. 2d automation game side-experiment with radiance cascades; fluid sim in 2d. |
| 2026-06 | Modding API finished, WESL module system, mod editor started, fluid sim being ported 2d → 3d. |

> ▸ *"reminds me of my old voxelchain engine"* (2026-05-21, posting an old screenshot)
> ▸ *"yeah some variant of it [LPV] … later started on CAGI to counter all the issues that I had when working with LPV"* (2026-05-24)

**The game:** deep-sea sandbox survival. Both **water and air are simulated as mass**, which
is the gameplay hook:

> ▸ *"both water and air are simulated and in order to create an underwater base, you had to
> create an underwater bell (like a room in the water, but with no floor) and then pump air
> into it from above, so the air with higher pressure than the water pushes the air out of the
> room through the floor … it's also somewhat like real underwater bases work"* (2024-02-28)

> ▸ *"I think in the deep sea a lot of gameplay can be formed around lighting — like either
> preventing certain monsters from spawning, or attracting specific monsters or fish"* (2024-02-15)

**Deliberately finite worlds**, 384³–512³:

> ▸ *"fine for me though my worlds are 384^3 to 512^3 and I'm still not sure if I go for
> infinite world sizes … I don't see much advantage for going with infinite worlds since I
> mostly care about dynamic worlds and not static ones — and static voxel worlds feel over
> explored compared to dynamic ones"* (2024-09-24)

He had **world-edge looping** working (items fall infinitely), considered horizontal-only
looping, and floated a **skyblock-style first demo**: centre platform, vertical conveyor
chain, infinite stone generator at the top, and a fully flooded world you drain by removing a
bottom block (2024-09-24).

---

## 2. The 2023 stack he later abandoned — worth reading anyway

This is the most technically detailed part of the channel and none of it survives in the
current engine's GI, but the numbers are hard-won and reusable.

### World-space radiance cache (SH)

- Cascaded clipmap centred on camera, follows the camera. Basis: James McLaren's *Technology
  of The Tomorrow Children* GDC talk, "*I'm using the approach mentioned at 9:00*" (2023-04-14).
- **Layout:** 2 ping-pong textures for temporal accumulation; **cascades embedded in the
  texture depth**, and the **3 SH buffers embedded in the height (3×)** (2023-04-14).
- 2–3 cascades in voxelchain; later "*each cascade is 64^3*", "*multiple volume cascades
  centered around the camera, each doubling in their scale*" (2024-02-15).
- SH buffers `rgba16f`. 2nd-order SH ≈ 3–4 encodable light directions; 3rd order for near
  camera (2023-03-26).
- Cascade-edge blackening on scroll fixed by **injecting radiance from the next-higher
  cascade** — "*makes the transition between cascade edges almost invisible*" (2023-04-14).

### Infinite bounces for free — the trick people kept asking him about

> ▸ *"found this neat trick to just resample my world-space radiance cache during tracing and
> get infinite bounces for pretty much free (just one additional texture read)"* (2023-03-27)

> ▸ *"I use a world space radiance cache that moves with the camera and every cell traces
> radiance in a uniform pattern. for infinite bounces you can just sample the world cache at
> the given hitpoint and you get almost free infinite bounces"* (2024-01-06)

Honest about the cost: "*there is definitely a quality loss with resampling — saturation is a
bit less and it's also brighter*" (2023-03-27), traced to "*the simplified representation of
the radiance cache*". It also **acts as a natural spatial filter** (2023-04-01). And it bit
him once: unbounded energy growth — "*at some point it gets so bright, it looks like the
gateway to heaven*" — caused by something attenuating above 1.0 (2023-03-28).

### Sample counts and temporal filtering — the numbers

- **64 samples per probe** to converge complex indoor light (small emissive sources); 8–16
  outdoors. "*it's identical to the number that they mentioned in the lumen dev talk*" (2023-03-28).
- Temporal blur **48 frames** for both world- and screen-space caches; world-space could drop
  to 16–32 at the cost of a stronger spatial filter (2023-04-01).
- Spatial filter over **6 neighbours only** (not 8 or 26) — "*the filter actually gets quite
  expensive because of so many texture reads with sh*" (2023-04-01).
- 1 short extra bounce from probes to offset probe-filter blur (2023-03-29).
- Probe sampling offsets **jittered**, leaning on aggressive TAA to clean up (2023-03-29).

### Skylight — the part he hated

The 2023 solution, replacing a 6-side heightmap and flood-fill skylight that both looked bad:

> ▸ *"I'm generating a bunch of random points distributed over a hemisphere and then over time
> render the shadow maps for the sky"* (2023-04-10)
> ▸ *"the sky shadow maps are 32× 128² and the sun shadow map is currently only 1 with a
> resolution of 4096 … every frame, 1 sky shadow map and the sun shadow map are updated,
> though not fully but in tiles"* (2023-04-11)

Verdict a year later: "*me and Sam use hemispherical shadow maps to capture skylight, but it
has a ton of artifacts and is horrible to work with*" (2023-09-15). And on shadow mapping in
general: "*I hate shadow mapping so much but performance wise it's just unbeatable*"
(2023-04-01) / "*taa is definitely an anti fun thing to implement — sucks as hard as shadow
mapping*" (2023-07-02).

### G-buffer and the two-day float bug

> ▸ *"my gbuffer uses uints for storing various data like main voxel id, sub voxel id and
> normal and it's a uvec4 texture"* (2023-10-16)
> ▸ normals: *"just an id from 0-2 based on ray direction and the voxel normal"* — only the
> **3 possible visible directions**, not 6, "*to save some bits for other stuff — stuff like
> lod level, animation frame*" (2023-10-16)

The bug worth remembering: checkerboard-upscaler edge detection **must compare as uint, not
float**. Two days lost. "*f floating points*" (2023-10-16).

### Everything else in the 2023 renderer

| Feature | How |
|---|---|
| Denoising | Filter **raw irradiance with albedo forced to white**, apply textures after denoise. Emission also whitened. SVGF rejected: "*massive over blurring and unfortunately is also very slow*" (2023-04-25). |
| Primary rays | Checkerboard upscaling — "*for reducing the primary ray cost it's definitely super effective*". Interlaced update of the screen-space cache "*almost doubled performance*" (2023-10-21). |
| Reflections | Lumen's **glossy GI mip filter**: sharp reflection rays + mips + bilateral blur. "*pretty dumb but works surprisingly well considering how fast it is*" (2023-03-29). Knows screen-space cone tracing is better, never tried it. |
| Volumetric fog | March the world-space radiance cache, **half res + heavy jittering + strong blur upscale**. Omni lights only (2023-03-29). |
| DOF | Fake: sample neighbour pixels with jitter scaled by CoC distance (2023-04-25). |
| Tonemapping | `tony-mc-mapface`. Shared a DDS-parser workaround: raw colour data at **byte offset 148 → 1769472**, upload as **48×48×48 `rgba32f` 3D texture** (2023-04-02). |
| Also tried | Anisotropic voxels / "ambient cubes" for radiance before switching to SH — "*both techniques work well*" (2023-04-25). Crytek-style LPV with **26 cube directions instead of SH**, dropped because multi-bounce is tricky; lost the implementation to an SSD failure (2023-10-15). |
| Dec 2023 WIP | "*currently working on adding restir gi — and entity rendering*" (2023-12-06). |

### Why he quit ray-traced GI — the stress test

This is the pivotal argument, stated twice:

> ▸ *"could you record a scene with a large room with flat walls (and like 4× larger space than
> the torch cave scene), no sunlight and only place and remove a single torch a few times? such
> scenes were the ultimate stress test for my path tracing implementations, and they all failed
> unless I really cranked up the temporal filter. that's also the reason why I moved away from
> solving lighting with ray tracing, as 1 sample per frame is just not enough budget to work in
> like 99% of different and complex scenes — and the only way to tackle this problem is smarter
> sampling and most importantly massive spatial blurring and temporal smoothing, introducing a
> ton of new issues"* (2024-04-17)

> ▸ *"it's just not there yet unless you crank up temporal accumulation and accumulate much
> more samples — basically came to that realization after years of experimentation"* (2024-06-23)

○ Note the shape of the argument: it is not "ray tracing is slow", it is "the *variance* is
irreducible at 1 spp, and every fix for variance is itself a defect". Same reasoning shows up
later as **"ray tracing is cache nuking"** (§4).

---

## 3. CAGI — the algorithm, in as much detail as he ever gave

Named, dated, and his own: "*yes it's called CAGI, I've developed the algorithm specifically
for this game*" (2025-10-30). First appears in this channel 2024-02-08 as a 2d prototype:

> ▸ *"only doing it for prototyping at some point, in case I get something working I extend it
> to 3d and use a compute shader — which then gives me really cheap cellular automata based
> global illumination"* (2024-02-08)

### What it is, in his words

> ▸ *"an own grid based light propagation algorithm that I call CAGI"* (2024-07-03)
> ▸ *"it's like a directional extension of minecraft's flood fill and a more restricted (and
> faster + stable) variant of light propagation volumes"* (2025-10-30)
> ▸ *"mine is kind of like a radiosity solver too, but with cellular automata based ray tracing
> kind of"* (2024-02-10) — inspired by tmpvar's [2d CPU radiosity article](https://tmpvar.com/articles/radiosity-2d-cpu/)

### The propagation rule — the most explicit statement in the export

> ▸ *"propagate each axis by looping through each adjacent neighbor and do
> `max(srclightpy, neighborlightpy) * propagation_loss`, add slight diffusion for each axis
> e.g. `mix(lightpy, lightny, x)`, bounce light from surfaces with color for each axis by doing
> e.g. `lightpy * neighbor_solid_color * bounce_loss`"* (2025-12-24)

Loss is **subtractive**, not multiplicative: `max(LOSS, LIGHT) - LOSS` (2024-02-28). Injection
energy and loss factor jointly set reach — "*the light loss is a general factor how drastic
the falloff is at each step, and the injected light energy determines too how far it will
reach*" (2024-02-28).

### Storage and anisotropy

| Property | Value |
|---|---|
| Directions | **4 buffers in 2d, 6 in 3d** — one texture per direction (2025-04-28) |
| Bits | **10 bits per direction × 3 (RGB)** (2025-12-22) |
| Arithmetic | **Integer / fixed-point only** → deterministic (2024-02-28, 2025-01-07) |
| Resolution | **1/8 of voxel res** — main-voxel scale, not sub-voxel (2024-07-03) |
| Buffering | Double-buffered → **1 texture write per cell**; shared memory can do partial double-buffering (2024-04-20, 2025-06-20) |
| Reads | **10–20 reads per cell in 2d** with bouncing enabled (2025-06-19) |
| Solids | Doesn't care about solids at all — only **opacity and light transfer** (2024-02-25) |

He also traces the **earliest** design, which is a nice minimal version to prototype:

> ▸ *"the earliest versions of cagi actually didn't use separate light buffers for each
> direction, but used only one buffer with an 'angle' value and an intensity. the light spread
> uniform from the source with regular flood fill, but used an additional rule that when the
> current light cell is next to a solid then the angle got changed … the angle was a bitfield
> (in 2d 4 bits) and based on the direction of the current cell towards the solid cell the bits
> got disabled resulting in creating shadow trails. you can also do diagonals with this idea,
> basically just depends on how many angle bits you use"* (2025-04-27)

The failure mode of that isotropic version, and why anisotropy is required: two lights of
equal intensity with a wall between them **wash each other's shadows out**. "*with anisotropic
the issue gets reduced since you handle each propagated light direction (mostly)
individually*" (2025-04-28).

### Performance — the only numbers he ever published

> ▸ *"regarding performance 256^3 under full workload takes about 2ms but with dirty buffers
> and checkerboard updating it takes around 0.4ms and with some effort it can definitely be
> further optimized like using indirect workgroup dispatching with partial updates instead of
> always doing a full resolution dispatch"* (2024-09-24)
> ▸ *"with cascading I expect similar performance as my previous cascaded light solutions,
> 128^3 per cascade and around 0.2ms per cascade update without the need of using dirty
> buffers"* (2024-09-24)
> ▸ *"I got mine being able to handle 512^3 volumes with ease using a double buffered solution
> (so only 1 texture write each voxel) and dirty buffers to cull chunks from further compute
> updates once they stabilize and can be cached"* (2025-06-18)
> ▸ bottleneck: *"the propagation … is the bottleneck"*, not injection (2025-06-18)

**Dirty culling** is a Noita idea, adapted: "*a 1bit buffer where each dirty chunk/area spans
over an 8x8 or 16x16 area … noita extends it by using bounding boxes and not just one state
for the entire chunk, but on the GPU the performance benefit of such bounding boxes isn't that
big*" (2024-03-18). Once an area's lighting stabilises it is never updated again (2024-02-10).

### Light leaking and injection — where the real work is

He insists CAGI doesn't leak, and the reason is a **precomputed per-block-face surface
approximation** driving anisotropic injection:

> ▸ *"cagi doesn't have leaking problems … basically what you do is to gather surface
> information of each block face, which is then used for smart anisotropic injection (remember
> cagi is anisotropic too). so basically you loop through each block face plane, cast rays into
> that plane vertically down and then gather how many surface voxels are solid and how many
> aren't, which colors the surface voxels are and which ones are emissive … of course you do
> this at setup time not at runtime and cache the surface approximations somewhere and then use
> it during injection"* (2024-07-03)

If you already keep a world LOD it's "*mostly for free*"; worst case, fall back to an
isotropic voxel average (2024-07-03). Related later remark: **voxy and Distant Horizons don't
prevent leaking in their LOD generation** — "*it just downsamples with opacity weighting*"
(2026-03-20).

### The colour bug and the "colour diffusion" hack

Non-white sun colours produced dark-red bleeding (2024-02-28). Cause: subtractive per-channel
loss at 10-bit precision — unequal channels diverge visibly as they darken. His fix, applied
per cell:

```wgsl
let diw = 0.925;              // diffusion weight
let dia = (1.0 - diw) * 0.5;
let dr = (r * diw + g * dia + b * dia);
let dg = (r * dia + g * diw + b * dia);
let db = (r * dia + g * dia + b * diw);
```

> ▸ *"light physicians probably want to slap me for this lmao"* (2024-02-28)

Constraint behind it: **10-bit colour is the memory ceiling**. Sun colour is
`4.0 * vec3<f32>(1.0, 0.9, 0.75)`, already near the 10-bit integer limit. "*can't afford more
than 10bit per channel*"; `rg11b10` rejected for its yellow tint (2024-02-28).

### What CAGI is used for, beyond looking nice

This is the part most relevant to anyone integrating simulation with rendering:

> ▸ *"the skylight and shadows are propagated with the CA lighting too, so it's like a complete
> drop-in light solution that works both for visuals but also gameplay/simulation"* (2024-03-18)
> ▸ *"I only use ray traced ambient occlusion and ray traced reflections on top. everything else
> is handled by the CAGI"* (2024-03-18)
> ▸ *"something I've always wanted to create myself and what was the initial idea of CAGI too,
> having it be used for both lighting and gameplay by doing it with fixed point operations only
> to stay deterministic"* (2025-01-07)

Unresolved tension he flagged himself: precise shadow-map shadows reach places CAGI shadows
don't, "*so it can create confusion*" — his plan was to cancel shadow-map shadows against CA
shadows (2024-02-17).

### CAGI, honestly appraised

> ▸ *"I've gone with the quality goal of just convincing enough, there are far more realistic
> techniques out there each with their pros/cons"* (2025-11-30)
> ▸ *"it's exactly the aspect I like the most about CAGI: it's reliable in terms of stability
> and performance. it works in mostly every scene, no matter how complex the light situation
> is"* (2024-12-19)

Open ideas: **caustics** — "*I actually did some drafting about caustics a few days ago, it's
definitely possible since both sunlight shadows and skylight is encoded in the light volume*"
(2025-10-30). And **matrixification**:

> ▸ *"a few days ago I realized that I can almost completely matrixify my cagi algorithm …
> convert the whole propagation stuff and everything into matrix-only ops. the only non matrix
> operation would be the light injection part for stuff like emission"* (2024-04-17)

Motivation was tensor cores / AI chips; WebGPU doesn't expose `VK_NV_cooperative_matrix`, so
it stayed an idea.

### Why not radiance cascades

Consistent, measured position — not dismissal:

- 2023-10-15: a 2d RC shadertoy runs at **60 ms/frame on his M1 MacBook**.
- 2024-07-03: *"all implementations I saw ran quite slow and also had leaking issues which in a
  voxel world is not good"*.
- 2023-11-09: *"with voxel scenes I found that the light leaking part is crucial or sometimes
  even more visible than with triangle scenes — had a lot of leaking issues with the previous
  cone tracing which is why I had to drop the technique"*.
- 2023-11-08: genuinely impressed when Mytino got RC to **0.3 ms/frame on a GTX 970**.
- 2024-10-17/18: **he did implement RC himself** — 2d screen-space, for a *2d automation game*,
  with a JFA SDF; hit ringing artifacts dependent on camera zoom.
- 2026-07-29: *"that's definitely the best looking implementation of RC that I've seen so far —
  I wish the paper provided more performance related numbers"*.

---

## 4. The recurring thesis: caching beats sampling

Worth extracting because it explains every architectural choice he makes.

> ▸ *"what's mostly interesting about grid based visibility is the caching aspect. ray tracing
> performs so much worse compared to it, because it's usually applied without any potential
> caching mechanism"* (2024-12-19)
> ▸ on rays re-reading the same tile: *"yep it's essentially cache nuking"* (2024-12-19)
> ▸ *"even if it wouldn't perform as good, you could just use ray tracing instead of propagation
> to fill the grid volume instantly and it would still perform most likely better than ray
> tracing"* (2024-12-19)
> ▸ *"I like these approaches because they stabilize in a finite time"* → *"**YES**"* (2024-12-19)
> ▸ *"and I can't hear the argument anymore that world-space techniques don't scale because of
> storage requirements. **just cascade it ffs**"* (2024-12-20)
> ▸ *"if you want to solve for multiple bounces, then usually world-space probing and filtering
> is the way to go"* (2024-12-19)

He also gave a **1-bit CA shadow propagation** technique (Dec 2024) as a GBV alternative: per
cell, a single directional neighbour lookup toward the light propagates the neighbour's shadow
state; extensible to multiple bits for soft shadows and filterable (2025-12-12). For many
lights: a **priority queue** with update rate falling off with distance, and cascades where
distant lights degrade to 1-bit hard shadows (2025-01-09).

And a speculative "grid-based ray cache" he sketched on paper (2024-12-19): probes store 4
diagonal directions, spawn rays capped at ~4 probes, and if they miss they **combine their own
value with the last probe hit** — so a probe lookup tells you whether that direction hits,
without long rays.

---

## 5. Entities — three designs in eight months

### The Nov 2023 design exploration (before splatting)

The engine's world representation, stated plainly:

> ▸ *"there is a main grid where each voxel contains a material id, and that material id points
> to a 8x8x8 volume"* (2023-11-23) — i.e. **sub-voxels at 8³ per main voxel**

The entity idea he reasoned out live: a **second volume centred on the camera** where each
cell holds an *index into an entity volume*; entities voxelised into their own volumes in
real time at distance-dependent LOD; traced with the same non-sparse octree as the world;
cascadable so distant entities still render at low res. Edge case he spotted immediately:
multiple entities in one reference cell — "*maybe the entity reference volume could hold
potentially multiple references … like maybe up to 4*". Entity volume at **main-voxel scale,
not sub-voxel** — "*if I do it at sub-voxel scale it would likely be way too slow and take too
much storage*" (2023-11-23).

### Voxel splatting (2024-02, replacing teardown-style box rasterisation)

Teardown's approach — render box backfaces, derive the front plane, then DDA — was what he
started with. He replaced it after finding [voxplat](https://github.com/kosshi-net/voxplat):

> ▸ *"I just need the screen uvs and clamp the ray origin to the entity boundings, so it doesn't
> really matter which geometry I use, splats seem a perfect fit here"* (2024-02-22)
> ▸ *"will take 20mins to implement and be more efficient, simple as that"* (2024-02-22)

The whole splat-bounds computation, posted verbatim (2024-02-23):

```wgsl
var mi = vec3<f32>(FLT_MAX);
var ma = vec3<f32>(FLT_MIN);
for (var ii = 0u; ii < 8u; ii++) {
  let offset = (vec3<u32>(ii) >> vec3<u32>(0u, 1u, 2u)) & vec3<u32>(1u, 1u, 1u);
  let worldPos = entityPos + vec3<f32>(offset) * entitySize;
  let vertexPos = uCamera.ViewProjectionJitterMatrix * vec4<f32>(worldPos, 1.0);
  let clipPos = (vertexPos.xyz / vertexPos.w);
  mi = min(mi, clipPos);
  ma = max(ma, clipPos);
}
```

**Frustum culling** falls out of it for free: test that 2d clip rect against −1…+1, `atomicAdd`
survivors into an indirect draw list rebuilt every frame. Same mechanism as his particles.
"*it's absurdly fast*" (2024-02-23). Everything is indirect "*because the entire game state is
on the GPU*".

Measured the same day: **2 048 individually animated entities** — "*my GPU is completely bored
by it*" — then **50 000 entities**, at which point "*frustum culling is really easy to add too,
but not really necessary since there is barely any overhead in the vertex shader*" (2024-02-23).

### Final form: analytic OBB + DDA, no voxelisation volume

> ▸ *"first I voxelized into 3d textures but it scaled horrible. now I use a voxelization shader
> that uses a mix of analytic ray obb intersection and then voxel dda to give the pixelated
> look"* (2024-07-20)
> ▸ *"I calculate the screen space boundings based on the world space boundings of the entity and
> then rasterize a quad with that. and for each pixel I loop through the entity bones and perform
> an analytic obb intersection and then within the intersection boundings I do a regular dda
> resulting in the pixel perfect voxel look"* (2026-05-26)

Practical advice he gave Dapper Core on holes: **slightly expand the bounding box**, compute
min/max ray length, and cap DDA steps — "*often you can also limit the dda steps to a
relatively low max count since artifacts won't be too visible if voxels are tiny*" (2025-05-23).

Clipmap insertion folded into the existing raster fill pass: front intersection positions
written during fill, back positions computed too — "*basically just 2 texture writes for the
clipmap volume insertion per entity node, and that's pretty damn efficient. I think that's the
best entity rendering system I've come up with so far*" (2024-02-26).

**Pass count grew visibly:** "*9 passes just for entities so far lol*" (2024-02-24) → 11 by
March (per the other doc). ○ Useful as a cost datapoint for GPU-driven designs.

**Grid-aligned entity voxels are a style choice, and expensive:** "*by choice and was actually
pretty hard to implement*" (2024-07-20); "*I'm just sick of hytale and minecraft sharp entity
geometry*" (2023-11-23). Warned that his GI would make them flicker; his answer was that with
the grid approach they're treated exactly like world voxels, leaving only temporal lag — and
that he might not include entities in GI at all, "*maybe for shadows and some fake ambient
occlusion*" (2023-11-23).

---

## 6. The function-pointer problem — a full worked solution

This is the single most complete piece of engineering reasoning in the channel, running
2025-10-23 → 2026-06-26.

**The problem.** Entity types (`fish`, `muraine`, `zombie`) share base behaviour but need
per-type code. GPUs have no function pointers, so it becomes a switch table.

> ▸ *"I know from falling sand sim shaders that GPUs scale horribly with different code
> execution paths — I usually ended up just smashing each cell behavior into one giant switch
> table and yeah it performed pretty bad"* (2026-06-26)
> ▸ *"I also found that at least in WebGL, the more code you have in a shader, the worse it
> performs even if the code is inside a big `if (false) {}` statement. so I'm generally trying
> to keep shaders as small as possible and otherwise distribute the logic among threads"* (2026-06-26)
> ▸ *"what I really wish for are native function pointers in shaders … the problem is that for
> native function pointers you need access on stack level. languages like slang etc. probably
> just compile to switch tables"* (2025-10-23)

**The before**, posted verbatim (2025-12-02) — note the sound queue, which matters for §8:

```wgsl
var soundEntry: SoundQueueEntry;
if (entity.Type == ENTITY_TYPE_FISH) {
  OnUpdateFish(&entity, &uPlayerBuffer.Entries[playerId], &soundEntry, delta, &seed);
} else if (entity.Type == ENTITY_TYPE_MURAINE) {
  OnUpdateMuraine(&entity, &uPlayerBuffer.Entries[playerId], &soundEntry, delta, &seed);
} else if (entity.Type == ENTITY_TYPE_ZOMBIE) {
  OnUpdateZombie(&entity, &uPlayerBuffer.Entries[playerId], &soundEntry, path, delta, &seed);
}
if (RandF01(&seed) < listenerPlayChance && soundEntry.SoundId > 0u) {
  if (atomicAdd(&uSoundQueue.Index, 0u) < MAX_SOUND_QUEUE_ENTRIES) {
    uSoundQueue.Entries[atomicAdd(&uSoundQueue.Index, 1u)] = soundEntry;
  }
}
```

…and the compaction pass that feeds it:

```wgsl
fn main(input: ComputeInput) {
  let entityIndex = input.GlobalInvocationId.x;
  var entity = uEntitySrcBuffer.Entries[entityIndex];
  if (entity.Health > 0u) {
    let dstIndex = atomicAdd(&uEntityDstBuffer.InstanceCount, 1u);
    uEntityDstBuffer.Entries[dstIndex] = entity;
    atomicAdd(&uEntityDstBuffer.EntityTypeCount[entity.Type], 1u);
  }
}
```

**The solution.** Push into a **per-entity-type buffer** as well as the shared one (storing the
shared index for the relationship), then run **one indirect dispatch per entity type with a
type-specific pipeline** — "*which I think solves the thread divergence problem allowing
hundreds of different entity types*" (2025-12-02). Realised in the same thread that this is
what RT drivers do: "*SBT is basically just about assigning a material to a specific shader and
dynamically invoke it as efficiently as possible, it's pretty much the same problem*" (2025-12-02).

**Shipped and confirmed:** *"implemented this today and it turned out to work indeed —
parallelizable multi type entity code (almost) for free"* (2026-06-25). Mechanics: CPU records
one dispatch per entity type into a command buffer once; **dispatch size comes from a GPU
buffer** (2026-06-26). He never benchmarked the old path.

Two smaller findings from the same thread:

- **A datarace he was talked out of.** Jasper pointed out the double `atomicAdd` guard is racy —
  threads can all pass the check then all increment past `MAX`. He removed the second add.
  Out-of-bounds writes are discarded on Vulkan and WebGPU, so it was latent, not fatal (2025-12-02).
- **Zero-thread dispatch overhead** is the known remaining cost, and what he actually wants is
  GPU-side dispatch: "*I wish WebGPU would support something to skip the CPU roundtrip
  entirely, since the CPU is really just responsible for invoking stuff, but has no state of the
  actual game at all. Vulkan got support for something like this where the GPU is basically
  treated as a standalone unit that can create and dispatch shaders on its own*" (2026-06-26).
  ○ He means device-generated commands (`VK_EXT_device_generated_commands`).

He also flagged a WGSL papercut: **structs must be duplicated when some fields need atomics**
(2026-06-26) — an `EntityAtomicBuffer` / `EntityBuffer` pair differing only in `atomic<u32>`.

---

## 7. The engine shader tree — the most valuable single artifact in this export

Posted 2026-06-17 with LOC per file, while porting to WESL. This is effectively a full engine
map. Reproduced verbatim:

```
shaders/
├── auto-exposure/   auto-exposure.wgsl (49), luminance-downsample.wgsl (64)
├── bloom/           bloom-downsample.wgsl (63), bloom-filter.wgsl (54), bloom-upsample.wgsl (57)
├── entity/          entity-depth-blit.wgsl (21), entity-fill.wgsl (230), entity-grid-clear.wgsl (20),
│                    entity-grid-update.wgsl (34), entity-indirect-clear.wgsl (20),
│                    entity-indirect-update.wgsl (33), entity-insert.wgsl (130),
│                    entity-model-transform.wgsl (188), entity-rasterize.wgsl (46),
│                    entity-splat.wgsl (69), entity-update.wgsl (701)
├── include/
│   ├── entities/    fish.wgsl (104), muraine.wgsl (140), zombie.wgsl (218)
│   ├── animation.wgsl (23), bits.wgsl (62), cell-light.wgsl (117), cell.wgsl (80),
│   ├── color.wgsl (400), dirty.wgsl (39), entity.wgsl (290), fluid.wgsl (94),
│   ├── inventory.wgsl (28), item.wgsl (108), particle.wgsl (133), path.wgsl (67),
│   ├── player.wgsl (87), restir.wgsl (93), sdf.wgsl (35), sh2.wgsl (74),
│   ├── shading-shared.wgsl (81), sound.wgsl (90), structs.wgsl (119),
│   ├── trace-packing.wgsl (84), trace-shared-cardinal.wgsl (9), trace-shared-fixed.wgsl (32),
│   └── trace-shared.wgsl (151), utils.wgsl (707), voxel.wgsl (203)
├── item/            item-{depth-blit,fill,grid-clear,grid-update,indirect-clear,indirect-update,
│                    insert,rasterize,splat,update}.wgsl  (21…359)
├── particle/        particle-{fill,indirect,manager,rasterize,update}.wgsl (19…161)
├── player/          player-entity-update.wgsl (170), player-local-camera.wgsl (81),
│                    player-path-find.wgsl (215), player-update.wgsl (800)
├── registry/        cell-materials.generated.wgsl (32), entity-types.generated.wgsl (8),
│                    particle-types.generated.wgsl (9), sound-ids.generated.wgsl (49)
├── shading/         shading-ambient-occlusion.wgsl (82), shading-box.wgsl (92),
│                    shading-resolve.wgsl (160), shading-temporal.wgsl (105),
│                    shading-upscale.wgsl (126), shading-water.wgsl (220), shading.wgsl (240)
├── simulation/      simulation-animation.wgsl (181), simulation-dirty-clear.wgsl (24),
│                    simulation-dirty.wgsl (61), simulation-fluid-flow-0.wgsl (236),
│                    simulation-fluid-flow-1.wgsl (196), simulation-fluid-flow-clear.wgsl (76),
│                    simulation-light-diffusion.wgsl (271), simulation-light-dirty-clear.wgsl (24),
│                    simulation-light-dirty-merge.wgsl (39), simulation-light-dirty-propagate.wgsl (43),
│                    simulation-light.wgsl (486), simulation-terrain.wgsl (157)
├── sound/           sound-simulation.wgsl (242), sound-simulation-resolve.wgsl (93)
├── volumetric-fog/  volumetric-fog.wgsl (125), volumetric-fog-resolve.wgsl (113)
└── blit (19), dof (67), firefly-filter (57), fxaa (75), icon-render (129), motion-blur (57),
    post-processing (186), quad (19), sharpen (41), taa (92), trace-merge (63), trace (97),
    voxel-mips (38)
```

○ What the tree tells you that the prose doesn't:

- **CAGI is `simulation-light.wgsl` (486) + `simulation-light-diffusion.wgsl` (271) + three
  dirty passes.** ~860 lines total for the entire GI system. The `dirty-merge` /
  `dirty-propagate` split confirms the dirty mask itself is propagated as a CA.
- **`restir.wgsl` (93) and `sh2.wgsl` (74) survive.** Residue of the 2023 stack — 2nd-order SH
  and ReSTIR are still compiled in, presumably for the AO/reflection path.
- **`trace.wgsl` is only 97 lines**, with `trace-shared.wgsl` (151) and a `cardinal` / `fixed`
  variant split. DDA is small; the shared helpers carry it.
- **`utils.wgsl` (707) and `color.wgsl` (400)** are the two biggest includes — colour handling
  is a first-class concern, consistent with §3's 10-bit fight.
- **`player-update.wgsl` (800) and `entity-update.wgsl` (701) are the largest files.** Game
  logic lives in shaders, which is exactly the divergence problem of §6.
- **`registry/*.generated.wgsl`** — the mod system generates WGSL for materials, entity types,
  particle types and **sound ids**.
- **Fluids are three passes** (`flow-0`, `flow-1`, `clear`), matching his advice to split CA
  axis transfers across passes for stability (§9).
- **Sound is a real subsystem**: 242 + 93 + 90 lines. See §8.

---

## 8. Audio — the atrium-relevant thread

Runs the whole length of the channel. He has wanted ray-traced, GPU-synthesised sound since
2023 and now has `sound/` passes in the tree.

**2023-04-25, first mention** — and he asked *for* help, which is notable:

> ▸ *"how good is your experience with audio processing? this is something I have no idea about
> yet, but something I want to add to my engine at some point — sort of like allowing people to
> synthesize sounds with some basic tools and also ray trace the sound"*
> ▸ *"in case you didn't know I have this redstone/circuit system which lets you build quite
> complicated systems and I'm planning to allow doing sound synthesis there"*
> ▸ *"I just literally have no idea where to even start yet hah"*

Interested specifically in **8-bit retro sounds**, and had noticed demoscene on-the-fly
synthesis and shader-based audio on Shadertoy.

**2023-12-06, the CPU/GPU split — the architecturally important message:**

> ▸ *"I want to do procedural audio on the gpu so badly, but there is so much other shit I have
> to do"*
> ▸ *"for stuff like reverb it's definitely best to do it on the cpu/audio card, but audio
> generation and ray tracing the sound has to be done on the gpu in my case"*
> ▸ *"to my understanding stuff like reverb effects work with resampling and fft or something,
> which the gpu isn't too good at. so with ray tracing you only calculate the reverb intensity,
> but do the actual reverb effect on the cpu"*
> ▸ *"I think that's how unreal engine does it — they propagate sound through spherical harmonics
> volumes if I'm not mistaken"*
> ▸ *"webaudio has a bunch of existing tools to do reverb, you just calculate a bunch of
> parameters with ray tracing and then feed it in"*

○ That is exactly the seam atrium already has: **trace on GPU to derive parameters, synthesise
and filter on CPU.** He arrived at it independently and for the same reason (FFT is a bad GPU
fit).

**On prior art:** SEUS's Minecraft audio mod is "*a bidirectional path tracer and was limited
on the amount of audio sources*" and "*really slow*" (2023-12-06). Lin's water sound had broken
locality — "*maybe he calculated the sound based on the center of the water sim volume*"; and
"*doing the sound per particle is probably too slow*" (2023-12-06). He also asked Gabe about
Teardown's room-size-based reverb: "*would be really interesting to get some details about
that*" (2024-12-30) and floated "*you could probably use hwrt for realistic sound too*".

**Where it landed.** In the 2025-12-02 code (§6), sound is a **GPU→CPU queue**: entity update
functions fill a `SoundQueueEntry`, gated by `RandF01(&seed) < listenerPlayChance`, atomically
appended to `uSoundQueue`. `sound-ids.generated.wgsl` means **sound ids are mod-registry
data**. And by 2026-06-21 the CPU side is explicit:

> ▸ *"my cpu code is mostly render and compute pipeline spawning code — and some audio
> processing code. most logic is on the gpu written in shaders"* (2026-06-21)

---

## 9. CA as the universal hammer — physics, terrain, fluids

> ▸ *"I'm planning to model the entire physics through pure CA. light was the first step, fluid
> is next and I hope together with fluid I can also solve other materials"* (2024-02-23)
> ▸ *"my favorite CA is definitely flood fill — so simple yet so powerful"* (2024-02-23)
> ▸ *"ray tracing will always be a ton slower than CA"* (2024-02-09)
> ▸ *"classic rigid body sim sounds horrible to implement on the GPU and bad for mass scale. if
> you take advantage of the fact that you don't deal with objects and instead atoms/cells and
> their interaction then it sounds a lot more suitable to the GPU"* (2024-02-23)

### Structural integrity — a years-old idea, never built

> ▸ *"one thing that I literally want to figure out for years is modeling structural integrity
> through pure CA … my method was just flood fill based: propagating a flood fill through
> materials based on their mass, and another flood fill for a sort of negative/inverse mass —
> like imagining two separate fluids going through solids and another one through air. and by
> the difference between them you can have structural integrity"* (2024-02-23)

Inspiration: the Medieval Engineers structural-integrity wiki image. Extension to rigid bodies:
**sticky energy** to hold materials together, **force energy** to break them — "*pull and push
energy*" — and he claims rotation falls out of it (2024-02-23). All still unproven; he promised
an article "*at some point*".

### Practical CA advice he gave others (reusable)

On **oscillation / checkerboard artifacts** (2024-02-25):
1. Split the update — pass 1 transfers top/bottom faces, pass 2 left/right.
2. Better: don't read the neighbour's *last* state, **compute the neighbour's future state with
   the same rule** when reading it.
3. *"just design your algorithm that problems like this just don't happen lol — sketching on
   paper usually helps a ton"*.

Also: double-buffer and only ever write the cell you own; single buffering is possible but
trickier (2024-04-20). Shared memory helps when memory fetch is the bottleneck (2025-06-20).
`multigrid` / mipmap diffusion acceleration was something he was "*trying to figure out*"
(2024-02-13) but ruled out for light (propagation is already near-instant) while flagging it as
potentially "*incredibly useful*" for the fluid/mass sim (2024-02-25).

### Terrain generation — CA, in passes

His answer to Pascal's biome question, 2026-08-01 (the exchange that prompted this document):

> ▸ *"usually it's stacking and blending noise layers together — that's usually the foundation
> of all these methods. otherwise, what I'm using myself for terrain generation is cellular
> automata"*
> ▸ *"it's done in multiple passes where each pass has a specific task, like the first task being
> the basic terrain shape with caves etc. and the second pass adding vegetation etc."*
> ▸ *"no you can use pure cellular automata, you just need some basic noise RNG for randomness …
> randomness for like spawning grass randomly etc."*
> ▸ biomes: *"part of the terrain pass or a second pass that e.g. spawns biome cells into the
> terrain, which then get propagated around randomly"*

Trees grow over multiple GPU ticks, "*mostly depends on the initial tree stem value, which
indicates how high the tree will grow*" (2024-04-30). The gating mechanism for multi-pass
generation is the **dirty-rect system**: when the world state stops changing, run the next
rule set — which is how he planned to generate tree houses, houses and villages (2024-04-30).

### Fluids

- 2d prototype first, pressure-based CA. On the classic Terraria/Starbound pressure method:
  "*it's single buffered though and fp based and loses mass — experimented with it a while until
  I started my own implementation, it's a good basis though for a scalable fluid sim*" (2026-04-12).
- Rejected particle methods outright. On MPM/particle-in-grid: "*doesn't it boil down to
  basically just being an optimization for speed?*" (2024-02-22); on Lin's 200k-particle water at
  30 fps: "*that's actually not that much … definitely not going to do that lol*".
- Ray marcher for fluid rendering added ~Sept 2024, "*relatively slow and there is not much room
  to accelerate it except upscaling*" — the reason a GTX 1070 target is "*ambitious*" (2024-09-24).
- **Port 2d → 3d was starting 2026-06-25**: "*soon going to port over the fluid sim from 2d back
  into the voxel engine, super excited about that*".

### Transparency

Never solved to his satisfaction. Plan: "*in my miptree I could just use another bit to indicate
transparency mode and then switch the ray tracing from air to transparency or the other way
around*" — i.e. reuse empty-space skipping (2024-02-15). Later, shown a transparent-voxel
shadertoy: "*when I experimented with transparent voxel tracing I got similar [quad] artifacts …
unfortunately I don't remember exactly how I fixed the transparency issue with dda*" (2025-05-20).

○ Directly relevant to `docs/transparent-voxels-plan.md`: his conclusion is that transparency
classes belong as **bits in the acceleration structure**, not as a separate pass.

---

## 10. Determinism — the constraint under CAGI

> ▸ *"so yeah… no floats for anything requiring determinism. at least on the GPU it's a no-no"*
> (2024-03-29)
> ▸ *"what I'm referring to are the fp operations itself — like just dividing a float by a float.
> that's the part where apparently the determinism breaks because the spec doesn't seem to
> enforce it"* (2024-03-29)
> ▸ *"yeah it seems the only way is to emulate float operations through uint"* (2024-03-29)
> ▸ found and shared `VkPhysicalDeviceFloatControlsPropertiesKHR`, then: *"ok vulkan seems the
> only api exposing it"* (2024-03-30)

He proposed building **a website to crowdsource GPU determinism data** across GPUs, browsers,
WebGPU implementations and backends (2024-05-24), noting the WebGPU CTS checks reproducibility
rather than determinism. ○ Never built, as far as this channel shows.

This is *why* CAGI is integer-only, and why it can drive gameplay (mob spawning, mining, fluid
interaction) rather than only visuals.

---

## 11. Platform, tooling, working style

| Topic | Detail |
|---|---|
| API | WebGPU/WGSL. **~15 000 lines of WGSL by 2024-03-29.** Benchmarks "*directly inside webgl/webgpu*". |
| No wasm | *"no … my cpu code is mostly render and compute pipeline spawning code and some audio processing code"* (2026-06-21). |
| Shader languages | *"glsl was horrible and hlsl was the one I found the nicest (except the extremely buggy compiler). wgsl is somewhere between both and just lacks a few extras like a module system which wesl adds on top"* (2026-06-17). |
| Debug UI | `dat.gui` (2024-03-20). Later: `html-in-canvas` — *"simplifies GUI stuff on the GPU a lot"* (2026-04-10). |
| Profiling | Nsight attached to Chrome (needed specific Chrome args + disabling the separate render thread; works for WebGL, **not WebGPU**) — 2024-03-29. Also the ShaderToy Chrome plugin for per-pass GPU timings, and the **webgpu-inspector** extension ("*like an early nsight alternative, even lets you edit the shaders within dev tools*"). |
| Style | *"strictly doing bare metal data oriented programming"* (2026-06-17). Anti-OOP: *"few years ago I switched to the more 'dumb' function/data oriented style and I never wanna go back"* (2023-04-29). |
| Environment | Windows, VS, VSCode, GitHub. Rust jokes are a running bit, not a technical position. |
| WGSL gripes | No right-shift on integer *vectors* (tint limitation, 2023-12-19); wants native function pointers; struct duplication for atomics. Pointer syntax sugar landed 2025-10-23 and he liked it. |

**On building GPU-driven engines** — the most quotable thing in the channel, aimed at why Gabe
Rundlett's engine stalled:

> ▸ *"I could be wrong but I feel like gabe implemented a ton of features at the same time and
> didn't focus on a few ones instead, get them to fully work and be really stable and only then
> move on to the next thing. so at some point he had a lot of features but a lot of them didn't
> properly work or weren't properly enough implemented. this is what I found to be absolutely
> crucial because otherwise you kill your whole codebase very quickly. I had exactly that problem
> in the beginning too when I moved to gpu driven, and found that you have to be really patient
> to add new features to it. and in the beginning I had exactly the same moments too where I just
> wanted to give up on it since it became too hard, but patience and careful design are the key
> things to solve this issue"* (2024-07-01)

**On not publishing** — the reason this scrape is the documentation:

> ▸ *"I shared multiple times how it works, just not directly the code but the most important
> core ideas of it instead. on vgd multiple people managed to somewhat replicate the results by
> that"* (2024-09-24)
> ▸ *"not a secret just don't have enough energy atm to write an article, but checkout the vgd
> discord, a few people there experimented with the cagi idea"* (2024-09-24)
> ▸ *"yeah I was thinking about a blog post, but soon people started replicating it based on the
> information I've shared so I thought it's just fine — and after all I'm developing a game and
> have no plans to run a blog on top of that"* (2026-08-03)

---

## 12. The 2026 modding architecture

> ▸ *"currently refactoring my voxel engine to be mod friendly and discovered wesl-lang.dev"* (2026-06-17)
> ▸ *"the core game logic is defined as a mod itself through a mod registry system like minecraft
> has — so even the 'vanilla' game code can be modded"* (2026-06-17)
> ▸ *"will be fun to see people write WGSL based mods, I don't think this was done before lol"* (2026-06-17)
> ▸ *"modding api is finished now and tomorrow I'll start on the mod editor, which basically is
> just an UI wrapper over the file based mod api"* (2026-06-25)
> ▸ *"also gonna rework the game UI from scratch. I'm also thinking about turning the game menu
> background into a live engine scene (like a calm cave scene), which should now easily be
> possible through the mod api"* (2026-06-25)

○ The `registry/*.generated.wgsl` files in §7 are the output of this: materials, entity types,
particle types and sound ids are all mod-declared and codegen'd into WGSL.

---

## 13. Activity timeline and gaps

Messages per month (x1m4 only). Bursts correspond to active development on a subsystem;
the two 400+ months are CAGI (2024-02) and the entity renderer (2024-03).

```
2023-03 ███████████████████ 208   2024-09 █████ 56      2025-11 ▌ 6
2023-04 █████████ 94              2024-10 █ 11          2025-12 ██ 27
2023-05 ▌ 9                       2024-11 ▌ 7           2026-02 ▏ 2
2023-06 ▍ 6                       2024-12 ██████ 69     2026-03 █ 18
2023-07 ▊ 12                      2025-01 █ 18          2026-04 ▊ 9
2023-08 █ 17                      2025-04 █ 18          2026-05 ▊ 9
2023-09 █ 17                      2025-05 ██ 22         2026-06 ██████ 61
2023-10 █████ 55                  2025-06 ▍ 7           2026-07 ▋ 8
2023-11 █████ 57                  2025-08 ▏ 1           2026-08 █ 17
2023-12 ███████ 74                2025-10 █ 16
2024-01 ██ 24
2024-02 ████████████████████████████████████████████ 484
2024-03 ██████████████████████████ 263
2024-04 █████ 56
2024-05 ██ 21
2024-06 ▍ 4
2024-07 █████ 57
2024-08 █ 14
```

**Silences longer than 20 days** — each is a stretch where his engine work is invisible in this
channel and likely happened on the voxelgamedev server or nowhere public:

| Gap | Length |
|---|---|
| 2025-01-14 → 2025-04-27 | **103 days** |
| 2025-08-01 → 2025-10-02 | 62 days |
| 2025-12-24 → 2026-02-10 | 48 days |
| 2025-06-20 → 2025-08-01 | 42 days |
| 2026-02-10 → 2026-03-16 | 34 days |
| 2023-07-03 → 2023-08-03 · 2025-10-30 → 2025-11-30 | 31 days each |
| 2025-05-23 → 2025-06-18 · 2023-08-22 → 2023-09-15 | 26 / 24 days |
| 2024-05-31 → 2024-06-23 · 2026-04-29 → 2026-05-21 · 2025-10-02 → 2025-10-23 | 23 / 22 / 21 days |

○ **2025 is the notable hole.** He posted 82 messages in all of 2025 vs 1 022 in 2024. Between
the 2024-09 "*ready for a public demo*" claim and the 2026-06 modding refactor there is almost
no engine-progress reporting here at all. Whatever happened to the demo happened off-channel.

---

## 14. What to look up next, if you want it

Ranked by how much it would add:

1. **The voxelgamedev (VGD) Discord, keyword `cagi`.** He says three separate times that the
   real explanations are there, not here (2024-09-24, 2025-10-30, 2026-08-03). Also named:
   people who **independently reimplemented CAGI** — `sweg`, `👾Rareș👾`, `Dapper Core` — whose
   attempts would validate or contradict §3. This is by far the highest-value follow-up.
2. **His 2024-02-22/23 Discord links into VGD** (`661650973382672384`) chasing the John Lin
   density-field image and Ken Silverman material — dead links here, live there.
3. **The 130 image/video attachments** in his messages (`Screenshot_*.png`, `*.mp4`). The export
   has filenames only. The CAGI colour-diffusion comparisons (2024-02-28: `Screenshot_271/273/
   274/275/276/277.png`), the entity splatting videos (2024-02-23: `asfd3rfsd.mp4`,
   `dokmasd.mp4`), the 9-pass entity screenshot (2024-02-24: `Screenshot_259.png`) and the
   dirty-propagation video (2024-12-19: `4tdfsgsdfg_1.mp4`) are the ones that would settle open
   questions.
4. **The other channels in this guild** — the general/chat channels you offered. His engine
   posts here are answers to other people's questions; announcements and screenshots of his own
   milestones plausibly live elsewhere, which would fill the 2025 hole.
5. **`temp/folder 1/*.bin` and `*.txt`** — the eight `science.bin` files were not parsed for
   this document, and `0197-message.txt` / `0210-message.txt` / `0368-dreams_renderer_2019.txt`
   are other people's material (a Vulkan RTX pipeline in C#, Ken Silverman's PND3D rendering
   tricks, and Dreams renderer notes respectively). Worth reading on their own merits, not for
   x1m4.

**Dates worth confirming from a source that has them:** when the deep-sea idea actually started
(he says "*about a year old*" in 2024-02, implying ~early 2023, but the channel starts too late
to see it); when CAGI was first written (he says "*a few years ago*" in 2026-08-03, and "*yes
I've created cagi a few years ago*", but the first prototype visible here is 2024-02-08 — so
either the 2d version predates the channel or he is rounding).

---

## 15. Ten things worth stealing

1. **The 1-spp argument.** Test any GI implementation with a large flat-walled room, no
   sunlight, one torch placed and removed. If it only works with a cranked temporal filter, the
   approach is wrong, not the tuning.
2. **Caching over sampling.** Techniques that *stabilise in finite time* (CA, GBV, propagation)
   beat techniques that re-derive the same answer every frame. "Ray tracing is cache nuking."
3. **Cascade everything, and stop worrying about volume memory.** Cascades embedded in texture
   depth; inject from the higher cascade at scroll boundaries to hide edges.
4. **Integer/fixed-point simulation buys determinism, which buys gameplay.** GPU float ops are
   not deterministic across vendors. This is the reason CAGI can drive mob spawning.
5. **Dirty masks propagated as a CA.** 1 bit per 8×8/16×16 chunk, 5× speedup measured
   (2 ms → 0.4 ms at 256³). Skip Noita's bounding boxes on GPU — the win doesn't materialise.
6. **Per-type indirect dispatch instead of switch tables.** Compact into per-type buffers,
   dispatch one pipeline per type with GPU-supplied thread counts. This is what an RT shader
   binding table is.
7. **Keep shaders small even if branches are never taken** — in WebGL, dead code inside
   `if (false)` still costs.
8. **Denoise raw irradiance with albedo forced to white; apply textures afterwards.** Cheap and
   preserves texture detail.
9. **Analytic OBB + capped DDA beats voxelising into a 3D texture** for entities, on both
   memory and speed. Expand the bounds slightly to avoid holes.
10. **Trace on the GPU to derive acoustic parameters; synthesise and filter on the CPU.** He
    reached atrium's architecture independently, for the same reason: FFT is a bad GPU fit.
