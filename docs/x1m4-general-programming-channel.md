# x1m4's engine — the #general-programming channel, 2022-08 → 2026-03

**Source:** `temp/general-programming` — channel `1007957399312805898` in guild
`1003288330391273492`. 9 645 unique messages, **2 744 of them x1m4's**, 2022-08-13 → 2026-03-06.

This is the fourth of six channel exports. Running total: **13 905 of his messages**
across four channels.

| Doc | Channel | His msgs |
|---|---|---|
| [x1m4-graphics-programming-channel.md](x1m4-graphics-programming-channel.md) | #graphics-programming | 1 854 |
| [x1m4-showcase-chat-channels.md](x1m4-showcase-chat-channels.md) | #showcase + #chat | 4 851 + 4 456 |
| **this file** | #general-programming | **2 744** |
| [x1m4-architecture-notes.md](x1m4-architecture-notes.md) | earlier 6-channel sample | 210 |

**What this channel is:** the *help desk*. Other people ask questions, he answers at length. That
makes it the channel where he explains his own engine most explicitly, because explaining is the
point. It also contains the only sustained accounts of **the audio implementation's actual
problems**, **the two-simulation architecture**, and **why upscalers fundamentally cannot help his
renderer**.

**Confidence marking:** ▸ = his words, quoted and dated. ○ = my inference.

---

## 1. Five things that change the earlier docs

**1. Shadow mapping was deleted.** The earlier docs treat sun shadows as shadow-map-based with
CAGI on top. That stopped being true on **2024-07-25**:

> ▸ *"got rid of shadow mapping based shadows, shadows are now propagated similar to how skylight
> is propagated and uses cellular automata too. now they are not as sharp as the shadow mapping
> based ones as they are stored and propagated at main voxel scale, but they still do the job and
> are also a ton faster"*
> ▸ and the tick rate: *"cagi runs at 60fps so it's pretty fast anyway"* — asked whether that means
> the speed of light is 60 voxels/s: *"yes just like in reality"* (2024-07-25)

○ So the final pipeline has **no shadow maps at all**: CAGI propagates emission, sun, sky *and*
shadow, with ray tracing left only for AO and reflections. The 45°-locked sun (noted in the
showcase doc) is a direct consequence.

**2. The entity renderer has three interchangeable geometry modes**, not one:

> ▸ *"the entity renderer actually supports 3 geometry modes (since I'm still not 100% sure which
> one to keep): world-space intersection (currently used), object-space intersection (like teardown
> objects) and analytic intersections (which is what minecraft uses for objects giving sharp
> geometry) and which is also the fastest one since it doesn't require ray marching"* (2024-03-24)

And the SDF stage sits *after* the OBB test, as a shaping pass:
> ▸ *"the model bones/nodes consist of transformed and animated OBBs which for the ray tracing I
> first do analytic OBB intersection and after that an SDF intersection to e.g. make the OBBs turn
> into rounded boxes instead (you could also do SDF capsules, ellipsoids etc.)"* (2024-03-24)

**3. Infinite worlds were built, then deliberately abandoned.**
> ▸ *"gave up on infinite worlds as it bites too strong with my plans for the project :< … like
> there is a reason why games like factorio and mindustry have finite worlds"* (2024-06-26)
> ▸ *"the inf world is in a branch, so at least I now know how to implement it as it was a lot more
> complicated than I expected — since you have to scroll everything around the player, who is
> always centered in the world"*

**4. The terrain generator moved to the GPU** on 2024-06-23, which was the last CPU code in the
engine:
> ▸ *"I just found out that my terrain generator that is currently CPU based can be run on the GPU
> … it's actually very easy to parallelize … so I can finally move the very last bit of the engine
> code over to the GPU"*
> ▸ *"even chatgpt said that it's really hard to make a diamond square fractal processable in
> parallel + have it infinite in all directions. chatgpt L"*

**5. The engine was rewritten ~8 times, not 5–6.** ▸ *"rewrote mine probably 8x times — not
completely from scratch though"* (2024-06-20).

---

## 2. Audio — the implementation's real problems

The showcase doc has the architecture. This channel has the **bugs, the limits, and the one
genuinely nasty platform constraint**, which is the part worth knowing before copying anything.

### The WebAudio main-thread problem, and his workaround

> ▸ *"one great thing I learned yesterday: WebAudio needs to be run on the main thread. so play 3
> sounds at the same time and you immediately get micro stuttering and gameplay becomes
> unresponsive. huuge L"*
> ▸ *"afaik OpenAL directly comes with multi threading and I don't get why WebAudio just doesn't"*
> ▸ **the hack:** *"my current workaround is to create an iframe with a different url domain than
> the one of the main page. chrome seems to strictly put such iframes into a separate thread, which
> after some testing seems reliable enough to do it like this. so this dirty hack at least lets me
> play audio in a separate thread now and it won't cause any lag spikes or stuttering in my voxel
> engine anymore"*
> ▸ *"it seems to work on all browsers except safari"* (all 2024-01-11)

### Browser audio ceilings, measured

> ▸ *"firefox's audio engine seems to be so much better than what chrome has — like running this on
> chrome makes my audio glitchy after playing about 128 sounds in parallel … in firefox I can easily
> get above 256 and it still plays perfectly fine"* (2024-01-11)
> ▸ his test page: **`voxelchain.app/audio/`**

### A leak he never diagnosed

> ▸ *"god I somewhere have a performance bottleneck in my sound ray tracing — not with the ray
> tracing itself, but on the CPU side when I mix together the ray traced results into the according
> audio filters. for some reason it gets slower and slower over time and I have no idea why. feels
> similar to the Noita chainsaw bug where too many played sounds make the game run slower and slower
> over time until it crashes"* (2024-01-09)

○ Combined with the showcase doc's *"reverb is the bottleneck, not the tracing"*, the picture is
consistent: **the GPU side of his ray-traced audio was never the problem; the CPU DSP and the
browser audio stack were.** That is worth weighing against atrium's native-Rust CPU DSP, which
doesn't have either constraint.

### Smaller findings

- ▸ *"I'm surprised at how powerful webaudio is — it even supports hrtf"* (2024-01-04)
- The eardrum incident, which is how you learn to clamp: ▸ *"nearly blasted out my ear drums today
  — currently implementing ray traced audio and accidentally swapped the calculated reverb intensity
  into the gain intensity. it was so loud it sounded like a gun shot"* (2024-01-04)
- Footsteps are emitted from inside the entity movement shader (2024-01-10).
- **Loop sounds for machines** are the one place he wants continuous directionality: ▸ *"I plan on
  only using this for loop sounds for stuff like machines that are nearby to give them some
  directionality"* (2024-06-17), prompted by the Noita devs' position-based-sound talk.
- ▸ *"so weird that in comparison to light simulation, for sound there are almost no resources
  available"* (2024-01-09)
- The unbuilt ambition, twice: ▸ *"would love to have something like this in my engine — like VST
  plugins but represented as circuits with voxels, and ray traced"* (2023-11-11, about
  [dittytoy.net](https://dittytoy.net/)).

---

## 3. The two-simulation architecture

The clearest statement of how the engine's simulation is actually split (2023-02-05):

> ▸ *"for noita like behavior it's enough to have a single buffered cell sim, so what I'm planning
> is to actually have 2 separately running sims which can act with each other:*
> - *a sim that is double buffered and used when you need very precise and predictable behavior
>   (e.g. water flowing, circuit power spreading etc.)*
> - *and another sim that is single buffered for all kinds of cell behavior that don't need
>   consistency but are mostly important for reactions between elements and moving around (i.e.
>   falling down)"*

Cost at the time: ▸ *"each voxel in my sim is currently 2× 32bit because of double buffering"*, and
▸ *"the whole world is currently double buffered"*. For a pure flow sim ▸ *"you can get away with
8bit"*.

○ This is the design that later collapses into "single buffering everywhere" (§6 of the showcase
doc). Worth knowing that the original architecture was explicitly dual, and *why*: precision where
you need determinism, Noita-style chaos where you don't.

---

## 4. Why upscalers can't save him — the best-argued thread in the export

August 2022, against Gabe Rundlett's plan to drop FSR2 in. It matters because it's the same
argument he later makes about 1-spp path tracing, applied to reconstruction.

> ▸ *"with very detailed scenes like in voxelchain where voxels often are close to filling 1 pixel
> on the screen (**my effective rendered resolution is up to 2048 or even 4096**), I expect any
> regular upscaler to fail here since there is simply a loss of information"* (2022-08-14)
> ▸ *"an upscaler can't upscale what's simply not there or temporally is too unstable"*
> ▸ *"what dlss essentially does is fixing these temporal artifacts, glitches and flickering,
> because no temporal information can help you here, the information is too corrupted. so what you
> have to do is smartly guessing and blurring, which is where neural networks come into play —
> because neural networks are good at extrapolating information that is simply not there. or in
> other words, they are good at guessing"*
> ▸ **and a claim I haven't seen elsewhere:** *"I'm sure that rasterizers are better at handling
> floating point imprecision than software DDA algorithms … in DDA you actually step through the
> scene with many steps at smaller and smaller scale; the further you step, the more the result is
> dependent on the previous step result, and the more precision errors you get. rasterization on the
> other hand is simply projection"* (2022-08-14)

○ That last point is the root cause of his later switch to **integer DDA** — the error is
*compounding*, not just present. His mitigation for LOD flicker: ▸ *"when my sub voxel volumes get
too small on screen, then I just pick a higher LOD level when sampling them, which improves
performance, but most importantly reduces the flickering"*.

A WebGPU FSR2 port stayed on his list for years; the blocker was always the same: ▸ *"only this part
will be difficult, because they solved it with atomics and random writes, which webgl doesn't
support"* (2022-08-16).

---

## 5. Per-voxel vs per-voxel-face lighting — with the diagnosis

March 2023, the experiment that produced the engine's signature look. Two-image comparisons posted;
the reasoning is the useful part:

> ▸ *"the problem with per-voxel lighting is that the voxels tend to look very transparent or 'wax'
> like … the wax look comes from tracing the rays from the center of the voxels"* (2023-03-16)
> ▸ *"the per voxel approach is basically shooting the rays from the center inside the voxels (so
> about half of the rays collide with the voxels within the own surface). the per face approach puts
> the rays to the center of each voxel face and shoots the rays from there"* (2023-03-22)
> ▸ mitigation if you insist on per-voxel: *"I mask out the voxel that is at the ray origin — within
> the traversal I only allow intersection if the subvoxel isn't the one started at"* (2023-03-23)
> ▸ and per-voxel leaks: *"per voxel also results in leaking as you can see on the left post (a sea
> lantern is hidden below it)"* (2023-03-22)
> ▸ verdict: *"right still doesn't look too bad, but it has this wax/transparent look and misses
> shape"*

Two adjacent findings from the same weeks:
- **Stratified sampling**: ▸ *"now using stratified sampling with 64 scale, really helps performance
  and makes rays more coherent — performance more than doubled"* (2023-03-22). ○ He got Battlefield
  5's ray-binning win (~7%) for free by sampling coherently instead.
- **Sky occlusion before shadow maps**: ▸ *"I'm building a cubic heightmap (6 sides) of the world, so
  I don't have to trace sky rays but instead can check sky occlusion with a single texture lookup"*
  (2023-04-01) — the predecessor to the 32×128² hemisphere shadow maps, and to CA skylight.

---

## 6. Entity and item interaction — grids instead of loops

The recurring GPU problem: you cannot search neighbours in a loop. His answer, developed over four
months and applied to three separate features.

**The general pattern** (2024-06-05):
> ▸ *"for example if an entity does an attack move, you have to scan nearby entities and check which
> entities are affected by the attack, which on the GPU is hard to do as you can't really do loops.
> so instead I think what could work is to have an additional grid that spans over the world on
> main-voxel scale and where stuff like collisions or attacks get written into. so if the player does
> an attack, it adds damage numbers into the grid cells in front of it, and if an entity intersects
> with those filled grid cells, then it receives damage. it's also possible to calculate a direction
> vector based on the grid cells to push the entity away with force accordingly"*

Applied to **entity cramming** first (2024-03-05): entities `atomicAdd` into a 3D texture at their
position; the sum gives a push-away direction vector; ▸ *"an 8bit or even 1bit per cell texture could
already be enough"*. Debug-visualised by using the field to push vegetation aside (2024-06-05).

Mytino confirmed it's standard practice elsewhere: *"like hybrid fluid sims with particles atomically
adding data to a grid and then reading from the grid after, as opposed to loops like in SPH"* →
▸ *"ohh nice didn't know that's how they do it. really good to know"* (2024-06-05).

**Flow-field pathfinding**, with numbers and a real argument against A\*. Against an RTS engineer
(FoneE, *Sanctuary: Shattered Sun*) who pointed out A\* is far fewer instructions:

> ▸ *"oh yeah totally believe that astar is by super far less instructions. just on the GPU it's a
> different matter and more about how parallelizable a task is — since updating the distance field
> is just 4 texture reads in 2D, it's very GPU friendly since every thread literally just does that
> simple operation, and doesn't do any branching (because the branching would completely stall the
> performance)"* (2024-02-04)
> ▸ *"loops are just horrible for GPUs … like instead with flow fields you spread the loop over a
> grid and you're fine then"*
> ▸ **the number:** *"flood filling a map of 256×256×256 takes about 0.2ms on my machine (an rtx
> 3080) so really the biggest limitation is just the memory cost"* (2024-02-04)
> ▸ multi-target solution: *"give every player their own 3d flowfield volume … size could be
> something like 32×16×32"*, and *"did some sketching yesterday and it should easily work for about
> 65k entities each with their own flowfield updated in real-time"*
> ▸ *"for each pixel the algorithm is really just finding the max value of each of the 4 neighbors
> and then subtracting 1 from it, it's so simple it's almost offensive"*
> ▸ the one honest drawback: *"the spreading is not synchronous, so it takes a few update ticks to
> complete"*

**Tick rate and what is *not* deterministic** (2024-03-05) — this is an important correction:
> ▸ *"my sim tick rate is 25 tps"*
> ▸ *"my world is deterministic, but entities use floats and stuff and I won't even try to make them
> deterministic too. instead only their interactions with the world are deterministic — i.e. the
> server decides that if an entity hits a block or something, of how it affects the world. pressing a
> redstone button is a good example of that idea: it doesn't really matter too much where that entity
> is standing or how it's rotated, only the action of the entity pressing that button is important"*

○ So the "everything is integer and deterministic" claim in the earlier docs applies to the **world
simulation only**. Entities are float, server-authoritative, and client-interpolated — Minecraft's
model.

---

## 7. Fluids — the July 2024 breakthrough, and the property list

The showcase doc has the arc. This channel has the month it came together, and the parameters.

| Date | Milestone |
|---|---|
| 2024-04-07 | Incompressible attempt: vertical scanline pressure capped at 16 |
| 2024-07-19 | ▸ *"revised my water sim, feeling good about its behaviour"* |
| 2024-07-22 | ▸ *"velocity feedback does wonders in speeding up the equalization"* — settles in seconds, no flicker, areas cullable |
| 2024-07-24 | Multi-fluid: water + oil, non-mixing |
| 2024-07-29 | Fire in five minutes; ▸ *"my sim is definitely something almost completely new and doesn't have the problems most fluid sims have — problems like randomly exploding, losing mass or never settling down"* |
| 2024-08-01 | Cohesion + surface tension |
| 2024-08-02 | Marching squares for surface reconstruction |

**Material properties are data, in JSON:**
> ▸ *"nope it's just a few material properties you have to define in json — properties like color,
> compressibility, density, surface tension, cohesion etc."* (2024-08-01)
> ▸ *"but if you want material morphing like water turning into stone when near lava then yeah you
> have to do it manually"*
> ▸ multi-material is free: *"nope makes no difference"* to performance (2024-07-29)

**Why it's fast, in his words:**
> ▸ *"sim is really fast, it's double buffered and only does a few texture reads per cell — and no
> performance pitfalls like 20x advection passes as in regular eulerian fluid sims"* (2024-07-29)
> ▸ *"it's not sped up btw, never expected it to run this fast naturally. usually grid sims have 20x
> forward steps to run smooth"* (2024-08-01)
> ▸ the credited cause: *"that's mostly because it's integer based and conservative, which all fluid
> sims I have seen so far lack"* (2024-07-29)

**Fire, as a recipe** (2024-07-29): ▸ *"the fire has negative gravity, is very compressible and has a
vertical velocity bias to it with some extra pseudo randomness applied on the velocity"*.

**Storage budget per block:** mass up to **2³²−1** plus ▸ *"each block has 16 bits of arbitrary data
available"* — used e.g. so dirt can accumulate water up to a threshold and act as a filter (2024-08-01).

**The design complaint he keeps returning to** — and it's a game-design point, not a technical one:
> ▸ *"what I don't like about terraria/sb style sims is that they are mass based, i.e. 1 water block
> isn't 1 water block"* … *"I have the same issue with my 2d water fluid sim, a non-compressible sim
> would be a ton easier to deal with"* … *"for gameplay I think only minimal compression is best, so
> it's easier to understand"* (2024-07-18, 2024-07-25)
> ▸ on Minecraft: *"minecraft water is ugly to deal with because it's compressible in a sense — i.e.
> there is more water than you expect"*; and *"I think minecraft water is genius especially for the
> time when it was invented, but it can definitely be improved"*

**Surface rendering** is unresolved: marching squares tried and judged *"doesn't fit too well"*; the
plan is to fold marching-cubes values into the existing **camera-centred animation volume** (the same
structure used for conveyor-belt interpolation) so the sub-voxel tracer voxelises the smooth surface
naturally (2024-07-27).

**Particle acceleration, benchmarked** (2024-07-18) — he seriously reconsidered particles once:
> ▸ *"I wrote a uniform grid acceleration structure for my entities in my voxel engine and initially
> prototyped the implementation in a 2d particle sim, and the results were pretty darn good — a ton
> better than I expected. **256³ particles with nearby particle collisions take about 1ms.** this
> tells me that particles might actually be feasible to use for such sims"* — then went back to the
> grid and succeeded (2024-07-30).

---

## 8. Deferred ray traversal — his most recent idea (2025-11-27)

Worth flagging because it's the newest technical thought in any of the four channels, and it's
directly transferable:

> ▸ *"recently had to optimize my entity to player voxel raycast and ended up using a deferred
> system where the ray sent is only traveled 1/8th per update tick to save performance. made me
> wonder if this could be used for accelerating ray tracing too"*
> ▸ *"in world-space it could work I think — like if you have a probe based lighting system"*
> ▸ *"it's really just a way to cause long rays to have better performance distribution, also pretty
> cheap since all you have to store is the ray length for the partial stepping"*
> ▸ the motivating precedent: *"teardown had a problem with large rooms too since the ray length was
> capped for performance"*

○ i.e. **amortise a long ray across frames by persisting only its travelled length.** ○ Directly
relevant to acoustic ray budgets: long reverb rays in a large space are exactly the case where
per-frame ray length gets capped.

Same day, his settled verdict on radiance cascades after his 2D experiments:
> ▸ *"when I've experimented with rc, it was nice not having to worry about temporal accumulation and
> it's pretty, but the high performance demand, vram usage and leaking didn't convince me. and I've
> only experimented with it in 2d, in 3d it's probably even harder to work with"*

---

## 9. Code architecture — stated as a deliberate position

He is unusually explicit that his codebase has no abstraction, and that this is the point:

> ▸ *"my voxel engine renderer CPU code for example is a single giant 4k lines of code file with
> just webgpu api stuff with no abstraction — and it's easy to work with it because of this and
> requires no thinking if you see the code for the first time"* (2024-07-12)
> ▸ *"to prevent the chances of a convoluted code base, I started to no longer give a shit about
> programming paradigms and go with a mix of zero abstraction and data oriented design. keeps things
> simple, minimal, productive and easy to refactor (if necessary at all)"* (2024-08-09)
> ▸ *"I use the webgpu api directly and don't use or put any abstraction on top of it"*
> ▸ earlier, on the WebGL era: *"my gl renderer is like 3000 lines of code in a single file. same for
> the simulation engine, one single C file with 2k lines of code. some might argue that this is a
> terrible idea, but if you wanna add stuff then you can literally just implement without worrying
> to break other stuff or having to learn about any abstractions. the productivity is definitely much
> higher than the approach with many abstraction layers"* (2022-11-19)
> ▸ and on cleanliness generally: *"I'd weight like 20% on clean code, the other part is just get
> things done reasonably"* (2023-11-27)

○ This sits in obvious tension with the modular-seams principle atrium follows, and it's the one
place where his practice is worth *arguing with* rather than borrowing. Note the counterweight he
himself supplies (showcase doc §10): every new GPU feature is prototyped in a **separate stripped
fork** and only copied in once fully understood. The discipline lives in the process, not the
structure.

Bit packing, for the record (2023-11-28):
```wgsl
fn GetCellMaterialId(cell: u32) -> u32 {return extractBits(cell, 0u, 8u);}
fn SetCellMaterialId(cell: u32, value: u32) -> u32 {return insertBits(cell, value, 0u, 8u);}
```
▸ *"no structs, only bitfields"* … *"has the nice advantage that you can have your data extremely
tight in memory, which is necessary if you want to go for relatively large scale sims"*.

---

## 10. Published code snippets and gists (all from this channel)

Small but genuinely reusable, and not in the other docs:

| What | Where |
|---|---|
| **Infinite CA terrain generator** — his own, ▸ *"generates more interesting shapes, supports dynamic width height and depth and also runs faster than the one by bwerness"*; 4 states; usage `lambda 0.35, iterations 7` | [gist 82823976975e345ce1c810676c932b19](https://gist.github.com/maierfelix/82823976975e345ce1c810676c932b19) (2023-09-11) |
| Voxel rotation in 5 bits, applied in shaders | [gist 2807ad81904748e87d3aa806b094d782](https://gist.github.com/maierfelix/2807ad81904748e87d3aa806b094d782) |
| Triple PRNG, matching C / JS / GPU | [gist d25d674b8129a4cb39f734a9b25b2c39](https://gist.github.com/maierfelix/d25d674b8129a4cb39f734a9b25b2c39) |
| TEA hash, JS port matching C | [gist ad8b40306e08ea705139cc49bc75e6d7](https://gist.github.com/maierfelix/ad8b40306e08ea705139cc49bc75e6d7) |
| Integer rotation for CA | [gist 29ca6fa3a2ae5f0e26404b8cab3a83d3](https://gist.github.com/maierfelix/29ca6fa3a2ae5f0e26404b8cab3a83d3) (2024-06-04) |
| Conveyor-belt smooth-movement interpolation (real engine code, incl. rotation + curved belts) | [gist cf46b556754a91872e30d8ee8b3094f3](https://gist.github.com/maierfelix/cf46b556754a91872e30d8ee8b3094f3) (2023-11-28) |

Inline, worth copying:

**Voxel UV from an arbitrary surface point + face normal** (2023-05-27):
```wgsl
fn GetVoxelUV(pos: vec3<f32>, normal: vec3<f32>) -> vec2<f32> {
  let x = dot(normal * pos.yzx, vec3<f32>(1.0));
  let y = dot(normal * pos.zxy, vec3<f32>(1.0));
  return vec2<f32>(x, y) % vec2<f32>(1.0);
}
```

**Angle slerp** (2024-06-06):
```wgsl
fn SlerpAngle(start: f32, end: f32, t: f32) -> f32 {
  let dt = fract((end - start + PI) * INV_TWO_PI) * TWO_PI - PI;
  return fract((start + dt * t + PI) * INV_TWO_PI) * TWO_PI - PI;
}
```

**Entity splat bounds + the vertex shader that consumes them** (2024-03-20) — the fuller version of
the snippet in the showcase doc, including the `max(0.00001, w)` guard and the four-corner expansion:
```wgsl
var splatMin = vec3<f32>(1.0);
var splatMax = vec3<f32>(-1.0);
for (var ii = 0u; ii < 8u; ii++) {
  let offset = ((vec3<u32>(ii) >> vec3<u32>(0u, 1u, 2u)) & vec3<u32>(1u, 1u, 1u));
  let worldPos = srcEntity.Position + vec3<f32>(offset) * srcEntity.Size;
  let vertexPos = uCamera.ViewProjectionMatrix * vec4<f32>(worldPos, 1.0);
  let clipPos = (vertexPos.xyz / max(0.00001, vertexPos.w));
  splatMin = min(splatMin, clipPos);
  splatMax = max(splatMax, clipPos);
}
// vertex shader: index the 4 corners of [mi, ma] by VertexIndex
```

**RNG seeding conventions** (2024-03-23): terrain uses `x + y + z + world seed`; lighting uses
`x + y + frame_count`.

---

## 11. Platform notes worth keeping

- **Textures over buffers, for swizzling.** ▸ *"GPU textures are usually not represented linearly in
  memory, but are tiled so memory lookups are more efficient and there are more cache hits"*
  (2024-01-03); ▸ *"I prefer textures for stuff that involves a lot of neighbour lookups because of
  hardware swizzling"* (2024-03-05). He tried manual Morton encoding on buffers: ▸ *"found it to be
  way slower … like 3x slower"* (2022-12-04).
- **Read-write storage textures were the first WebGPU wall he hit** (2022-12-04) — a Metal limitation.
- **Profiling**: `writeTimestamp` for per-pass GPU timings (since deprecated/redesigned upstream);
  ▸ *"this and also shader hotreloading are the biggest helpers for productivity and to avoid
  performance regression"* (2023-12-06). Chrome extensions: `webgpu-dev-extension` (greggman) and
  `webgpu-devtools`.
- **Don't use geometry shaders or point/line rendering.** ▸ *"very poorly standardized and differs a
  lot between hardware"* — he cites `pointSizeRange` diffs between an RX 6600, a GTX 1070 and the M1
  (`1-511`). ▸ *"you can make it slower by using a geometry shader"* (2024-03-20).
- **Hardware vs software RT, his measured position:** ▸ *"I tried that a few years ago with a voxel
  rint shader, but my software implementation is faster than what I got with hardware"* (2024-01-03),
  and the reason he prefers software: ▸ *"with an own implementation you have more control and can
  specialize, which with hwrt you can't. that's also why john lin created a own ray tracer at some
  point, because the one by drivers had too many performance pitfalls"*.
- **Target hardware, stated repeatedly:** develops on an RTX 3080, targets **GTX 1070/1080**, budget
  ≈ **2 ms** for the lighting pass, ▸ *"for primary rays … if you can consistently stay around 1ms on
  high-tier hardware then you should be fine"* (2023-03-22).
- **wasm era details** (2022-11-28): raw `clang --target=wasm32` with `--features=atomics,bulk-memory
  --import-memory --shared-memory`; SharedArrayBuffer plus **manually spawned workers**, no C
  threading API — ▸ *"basically you have to implement all the stuff that emcc does for you"*.

---

## 12. Business model and background — the only channel that covers it

Relevant because it explains the closed source, the shape of the project, and the 2025 gap.

> ▸ *"I work as a freelancer in graphics programming and mostly do 1/2 of the year work for
> companies, and the other half working on my own shit — if you live sparely then this lifestyle
> totally works"* (2022-08-16)
> ▸ *"I already got a few investor/company offers who wanted to buy or license my engine … many
> wonder how I get this many voxels rendered in a shitty environment like the browser. since I don't
> have any competitors in the browser, they see an opportunity to create a market"* (2022-08-16)
> ▸ on why *this* project is closed, having open-sourced everything before: *"I'm a huge fan of
> open-source too, and I open-sourced every single personal project I made in the past years. but
> this one is the first I actually want to try making a living out of it, so I try a different path
> now"*; and bluntly, *"because my engine would be stolen away the moment I make it public — either
> creating something new with it or just selling it 1:1"* (2023-03-27)
> ▸ monetisation model: **Patreon + an early-access area on the site**, with keys given to
> consistently active community members (2022-09-01). He cites SonicEther's ~$50k/month on Patreon as
> the proof it can work.
> ▸ ▸ *"my github is mostly responsible for the job offerings"*; later, *"all good offers were from
> twitter, youtube or github"* and he deleted LinkedIn (2024-05-12).
> ▸ Self-taught, started ~age 12 with Visual Basic, 26 as of 2022-08. Never attended university.
> Works remotely for US companies; ▸ *"never found anything interesting in Germany"* (2023-11-09).
> ▸ Learned graphics and Vulkan during a school year when half the teachers had quit — with
> `316239158584803328`, ○ the same collaborator who later co-built the fluid/mass sim and the 2D pixel
> engine (2023-04-16).

○ The collaborator identity is the single highest-value unknown left: **the same person appears in
the school-era Vulkan learning, the 2023 fluid sim, and the 2D pixel engine.** Two people built the
mass simulation.

---

## 13. Activity and what remains missing

Messages per month, x1m4 only. Note the shape: heavy 2022-08 (upscaler/architecture debates),
2023-11 and 2024-01/03 (entity + audio + fluid work), then near-silence after 2024-09 — same NDA-job
gap the showcase doc identified.

```
2022-08 ████████ 192   2023-08 ██ 57      2024-06 ██ 60
2022-09 ████ 90        2023-09 █ 29       2024-07 █████ 123
2022-11 ██ 63          2023-10 █ 28       2024-08 ██ 56
2022-12 █ 33           2023-11 ████████████ 299   2024-09 ▎8
2023-01 ██ 68          2023-12 ██ 61      2024-11 ▏2
2023-02 █████ 125      2024-01 ███████████ 264    2024-12 ▏6
2023-03 ███ 88         2024-02 ████ 105   2025-01 ▏3
2023-04 ██ 59          2024-03 ████████ 201       2025-03 ▏2
2023-05 ▎8             2024-04 ███ 81     2025-06 ▎7
2023-06 ▊20            2024-05 ▉23        2025-11 ▌12
2023-07 █▌40                              2025-12 ▏4
                                          2026-03 ▏3
```

**Now covered across the four docs**, so no longer worth chasing: engine identity and history,
bits-per-voxel, the CAGI lineage and rule, entity voxelisation, the fluid arc, audio architecture
*and* its failure modes, determinism, tooling, business model.

**Still missing, unchanged and now firmer:**

1. **The voxelgamedev (VGD) server, keyword `cagi`.** Still the only place the algorithm is
   explained in depth by his own account. Reimplementers to look for: `sweg`, `👾Rareș👾`,
   `Dapper Core`, `bob08022010`; people he taught directly: `bonisdev`, `KosmosisDire`, `𝕶𝖊𝖑𝖛𝖎𝖓`.
2. **The collaborator `316239158584803328`** — co-built the mass sim and the 2D engine. Their
   messages would double the fluid-sim detail.
3. **His YouTube channel + video comments** — the canonical demo of each era. Videos referenced here:
   `f2_5RREfH-g` (most-watched), `RYr4jYLoXpU` (per-voxel path-traced lighting prototype),
   `Fh9oR76ZSx8` (ambient cubes attempt), `yf3ckx4O4sM` (procedural voxel noise), `8cWogE6dJoY`
   (accretor CA), `w-pgOnpZefg` (heat sim), `kciz8Ab9c_c` (mass sim with a friend).
4. **The Patreon posts and the site's early-access area** — the actual release notes, which no Discord
   channel replicates.
5. **`temp/general-programming/0048-message.txt`, `0104-update.rs`, and 8 `science.bin` files** — not
   parsed for this document.

---

## 14. What's newly worth stealing from this channel

1. **Amortise long rays across frames** — persist only the travelled length, advance 1/8 per tick.
   His newest idea, and it maps straight onto capped acoustic ray budgets in large rooms.
2. **Shoot secondary rays from face centres, not voxel centres.** Centre-origin rays waste ~half
   their samples on your own surface and produce a translucent "wax" look.
3. **Stratified sampling beats ray binning** for coherence, at a fraction of the complexity — he
   measured >2× on his renderer.
4. **Replace GPU neighbour searches with a write-then-read grid.** Attacks, entity collision,
   crowding, and force fields all become `atomicAdd` into a coarse grid plus a local read. 1–8 bits
   per cell is enough.
5. **Flow fields, not A\*, when the target platform is a GPU** — 4 texture reads and no branching per
   cell; 256³ flood fill = 0.2 ms on a 3080; ~65k independent targets is affordable.
6. **Split determinism by domain.** World simulation integer and deterministic; entities float,
   server-authoritative, interpolated; only entity→world *actions* need to be deterministic. This is
   much cheaper than making everything deterministic and loses nothing that matters.
7. **Two simulations, not one** — double-buffered for signal propagation where precision matters,
   single-buffered for mass/material movement where it doesn't.
8. **Make material behaviour data.** Colour, compressibility, density, surface tension, cohesion in
   JSON; only cross-material *morphing* needs code. Multi-material cost him nothing.
9. **DDA error compounds; rasterisation error doesn't.** If a stepping algorithm feeds its own next
   step, precision is a first-class design constraint — hence integer DDA.
10. **Beware the audio platform, not the audio maths.** His ray tracing was never the bottleneck; the
    CPU convolution, a main-thread audio API, and a per-browser parallel-voice ceiling were. Check
    those limits before designing around them.
