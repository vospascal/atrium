# DDA + CAGI: a build guide from the corpus

Everything here is sourced from the six-channel Discord export in `temp/` — **not just x1m4**, but the
seven other people in that server who independently built the same things and posted numbers.
Attributions are load-bearing: when two people disagree, you need to know who's talking.

Cast, for reading the quotes:

| Who | Built | Why they matter here |
|---|---|---|
| **x1m4** | CAGI, VoxelChain | invented the technique; won't publish it |
| **TooManyLimits** | a CAGI kernel in Rust | the only complete propagation code in the corpus |
| **👾Rareș👾** | CAGI in 2D then 3D | x1m4 called it *"the best implementation of CAGI that I've seen"* |
| **sweg** | CAGI with >6 directions | the sharpest critic of it |
| **Dapper Core** | grid-based visibility, occupancy-bitmask tracing | best DDA optimization ideas; strongest argument against camera-relative probes |
| **Mytino** | radiance cascades, GPU fluids | only person with hard RC numbers and leak fixes |
| **Nameless** | real-time PT + fractals, professionally | best reflection/shadow consolidation |
| **Sam** | "candela" renderer | the maths behind infinite bounces |
| **Ivo** | screen-space radiance cache | probe reuse, reprojection |
| **dotted**, **eternal**, **IchBinAlex**, **Gabe Rundlett**, **sus**, **Kelvin**, **ReversedCausality** | various voxel engines | structure choices, packing, perf ladders |

Cross-refs: [cagi-reference-implementation.md](cagi-reference-implementation.md) for the annotated
kernel, [x1m4-index.md](x1m4-index.md) for the corpus map.

---

# Part 1 — DDA

## 1.1 Pick a 64-tree, not an octree

The single most actionable structural recommendation in the corpus, from **dotted** (2025-03-24):

> ▸ *"64 trees are pretty much the same as octrees, except instead of 2×2×2 nodes, you have 4×4×4
> nodes. this change makes for a few things: since your hierarchy is less deep (you don't go up/down
> the tree as often), you have **better cache coherency** during tracing, the tracing itself is **less
> costly** (because changing levels requires an 'expensive' division), you can store the **occupancy
> bitmask of the node inside a 64bit uint** in order to accelerate tracing within a node and other
> such tricks which you can't really do on octrees because of how tiny the nodes are"*
> ▸ and the bonus: *"there's a neat little trick using bitcount on the 64 bit occupancy bitmask of the
> node that allows you to know exactly how many childs it has"*

○ **This independently confirms your own brick-tagging measurement** that 4³ is the sweet spot and 8³
is too big to dedup. The 64-bit-mask-fits-a-register property is the reason, and it's what unlocks
§1.3.

**sus** shipped exactly this: 4³ bricks, traced by DDA via a 64-bit occupancy mask, attributes in 8×8
pixel blocks in one 16k texture.

x1m4's variant is a **non-sparse octree encoded in the mip levels** of the volume texture, built
without atomics:

> ▸ *"basically just fetch the 8 neighbors and then make the current voxel in the current mip level
> either 0 or 1 based on if any neighbor was solid or not"* — and the same pass can compute an
> **opacity factor** instead of a bit, which he used for *"super fast large scale ambient occlusion"*
> (2023-12-30)
> ▸ traversal: *"do `pos >> 1` and read the voxel from mip level 1 — this essentially gives you the
> information if the area of 8 voxels is empty or not with just 1 texture read"*
> ▸ why non-sparse: *"rebuilding times are really fast and cheap for non-sparse, which is necessary if
> potentially your whole scene is changing every frame"* (2023-03-16)

And **eternal**'s summary of why 3D-texture-plus-mips beats an SVO, quoting a practitioner:

> ▸ *"while sparse voxel octrees sound nice (and could possibly scale even to HUGE scenes), **using 3D
> texture with mipmaps (which is an octree!) outperforms it by a lot** … the only drawback is higher
> memory usage"*

## 1.2 Integer DDA, and why

x1m4 switched and stated the reason twice:

> ▸ *"in DDA you actually step through the scene with many steps at smaller and smaller scale, the
> further you step, **the more the result is dependent on the previous step result, and the more
> precision errors you get**. rasterization on the other hand is simply projection"* (2022-08-14)
> ▸ *"that exact problem is what made me switch to integer dda"* — integer for the stepping, float only
> for the distance `t` (2024-04-04, 2024-04-07)

○ The point isn't that floats are imprecise; it's that **the error compounds**, because each step
consumes the previous step's result. That's also why the fix for edge cases is padding, not more
precision: ▸ *"pad the ray position slightly away or inside the surface using the surface normal"*
(2024-12-09, to Alex who had exactly the "lands on the edge, samples the empty voxel" bug).

The canonical loop he hands people, in GLSL (2024-01-11):

```glsl
float t = 0.0;
float maxDist = 64.0;
while (t < maxDist) {
  if (getVoxel(mapPos)) break;
  mask = lessThanEqual(sideDist.xyz, min(sideDist.yzx, sideDist.zxy));
  t = dot(sideDist, vec3(mask));
  sideDist += vec3(mask) * deltaDist;
  mapPos += ivec3(vec3(mask)) * rayStep;
}
```

Reference shadertoys he points at repeatedly: `4dX3zl` (the classic voxel DDA) and
[iq's intersectors article](https://iquilezles.org/articles/intersectors/) for ray-box.

## 1.3 The optimization nobody else has written up: precomputed ray bitmasks

**Dapper Core**, 2024-07-16. This is the best DDA trick in the corpus and it's specifically for
*light* rays, not primary rays.

> ▸ *"It abuses the fact that for emissive blocks, the rays you trace have a **fixed max length before
> the contribution is effectively 0**"*
> ▸ *"so you can have fixed ray lengths and directions that you store in a precomputed bitmask … all
> you're storing is a bitmask that represents **which voxels the ray traverses through**"*
> ▸ *"**Determining whether a ray hits anything in a brick/tile is as simple as ANDing the ray's
> bitmask with that of the occupancy bitmask**"*
> ▸ *"you can compute the ray bitmask given just the ray origin and angle, and the bitmask remains the
> same for the traversal of the ray. We talked about how that overhead would likely be masked by the
> first memory load when traversing"*
> ▸ scope: *"you wouldn't use this for primary rays, though you could use the technique outlined here
> to **speed up DDA by a ton**"*
> ▸ honest cost: *"precomputed occlusion bitmasks, which has really bad memory scaling but is fairly
> fast at runtime"* (2024-11-17)

His worked example — AND the ray mask against the occupancy mask and read off the overlap:

```cpp
auto occupancy_bitmask = _mm256_set_epi16(
        0b0000000000000000,  0b0000000000000000,  0b0000000000000000,
        0b0000001111111000,  0b0000001111111000,  0b0000000000011000,
        /* … */ );
// AND with the ray's bitmask → the voxels where the ray overlaps solid blocks
```

He also credits **JellySquid** (author of Sodium) with a related idea: *"use **bitshifts to move the
cursor around**"* — which applies directly to CA propagation, not just tracing.

○ For a light volume this is the whole ballgame: emissive rays are short and bounded, directions are
few and fixed, so the mask set is small and the intersection test is one AND.

## 1.4 Three free wins

- **Fog.** **Salami**: ▸ *"you can easily add voxel fog by accumulating the density in the traversal
  loop."* No extra pass.
- **Smooth voxels, if you want them.** Salami again: ▸ *"DDA the voxels with a surface, then intersect
  the trilinear surface when you hit a voxel"* — [JCGT 11/03/06](https://jcgt.org/published/0011/03/06/),
  *"basically this paper, just without brickmaps."*
- **LOD to kill flicker, not to gain speed.** x1m4: ▸ *"based on camera distance and angle to a
  surface, I calculate a voxel surface subdivision amount and mip level"* — *"I'm doing lod too in my
  ray tracer to reduce flickering in the distance, but also **help the denoiser smooth out things more
  easily**"* (2023-09-26). He later found the raw speed gain wasn't worth it on its own: ▸ *"actually
  had LODs/mips for the sub-voxel models before, but the small performance gain was not really worth
  it"* (2024-03-23).

## 1.5 Two known unsolved costs

- **Transparency is expensive for everyone.** KosmosisDire: ▸ *"I have to step through every voxel in
  transparent areas to make sure each voxel is transparent until I hit a solid voxel. How are you
  getting around that?"* → radiofonika: ▸ *"I don't, I still have to step through the transparent
  voxels which sucks balls."* x1m4's plan was to put transparency in the acceleration structure — ▸ *"in
  my miptree I could just use another bit to indicate transparency mode and then switch the ray tracing
  from air to transparency or the other way around"* — i.e. **empty-space skipping, but for a
  transparency class**. Relevant to your `transparent-voxels-plan.md`.
- **Sparse sub-volumes behind each other.** ▸ *"if there are many sparse subvoxel volumes behind each
  other then I got really bad performance"* — fixed with a sub-voxel bounding clip (2023-03-22).

---

# Part 2 — CAGI

The annotated kernel lives in [cagi-reference-implementation.md](cagi-reference-implementation.md).
This part covers the things that document doesn't: energy conservation, injection, the directionality
weakness, and cascades.

## 2.1 The question everyone asks, answered three ways

**Rice7th** asked it exactly (2024-03-20), and it's probably your question too:

> *"The way I thought it works is to check the neighbors and get some of the light, but then next step
> everyone checks again and everything becomes brighter and brighter. **How do you do energy
> conservation with cellular automata?**"*

**Answer 1 — subtractive loss (x1m4).** Loss is applied unconditionally, per tick, *before* gathering,
and it's a subtraction, not a multiply: `max(LOSS, LIGHT) - LOSS`, which in the reference kernel is
`saturating_sub(DECAY)`. This matters more than it looks:

○ Multiplicative decay (`light * 0.98`) asymptotes and never reaches zero in fixed point, so cells stay
dirty forever and your dirty-chunk culling buys nothing. **Subtractive decay hits exactly zero**, which
is what makes culling pay. That single choice is why CAGI is cheap.

**Answer 2 — divide by slightly more than the neighbour count (ReversedCausality).** His reply to
Rice7th, and it's the most compact statement of the whole method:

> ▸ *"The easiest way is just you check nearby cells and blur them, but you need to have some amount of
> loss so like `this_cell = (manhattan nearby cells sum) / 4.1`. note that you also do **`=` instead of
> `+=`** so you basically know how much is being emitted"*

○ The `/4.1` instead of `/4` *is* the loss. And the `=` rather than `+=` is the trap — accumulate into
the cell and you've built a feedback amplifier.

**Answer 3 — take the max and don't worry (ReversedCausality again).**

> ▸ *"energy conservation is overrated; something like minecraft's style of just taking the max can work
> decently well (although doesn't support bounces)"*

○ Max-propagation is unconditionally stable but can't sum, which is where CAGI's real limitations come
from (§2.3).

**And the failure mode if you get it wrong** — x1m4's own, March 2023:
> ▸ *"my bounced lighting just infinitely spreads and never stops getting brighter … at some point it
> gets so bright, it looks like the gateway to heaven"* — cause: *"something attenuating above 1.0."*

Kelvin's version of the same bug: ▸ *"my direct rays at one point though were so biased that a single
voxel with emission could light up the entire world."*

## 2.2 Injection is where the work is

x1m4 is explicit that propagation is a few `max` ops and the difficulty is all in getting light *into*
the volume correctly.

**Emissive injection must be diagonal.** His own bug (2024-03-20): ▸ *"I just didn't output the emission
diagonally, but only face wise"* → square halos around point lights. Propagation is face-wise;
**injection is not**.

**The anti-leak mechanism — a precomputed per-face surface gather** (2024-07-03):
> ▸ *"you loop through each block face plane, cast rays into that plane vertically down and then gather
> **how many surface voxels are solid and how many aren't, which colors the surface voxels are and which
> ones are emissive**"* … *"of course you do this at setup time not at runtime and cache the surface
> approximations somewhere and then use it during injection"*
> ▸ and: *"cagi doesn't have leaking problems"* because of it.

Its ancestor was simpler and is a fine starting point: ▸ *"I generate an opacity value by **the amount
of active voxels in a sub-voxel volume** and use this value during the path tracing. if blocks are empty
inside then the lighting gets way too bright and light actually travels through"* (2022-11-16).
Isotropic version = one opacity per block; anisotropic = one per face. He confirms the per-face version
to KosmosisDire (2025-03-16): ▸ *"I pre-calculate an opacity map for each subvoxel model face (6 faces
in total) and use that during propagation to dim the passed through lighting — you can use isotropic
too but it's definitely a lot less realistic."* Limit: **16³ sub-voxels before inner leaking shows.**

**Sunlight injection goes in from the shadow map, including into air** (2024-07-16): ▸ *"so not only the
hit point at surfaces gets injected but also some sunlight within air"* — that's what makes volumetrics
free later. He later replaced the shadow map with CA-propagated shadow (2024-07-25) and locked the sun
to 45°, losing the day/night cycle.

**Injection can become the bottleneck if you do it naively.** Rareș hit this (2025-06-18):
> ▸ *"its quite bottleneck in the light injection phase since I am doing a lot of texture writes to
> **stamp some directions starters** around the light sources"*
> x1m4's counter: ▸ *"texture writes or texture reads? I got mine being able to handle 512³ volumes with
> ease using a double buffered solution (**so only 1 texture write each voxel**) and dirty buffers"* —
> and *"the propagation is the bottleneck"*, not injection.

○ Lesson: one write per cell per pass. If you're stamping patterns around lights you've moved work into
the wrong pass.

**Range is a performance parameter, not just an aesthetic one:**
> ▸ *"in 3d lights have to spread over a **lot smaller range** than in my 2d prototype, because otherwise
> a single light block just illuminates everything — **less range means a lot faster propagation**"*
> (2024-02-18)

## 2.3 CAGI's real weakness, from the people who built it

Don't take this from me — take it from the implementers, because it's the thing that will decide whether
you ship it.

> **sweg**: ▸ *"cagi is cool in theory but its **really bad at preserving directionality** so it always
> looks weird to me"* (2025-04-23)
> **x1m4**, conceding: ▸ *"I've gone with the quality goal of just **convincing enough**, there are far
> more realistic techniques out there"* (2025-11-30)
> **x1m4** on the mechanism: ▸ *"if too many light sources inject themselves nearby then the
> directionality gets worse and worse … **brighter light sources tend to eat up less bright ones** and
> screw up their directions"* (2024-07-16)
> **Rareș**, the specific artifact he never solved: ▸ *"what happens when you place multiple lights next
> to each other? Wouldn't directions cancel each other?"* … *"if they mix together wouldn't you have
> shaded areas between lights when there are no walls? Because at a certain point the directions
> intersect and cancel each other"*
> **x1m4**'s honest answer: with an isotropic buffer *"this looks like an unavoidable issue"*;
> anisotropic *"reduces"* it. Two equal lights either side of a wall **will** wash each other's shadows
> out. It's inherent to `max` over a shared volume.
> **Dapper Core**'s framing: ▸ *"CAGI is like LPVs but instead of spherical harmonics, the directionality
> is fixed to the 45deg angles."*

**Two attempted fixes worth knowing:**

- **Rareș's normal/direction texture** (2025-04-28): ▸ *"a light information texture, and another one for
  normals and directions. And walls would emit some sort of 'normals' back in the scene and you can
  determine shadows more or less by doing some **dot product between the normal directions and light
  directions**."*
- **IchBinAlex's corner re-emission** (2025-04-27), which is the cleanest penumbra idea in the corpus:
  ▸ *"allow light to travel on 45 degree angles and during propagation you **create a new light emitter
  at a different angle with less intensity every time light hits a corner**."*
- **sweg** went the other way and added directions — more than 4/6 — with directional weighting; his
  complaint that CAGI *"doesn't produce any penumbra at cardinal directions"* is the artifact both fixes
  target.

## 2.4 Dirty culling — and a correction to the Noita talk

x1m4's numbers: **256³ = 2 ms** full, **0.4 ms** with dirty buffers + checkerboard. 1 bit per 8×8 or
16×16 chunk. Noita's bounding-boxes refinement isn't worth it on GPU: ▸ *"the performance benefit of
such bounding boxes isn't that big."*

He corrects a claim in the Noita GDC talk at 28:06 (2024-01-06):
> ▸ *"you don't have to update everything in a double buffered sim, it works just exactly like with
> single buffering. to detect changes and cull chunks from being updated, you can just once calculate
> the new state of a cell, and **if the value changed compared to the one in the previous buffer, then
> trigger a dirty state**"*

His code, given to bonisdev (2024-11-07):

```rust
// in the cell update shader
let dirtyPos = srcPos / 8;
if (DirtyBuffer[dirtyPos] == 0u) { return; }          // early out
// … compute srcCell …
if (srcCell != CellSrcBuffer[srcPos]) {
  atomicAdd(DirtyBuffer[dirtyPos] + 1u);              // mark chunk (and neighbours) dirty
  CellDstBuffer[srcPos] = srcCell;
}

// dirty-update shader, run at 1/8 sim scale
srcDirty = min(2u, srcDirty);                         // cap at one dirty round
srcDirty = max(1u, srcDirty) - 1u;                    // fade out
```

○ And the corollary from his fluid work, which generalises to any CA: ▸ *"**synthetic fluid dampening**
is getting better which is essential for dirty chunks."* If your sim never settles, culling buys nothing
— so add unphysical damping until it does. **The performance system dictates the physics.**

## 2.5 Cascades — and a serious objection to camera-relative probes

Your `cagi-cascades-plan.md` targets camera-relative toroidal cascades. The corpus supports the cascade
part strongly and **contests the camera-relative part**. You should decide knowingly.

**The implementation details, from x1m4 following the McLaren GDC talk** (2022-12-16) — four specifics
worth copying verbatim:
> - ▸ *"**Grid snapping** for stable scrolling (snapped to a multiple of the cascade scale)"*
> - ▸ *"Cascades are stored **within a single 3d texture** to get fast trilinear filtering during
>   blending"*
> - ▸ *"Cascades are offset based on the camera position, **view direction** and world boundings"*
> - ▸ *"**Only 1 cascade is updated per frame** (Cascade 0 updated every 2nd frame, cascade 1 every 4th
>   frame etc.)"*

○ That last one is free: cascade *n* covers 2ⁿ× the volume, so it needs 1/2ⁿ⁺¹ the update rate. He'd
posted the video of what broken cascade scrolling looks like nine days earlier.

Also: ▸ *"for cascade border scrolling areas becoming black, his idea of **injecting the cascade radiance
from the next upper cascade** helps a ton — makes the transition between cascade edges almost
invisible."* And the hard prerequisite: ▸ *"it also requires an LOD representation of the world so you
can feed the according world block LODs into the cagi cascades"* — ▸ *"the important thing is having a
proper lower res representation of your scene, which I found **the hardest part** of implementing
cascading for any kind of volumetric light solution."*

**Now the objection. Dapper Core, December 2024:**

> ▸ *"worldspace techniques have issues around **disocclusion/occlusion artifacts**. They're really bad
> whenever an object moves into a probe and now that probe is pitch black"*
> ▸ *"**when you move the probes, you now have greater probe density in some areas and less probe density
> in other areas**"* … *"if a probe moves far away enough, you end up in a really bad spot. You have to
> do weird things where you spawn new probes to make up for the shift"*
> ▸ *"moving probes causes serious artifacts as the probes are moving since they have to **reaccumulate a
> lot of samples**, and you need tons of bandages to handle some really bad edge cases"*
> ▸ his recommendation: *"stuff like lumen where you have a worldspace cache but your **probes are spawned
> on the depth buffer** are a strictly better solution"* … *"I still haven't seen a solution to probe
> occlusion/disocclusion that doesn't involve some screenspace element"*

**dotted** independently: ▸ *"I think making the light probe placement follow the camera is a bad idea,
you should look into aligning them with worldspace geometry instead to get better looking results."*

And the current state of the art per Dapper Core: ▸ *"The radiance cascades people have been working on
some interesting stuff where you **spawn probes on minmax mips of the depth buffer**, trace these probes
in world space, and do some weird interpolation between the two sets."*

○ **How much this applies to you depends on one thing: whether your cells are probes or cells.** CAGI's
volume isn't a probe set — it's a simulation grid where every cell is filled and propagation is local, so
"a probe went dark because an object moved into it" is just… correct occlusion, resolved in a few ticks
at ~1 cell/tick. Toroidal scrolling *does* still reintroduce cells at the trailing edge with no history,
which is exactly the "black cascade border" x1m4 fixes by injecting from the coarser cascade. **So the
objection lands on the density-variation and reaccumulation points, not the disocclusion one.** Worth
budgeting for: a scroll event invalidates a slab, and that slab needs bounded re-propagation — which
your Stage 1 already plans.

---

# Part 3 — Emission

Concrete answers to "good light emitting", scattered across the corpus:

**Store emission in the material, and reuse a field for it.** Gabe Rundlett's `u32` packing is the
tidiest trick here: ▸ *"6bpc colors (18 bits), 8 bit normals, 2 bit material, **4 bit 'roughness' (which
also acts as emissive brightness when the material is emissive)**."* One field, two meanings, selected by
the material bits.

**x1m4's material set** is deliberately tiny: diffuse, emission, metal, glass — per *sub*-voxel, not per
voxel (2022-11-03).

**Emissive cells in a CA are trivial; sunlight is the hard part.** Dapper Core: ▸ *"hytale doesn't seem to
care about sunlight bounces, **it's trivial for emissive light blocks**"*, and his own split: ▸ *"I plan on
using [radiance cascades] only for sun/skylight. **I like the grid based visibility / CA more for
emissive**."*

○ That split is worth taking seriously: emissive light is local, bounded and numerous — ideal for CA.
Sun/sky is a single distant source with long throw — ideal for something else. x1m4 puts both in CAGI and
pays for it with a 45°-locked sun.

**Emissive volumetrics for free, if you already have a light volume.** Nameless (2024-03-05):
> ▸ *"every frame, if the block is emissive I set it in a 3d texture as the colour of the emission and the
> alpha as the strength. If the block is not emissive and is not air, I set 0. If the block is empty I
> **average the colour and strength from nearby blocks and subtract the averaged strength** with some
> number. Finally I traverse the 3d texture with raymarching/dda and offset the position to remove the
> blocky look, `exp(-emissiveStrength) * emissiveColour`"*

○ Note what that is: a flood-fill light volume, built independently, for fog. If you have CAGI you already
have the volume — march it. x1m4 does exactly this: ▸ *"I sample both the 3d ca gi volume and also the
shadow map for sharp sun shadows"*, at half res with heavy jitter and a blur upscale.

**A cheaper trick for transmittance-only volumes.** Alex, for clouds: ▸ *"just the amount of light reaching
the world space point that corresponds to each texel (transmittance) … **128×16×128 looks good** and it's
infinitely faster than computing the lighting of each ray march sample."*

**Bilateral depth-aware upscale** if you render emissive fog at reduced res — x1m4 shows 2× and 3× and
notes ▸ *"naive upscaling looks so bad on edges"* (2024-06-27).

---

# Part 4 — Reflections

## 4.1 The single biggest win: one shadow map replaces four ray types

**Nameless**, 2024-02-22. If you take one thing from this section, take this:

> ▸ *"I also successfully did a **raymarched shadowmap**, which makes it much faster. Because before I'd
> need to shoot **4 shadow rays: 1 for the visible shadows, 2 for diffuse and specular bounces and 1 for
> the radiance cache**. Having a shadow map also allows me to do really fast volumetrics (which I can
> optimise further by rendering them at a lower resolution). Another optimisation I can do is to apply
> **bayer's matrix to the shadow map**, which works as long as I'm not updating the shadows a lot"*

Same message: a 360° sky view rendered with a Bayer matrix, *"essentially only calculating 1/16th of the
pixels per frame."*

x1m4 arrives at the same conclusion from the other direction, repeatedly and grudgingly:
> ▸ *"I hate shadow mapping so much but performance wise it's just unbeatable"*
> ▸ *"with shadow mapping I can just calculate the shadows once and then use them in the **main pass,
> reflection pass and GI pass**"* (2022-11-28)
> ▸ *"from my own projects I just can confirm that **shadow rays are a complete performance killer when
> used within light bouncing**, while in comparison a shadow map is just one texture read"*

**Kelvin's measured ladder** shows the shape of the problem (2024-03-28):

| Configuration | fps |
|---|---|
| no shadow rays | 100–120 |
| + shadow rays | 80–90 |
| + volumetric fog | 60 |
| **+ two-bounce GI** | **30** |

and ▸ *"if I even do one ray for reflective surfaces or a shadow ray without beam optimization my fps gets
cut in half"*, ▸ *"I'm getting 5ms on bounce rays at 270p."*

## 4.2 Rough reflections without noise

Three independent approaches, all avoiding stochastic specular:

**Sam — two copies of the probe map.** ▸ *"I filter the 8×8 map … I stored one filtered map and one
unfiltered map. **I use the latter for a rough specular approximation**"* → ▸ *"actually gives you decent
infinite bounce rough specular."* Cheapest option if you already have probes.

**x1m4 — screen-space cone tracing over a mip chain.** ▸ *"do cone tracing in screen-space and sample from
a reflection buffer with perfectly sharp reflections — **the mip tree of the reflection buffer + the cone
based sampling should give noiseless rough reflections**"* (2022-11-03). His shipped version is the same
idea from Lumen: ▸ *"you shoot sharp reflection rays and then use mips + a bilateral filter for blurring
… pretty dumb but works surprisingly well considering how fast it is."* He knows the better option:
▸ *"there is a technique called screen-space cone tracing that probably works better, but never got
around trying it"* — and Sam pushed him at it: ▸ *"if you were doing screenspace then screenspace cone
tracing would be a much better alternative."*

**Nameless — let the radiance cache do it.** ▸ *"because of the radiance cache, **specular doesn't need a
lot of denoising**"* and ▸ *"if everything has really low roughness, there is no noise whatsoever."*
Caveat he also reports: ▸ *"only issue with the radiance cache is that **it's visible in the
reflections**"* — a low-res cache read by a sharp mirror shows its resolution.

**Current state of x1m4's reflections** (2026-07-30): ▸ *"ray traced reflections (with **stable temporal
reprojection**!)"* at roughness 0 / 20 / 80 %, with ▸ *"per-voxel diffuse and per-pixel specular"* —
because ▸ *"per-voxel reflections can look a bit annoying in certain scenarios."*

○ That last line is the rule: **diffuse can be per-voxel; specular cannot.** Specular needs per-pixel or
it looks quantised.

## 4.3 The trap: specular in a CA light volume gains energy

**ReversedCausality**, 2024-02-09, and this one will bite you specifically because you're doing CA
lighting:

> ▸ *"there's a weird thing with the lighting: like lets say you have a reflection at some point, then
> **the amount of light that goes through the air near it is twice as much as elsewhere, so it can get
> brighter than the source**"*

And the cost that made him abandon it:
> ▸ *"the actual reflections with this approach require **quadrupling the memory usage** … I'm not going
> to use that"*

His workaround for diffuse, which is the safe version: ▸ *"I sum up the light hitting a diffuse thing and
**distribute it evenly among the directions**."*

○ Why it happens: a specular bounce re-injects into a *single* direction at near-full strength, while the
diffuse path spreads the same energy over N directions. In a `max`-based volume the specular contribution
therefore wins every comparison in its direction and never pays the 1/N cost. **If you add specular to a
CA volume, it must be attenuated by the same factor the diffuse path pays**, or normalised by direction
count.

○ Which is an argument for keeping specular *out* of the light volume entirely — trace it per-pixel
against the world, as x1m4 does, and let CAGI own only diffuse.

---

# Part 5 — Every measured number in one table

Useful for sanity-checking your own profiler.

| Thing | Number | Who, when |
|---|---|---|
| CAGI 256³, full workload | **2 ms** | x1m4, 2024-09 |
| CAGI 256³, dirty + checkerboard | **0.4 ms** | x1m4, 2024-09 |
| CAGI 512×256×512 brute force | 6–10 ms | x1m4, 2024-03 |
| CAGI cascaded, expected | ~0.2 ms / 128³ cascade | x1m4, 2024-09 |
| CAGI propagation speed | ~1 cell/tick, 60 fps ⇒ 60 voxels/s | x1m4, 2024-03 |
| CAGI global update rate | 1/8 of volume per frame; full rate near player | x1m4, 2024-03 |
| CAGI reads per cell (2D, bouncing) | 10–20 | x1m4, 2025-06 |
| Radiance cascades, 2D | **0.3 ms on a GTX 970** | Mytino, 2023-11 |
| ↳ its config | 25 % screen res, probes every 2×2 px in cascade 0, **5 cascades on mips 0–4** | Mytino |
| Radiance cascades shadertoy | 60 ms/frame on an M1 | x1m4, 2023-10 |
| Path tracing, 1 spp 2 bounces, fullscreen | 1 ms + **0.8 ms spatial filter** | x1m4, 2022-09 |
| Path traced vs cone traced | +0.7 ms for path tracing | x1m4, 2022-09 |
| Samples/probe to converge indoors | **64** (matches the Lumen talk) | x1m4 |
| ↳ Sam's counter | **1** | Sam, candela |
| Temporal blur | 48 frames world+screen; skylight 1/128, sun instant | x1m4, 2023 |
| Sky shadow maps | 32 × 128², + 1 × 4096 sun, tiled updates | x1m4, 2023-04 |
| Bwerness skylight slice | **0.5 ms/slice**, 9 hemisphere samples, 32-bit uint (8 propagation + 8 filter) | x1m4, 2023-09 |
| Flood fill 256³ | **0.2 ms** on a 3080 | x1m4, 2024-02 |
| Checkerboard upscale | ~2× on the irradiance pass; +50 fps overall | x1m4 |
| Bounce rays | 5 ms at 270p | Kelvin, 2024-04 |
| 2-bounce GI cost | 120 fps → 30 fps | Kelvin, 2024-03 |
| Transmittance-only cloud volume | 128×16×128 "looks good" | Alex, 2024-09 |
| Target hardware | dev on RTX 3080, target GTX 1070/1080, ~2 ms lighting budget | x1m4 |

---

# Part 6 — Order of work, and the traps

**Order** (this is x1m4's own sequence, which is also the dependency order):

1. **DDA with empty-space skipping** — 4³ nodes, 64-bit occupancy masks, integer stepping, normal-padded
   ray origins. Verify it with ray-traced AO only, no lighting, exactly as he did during the WebGPU port:
   ▸ *"there is no lighting yet, only ray-traced ambient occlusion in order to verify my ray traversal
   methods."*
2. **CAGI, single volume, emissive only.** Subtractive decay, `=` not `+=`, diagonal injection, face-wise
   propagation. Get the corner-seal case right before anything else.
3. **Per-face opacity from sub-voxel occupancy** — otherwise thin geometry leaks and you'll blame the
   propagation.
4. **Dirty masks** — 1 bit per 8×8/16×16 chunk, propagated as a CA itself, with the fade-out. This is
   where the 5× comes from.
5. **Sun/sky injection** — from a shadow map first (easier to debug, sharper), then decide whether to move
   it into the CA and accept the 45° lock.
6. **A shadow map you reuse everywhere** — main pass, reflections, GI, volumetrics. Nameless's four-into-one.
7. **Reflections per-pixel, outside the light volume** — sharp rays + mips + bilateral, or screen-space
   cone tracing.
8. **Cascades last**, because they need a world LOD, which x1m4 calls the hardest part.

**Traps, ranked by how much time they'll cost you:**

1. **Multiplicative decay.** Kills dirty culling silently — everything works, nothing is fast.
2. **`+=` instead of `=`** when gathering. Feedback amplifier.
3. **Face-only emissive injection.** Square halos.
4. **Any factor > 1.0 anywhere in the loop.** Gateway to heaven.
5. **Specular inside the CA volume.** Energy gain near reflectors (§4.3).
6. **Shadow rays inside the bounce loop.** The single largest avoidable cost in every engine here.
7. **Stamping patterns during injection.** One write per cell per pass, or injection becomes the bottleneck.
8. **Sub-voxels > 16³** with a block-scale light volume. Inner leaking.
9. **Believing the light volume can do everything.** x1m4 keeps ray tracing for AO and reflections; Dapper
   Core splits emissive (CA) from sun/sky (cascades). Nobody who shipped this put everything in one system.

**And the one philosophical point that's actually load-bearing**, because it explains every choice above —
x1m4, 2024-12-19:

> ▸ *"what's mostly interesting about grid based visibility is **the caching aspect**. ray tracing performs
> so much worse compared to it, because it's usually applied without any potential caching mechanism"*
> ▸ on rays re-reading the same tile: *"yep it's essentially **cache nuking**"*
> ▸ *"I like these approaches because **they stabilize in a finite time**"* — his reply: *"**YES**"*
