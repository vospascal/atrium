# x1m4's engine — the archived VoxelChain channel, 2022-08 → 2024-04

**Source:** `temp/archived` — channel `1003293185554001920`. 11 154 unique messages, **4 341 of
them his**, 2022-08-03 → 2024-04-06. **Zero overlap** with the other five exports.

This is the archived **VoxelChain project channel** — the one the devlog linked into for user demos.
It is where he answered questions about *his own* engine while building it, so it covers the
WebGL→WebGPU era and, critically, **the window in which CAGI was invented**.

Sixth and final channel. Verified total across all six: **18 532 of his messages** out of 68 386
in the server export.

**Confidence marking:** ▸ = his words, quoted and dated. ○ = my inference.

---

## 1. CAGI's origin — the answer, from him

Asked point-blank whether he invented it (2024-03-20):

> Sam: *"it seems like you're one of the first persons to do this technique of GI, right? Are you
> the inventor or did you see it somewhere else before?"*
> ▸ *"I wouldn't be surprised if something similar already exists somewhere, but it started with
> **some brainfart** and somehow turned into something quite useful heh"*

And the clearest single-paragraph description of what it is and why it works, same day:

> ▸ *"it's actually not using path tracing anymore, it's still very similar in the process though,
> but **the very expensive part of shooting rays is removed and instead it's propagated as some kind
> of fluid over time**. you could argue though that this is how real light waves propagate too in
> reality"*
> ▸ *"for the gbuffer and ambient occlusion, shadows and reflections I use ray tracing, but for the
> lighting like emission and sunlight+skylight injection I use cellular automata"*
> ▸ *"the cagi is really just a few **min/max operators and neighbour reads**"*
> ▸ *"an important aspect of it is that it's stable because it's integer based, so **once an area
> didn't change compared to the previous frame, it can be completely culled from further updates**"*

### Propagation speed — a number that isn't in any other doc

> ▸ *"it's about **1 cell per tick**, but the bouncing causes a few extra steps — mainly depends on
> the intensity of the injected light and the loss and reflection absorption factors"* (2024-03-22)

○ Combined with "CAGI runs at 60 fps" from the devlog: light travels ~60 voxels/second, and a change
takes distance/60 seconds to settle. That is the temporal-lag budget, quantified.

### Non-grid-aligned emissive entities

> ▸ *"yeah but you would also have to **inject the entities itself into the grid for occlusion**,
> otherwise the lighting will just go through them"* (2024-03-22)

### The diagonal-emission bug

> ▸ *"the diagonals on emissive sources is fixed in my 2d version of it"* → what was wrong:
> ▸ *"it was very simple, I just didn't output the emission diagonally, but only face wise"* (2024-03-20)

○ Worth flagging for anyone reimplementing: injection must be diagonal even though propagation is
face-wise, or emissive sources get square halos.

---

## 2. The bridge from path tracing to CAGI — a CA field steering the ray tracer

This is the missing conceptual step, and it happened **15 months before CAGI**. In Nov 2022 he built
a flood-fill field whose *only* job was to tell the path tracer where to work harder:

> ▸ *"had this idea to spread block change information like a fluid to indicate regions that should
> be accumulated faster"* (2022-11-09)
> ▸ *"when there is a voxel change I spread a value (e.g. 1.0) starting at the voxel and then
> propagate it volumetrically through the scene with a loss factor. it's somewhat similar to fluid
> propagation I guess, but instead used to accumulate the path traced lighting faster if there is a
> change nearby"*
> ▸ *"this is just an accumulation factor which increases the taken path tracing samples at the
> location, but also decreases the accumulation mix factor itself, so samples accumulate faster"*
> (2022-11-10)

He described the same mechanism a year later, and named it:
> ▸ *"when a block was changed then I propagated a value using flood fill starting from that block,
> and it influenced the path tracer so there are more pt samples done if an area was marked by the
> flood fill"* … ▸ *"**the flood fill was really just basic minecraft style light propagation**"*
> (2023-11-23)

○ So the sequence is: *Minecraft-style flood fill used as a hint for the path tracer* → *…used
instead of the path tracer*. He had the propagation machinery running alongside the ray tracer for a
year before deleting the ray tracer. That is a much less mysterious origin story than "brainfart".

### And the ancestor of CAGI's per-face opacity maps

> ▸ *"I currently generate an opacity value by **the amount of active voxels in a sub-voxel volume**
> and use this value during the path tracing. if blocks are empty inside then the lighting gets way
> too bright and light actually travels through"* (2022-11-16)

○ That is the isotropic version of the per-face opacity map described in the showcase doc — same
problem (coarse light volume vs fine geometry), same solution shape, two years earlier.

---

## 3. The cubic heightmap — the sky-visibility trick, fully explained

The technique the other docs only reference. Invented as a path-tracing optimisation (2022-11-03):

> ▸ *"currently for each path tracing bounce, I uniformly sample the sky light which is relatively
> slow (especially for larger worlds). one trick I had in mind is to **build a cubic heightmap of
> the entire world** and then trace height information from that: basically go to each face of the
> world and for each voxel, trace a ray into the face's direction until it hits something. then
> during the path tracing, I no longer have to trace rays randomly to the sky (which again is super
> expensive!), but instead I can just read each face of the heightmap and check if the voxel can see
> the sky or not"*

Refinements over the following four days:
- ▸ *"then during shading I'm planning to **project the 6 cube faces with spherical harmonics on a
  sphere** and encode the skylight if the voxel can see the sky"* (2022-11-06)
- ▸ *"the tracing of the cubic heightmap can be split over multiple frames, i.e. only 1 side is
  traced for every frame"* — shipped as ▸ *"the heightmap is traced every frame, but only in small
  **32×32 regions**"* (2022-11-07)
- Why it works for awkward geometry: ▸ *"I use this during the path tracing bouncing process, so for
  each bounce at the bounced position, I also sample the sky visibility through the heightmap. since
  it bounces, it even works for diagonal voxels"* (2022-11-07)
- Bonus reuse: ▸ *"reflections now also mirror the sky with the cubic heightmap trick"* (2022-11-26)
- And why it mattered: ▸ *"I found sun/sky shadow rays a huge bottleneck as they tend to have to be
  really long"* (2022-11-19)

○ This is the technique he later called *"a 6 side heightmap approach"* and abandoned for hemisphere
shadow maps, then for CA skylight. All three solve the same problem: **sky rays are long, and long
rays are the enemy.**

---

## 4. Cone tracing kept coming back — three times

A pattern the other docs miss entirely. He implemented cone tracing, dropped it, and tried to return
to it repeatedly:

| When | What | Why it was dropped |
|---|---|---|
| pre-2022-08 | **Cone-traced AO** — noise-free | ▸ *"I dropped it because of an annoying thing I couldn't get working regarding the sub voxels"* — ▸ *"it was just a weird bug with texture coordinates"*. He kept wanting it back: ▸ *"AO cone tracing is also a lot faster than the ray traced AO"*, ▸ *"the AO is actually the slowest part of the lighting atm"* (2022-08-27) |
| 2022-09-15 | Anisotropic cone tracing for full BRDF | ▸ *"with anisotropic cone tracing I might can reduce the light leaking to a degree where I can implement the full brdf, so we'd get really fast global illumination, reflections and refractions"* |
| 2022-12-01 | **Cone-traced shadows** | ▸ *"ok so ray traced shadows and shadow mapping sucks — both either look horrible, perform terrible, or require significant filtering techniques to look good (which result in slow performance again). I think I'll try cone tracing the shadows again"*. Only drawback: ▸ *"shadows are very smooth/wide and can't be sharp at all"* |
| 2022-11-03 | **Rough reflections via screen-space cone tracing** | ▸ *"do cone tracing in screen-space and sample from a reflection buffer with perfectly sharp reflections — the mip tree of the reflection buffer + the cone based sampling should give noiseless rough reflections"* |

And the honest cost comparison that explains why he kept coming back:
> ▸ *"the path tracer is so slow because of the many cache misses that ray tracing introduces. even
> when I trace at half resolution, it's a lot of cache pressure"* (2022-09-15)
> ▸ path-traced vs cone-traced, measured: ▸ *"path traced output is slower by 0.7ms"* (2022-09-09)

○ Cone tracing is the *other* answer to "long rays are the enemy" — trade angular precision for cache
coherence. CAGI is the third answer, and the only one where the trade is fully in his control.

---

## 5. The voxel-face denoiser, on the day he thought of it

> ▸ *"heh my screen-space filter is still pretty bad"*
> ▸ *"had this idea to instead of blurring just by nearby pixels, I could also **create a filter that
> blurs neighbor voxel faces in screen-space** — it shouldn't be too hard to project voxel faces in
> screen-space"*
> ▸ *"start at 0, project the neighbor voxel faces and take a sample from their center"* (2023-03-24)

Landed within months: ▸ *"I went with a box filter that gives this pixelated look while being cheap
to run and simple to implement"* (2023-07-01), and the aesthetic consequence he was pleased by:
▸ *"it would actually be possible to export the voxels + the lighting and 3d print it — if you look
close you can see that the lighting is actually pixelated and constant over voxel faces"* (2023-05-25).

Also from this channel, the **per-voxel vs per-voxel-face** origin: ▸ *"it basically combines the
lighting over full voxels, so it's like a per-voxel lighting instead of per-pixel — it's somewhat
like the old minecraft blocky lighting, but path traced"* (2022-08-07).

---

## 6. The simulation architecture, declared in advance and then followed

The 2023-01-14 roadmap post is remarkable for how accurately it predicted the next three years:

> ▸ *"the simulation will be revisited with the following approach:*
> - *the circuit compiler will be removed, instead a **programming-based API** will be provided which
>   lets you easily generate truth tables to program voxel behavior*
> - *there will be **3 separate kinds of simulations running in parallel**: a **cell simulation** (for
>   stuff like sand, grass, dirt and all sorts of solid materials), a **flow simulation** which can be
>   used for flow-based materials (such as water, lava, power i.e. redstone etc.), a **particle
>   simulation** which can interact with both the cell and the flow simulation — all these will get
>   the main benefit from the switch to webgpu and will be run in compute*
> - *I'm planning to add infinite world support and use my CA based terrain generator for that"*

### Why truth tables can't be dropped — the double-buffering cost, stated exactly

This is the most important architectural argument in the channel (2022-11-21):

> ▸ *"the thing about making a 100% stable + deterministic voxel/cell simulation is that you need
> double buffering. otherwise it's basically impossible to create circuits because update orders of
> blocks are essentially random if you don't do double buffering"*
> ▸ *"now there is one major disadvantage for double-buffered simulations, and that is that **in order
> to find out the next state of a cell you want to update, you need to iterate all direct neighbors of
> that cell, calculate their state, and then you can calculate the state of the current cell**. if
> instead of a truth table you'd implement all the behaviour of cells with regular programming, then
> this is insanely slow depending on how complex your overall simulation is"*
> ▸ *"that's why I can't get rid of truth tables, because they're the most efficient way to do this
> stuff (but come with quite some limitations as well)"*
> ▸ atomics rejected: *"you could use atomics to do this in a single buffered simulation, but atomics
> are way too slow for mass scale simulations since they're essentially spin locking. **iirc the noita
> devs tried atomics too but immediately dropped them because they're too slow**"*

And the split that follows from it, pre-declared in 2022 and still true in 2026:
> ▸ *"so there would be a **cell simulation (using truth tables for behavior execution)**, and an
> **entity/object simulation (using regular programming like js or something)**"* (2022-11-21)
> ▸ restated 2023-05-13: *"like 2 running simulations, one being the circuit sim and the other being
> a **single buffered** sim to allow movement/swapping of cells over large distances"* — because
> *"the problem with single buffered sand sims is that the behaviour is random, while when double
> buffered it's very precise and it's essential for circuit stuff"*

### The circuit compiler's actual implementation, and its execution tiers

| Pins | Representation | Cost |
|---|---|---|
| ≤16 | **Truth table** | ▸ *"insanely fast"*, constant time per cell regardless of circuit complexity |
| 17–26 | **ROBDD** | ▸ *"a bit slower since the execution time correlates with the total complexity of your circuit"* |
| >26 | not allowed | ▸ *"otherwise the truth tables go into gigabytes of size"* |

▸ *"basically if you want to precisely kill the simulation engine performance, then you have to use
more than 16 input pins"* (2022-08-27). Truth table I/O is 32-bit max, with **index remapping**
because pin indices can exceed 32 (2022-09-15). Compilation time grows **exponentially** with active
input pins and *"heavily benefits from multi threading"*.

The visual compiler's replacement, prototyped 2023-05-13 as a `CircuitCompiler` TypeScript API with
`INPUT_TYPE` / `OUTPUT_TYPE` declarations — and the reasoning:
> ▸ *"the style above is mostly just to make it clear that the **IOs are at setup time and cannot be
> changed during the compilation** of the program because that would potentially break the truth
> table"*
> ▸ *"with this basic stuff above you can probably achieve **Noita material complexity and beyond**"*
> ▸ ▸ *"the entire visual circuit compiler will be removed. instead I switch to raw truth tables and
> at first you'll be able to compile programming languages into the truth tables. visual circuit
> compilers (like node-based) could be created by the community"* (2022-11-21)

---

## 7. The VoxelChain-era data layout, exactly

More precise than anything in the other docs, because he was answering Gabe Rundlett's questions:

| Field | Size |
|---|---|
| Cell | 32 bit — ▸ *"cells currently use **25 bits**"*; contains material id, rotation, animation frame |
| Flow | 32 bit — ▸ *"flows use **17 bits**"*; contains power etc. |
| Swap | 8 bit — ▸ *"8-bit are used to allow to swap cells with each other"* |
| **Total** | **72 bit**, ▸ *"and the simulation is actually double buffered which means it's 2× the size"* |
| Power | 4 bits (range 0–15) |
| Light | ▸ *"4+4+4 bits for light (for each color channel)"* |
| Rotation | 5 bits, index into a rotation table, applied by **vector swizzling** not matrices |
| State bits | **8 per voxel**, readable/writable by IO pins; ▸ *"if a user clicks on a voxel, the 8th state bit is flipped — so this way, you can detect a user interaction in your circuit"* |
| Materials | 256 max (later 512), each with **up to 8 models** = animation frames |
| Animation | ▸ *"every voxel is individually animated every frame"*, with a **per-voxel animation frame base index** so ▸ *"animations are not synchronous everywhere but appear as individual"* |

Signal semantics, which is a nice piece of design: ▸ *"the formula used is simply
`if (newPower > previousPower) signal = 1;`"* — ▸ *"this is what the signal in voxelchain models,
it's digital signal processing from real hardware"* (2022-09-11).

**Sub-voxel scale history:** imported from MagicaVoxel at 32³ (2022-08-07); 8/16/32 comparison shots
(2022-11-16); ▸ *"before I went up to **256³ sub-voxel volume sizes** — that's definitely a performance
killer but it still ran fine on my GPU"* (2022-10-29). Effective traced resolution ▸ *"2048³"* in the
WebGL era; 512³ main × 8³ sub = ▸ *"68,719,476,736 effectively rendered voxels"*.

**23 render passes** as of 2022-11-03. **~40 shaders / 5 000 → 10 000 lines of GLSL** to port to WGSL.

---

## 8. Audio — the decision, made 16 months before implementation

The three-step arc on how sound would be computed, all in this channel:

**Step 1, the seed (2022-09-08)** — an outside expert put the idea in his head:
> ▸ *"I recently had a chat with an audio engineer and he mentioned the possibility of **tracing
> sounds using my flow simulation or with ray tracing**. beam tracing sounds similar to this"*
> ▸ and he rejected the flow-sim option on the spot, with reasons: *"I only have about 16bits left in
> my flow data, and I think to propagate audio in my flow simulation I'd also need to store direction
> vectors. I think ray tracing the sound would work much better, also **with flow the propagation of
> sound would be heavily delayed and slow**"*

○ Note the irony: he rejected propagating sound as a field because propagation is too slow — and
then two years later replaced his *light* solver with exactly that, accepting the same latency.

**Step 2, spherical harmonics considered and dismissed (2022-10-30)**:
> ▸ *"currently wondering if spherical harmonics could also be used for ray tracing and propagating
> sound on the GPU — since sound is frequencies too, probably?"*
> ▸ *"actually the propagation would probably be too slow. path tracing/bi-directional path tracing
> is probably the way to go"*

**Step 3, corrected by evidence (2022-11-19)**:
> ▸ *"I think I mentioned here recently that I was thinking about if spherical harmonics could be used
> to store and propagate ray traced sound — didn't think much of it but of course **I was proven wrong
> and it's indeed a viable technique**"* + the Unreal Engine spatialization docs.

### And the honest state of the implementation, day two (2024-01-04)

> ▸ *"my impl for muffling/direct occlusion is super crap right now, I currently just march a ray from
> the sound source towards the player, but **the proper way seems to do this for every bounced sound
> ray from both the player and the sound source**"*
> ▸ the footstep bug: *"for some reason I just can't remove the spatial lag of the player footsteps"* —
> *"they are played slightly delayed but mainly are affected by the listener's orientation even though
> I have it disabled"*, after trying both pinning them to the listener and disabling spatialisation
> entirely. ○ Never resolved in this channel.
> ▸ asset poverty: *"only have 3 stone step variants atm because I lack sound files"*
> ▸ per-material footsteps as an open question: *"like walking on sand vs stone gives a different
> sound?"*
> ▸ **wind as a sound source**, thinking out loud: *"I guess every material that isn't completely solid
> and is affected by wind should produce a sound"* … *"can you think of any other material than
> vegetation that clearly produces sound when affected by wind?"* … and on wind reverb: *"like the wind
> also having reverb"*
> ▸ synthesis, the plan and the block: *"[ZzFX] planned to use the ideas here for generating procedural
> sound"* — *"but generating realistic sounds and not just game/8bit sounds is just too complicated for
> me. I'm a too big noob in audio stuff"* … *"wind isn't too hard as you mostly get away with procedural
> noise"*
> ▸ and from 2022-08-21, a year earlier: he wanted to *"hire people … at least consulting (i.e.
> **low-level sound synthesis**)"*

○ Three things here are directly useful to atrium: (1) he independently identified that single-ray
occlusion is wrong and bidirectional bounced rays are right, and shipped the wrong one anyway;
(2) **wind-driven material sound** is a design axis he reasoned about and never built; (3) he
identified procedural wind noise as the tractable case, which is exactly where atrium's synth arc
started.

---

## 9. Other engine details worth keeping

**Tree generation = directional flood fill, with the rule** (2023-10-05):
> ▸ *"the first tree stem has a value of 16 assigned, then in the next update step the air voxel above
> or at the edges (randomly decided) sees the stem voxel, turns itself into a stem too and subtracts 1
> from it. similar thing happens for the leaves"*
> ▸ *"like no L-trees or anything just flood fill"*; the flood-fill value doubles as the growth limiter:
> *"used as a starting value to prevent trees from growing infinitely and also detect when to start
> growing leaves"* (2023-10-22)

**Particles, cascaded and fully lit** (2023-11-21 → 2023-12-21):
> ▸ plan: *"I'm planning to add a **particle voxel volume around the camera which uses falling sand to
> do the particle movement** — the particle volume can easily be ray traced. maybe I can also write the
> entity voxels into this volume, which then can be made part of the rest of the lighting"*
> ▸ shipped: *"they're fully integrated into the path tracing pipeline like the rest of the voxels in
> the world … you can do crazy stuff with it without really any performance limits in terms of particle
> count"*, and *"since the particle volumes are **cascaded**, it works even for very far distances"*

**Vegetation animation, the exact mechanism** (2023-09-28, 2023-12-21):
> ▸ *"I sample from a 3d noise texture and skew based on the noise result — noise input is just the
> world-space voxel + sub-voxel position"*
> ▸ *"I just skew the ray origin before traversing the sub-voxels. **sub-voxels can't ever exit their
> voxel boundings** — the skewing just gives the illusion that they slightly do"*
> ▸ *"the ray is always clamped right to the borders of the subvoxel volume, so it can't ever be inside
> it"*

**26 rotations exist for an art reason**, not a technical one:
> ▸ *"that's why I added 26 unique rotation support btw, for repetitive stuff like vegetation it's
> really useful to reduce the repetition"* — ▸ *"or it's like rotation + mirror"* (2024-03-24)

**Everything-on-GPU completed 2023-11-13:**
> ▸ *"finally got the camera and player object moved to the GPU recently — now literally everything is
> on the GPU and the player can actually collide with the voxels in the world"*

**Fluid sim: the literature survey result** (2024-01-22), which explains why he wrote his own:
> ▸ *"it's ridiculous that there don't seem to be any fully deterministic fluid sims out there — like I
> checked everywhere but found nothing except stuff like Minecraft fluids"*
> ▸ *"all the fluid sims I checked were not mass conservative and really often had oscillation and
> floating point precision issues. there was a paper by utrecht that sounded promising, claiming that
> they didn't have oscillation issues..but they did lmao"*
> ▸ *"afaik even unreal engine and unity don't have multiplayer support in their fluid sims"*
> ▸ debugging effort: *"to debug the total mass existing in the simulation I had to **implement 64 bit
> atomic add on the GPU**, because there was too much mass in the sim"*

**Determinism split, pre-declared 2023-12-25** (14 months before the general-programming quote):
> ▸ *"that's the good thing about voxels, it's mostly integer math — at least if the voxels are grid
> aligned. entities are the only non aligned objects and use float math, but they will be handled by
> the server and their actions and their interactions with the world are deterministic"*

**Multiplayer sync, the whole protocol in one message** (2022-12-22):
> ▸ *"every player starts with the same simulation state and when you draw a pixel, you send the pixel
> draw command to the server; the server stacks it into a pool of commands and for every simulation
> step then sends it to all players. once a player receives the collected commands, the player puts
> them into the simulation and afterwards executes one simulation step locally. this way **only player
> draw commands are sent over the network** while the whole simulation of each client runs completely
> independent on their machine"*
> ▸ ▸ *"it's super dumb and simple but very efficient"* — ▸ *"like 30 lines of synchronization code"*,
> and tested with artificial 300 ms lag: *"it just worked fine, like it wasn't game breaking at all"*

**Compression, the final word** (2024-04-26):
> ▸ *"I'm only culling empty/full air chunks because any sort of more advanced culling or compression
> adds significant overhead which for path tracing is quite a dead end in terms of performance"*
> ▸ *"more memory == more performance / less memory == less performance"*

---

## 10. Platform bugs and workarounds, dated

Useful if you ever touch browser GPU work:

- **Threads sharing one stack.** ▸ *"each thread is executed in the same stack memory location, so
  they just kept overwriting each other — it's something that emscripten fixes for you"* (2022-08-19).
  He hand-rolled clang+wasm threading and hit this.
- **Manual mip generation in WebGL** via `TEXTURE_BASE_LEVEL` / `TEXTURE_MAX_LEVEL` — legal per spec,
  ▸ *"mostly every browser is broken here"*, and on non-power-of-two textures it hits a
  [broken ANGLE D3D path](https://github.com/google/angle/blob/main/src/libANGLE/renderer/d3d/TextureD3D.cpp#L675-L693).
  Worked around with a POT reflection buffer (2022-11-09).
- **`KHR_parallel_shader_compile`** stopped a ~10 s main-thread freeze (2022-10-28).
- **Chrome without ANGLE**: shader compilation almost instant, ▸ *"but rendering performance is more
  than 2x slower"* (2022-10-26).
- **tint's `robustness` transform** is what inserts the slow texture bounds checks — the reason
  GLSL→WGSL output looked bad (2022-11-05). Full command line:
  `glslangValidator -V -S frag input.glsl -o input.spv && tint --format wgsl --transform fold_trivial_single_use_lets,renamer,robustness input.spv -o input.wgsl`
- **Read+write on one texture in a fragment shader** — legal per the OpenGL spec, but triggers a
  ~100 %-GPU slow path in Chrome's WebGL (2022-11-04).
- **Fragment shaders over compute, in 2023**: ▸ *"you should only use compute if there is an actual
  benefit of block processing larger than 2×2 and otherwise, e.g. for post processing effects etc.
  always use fragment shaders"* (2023-01-29). ○ He reversed this completely by 2024 — the shader tree
  is compute-dominated.
- **The WGSL arc**, worth quoting for the reversal: 2022-11-05 ▸ *"I've used wgsl for months and wrote
  thousands of lines with it and came to the conclusion that it's **the worst shader language I've used
  so far**"* → 2023-05-26 ▸ *"I was a full time hater on wgsl but it actually turned into a good
  language"* → 2023-10-05 ▸ *"wgsl was bad too but it got soo much better over the past months"*.

**Language split over time:** TypeScript + **C89** simulation + GLSL (2022-10-24) → 1 % TypeScript /
99 % WGSL compute (2023-12-21) → 10 % TS / 90 % WGSL / 0 % Rust (2024-03).

---

## 11. What this closes, and what's left

**Now answered:** CAGI's origin (an accumulation-hint flood fill that outlived the path tracer it was
hinting), its propagation speed (~1 cell/tick), the diagonal-injection requirement, the opacity-map
ancestor, the cubic-heightmap trick in full, why truth tables survive double buffering, the exact
VoxelChain bit layout, and the *audio* decision chain from an audio engineer's suggestion in Sept 2022
to a shipped implementation in Jan 2024.

**Still outside all six exports:**

1. **The voxelgamedev (VGD) server, keyword `cagi`.** Six channels in, this is the last real gap — he
   says three separate times that the detailed explanations live there. Reimplementers named across
   the exports: `sweg`, `👾Rareș👾`, `Dapper Core`, `bob08022010`; taught directly: `bonisdev`,
   `KosmosisDire`, `𝕶𝖊𝖑𝖛𝖎𝖓`.
2. **Patreon posts + the site's early-access area** — the actual release notes.
3. Two sibling channels this one references but that aren't exported: `1003294515408416769` (where
   mityankin's sand circuit lives) and `1003345384019595375`'s neighbours.
4. **The collaborator `316239158584803328`** — still only visible second-hand.

---

## 12. What's newly worth stealing

1. **Use a propagating field to steer your sampler before you replace your sampler.** His CA field
   started as "where should the path tracer spend more samples" and ended as the lighting itself. A
   cheap, low-risk way to get the machinery in place.
2. **Inject diagonally even when you propagate face-wise.** Otherwise point sources get square halos.
3. **Sky visibility from a 6-face heightmap**, traced incrementally in 32×32 regions, sampled *per
   bounce* so it works for awkward geometry, and reused for sky reflections. One texture read replaces
   an unboundedly long ray.
4. **Derive coarse opacity from occupancy** — "count the solid sub-voxels in the volume" is the
   isotropic version, and the per-face version is the anisotropic one. Either beats treating a coarse
   cell as fully solid or fully empty.
5. **Cone tracing is the middle option** between long rays and propagation: trade angular precision for
   cache coherence. He measured path tracing at only +0.7 ms over cone tracing, and the difference was
   almost all cache pressure.
6. **Double buffering costs more than memory.** To find a cell's next state you must first compute all
   its neighbours' next states — so per-cell behaviour has to be a *lookup*, not code. That, not
   elegance, is why his cell behaviour is a compiled table.
7. **Two simulations beat one**, and he knew it in 2022: double-buffered for anything that must be
   precise, single-buffered for anything that moves mass over distance.
8. **Sync only player commands.** ~30 lines, survives 300 ms of latency, and works because the
   simulation is deterministic. The server's only job is stacking commands and issuing ticks.
9. **Occlusion done properly is bidirectional** — bounced rays from *both* the source and the listener,
   not one ray between them. He wrote down the right answer and shipped the cheap one; the note is the
   useful part.
10. **Wind-driven material sound is an unexplored axis.** ▸ *"every material that isn't completely
    solid and is affected by wind should produce a sound"* — he asked what besides vegetation qualifies
    and never answered it.
