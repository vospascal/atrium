# x1m4's engine — his own devlog channel, 2022-07 → 2026-08

**Source:** `temp/show` — channel `1003345384019595375`. **286 messages, every single one his.**
2022-07-31 17:07 → 2026-08-03 11:13 UTC.

This is the channel the other three kept linking into. It is his **solo devlog / announcement
channel** — no discussion, just "here's what I built today" plus an attachment. That makes it the
**authoritative dated timeline** for the engine, and it settles several things the other docs got
wrong or left open.

Running total across five channel exports: **14 191 of his messages**.

**Confidence marking:** ▸ = his words, quoted and dated. ○ = my inference.

---

## 1. Corrections to the other four docs

### A playable public build exists — and it does sound synthesis at startup

> ▸ *"Decided to upload the latest local build of my voxel engine, you can try it here:
> **https://voxelchain.app/xima-sandbox/0.0.1-pre-alpha/example/index.html** — Remember that the
> whole engine runs entirely on your GPU → YOU NEED A DECENT GPU AND CHROME! When starting for the
> first time, it will take about a minute since a lot of pre-processing is done like **sound
> synthesis** and shader compilation"* (2025-07-16)

**This corrects the audio section of the showcase doc.** I wrote that he gave up on procedural
synthesis and fell back to pre-recorded samples, based on his 2023-12-31 message. He did go back to
it: the shipped build **synthesises sounds during startup pre-processing**. Combined with
2024-06-11 — ▸ *"dynamic high quality pitch shifting through granular analysis"* — the audio engine
does more DSP work than the earlier docs credit.

Controls, for reference: WASD, SPACE jump, X fly up, C down, SHIFT sprint, LMB break, RMB place,
wheel = inventory slot, and *"when placing a block with the first inventory slot active, then a
Zombie Entity is spawned"*. He also names the bottleneck: ▸ *"the rendering speed is currently
heavily slowed down by my lazy water rendering implementation as it uses quite expensive ray
marching to make the water have realistic depth"*.

### Blockbench is gone — entities are fully procedural

> ▸ *"entities are fully procedural now (no blockbench anymore) — much easier and faster to iterate
> with"* (2026-07-30)

○ Every earlier doc describes the entity pipeline as Blockbench-authored OBBs + skeletal tree. As of
2026-07-30 that's replaced by procedural generation. The OBB/DDA *intersection* work presumably
survives; the authoring tool doesn't. Followed two days later by ▸ *"human skeleton animation test
(next animation test will probably be fish)"* (2026-08-01).

### The cave-ambience heuristic was actually implemented

In the general-programming doc I quoted him *theorising* this in March 2024. It shipped:

> ▸ *"better cave ambient sound, now determined based on **ray traced luminance, stone blocks and sky
> occlusion**"* (2024-05-31)

○ That is his March sketch — room size / light level / nearby block type — built, with sky occlusion
substituting for room size. Directly comparable to atrium's ambience keying.

### CAGI drives mob spawning — confirmed, with the full criteria

The "lighting as gameplay data" claim finally has an implementation date and a parameter list:

> ▸ *"Added entity spawning (based on **occupied space, light level, block material, per entity type
> global count**), world loading & saving, block breaking resistance levels and slightly reworked the
> sound ray tracing"* (2025-11-19)

○ Note *"world loading & saving"* — the general-programming doc quoted him in 2024-03 saying the
engine had *"not even world saving lol"*. That gap closed in Nov 2025.

### Inverse kinematics shipped

Wanted since 2024-02, repeatedly deferred as *"probably something for far in the future"*:
> ▸ *"first inverse kinematics test"* (2025-11-26) → ▸ *"second test with leg walking animation"*
> (2025-11-26, same day)

---

## 2. CAGI's ancestor: the cascaded irradiance cache, implemented in detail

The single most technically specific message in this channel, and it predates CAGI by 14 months. This
is the McLaren *Tomorrow Children* cascade scheme as he actually built it:

> ▸ *"multi cascades are working now including scrolling. also based on the GDC talk by James McLaren
> I implemented:*
> - *Grid snapping for stable scrolling (**snapped to a multiple of the cascade scale**)*
> - *Cascades are stored **within a single 3d texture** to get fast trilinear filtering during
>   blending*
> - *Cascades are offset based on the **camera position, view direction and world boundings***
> - ***Only 1 cascade is updated per frame** (Cascade 0 updated every 2nd frame, cascade 1 every 4th
>   frame etc.)"* (2022-12-16)

Four things there that no other doc has: the snapping rule, the single-texture packing *for filtering
reasons*, the view-direction-aware offset, and the **geometric update cadence** (2ⁿ frames per cascade
level). ○ The last one is the cheapest idea in the whole export — cascade *n* only needs updating at
1/2ⁿ⁺¹ the rate because it covers 2ⁿ× the volume.

He had shown the failure mode nine days earlier: ▸ *"what failed cascade scrolling looks like"*
(2022-12-07), and the switch was flagged as the big blocker a month before that: ▸ *"currently the
largest task will be switching to a clipmap/cascade approach for the irradiance cache"* (2022-11-10).

**The sky-visibility fix** that preceded CA skylight, with the problem stated precisely:
> ▸ *"here you can see the new sky visibility system in action — before the lighting in these areas
> would be very unstable and flicker a lot because **only a few rays would exit the window**. with
> the new system the lighting is fully stable and smoothly spreads into the room"* (2022-11-08)

**The infinite-bounce trick, on the day he found it:**
> ▸ *"1 bounce radiance cache: … infinite bounces (for free): … was surprised I couldn't come up with
> this trick earlier"* (2023-03-27)

**And a humbling one worth remembering** — he had been showing off GI that wasn't GI:
> ▸ *"shocking news! in the last videos the lighting actually didn't have global illumination, as I
> messed up a parameter regarding albedo. now as you can see, it correctly bounces: there are white
> emissive light sources behind the wall, reflecting the light from the wall and floor which bleeds
> the color into the room behind"* (2022-11-01)

**Fireflies, diagnosed and filtered** (this is `firefly-filter.wgsl` (57 lines) in the shader tree):
> ▸ *"a few comments in my last video mentioned fireflies (small white flickering pixels) often found
> in dark areas — these are caused by **precision errors in my ray tracer**, but now get eliminated by
> my firefly filter"* (2024-08-12)

**Reflections, current state** (2026-07-30):
> ▸ *"ray traced reflections (with **stable temporal reprojection**!) — in this order: roughness 0%,
> roughness 20%, roughness 80%, disabled reflections"*

○ Reflections are the one ray-traced path left alongside AO, and as of mid-2026 they have proper
temporal reprojection and a roughness parameter. Also 2026-07-30: ▸ *"per-voxel diffuse and per-pixel
specular"* was confirmed in the showcase channel the same day.

---

## 3. The circuit compiler — ROBDDs, then deleted

The VoxelChain circuit system's implementation, which no other channel explains:

> ▸ *"In case anyone wonders, the circuits you see here are compiled into binary form and are
> represented as **ROBDDs**: https://en.wikipedia.org/wiki/Binary_decision_diagram"*
> ▸ *"BDDs are often used when designing hardware circuits and are a compact way to express and
> optimize binary logic"* (2022-08-07)
> ▸ the cost: *"Depending on the amount of active input pins, the compilation time increases
> **exponentially**, so being able to see the progress and abort if necessary should be handy"*

Three months later he threw them out:
> ▸ *"**ROBDDs were removed** and the simulation engine now uses only truth tables with a state
> complexity of 1^16 (16 input pins)"* (2022-11-20)

**Correction from the archived channel:** ROBDDs were never the primary representation — a **truth
table was the ≤16-pin fast path (constant time per cell), and the ROBDD was the 17–26-pin fallback
whose cost scaled with circuit complexity**. See [x1m4-archived-voxelchain-channel.md](x1m4-archived-voxelchain-channel.md)
§6 for the tier table. The 2022-11-20 change dropped the slow tier and capped everyone at 16 pins.

○ So the lineage of material/cell behaviour authoring is: **truth table + ROBDD fallback (2022) →
truth-table-only, 16 inputs (2022-11) → a code-based `CircuitCompiler` emitting truth tables (2023-05)
→ shader-authored behaviour (2023-08) → WGSL mod system (2026)**.

Users built real things with it, which is the part that validates the design: a **7-segment display**
(2022-08-29), a **full binary adder** with SUM and CARRY pins (2022-08-16), buttons and levers as
circuits (2022-08-16), and a working **casino gambling machine** (2022-09-06).

---

## 4. The data-structure limits, as they moved

A dated table of what the engine could hold, which the other docs only give a single snapshot of:

| Date | State |
|---|---|
| 2022-08-16 | Voxel rotation in **5 bits**, done with **vector swizzling and flipping, not rotation matrices** — ▸ *"which is faster than applying a rotation matrix"*. [gist 5799988d8fcc97af1262eb401d52efb7](https://gist.github.com/maierfelix/5799988d8fcc97af1262eb401d52efb7) |
| 2022-10-30 | Sub-voxel resolution **32** demonstrated |
| 2022-11-10 | Non-power-of-two world sizes; **512×32×512** shown |
| 2022-11-20 | ▸ *"world size is now dynamic on the horizontal and vertical axis (16x–256x); sub-voxel size is dynamic (**8x–32x**); material count is dynamic (up to **512** individual)"* |
| 2022-12-12 | **512×256×512** loading part of the 2b2t spawn area; ▸ *"I think I can even get to 1024×256×1024 with a few changes"* |
| 2026-06-23 | ▸ *"also testing higher sub-voxel resolutions (**16x** in that screenshot)"* |

○ Worth noting the direction of travel: the *maximum* sub-voxel resolution was 32 in 2022 and he's
back to testing 16 in 2026 — consistent with the showcase doc's *"16³ is the leak limit"* for CAGI's
per-face opacity maps. The renderer could always do more than the light sim can tolerate.

**Per-sub-voxel material properties** landed 2022-11-03: ▸ *"currently supported properties are:
diffuse, metal, emission, glass"*.

---

## 5. Fluids, the 2025–26 continuation

The general-programming doc ends the fluid arc at "100% stable, 2025-10-14". This channel carries it
further, and names the mechanism that makes it cheap:

> ▸ *"first attempt at material reactions with the WIP cellular automata fluid simulation"* (2025-12-05)
> ▸ *"experimenting with temperature, so it will be possible to refine and convert materials this
> way"* (2025-12-17)
> ▸ *"foam test"* (2026-07-28) · ▸ *"fire test"* (2026-07-31)
> ▸ **the important one:** *"**synthetic fluid dampening** is getting better which is essential for
> dirty chunks (the red squares)"* (2026-08-01)

○ That last message names the coupling explicitly: the fluid sim needs *artificial* damping, not
physically motivated damping, because the dirty-chunk culling only works once cells stop changing.
The performance system dictates the physics. Same trick as applying gravity virtually so it never
dirties state (general-programming doc §7).

**Underwater base entry**, the gameplay problem behind all of it:
> ▸ *"Experimenting with what's the ideal way to enter/exit a submerged base. Currently I'm using a
> **surface net** to prevent water from entering the base. Since the water has pressure, maybe players
> should be required to create an underwater lock system using water pumps?"* (2025-11-14)

---

## 6. Flow-field pathfinding, final form

The general-programming doc has the theory and the 0.2 ms number. Here's what it became:

> ▸ *"pretty happy with the flow field path finding results, the flow field also **encodes jumping and
> falling cases** and generally avoids paths that can't be traveled by walking entities, such as if an
> entity is too large to cross a path, if the fall distance would be too high or if it wouldn't be
> able to jump to the target location"* (2025-11-25)
> ▸ *"walk path finding now also supports jumping over gaps!"* (2025-11-25)

○ i.e. the flood-fill field is no longer just distance — it carries **traversability class** per cell,
per entity size. That's a lot of expressiveness for 4 texture reads.

Earlier milestones: ▸ *"basic flow fields"* (2024-06-08); the force-field solution to cramming
▸ *"Entity cramming and collisions solved with a GPU force field which is very efficient to update and
evaluate and **can probably be extended to handling entity attacks and damage too**"* (2024-06-07) —
which it was, four days later.

---

## 7. The engine's public artifacts, all in one place

Three GitHub repos, two of which the other docs missed:

| Repo | What |
|---|---|
| [VoxelChain/voxelchain-formats](https://github.com/VoxelChain/voxelchain-formats) | ▸ *"all necessary tools to work with the file formats of VoxelChain"* (2022-09-04) |
| [VoxelChain/voxelchain-programming](https://github.com/VoxelChain/voxelchain-programming) | ▸ *"boilerplate example of how you can do programming with the public API of VoxelChain"* (2022-09-05) |
| [VoxelChain/voxelchain-terrain-generator](https://github.com/VoxelChain/voxelchain-terrain-generator) | ▸ *"The cellular automata based terrain generator that I use in most of my demos is now open-source"* (2022-09-11) |

Plus, from this channel only:
- **Export a world to a single self-contained HTML file** for embedding (2022-09-01).
- The engine exposes **part of its API in the browser console** — the hook for the console-based
  modding he later formalised.
- Live demos: `voxelchain.app/previewer/?world=Demo` · `Jungle 1` · `Jungle 2` ·
  `previewer/RayTracing.html` · `previewer/Casino.html` · `previewer/7 Segment Display`.
- The pre-alpha sandbox build (§1).
- [maierfelix.github.io/strata-voxel](https://maierfelix.github.io/strata-voxel/) — ▸ *"This little
  tool lets you explore fractal worlds generated using chaos theory"* (2026-02-16).
- VoxelChain hit the **Hacker News front page** on 2022-09-06; 1 000 YouTube subscribers 2022-11-21.

**The video release timeline** — ○ useful because each video is a dated capability snapshot, and the
docs' era boundaries line up with them:

| Date | Video | What it demonstrates |
|---|---|---|
| 2022-07-31 | `hItrS6NaYW8` | circuit-driven falling voxels |
| 2022-10-23 | `nT6xTB0TteQ` | ray-traced glass |
| 2022-10-25 | `j77Pub-F2YI` | ray-traced reflections |
| 2022-11-24 | `05itZEDaj1A` | ray-traced sun + shadows |
| 2023-09-21 | `qfIkbo6r0-I` | first post-WebGPU-port showcase |
| 2023-11-23 | `TfOOtaJs9cg` | GI + GPU entity collision ("player can jump, fly and also collide") |
| 2024-01-06 | `of3HwxfAoQU` | first video with ray-traced sound |
| 2024-03-23 | `g4EHh9Or_X8` | ▸ *"testing the engine's limitations"* |
| 2024-08-10 | `8hUzbpE31QE` | the one whose comments prompted the firefly filter |
| 2024-12-23 | `fV6syMVRFaU` | last video before the NDA-job gap |

---

## 8. Dated milestones this channel fixes precisely

Things the other docs date loosely or not at all:

| Date | Milestone |
|---|---|
| 2022-08-22 | **Patreon integration complete** — early-access area on the site + Discord role |
| 2022-10-26 | Public early access closed again, Patron-only |
| 2022-11-14 | Minecraft map importer WIP → 2022-11-16 working **mesh→voxel conversion** |
| 2022-11-22 | Side-by-side comparison shots vs **SEUS PTGI HRR3** |
| 2023-03-05 | **WebGPU port begins** ("in progress webgpu port") |
| 2023-03-20 | ▸ *"got a big chunk of the webgl to webgpu port done over the weekend — there is no lighting yet, only ray-traced ambient occlusion in order to verify my ray traversal methods"* |
| 2023-03-22 | Per-voxel-face lighting prototype; **per-sub-voxel vs per-pixel RNG seeding** compared |
| 2023-09-26 | Minecraft-style flood-fill lava propagation |
| 2023-11-04 | **Conveyor belts** first appear (straight, curve, inverted curve, input splitter, input merger) |
| 2024-01-04 | ▸ *"Work in progress GPU ray traced sound"* — the public debut |
| 2024-02-23 | **100 000 animated entities** |
| 2024-03-04, 03-25 | Two separate sRGB-conversion bugs found and fixed |
| 2024-06-11 | Granular pitch shifting |
| 2024-06-18 | Basic mining |
| 2024-06-22 | ▸ *"first working infinite world prototype"* (abandoned four days later) |
| 2024-07-23 | Portals, second attempt |
| 2024-10-23, 11-06 | The two 2D game prototypes |
| 2024-11-18 | ▸ *"purely SDF-based shapes (WIP)"* |
| 2025-10-06 → 2025-10-29 | Return to work: wand model, island test, fish, underwater procgen |
| 2026-06-18 | Shader **hot reloading** test |
| 2026-06-26 | ▸ *"first test scene loaded as a **live main menu background** (done entirely through the modding api)"* |
| 2026-07-02 | Orthographic camera test |
| 2026-07-09 | ▸ *"UI and editor WIP"* |
| 2026-07-30 | ▸ *"everyone knows what this kind of physics is asking for"* → 2026-08-03 ▸ *"dismembering test"* |

○ The 2026-06/07 run is the clearest evidence the mod system is real and load-bearing: within eight
days of finishing the modding API he used it to build the main menu.

---

## 9. What's left to find

The five channels together are now comprehensive on the engine. Two gaps remain, both outside Discord:

1. **The voxelgamedev (VGD) server, keyword `cagi`.** Unchanged as the only place the algorithm is
   explained in depth by his own repeated account. Reimplementers named across the exports: `sweg`,
   `👾Rareș👾`, `Dapper Core`, `bob08022010`; people he taught directly: `bonisdev`, `KosmosisDire`,
   `𝕶𝖊𝖑𝖛𝖎𝖓`.
2. **The Patreon posts and the site's early-access area.** This channel proves they exist and carry
   release notes; no Discord channel replicates them.

And one that just got smaller but not closed: the collaborator `316239158584803328` — this channel
shows him crediting *users* (`463033610316939264`, `239722987539005440`) for demos, but the fluid-sim
co-author still only appears second-hand.

---

## 10. What's newly worth stealing

1. **Update cascade *n* only every 2ⁿ⁺¹ frames.** Cascade 0 every 2nd frame, cascade 1 every 4th, etc.
   Free, obvious in hindsight, and he attributes it to the McLaren talk.
2. **Pack all cascades into one 3D texture** specifically so hardware trilinear filtering does the
   inter-cascade blending for you.
3. **Snap cascade scroll to a multiple of the cascade's own scale.** This is what makes scrolling
   stable; he posted the video of what happens when you don't.
4. **Offset cascades by view direction, not just camera position** — spend resolution where the
   player is looking.
5. **Damp your simulation synthetically so it can be culled.** ▸ *"synthetic fluid dampening … is
   essential for dirty chunks."* If a sim never settles, dirty-rect culling buys nothing, so add
   unphysical damping until it does.
6. **Put traversability into the flow field, not into the agent.** Encode jump/fall/size feasibility
   per cell during the flood fill; the agent then just reads a gradient.
7. **Fireflies in a voxel path tracer are usually precision errors, not variance.** Worth checking
   before reaching for a better denoiser.
8. **Rotate by swizzle + flip, not by matrix** — 24 rotations in 5 bits, and faster than a matrix
   multiply in a shader.
9. **Synthesise audio once at load, not per frame.** His shipped build spends ~a minute of startup on
   sound synthesis plus shader compilation and then has zero synthesis cost at runtime — the same
   trade atrium already makes for impulse responses, applied to source material.
