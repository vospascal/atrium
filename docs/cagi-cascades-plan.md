# CAGI Cascades — a light volume that does not depend on world size

Make the CAGI light volume **camera-relative and cascaded** instead of a single
dense grid sized from the world's compile-time dimensions. Today the volume covers
the whole island; a procedurally generated or streamed world has no "whole", so
there is no number to size it from.

The target configuration is **12.5 cm cells around the player out to ±160 m, on a
world of any size, for ~325 MB and ~3.7 ms/frame** — four times finer than what
ships today, at a reach today's volume cannot express at all.

**Status: PLANNED, not started.** Each stage needs explicit approval and an app-run
gate before the next begins.

## Why now

Pascal, 2026-08-03: *"the issue is we will generate worlds in the future
procedurally so then .. we can't know the number right"*. Correct, and it is not a
tuning problem — the current design cannot be configured out of it.

The framing that makes it tractable came from the same conversation: *"we can define
this maybe .. to texels 8x8 (1) 4x4 (1/4) 2x2 (1/8) per voxel and maybe also scale by
distance :) like LOD"*. The instinct is right and the arithmetic is better than that
— in 3D each halving of cell size is **8×** the cells, not 4×, so the tiers are
1, 1/8, 1/64. Coarsening is far more powerful than it looks.

## Relation to existing work

- **Ledger 6.34/6.36** shipped the pattern texel LOD and its `log2` fix. This arc is
  the same instinct — coarsen with distance — applied to the light volume, but it
  **cannot use the same mechanism**; see "Why a CA cannot do a texel LOD" below.
- **The animation plan's P4 option 2** ("regional CAGI propagation — the correct fix
  and its own arc") is stage 1 here. That plan scoped it out; procedural worlds are
  what force it.
- **The streaming arc** (`docs/streaming-plan.md`) gave the world a `VoxelSource`
  seam for infinite terrain. The GI volume is the subsystem that did not get one.
- **`docs/voxel-automata-terrain.md`** runs the same "use the hierarchy you already
  have" argument on the geometry side: a diamond-square/CA generator whose
  subdivision levels ARE the traversal mip pyramid.
- **`docs/xima-engine-dossier.md`** records another engine hitting the same wall
  ("0.25 m measured at 258 MB and 6× cost … suggests he hit a comparable memory
  wall. Current Voxile may be multi-resolution or cascaded — unknown") and confirms
  he runs a **second, coarser CA** for point lights at macrovoxel level. That is
  multi-resolution by another name.
- **Bench section 5's `gi-cells2` cannot run on this world** (see
  `docs/voxel-rt-bench.md`) — the 25 cm rung needs a 188 MB binding against a
  128 MiB limit. That failure is this arc's problem statement in miniature.

## Current state — three things that assume a fixed island

Verified against the code, not remembered:

1. **Horizontal extent is compile-time.** `CagiGrid::for_world`
   (`crates/voxel-rt/src/cagi.rs:403`) divides `WORLD_SIZE_X/Z` — 1000 × 1000 detail
   voxels, i.e. 125 × 125 m — by `cell_voxels`. There is no camera in the sizing.
2. **Vertical extent is ONE global height.** The same function clamps to
   `max_occupied_brick_y` plus `SKY_MARGIN_CELLS` (`cagi.rs:128`). One number for
   the entire world. On procedural terrain with real height variation this
   degenerates to "the tallest mountain", and every flat region pays for it.
3. **There is no bounded invalidation.** `LightVolume::mark_dirty`
   (`passes/cagi.rs:217`) only sets `needs_reflood`; `CagiPass::encode` (`:509`) then
   `clear_buffer`s **both** volume buffers in full and floods from scratch. The
   source comment states the assumption directly: *"E4's world is static, so this
   global re-flood is the only invalidation there is."*

Point 3 is the load-bearing one. A camera-relative volume scrolls, and scrolling
with a global re-flood is **worse** than a fixed world — you would re-flood the
entire grid every time the player walks half a metre.

## The memory arithmetic today

The volume is 16 bytes per cell: two ping-pong light buffers at 4 bytes each
(`volume_bytes()`, `cagi.rs:453`) plus an 8-byte cell-data buffer holding attributes
and the packed 10:10:10 emission (`CELL_DATA_WORDS = 2`, `cagi.rs:85`).

| `cell_voxels` | cell size | grid | cells | whole scene |
|---|---|---|---|---|
| 8 | 1 m | 125 × 25 × 125 | 390 K | 6.3 MB |
| **4 (shipped)** | **50 cm** | 250 × 48 × 250 | 3.0 M | **48 MB** |
| 2 | 25 cm | 500 × 94 × 500 | 23.5 M | 376 MB |
| 1 | 12.5 cm | 1000 × 188 × 1000 | 188 M | 3.0 GB |

Each halving is ~8×. Dense 12.5 cm over this island is 3 GB and not viable; the same
detail *where it is visible* is 65 MB. **That gap is the whole argument.**

Note the shipped 48 MB is not a large allocation on either target — a 6 GB desktop
card or a Quest 3, whose app limit is 5.75 GiB of its 8 GB. **Memory capacity is not
what separates the tiers; bandwidth and compute are.** The reason 25 cm is not
shippable was never the 376 MB — it is that flooding 23.5 M cells costs an
extrapolated ~10.7 ms/frame, consistent with the dossier's independently recorded
"6× cost" at a smaller world.

## Why a CA cannot do a texel LOD

The pattern texel LOD (6.34) coarsens the sample grid with distance and it works
because **every pattern sample is independent** — a pure function of position.
Coarsening one pixel's grid cannot change its neighbour's answer.

A cellular automaton is a **stencil**: every cell reads its neighbours. Mixing
resolutions inside one grid breaks two things at once.

- **Flux across the seam.** A coarse cell adjacent to eight fine cells needs
  conservation, or light leaks or vanishes along every LOD boundary — and those
  boundaries move with the camera, so the artifact swims.
- **Propagation speed is tied to cell size.** A CA moves light one cell per
  iteration. A coarse region transports light 2× further per iteration than a fine
  one, so the light front arrives at different times in different places and
  convergence is no longer uniform.

That is adaptive mesh refinement. It is solvable and it is research-grade. The
graphics answer sidesteps it: **nested uniform grids with explicit boundary
injection**. Each grid stays uniform internally so the stencil is unchanged, and the
coarse-to-fine coupling happens at one defined interface instead of everywhere.

Cascades *are* the distance LOD. Same instinct, structured so the CA still works.

## The three obstacles, and how cascades answer them

The CAGI author (<https://rares-dumitru.dev/cagi/>) names these:

> *Managing memory efficiently since cellular automata ideally requires dense grids
> in order to compute*

Answered by **fixed cells per cascade**. Memory becomes linear in cascade count —
which we choose — instead of quadratic in world area, which we do not.

> *Managing light directionality and shadow propagation*

Not solved by cascades, but no longer open: **x1m4 answered it by paying 6x**
(2025-12-22) — *"I'm using 10 bits for cagi per direction times 3 because of RGB"*.
One `u32` per direction, RGB at 10 bits each, which is the SAME packing we already
use for a single isotropic value (`cagi_volume.wgsl:28-38`, chosen because
*"both fit one u32"*). No clever encoding; six words per cell instead of one. The
direction count is inferred — six is the natural guess and matches our AADF's six
`BOUND_DIRECTIONS` — but the per-direction word is stated.

**This re-prices the whole plan**, so it belongs in stage 0 rather than as a footnote:

| | isotropic (today) | directional |
|---|---|---|
| light payload | 4 B/cell x2 ping-pong | ~24 B/cell x2 |
| + cell data | 8 B | 8 B |
| **per cell** | **16 B** | **~56 B (3.5x)** |
| per cascade (160³) | 65 MB | 229 MB |
| **5 cascades** | **325 MB** | **~1.14 GB** |

1.14 GB is still fine on a 6 GB card and becomes a genuine tier decision on Quest's
5.75 GiB *shared* budget.

**The frame-time answer is genuinely open and may go the other way**, which is why it
is a stage-0 measurement and not an assumption. The naive reading — 6x the data means
6x the cost in a memory-bound stencil, so 3.7 ms becomes ~22 ms and the plan dies —
probably does not hold. Isotropic diffusion reads all six neighbours' single value
(24 B in, 4 B out); directional propagation computes each outgoing direction largely
from the same direction at one neighbour (~24 B in, ~24 B out). Reads are comparable,
writes grow — call it 2x per iteration, not 6x. And then the thing that could invert
it entirely: **a directional sweep marches where a diffusion stencil decays** (the
dossier records exactly this about xima's engine). If directional reaches the same
distance in 2 iterations where isotropic needs 8, it is CHEAPER for equal quality
despite costing more per iteration.

**Cascades relieve the pressure either way**: long-range transport moves into coarse
cascades where any stencil has fewer cells to cross.

> *Balancing light propagation speed and performance*

**This is the one cascades answer elegantly.** A CA moves light one cell per
iteration, so long-range transport in a fine grid needs many iterations — that is the
entire tension in that sentence. In a cascade scheme the coarse outer cascades move
light 2 m per iteration for the same cost as 12.5 cm inside. Distance becomes cheap
exactly where detail stops mattering.

## Target configuration

Fixed 160³ cells per cascade, camera-centred:

| cascade | cell | reach | memory | update |
|---|---|---|---|---|
| C0 | 12.5 cm | ±10 m | 65 MB | every frame |
| C1 | 25 cm | ±20 m | 65 MB | every 2 |
| C2 | 50 cm | ±40 m | 65 MB | every 4 |
| C3 | 1 m | ±80 m | 65 MB | every 8 |
| C4 | 2 m | ±160 m | 65 MB | every 16 |

**325 MB — about 5% of either target's budget.**

Flood cost, scaled from the measured 1.37 ms for 3.0 M cells at 2 iterations
(4.1 M cells ≈ 1.9 ms per cascade per update):

```
1.9 × (1 + 1/2 + 1/4 + 1/8 + 1/16) ≈ 3.7 ms/frame
```

Against ~1.37 ms today. **~2.7× the cost for 4× the resolution and 3× the reach** —
and the number is the same whether the world is an island or infinite.

Both figures are **extrapolations from one measured point**. Stage 0 exists to
replace them with measurements before anything is built on them.

## Stages

Each stage is independently useful and independently gated. Stage 1 is the
prerequisite for everything after it.

### Stage 0 — make the measurement possible

> **PARTLY DONE, 2026-08-03.** The adapter's real
> `max_storage_buffer_binding_size` is now requested in `gpu::device_descriptor`
> rather than accepting WebGPU's 128 MiB default, so `gi-cells2` runs. **Measured
> CAGI flood cost, which replaces the extrapolation this plan was built on:**
>
> | rung | cells | flood/frame |
> |---|---|---|
> | off | — | 0.07 ms |
> | 8 vox (1 m) | 390 K | 0.32 ms |
> | **4 vox (50 cm), shipped** | 3.0 M | **1.37 ms** |
> | 2 vox (25 cm) | 23.5 M | **9.5-10.0 ms** |
>
> The extrapolation in this doc said ~10.7 ms; measured 9.5-10.0. **Scaling is close
> enough to linear to trust the cascade budget** — 0.40-0.46 ms per million cells
> across a 60x range. Re-derived from the measured curve rather than one point:
> 4.1 M cells is ~1.76 ms per cascade, so the amortized five-cascade figure is
> **~3.4 ms**, slightly better than the 3.7 ms estimated below. The estimates in the
> target-configuration table stand.
>
> Still open in this stage: the directional-vs-isotropic transport question, the
> payload-layout sweep, and the bind-group-count check.

Nothing here changes the renderer.

- Raise the bench's requested `max_storage_buffer_binding_size` so `gi-cells2` can
  run. 128 MiB is WebGPU's **default**, unrelated to available VRAM — a 24 GB card
  hits the same wall until the device asks for more. **Read the adapter's real limit
  rather than assuming one**, especially on the Adreno/Vulkan path.
- Record the actual 25 cm flood cost, replacing the ~10.7 ms extrapolation.
- Measure flood cost against cell count across all three rungs, to confirm the
  linear scaling the 3.7 ms estimate assumes. If it is superlinear the cascade
  budget is wrong and the plan needs re-pricing before stage 2.

- **Price the payload layout: bit depth x direction count.** These interact, so
  sweep them together rather than one at a time:

  | layout | bytes/cell (x2 ping-pong) | vs today |
  |---|---|---|
  | isotropic 10:10:10 (today) | 8 | 1x |
  | 4-dir 10:10:10 (tetrahedral) | 32 | 4x |
  | 6-dir 8:8:8 | 36 (18 packed, alignment-dependent) | 4.5x |
  | 6-dir 10:10:10 | 48 | 6x |

  Two things worth testing that are not just "fewer bits". **A tetrahedral 4-direction
  basis** saves 33% against six axis directions with no precision loss — a known trick
  (Half-Life 2's radiosity normal maps, several DDGI variants) and probably the better
  saving than dropping to 8 bits. And **8:8:8 leaves 8 spare bits** in the word where
  10:10:10 leaves 2, so if the CA ever wants an age, confidence or direction tag, the
  cheaper format may pay for itself elsewhere.

  The prediction, to be falsified rather than assumed: 8 bits loses. The shipped rule
  settles cells at **45/1023** — the useful signal lives in the bottom 5% of the range,
  which is ~11 distinct levels at 8 bits, and the decrement rule is *expressed* in
  1/1023 units (`cagi.wgsl:72`). Banding in a long flood is the failure mode, so the
  gate is a visual one, not a millisecond one.

- **Keep the arithmetic integer.** `RGB9E5` fits the same 32 bits with far more range
  and relative rather than absolute precision, which arguably suits multiplicative
  decay better. It is deliberately NOT on the lever list: unsigned-integer decrement is
  exact, deterministic and reproducible, and float rounding inside a feedback loop is
  not. The volume stores normalised light with the emissive scale applied outside it,
  so it never needs the range. Revisit only if that stops being true.

- **Price directional propagation against isotropic, per unit of TRANSPORT DISTANCE
  rather than per iteration.** This single number decides whether the cascade budget
  is 325 MB or 1.14 GB, and whether directional is a cost or a saving. See the
  obstacle-2 section above for why the naive 6x reading is probably wrong.

**Gate:** a bench section that prints cost-per-million-cells and iterations-to-reach-N-cells,
and does not lie when a dispatch is dropped (ledger 7.22 added the error banner; this
is its first real use).

### Stage 1 — bounded regional invalidation

**The prerequisite.** Replace "dirty ⇒ clear both buffers and re-flood" with
"invalidate a region and re-flood only what it reaches".

- A dirty-region list instead of `needs_reflood: bool`.
- A bounded dispatch over the region rather than the whole grid.
- **The correctness argument is the hard part, and it is about light LEAVING the
  region**: a change inside a box affects cells outside it, out to the propagation
  distance. Either dilate the region by the CA's reach per iteration × iteration
  count, or accept a bounded error and state it.

Useful on its own: it is what makes E2 world edits update GI without a full
re-flood, and what the animation plan's P4 needs for event lights.

**Gate:** an edit in one corner produces the same volume as a full re-flood, within a
stated tolerance, and costs a fraction of one.

### Stage 2 — camera-relative, toroidal, single cascade

Still one volume; make it move.

- Size from a chosen extent, not from `WORLD_SIZE_*`.
- **Toroidal addressing**: the volume does not move, the *indexing* wraps (modulo the
  cell coordinate). Crossing a cell boundary invalidates the one-cell slab that just
  entered the far side. Update cost becomes **O(surface), not O(volume)** — which is
  exactly what stage 1 makes possible.
- Retire the single global `max_occupied_brick_y` clamp. It is a world-shaped
  assumption; a camera-relative volume clamps to its own extent.

**Gate:** walking a long distance costs a bounded per-frame update, and standing
still costs nothing. Frame time must not spike on cell-boundary crossings — if it
does, the slab update needs amortizing over several frames.

### Stage 3 — the cascade stack

- N cascades, fixed cells each, doubling cell size and extent.
- **Boundary injection**, and this is the design question specific to a diffusion CA
  rather than to probes: light must *cross* cascade boundaries. Inject the coarser
  cascade's result as a boundary condition into the finer one, or the inner volume
  goes dark at its edges.
- Sampling picks a cascade by distance and blends across the overlap, or the seam is
  visible as a lighting discontinuity that moves with the player.
- A fallback past the last cascade: sky ambient, not black.

**Gate:** no visible seam at a cascade boundary while walking through one, and a
lit interior at the centre of C0 fed by sun that entered through C3.

### Stage 4 — amortized update rates

- Per-cascade update cadence (1, 2, 4, 8, 16), as a lever.
- Outer cascades cover more world but change more slowly in screen terms, so the
  error should be invisible. **Verify that rather than assuming it** — the failure
  mode is a slow-moving light front in the distance as the player turns.

**Gate:** the measured per-frame cost matches the amortized model, and turning
quickly does not reveal the cadence.

## Levers this arc adds

Per the variant-hygiene rule, everything measured stays selectable:

- `CagiCascadeCount` — the memory/reach knob, and the natural Quest tier.
- `CagiCascadeCells` — cells per cascade.
- `CagiCascadeCadence` — the amortization schedule.
- `CagiRegionalInvalidation` — stage 1 on/off, so the global re-flood stays
  measurable as the baseline it is.
- `CagiChannelBits` — **8:8:8 vs 10:10:10**. **NOT a shader-const flip, and more
  expensive to even try than a lever normally is.** `CHANNEL_MAX` (1023) derives the
  transport physics, not just the storage: `reach_meters = CHANNEL_MAX / attenuation
  * cell_meters`, and `transport_coefficients_are_resolution_independent`
  (`cagi.rs:1532`) asserts the fixed-point numerators still round to something
  usable. Dropping to 255 means rescaling `ATTENUATION_PER_METER`, re-deriving the
  numerators and re-verifying that test — a rework of the transport derivation, not a
  const.

  It is also expected to LOSE on quality: the shipped rule settles cells at
  **45/1023**, so the useful signal lives in the bottom ~5% of the range and 8 bits
  leaves about **11 distinct levels** for the whole propagation. The failure mode is
  banding over distance, so the gate is visual, not a millisecond count. 10:10:10
  ships today in both the ping-pong light word and the emission word, and it costs
  nothing (`cagi_volume.wgsl:25-40`) — there is no upgrade available here, only a
  possible downgrade. Sequence it AFTER the directional decision: it is only worth
  the rework if the volume has already become the largest buffer in the engine.

  Note also there is **one** spare bit, not two: bits 30 and 31 sit above the
  channels and bit 30 is already the sun-source pin.
- `CagiDirectionBasis` — isotropic / 4-direction / 6-direction. The axis that
  actually moves the budget.

**Both are runtime-switchable, and they are not equally cheap.** The engine already
has two distinct rebuild paths — `RenderQuality::requires_pipeline_rebuild` (a shader
const moved) and `CagiSettings::requires_volume_rebuild` (`cagi.rs:367`, the size or
static attributes changed). `cell_voxels` already uses the second, so switching the
volume's shape at runtime is an existing operation rather than a new capability.

| lever | pipeline rebuild | volume rebuild | notes |
|---|---|---|---|
| `CagiChannelBits` | yes | **no** | both depths fit one `u32`, so the allocation is unchanged |
| `CagiDirectionBasis` | yes | **yes** | 4 B/cell -> 24 B/cell changes the allocation |

Either way the switch needs a **re-flood**, because the resident contents are in the
old format — a 10-bit value read as 8-bit is garbage. That is the same user-visible
cost a `cell_voxels` switch already pays, so it is consistent behaviour, not a new
class of hitch. Preset permutations are precompiled at startup, so a Potato<->Balanced
switch stays a hash lookup rather than a compile.

**`CagiChannelBits` is a well-shaped Potato lever specifically**: it cuts the largest
buffer in the engine and pays for it in QUALITY (banding in a long flood), which is
what the Potato tier already trades away — rather than in correctness, which it must
not.

## Known caveats

- **Directionality is priced but not decided.** Obstacle 2 has a known reference
  answer (6x storage, one 10:10:10 word per direction) and an unmeasured frame-time
  question that stage 0 must answer before the cascade budget means anything.
- **The 3.7 ms and 325 MB targets are extrapolated** from a single measured point.
  Stage 0 exists to replace them, and stage 2 should not start on unverified
  scaling.
- **A moving volume is a temporal-stability question.** The current renderer is
  deliberately bit-stable for a still camera (`dda.wgsl:309`); a scrolling volume
  re-lights cells as they enter, so a *moving* camera will see light change behind
  it. Whether that reads as flicker is a gate, not a calculation.
- **Cascades multiply the CAGI bind-group surface.** Five volumes is five sets of
  bindings, against a `max_storage_buffers_per_shader_stage` of 11 that this engine
  already requests. Likely needs one array-of-cascades binding rather than N
  bindings — worth checking in stage 0, since it constrains stage 3's shape.
- **Stage 1 is the only stage that is hard.** Two, three and four are plumbing on top
  of it. If regional invalidation turns out to be intractable, the honest fallback is
  a fixed volume around a streamed world's loaded region, re-flooded on chunk
  boundaries — worse, but bounded.

## Why this order

Regional invalidation → toroidal → cascades → amortized rates. Each step is required
by the next and useful alone, and the first one is already on the board for two other
reasons (E2 edits, P4 event lights). Nothing here is speculative except the numbers,
and stage 0 fixes that.
