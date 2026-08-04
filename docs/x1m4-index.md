# x1m4 / VoxelChain — index to the complete Discord corpus

**Status: the export is complete; the *reading* is not.** All six channels of the Graphics Programming
server (guild `1003288330391273492`) are in `temp/`. What has actually been read is x1m4's own
messages — fully in three channels, keyword-filtered in the other three — and **almost none of the
other 49 854 messages**. See §Coverage below before trusting an absence. Verified counts:

| | |
|---|---|
| Unique messages in the export | **68 386** |
| **x1m4's messages** | **18 532** |
| Date range | **2022-07-31 → 2026-08-03** (4 years, 1 day) |
| Channels | 6 (complete — no gaps) |

| Channel | id | folder | his msgs | doc |
|---|---|---|---|---|
| #showcase | `1006843475494445137` | `showcase` | 4 851 | [showcase-chat](x1m4-showcase-chat-channels.md) |
| #chat | `1003298200909778984` | `chat` | 4 456 | [showcase-chat](x1m4-showcase-chat-channels.md) |
| #voxelchain (archived) | `1003293185554001920` | `archived` | 4 341 | [archived](x1m4-archived-voxelchain-channel.md) |
| #general-programming | `1007957399312805898` | `general-programming` | 2 744 | [general-programming](x1m4-general-programming-channel.md) |
| #graphics-programming | `1089129949228695633` | `folder 1` | 1 854 | [graphics-programming](x1m4-graphics-programming-channel.md) |
| #devlog | `1003345384019595375` | `show` | 286 | [devlog](x1m4-devlog-channel.md) |

A seventh doc, [x1m4-architecture-notes.md](x1m4-architecture-notes.md), predates these and was
built from a small 6-channel sample (210 msgs). Superseded but kept for its synthesis.

---

> **Building it?** Start with [dda-cagi-build-guide.md](dda-cagi-build-guide.md) — DDA + CAGI assembled
> into implementation order, with every measured number, sourced from all eight implementers in the
> server rather than x1m4 alone.

> **Annotated:** [cagi-reference-implementation.md](cagi-reference-implementation.md) — line-by-line
> read of the propagation kernel below, its five tuning constants, the corner-seal test, and what
> porting it to 3D costs.

## The one artifact to read first

**`temp/general-programming/0104-update.rs`** — posted by **TooManyLimits, 2024-06-04 21:33**, with
the caption *"Here's the update function (rust) if anyone wants to see our algorithm."*

This is **the closest thing to a CAGI reference implementation anywhere in the corpus**, and it is not
x1m4's — it's an independent reimplementation by another server member, prompted by x1m4's posts. It
was a bare `.rs` attachment, not a message, which is why it's easy to miss.

Their cell type (posted the same day):
```rust
pub enum RgbDirectionalCell3 {
    Air([[u8; 4]; 3]),    // 3 colour channels × 4 directions, u8 each
    Block(Vec3),          // block colour
    Light([[u8; 4]; 3])
}
```

And the update rule, structurally:

1. **Decay** every direction of every channel: `saturating_sub(DECAY)`.
2. **Direct neighbours** — for direction `dir`, read the neighbour at `(dir+2)%4` (i.e. the one
   *behind* the flow) and take `max(current, neighbour[c][dir] - DIRECT_ATTENUATION)`. Then also pull
   its two *sideways* components with a separate, larger `SIDE_LIGHT_ATTENUATION`. This is x1m4's
   "add slight diffusion for each axis" as actual code.
3. **Reflection off solids** — if that neighbour is a `Block(color)`:
   `output[c][dir] = max(output, (input[c][(dir+2)%4] * color[c]) - 1)`. The *incoming opposite*
   direction is multiplied by the block's colour and re-emitted. This is x1m4's
   `lightpy * neighbor_solid_color * bounce_loss`.
4. **Diagonal neighbours** contribute with three separate attenuations depending on the two connecting
   direct neighbours: full `DIAGONAL_ATTENUATION` when both are air, a larger
   `DIAGONAL_PARTIAL_OCCLUDED_ATTENUATION` when one is a block, and **nothing when both are blocks** —
   which is how you stop light leaking through diagonal seams.

○ Cross-check against x1m4's own quoted rule (2025-12-24 — see [graphics-programming](x1m4-graphics-programming-channel.md) §3):
`max(src, neighbor) * propagation_loss`, per-axis `mix()` diffusion, `light * neighbor_solid_color *
bounce_loss`. **The two match on every point**, with `saturating_sub` standing in for the
multiplicative loss (integer arithmetic, as x1m4 insists on). Four separate attenuation constants —
direct, sideways, diagonal, diagonal-partially-occluded — is the tuning surface neither of them
spells out in prose.

---

## Reading order by purpose

**If you want CAGI:** the `.rs` file above → [archived](x1m4-archived-voxelchain-channel.md) §1–2
(origin, speed, diagonal injection) → [graphics-programming](x1m4-graphics-programming-channel.md) §3
(the quoted rule, perf numbers, leak fix) → [devlog](x1m4-devlog-channel.md) §2 (the cascade scheme it
grew out of).

**If you want the audio architecture:** [showcase-chat](x1m4-showcase-chat-channels.md) §4 (the
architecture with numbers) → [general-programming](x1m4-general-programming-channel.md) §2 (the
failure modes, which are the interesting part) → [archived](x1m4-archived-voxelchain-channel.md) §8
(the decision chain, and the occlusion approach he knew was wrong).

**If you want the engine as a whole:** [showcase-chat](x1m4-showcase-chat-channels.md) §1–2 (identity,
voxel budget) → [general-programming](x1m4-general-programming-channel.md) §1 (five corrections) →
[devlog](x1m4-devlog-channel.md) §8 (dated milestones).

---

## CAGI, assembled from all six channels

The one place the complete picture sits together.

**What it is.** ▸ *"a directional extension of minecraft's flood fill and a more restricted (and
faster + stable) variant of light propagation volumes"*. Emission, sunlight, skylight **and shadow**
all propagate through it. Ray tracing is left only for AO and reflections.

**Where it came from.** Not invented whole. In Nov 2022 he built a flood-fill field to tell his *path
tracer* where to spend more samples; ran it alongside the ray tracer for a year; then deleted the ray
tracer. His own account: ▸ *"it started with some brainfart."*

**The rule.** Per axis: `max(src, neighbour) * propagation_loss`, then per-axis diffusion
`mix(lightpy, lightny, x)`, then bounce `light * neighbour_solid_color * bounce_loss`. Loss is
**subtractive** (`max(LOSS, LIGHT) - LOSS`), not multiplicative. Injection must be **diagonal** even
though propagation is face-wise.

**Storage.** 10 bits per direction × 6 directions × RGB. Anisotropic — one texture per direction.
Integer/fixed-point only, so it's deterministic and usable for game logic. Runs at **1/8 of voxel
resolution** (main-voxel scale, not sub-voxel).

**Cost.** 256³ = **2 ms** full, **0.4 ms** with dirty buffers + checkerboard. 512³ handled.
512×256×512 brute force = 6–10 ms. Cascaded expectation 128³/cascade ≈ 0.2 ms. Two passes:
propagation + diffusion. Double-buffered → one texture write per cell; 10–20 reads per cell in 2D
with bouncing on. **Propagation is the bottleneck, not injection.**

**Speed of light.** ~1 cell per tick at 60 fps ⇒ ~60 voxels/second. That is the temporal-lag budget.

**Leaking.** Solved by a precomputed **per-block-face surface gather** (cast rays down into each face
plane; count solid/emissive; store colours) driving anisotropic injection. Sub-voxel volumes up to
**16³** before inner leaking becomes visible.

**Known weaknesses, his own list.** Bright sources ▸ *"eat up less bright ones and screw up their
directions"*. Two equal lights either side of a wall wash each other's shadows out (inherent to a
shared volume). 10-bit colour is the memory ceiling, and unequal channels diverge visibly as they
darken — patched with an unphysical cross-channel "colour diffusion". Quality goal is explicitly
▸ *"just convincing enough."*

**Verification.** ~860 lines total in the shipped engine: `simulation-light.wgsl` (486) +
`simulation-light-diffusion.wgsl` (271) + three dirty passes. Drives mob spawning (occupied space,
light level, block material, per-type count). Sold as contract work into the **Tesera** engine.

---

## The remaining gap — and it isn't in this server

Every question left points at **one place**, which is a *different* Discord server:

> **The voxelgamedev server, keyword `cagi`.** Guild `661650973382672384`, not exported.

x1m4 says three separate times across three channels that the detailed explanations live there and
not here:
- ▸ *"I have explained it multiple times on the voxelgamedev server, if you dig through the message
  history there then you should find it"* (2026-08-03, to you)
- ▸ *"not a secret just don't have enough energy atm to write an article, but checkout the vgd
  discord, a few people there experimented with the cagi idea"* (2024-09-24)
- ▸ *"there are a few people here and on the voxelgamedev server that wrote their own
  implementations"* (2025-10-30)

**Who to look for there** — reimplementers: `sweg`, `👾Rareș👾`, `Dapper Core`, `bob08022010`,
`TooManyLimits`. Taught directly by him: `bonisdev`, `KosmosisDire`, `𝕶𝖊𝖑𝖛𝖎𝖓`. `👾Rareș👾`'s 3D port
is the one x1m4 called ▸ *"the best implementation of CAGI that I've seen so far"* (2025-02-06).

**Two smaller gaps, both outside Discord:**
1. **His Patreon posts + the site's early-access area** — the actual release notes, which no channel
   replicates.
2. **The collaborator `316239158584803328`** — co-built the mass/fluid sim and the 2D pixel engine, and
   learned Vulkan alongside him in school. Visible only second-hand in all six channels.

---

## Coverage — what has and hasn't been read

Stated plainly so an absence in these docs isn't mistaken for evidence.

**x1m4's own messages (18 532):**

| Channel | His msgs | Read how |
|---|---|---|
| #graphics-programming | 1 854 | **fully** |
| #general-programming | 2 744 | **fully** |
| #devlog | 286 | **fully** |
| #showcase | 4 851 | keyword-filtered (~72 % of lines kept) |
| #archived | 4 341 | technical-filter (~46 %) |
| #chat | 4 456 | technical-filter (~20 %) — weakest; its personal/political bulk was deliberately excluded |

**Everyone else (49 854): essentially unread as a systematic pass.** Two exceptions, both targeted:
Nameless was profiled in [showcase-chat](x1m4-showcase-chat-channels.md) §12, and a
DDA/CAGI/reflection/optimization sweep across *all* authors fed
[dda-cagi-build-guide.md](dda-cagi-build-guide.md) — that's where Sam, Ivo, Mytino, Dapper Core,
IchBinAlex, dotted, sus and Kelvin come from. Everything else those people said is unexamined.

Message counts for the people most likely to be worth reading further:

| Msgs | Who | Why |
|---|---|---|
| 11 748 | dotted | x1m4's main interlocutor for four years |
| 5 538 | ReversedCausality | built his own CA lighting and fluid sim; argued CAGI constantly |
| 5 382 | eternal | acceleration-structure taxonomy, SDF tracing |
| 2 640 | sus | HWRT voxel engine, brick layouts |
| 1 324 | Mytino | radiance cascades + GPU fluids, with numbers |
| 482 | Dapper Core | grid-based visibility, occupancy-bitmask tracing |
| 231 | sweg | CAGI with extra directions |
| 102 | 👾Rareș👾 | the CAGI reimplementation x1m4 rated best |
| 45 | TooManyLimits | wrote the kernel |

**Also unread: 781 of his messages carry attachments** — filenames only, no media in the export.

---

## Corrections applied across the set

For the record, since these docs were written incrementally and three claims changed:

| Claim | Corrected to | Where |
|---|---|---|
| "Gave up on procedural sound synthesis, uses samples" | True for 2023–24 only. **The 2025 shipped build synthesises sounds at startup**, plus granular pitch shifting (2024-06-11) | [devlog](x1m4-devlog-channel.md) §1 |
| "Shadow maps + CAGI on top" | **Shadow mapping was deleted 2024-07-25.** Shadows are CA-propagated; no shadow maps anywhere | [general-programming](x1m4-general-programming-channel.md) §1 |
| "Everything integer and deterministic" | **World sim only.** Entities are float, server-authoritative, interpolated; only entity→world *actions* are deterministic | [general-programming](x1m4-general-programming-channel.md) §6 |
| "ROBDDs were the circuit representation, replaced by truth tables" | Truth table was always the ≤16-pin constant-time path; **ROBDD was the 17–26-pin fallback**, dropped 2022-11-20 | [archived](x1m4-archived-voxelchain-channel.md) §6 |
| "Entities are Blockbench-authored" | True until **2026-07-30**, when they became fully procedural | [devlog](x1m4-devlog-channel.md) §1 |
| Running message totals | Understated by 1 854 (omitted #graphics-programming). Verified total **18 532** | this file |

---

## His published code — what actually still exists

He shared **18 distinct links to his own code** across the six channels. Checked 2026-08-03; most of
the gists are gone.

**Alive and worth having:**

| Link | What | Value |
|---|---|---|
| [gist 2807ad81…](https://gist.github.com/maierfelix/2807ad81904748e87d3aa806b094d782) | **The 24-rotation implementation, GLSL.** Two const tables — `ROT_24_SWIZZLE_TABLE` (`uvec3[24]`) and `ROT_24_FLIP_TABLE` (`bvec3[24]`) — then flip (`1.0 - pos.c`) followed by swizzle (`pos[rs.x], pos[rs.y], pos[rs.z]`). This is the *"vector swizzling and flipping instead of rotation matrices"* he kept referring to, in full. | **high** |
| [gist d25d674b…](https://gist.github.com/maierfelix/d25d674b8129a4cb39f734a9b25b2c39) | **TRIPLE PRNG**, C + JS side by side, bit-identical (JS via `BigInt.asUintN(32, …)`). Two init hashes + an LCG `next`. His cross-language determinism harness. | medium |
| [gist ad8b4030…](https://gist.github.com/maierfelix/ad8b40306e08ea705139cc49bc75e6d7) | **TEA PRNG**, C + JS, same purpose, earlier. `TEAInit(val0, val1, count)` with the `0x9e3779b9` golden-ratio schedule. | medium |
| [voxelchain-terrain-generator/src/index.ts](https://github.com/VoxelChain/voxelchain-terrain-generator/blob/main/src/index.ts) | **The CA terrain generator.** See the correction in [showcase-chat](x1m4-showcase-chat-channels.md) §8 — it is *not* diamond-square. | **high** |
| [voxelchain-formats/src/vxmo.ts](https://github.com/VoxelChain/voxelchain-formats/blob/main/src/vxmo.ts) | **The module file format** — the on-disk form of the circuit compiler: `input` / `inputRemap` / `output` / `outputRemap` (Uint32/Uint8Array) plus `tt`, the truth table. Confirms the pin-remapping he described in [archived](x1m4-archived-voxelchain-channel.md) §6. | medium |
| `voxelchain-programming`, `maierfelix/{tiny-rtx, dawn-ray-tracing, chromium-ray-tracing, WebGPU-Path-Tracer, webgpu-examples}` | Public API boilerplate and his pre-WebGPU hardware-RT forks. | low |

**Dead (404 as of 2026-08-03):**

| Link | What was lost |
|---|---|
| gist `82823976…` | **The infinite CA terrain generator** — his own improvement on the repo version: *"generates more interesting shapes, supports dynamic width height and depth and also runs faster than the one by bwerness"*. The repo version survives; this refinement doesn't. |
| gist `5799988d…` | The 2022 rotation gist (superseded by `2807ad81…`, which is alive) |
| gist `cf46b556…` | Conveyor-belt smooth-movement interpolation — real engine code, including the curved-belt cases |

**Not code:** gist `e43383ef…` (2024-07-04) is a political essay on the French legislative election,
not engine material.

### Recovering the lost terrain generator

Gist `82823976…` is the loss that matters, and there is **no Wayback snapshot** (checked 2026-08-03).
But it is reconstructable, because he documented the whole public API in the messages around it:

> ▸ *"aside being infinite, it also generates more interesting shapes, supports dynamic width height
> and depth and also runs faster than the one by bwerness"* (2023-09-11)
> ▸ usage, posted 2023-09-12:
> ```js
> const seed = Math.floor(Math.random() * 0xFFFFFFFF);
> const lambda = 0.35;
> const iterations = 7;
> const terrain = new TerrainAutomata();
> terrain.initialize(seed, lambda);
> const states = terrain.generate(x, y, z, width, height, depth, iterations);
> ```
> ▸ *"states is an array containing the terrain voxel data and a state above 0 is considered a solid
> voxel"* · ▸ *"the generator currently supports 4 different states"*
> ▸ the seam trick, from 2023-08-13: *"the idea was to **skip over chunk edges** (like slightly zooming
> out and then run the algo)"*

○ Note the class is `TerrainAutomata`, **not** the repo's `TerrainGenerator` — a genuine rewrite, not
a tidy-up. What the API tells you about the delta:

| Repo `TerrainGenerator` (alive) | Lost `TerrainAutomata` |
|---|---|
| power-of-two cubic resolution | **dynamic `width, height, depth`** |
| iterations implicit = log₂(resolution) | **`iterations` explicit** (7 in his example) |
| generates the whole grid | **`generate(x, y, z, …)` — world-space offset**, which is *how* it's infinite: request any region and it's consistent with its neighbours |
| — | faster, and *"more interesting shapes"* |

○ So the reconstruction is: take the repo's totalistic rule tables and subdivision loop (which survive),
add offset addressing so the rule evaluation is a pure function of absolute world coordinates rather
than grid indices, decouple `iterations` from resolution, and handle seams by evaluating one level
coarser across chunk boundaries. `lambda = 0.35`, 4 states, external RNG. That's a day's work, not a
reverse-engineering project — the algorithm was never the secret, the addressing was.

○ Net: the rotation tables and the terrain generator are the two real artifacts. The PRNGs are useful
only if you need bit-identical CPU/GPU randomness. Everything else is either superseded or gone.

---

## Non-message files in the export

`temp/` also contains 55 non-JSON files. Almost all are `science.bin` / `v2.html` / `basic.json`
scrape metadata. The five with real content, none of them x1m4's:

| File | What |
|---|---|
| `general-programming/0104-update.rs` | **TooManyLimits' CA light propagation** — see above. The most valuable file in the corpus. |
| `folder 1/0210-message.txt` | **Ken Silverman's PND3D rendering tricks** — his own write-up: 8-way octree, 64-bit non-leaf nodes (child mask + solid mask + child pointer), front-to-back depth-first traversal for occlusion skipping, screen bounding rects via a single `divps`. |
| `folder 1/0368-dreams_renderer_2019.txt` | Notes on the *Dreams* renderer. |
| `folder 1/0197-message.txt` | A C#/Silk.NET Vulkan RTX pipeline (`ARtxPipeline`, SBT regions) — someone else's code. |
| `general-programming/0048-message.txt` | A summary of an "Organic Neural Network" concept — locally-connected neurons, continuous evolution, emergent behaviour. |
| `showcase/0131-noise.txt` | Alex's Worley noise function (octaves, HLSL-ish), 2025-07-01. |
