# voxel-rt — External Research Dossier

*Papers, articles and released engines brought in by Pascal, triaged against the
measured state of voxel-rt. Companion to `xima-engine-dossier.md` (one engine,
reverse-engineered) — this doc collects **published sources with citable
methods**. Every entry ends in a verdict and, where it changes anything, the
ledger row (`voxel-rt-optimization-ledger.md`) and experiment slot
(`voxel-rt-plan.md`) it lands in.*

*Confidence is stated per claim. Paywalled or unread numbers are marked
unverified rather than paraphrased as fact.*

---

## Batch 1 — 2026-07-31 (10 sources)

Headline: **R1 was built and measured the same day it arrived, and inverted its
own paper's emphasis** — the technique NAADF is named after lost in all four
scenarios, while an unadvertised data-layout detail cut the brickmap in half
*and* made it faster. R2 (Pascal's own finding) is the one candidate still
standing, affordable at our measured ray prices and pointed at a grain problem
E1d shipped with. Two sources close as documented negatives whose reasons changed
under them within a day, and the rest contribute one transferable rule each.

| # | Source | Verdict |
|---|---|---|
| R1 | **NAADF** (CGF 2026, MIT engine) | ✅ **BUILT AND MEASURED same day.** The headline technique **lost** (1.13, lever); an unadvertised structural detail **won big** (1.12 uniform-brick tag: 6.4–10.1% and 45.2 → 21.9 MB). Leaves the cheapest open shot at reviving ledger 2.11 |
| R2 | **Pascal's coarse-probe finding** (own experiments) | 🔜 **Real candidate — proposed slot E4b.** In budget at E1's own per-ray prices; targets E1d's shipped grain and E10c's lobe noise |
| R3 | ReSTIR course notes (2023) | ❌ Dead **for now**, and no longer for the old reason — E10 builds most of its prerequisites. Re-read at E10c/E10d (ledger 2.16a) |
| R4 | AMD neural supersampling + denoising | ❌ Dead — E10a would supply half the inputs, but the deps and the Quest tier don't survive (ledger 2.16b) |
| R5 | Real-time collision between general SDFs (GMP 2024) | 🔜 Narrowed to **B5**. **Not** a B3 candidate any more: E2b shipped a swept box at 0.62–0.96 µs (ledger 4.12) |
| R6 | Fast Octree Neighborhood Search for SPH (SA 2022) | Marginal — one transferable lesson for **B4** (ledger 4.13) |
| R7 | Mipmapping alpha-tested textures | Not applicable directly; one design rule for ledger **1.9** |
| R8 | Input in a fixed timestep | Useful at **E9/B6**, concrete and cheap |
| R9 | Fixed timestep without interpolation | ❌ Its method is priced dead by E2 — but it names a bug we own in two places (ledger 7.11) |
| R10 | LinkedIn post (Ulschmid) | Not retrievable (auth wall). Assumed the R1 demo; open question for Pascal |

---

### R1 — NAADF: Globally Illuminated Voxel Worlds Accelerated with Nested Axis-Aligned Distance Fields

Annalena Ulschmid, Marvin Ott, Jonas Macho, Michael Wimmer, Stefan Ohrhallinger
· Computer Graphics Forum, 2026 · [paper (paywalled)](https://onlinelibrary.wiley.com/doi/10.1111/cgf.70413)
· [engine, MIT](https://github.com/cg-tuwien/NAADF) · TU Wien.

**What it is.** A complete, source-available voxel engine: real-time GI, dynamic
entities, live editing, worlds up to 16384³, and a teaser rendering an 8256²
Minecraft world **with no LOD at all**. C# / MonoGame (cpt-max compute-shader
fork), HLSL `.fx` compute shaders, custom `.cvox` format (chunk/block/voxel
buffers zipped).

**The core idea, and why it matters to us.** Their acceleration structure is a
3-layer nest — 4³ voxels → 4³ blocks → chunks — where empty space carries an
**axis-aligned distance field (AADF)** instead of a scalar signed distance. From
the README, verbatim: *"In contrast to Signed Distance Fields (SDF), AADFs are
directional along each axis (x-, x+, y-, y+, z-, z+) resulting in significantly
improved performance."* Per the search-surfaced abstract (**unverified — the
paper is paywalled**), the fields are *in-cell*: each distance is clamped to the
cell's own bounding box, *"so only a few bits are required"*, and the structure
acts as a **cache**.

This sits directly on top of **ledger 1.2 — Pascal's chebyshev distance-field
skip, our S2 headline win (17–27% under baseline)**. The difference is isotropy:

- **Ours:** one scalar clearance byte per brick, bounded by the nearest obstacle
  in *any* direction. A ray running parallel to a wall reads clearance 1 even
  though it could travel 100 voxels forward unobstructed.
- **Theirs:** six directional distances per cell, so the ray gets the distance in
  the direction it is *actually travelling*. Clamping to the parent cell's box is
  what keeps it to a few bits — the value structurally cannot exceed the fanout.

**Claimed result (unverified):** *"accelerates ray tracing 3-5x compared to
state-of-the-art dense spatial structures, such as variants of directed acyclic
graphs (DAG)"*, plus much faster editing than SVO/DAG because there are only 3
layers, no restructuring, and separate buffers.

---

#### MEASURED, 2026-07-31 — and the result inverts the paper's own emphasis

This entry was written as a proposal and overtaken within hours: both halves were
implemented and benched (`ENABLE_DIRECTIONAL_SKIP`, `bench_dda --no-collapse`,
`src/traversal.rs`, `shaders/world.wgsl`). Numbers in the bench doc's lever
section; rows are ledger **1.12**, **1.13**, **1.14**.

**The headline technique lost. In all four scenarios.**

| Scenario | Chebyshev (shipped) | AADF directional bounds |
|---|---|---|
| A | **4.748 ms** | 5.011 |
| B | **6.550 ms** | 6.791 |
| C | **4.408 ms** | 4.565 |
| D | **4.973 ms** | 5.055 |

And yet **the field itself is genuinely better, exactly as predicted**: mean reach
rises 9.10 → 10.82 cells, and **27,578 empty cells that chebyshev grants reach 0
get a mean 5.19 cells**. The reach win was real; it just did not pay. Three
reasons, and the first is the transferable one:

1. **Our chebyshev byte is not a distance — it is a distance AND the occupancy
   test, answered in one load.** A directional bound cannot double up, so it is a
   *second* load. The scalar wins on load count even where it loses on reach.
2. 2 MB stops being cache-resident where 500 KB was.
3. Six 5-bit fields cost shifts where a byte costs a compare.

(My pre-measurement estimate was ~1.5 MB at 4 bits per direction; the built form
is 2 MB at 5 bits. The estimate was close and the *conclusion* still turned on
something the sizing exercise never touched — the load count.)

**Meanwhile the thing the paper is not named after won, and won on two axes at
once: the uniform-brick tag (ledger 1.12).** A brick that is one material in all
512 cells is hit at its entry face with no descent and no level-1 fetch.
**40,531 of 69,977 occupied bricks qualify — 57.9% — taking the brickmap from
45.2 MB to 21.9 MB, and scenario A from 5.069 to 4.744 ms (6.4%), scenario C from
4.899 to 4.402 (10.1%).** Note what that is: **a >50% memory cut *and* a speedup**,
which is the rarest shape of win in this engine, and it came from reading the
paper's data layout rather than its algorithm. It is deliberately **not** a lever —
tag and fast path are one data format, so "off" means building different data.

**Lesson, now ledger 7.13:** a paper's transferable half is often not its
headline. Also worth keeping: **a loss can be a loss for one consumer only** — see
below.

---

**The consequence that survives the negative: this is now the cheapest open shot
at reviving ledger 2.11 / T1 (soft shadows), which we closed as DEAD and confirmed
broken in-app.**
Our own recorded reason for the 1 m lattice and sun-aligned streaks was *"every
ray that could form a penumbra grazes distance-1 bricks whose clearance is
bounded by half a brick."* That is a description of **scalar isotropy**, not of
resolution — and it is exactly the failure a directional field removes. **1.13
measured that population directly: 27,578 zero-reach cells gaining mean 5.19.
Zero-reach grazing cells are the ones that produced the lattice.**

**The traversal loss does not transfer to this consumer, and that is the whole
point.** 1.13 lost because the chebyshev byte answers *distance and occupancy in
one load* — but a penumbra term needs the reach and does **not** need the
occupancy answer, so it pays a different bill. The revival order therefore
becomes: **(1) the AADF field behind `ENABLE_DIRECTIONAL_SKIP` — data already
built, so this is a lever flip plus a penumbra term**, (2) trilinear
interpolation of the per-brick clearance, (3) voxel-level clearance at ≈37 MB
(ledger 3.4). It is the cheapest open shot at a documented in-app negative
anywhere in the ledger.

**And the ledger's own governing principle predicts this.** The new intro section
— direct light needs accurate visibility, indirect light masks visibility error
(Yu et al., ACM TAP 2009) — says coarseness is expensive to spend on *direct*
terms. 2.11's lattice landed in the direct term, which is why nothing masked it.
A directional field is precisely an **accuracy** improvement to visibility, so
the principle says this is the right kind of fix in the right place, rather than
another attempt to hide a coarse estimate.

**What we did not build, and why: their per-VOXEL in-cell form (ledger 1.14).**
Their fanout is 4³ where ours is 8³, and the innermost formulation is per-*voxel*
bounded by the parent. At 8³ that is 512 voxels × ~18–24 bits ≈ **1.2–1.5 KB per
brick against our 64 B occupancy mask — roughly 100 MB**. Dead by arithmetic,
before any measurement. 1.13 tested the brick-level form instead, which is the
only rung where the memory works.

**Still open if 1.13 is ever revived for traversal on Quest: the edit path.** E2's
asymmetry argument (4.7a) becomes per-direction. Adding solid still only shrinks
clearance, but in six independent fields; the *remove* case — a bounded radius-8
local recompute at 258 µs — becomes six of them. **Budget ~6× E2's numbers or find
a per-direction bound.** The lever as built is a static-world structure; nobody has
priced it under live editing, and E9 must not assume the desktop lever is
edit-ready.

**On the 3–5× claim.** They beat DAGs and SVOs; we ship a brickmap with a scalar
chebyshev skip that already bought 17–27%. The pre-measurement note in this entry
read *"NOT a prediction for us … a real chance of being a small win or a wash"* —
the measurement came in slightly worse than a wash. Recorded because the caveat
was the load-bearing part of the entry, not the claim.

**Also worth recording: their architecture independently confirms E2's verdict.**
README, verbatim: *"World generation happens on the GPU. Editing and entity logic
is done on the CPU and then synchronized with the GPU."* That is E2 variant B
(CPU-authoritative + GPU deltas) plus E3's premise, from a paper that shipped it.

**Files to read first** (their own "points of interest", so the map is given):
`rayTracing.fxh` (traversal using the NAADFs) · `chunkCalc.fx` (AADF generation
for voxels and blocks) · `boundsCalc.fx` (AADF generation for chunks) ·
`renderGlobalIllum.fx`, `renderSampleRefine.fx`, `renderSpatialResampling.fx`,
`renderTaaSampleReverse.fx` (the GI / resampling / TAA chain — see R3).

**Still unread from source:** the exact bit layout. `chunkCalc.fx` calls
`ComputeBounds4(..., 15, 0x1, curVoxel)` for voxels and
`ComputeBounds4(..., 30, 0x3, curBlock)` for blocks, so the packing lives in
`boundsCommon.fxh` / `settings.fxh`, which the summarising fetch would not return
verbatim. Our 1.13 uses six 5-bit fields, chosen independently. Worth a local
read only if 1.13 is revived — a tighter packing attacks reason (2) of its loss
(cache residency) directly, and is the one axis of that verdict that could move.

→ **Ledger 1.2 (weakness + the load-count finding), 1.12 (USED), 1.13 (LEVER),
1.14 (DEAD), 2.11 revival order, 7.13. T1 in the technique bank still needs the
same update.**

---

### R2 — Pascal's coarse-probe finding (own GI experiments)

Pascal, verbatim: *"during my rt-based global illumination experiments I found
that as soon as you add indirect bounces then there is only so much you can do to
make an convincing image without increasing ray sample count — i.e. you can't
really cheat at some point and need more samples — or ramp up your temporal
accumulation. What I found that works relatively well though is to use a much
coarser probe resolution for indirect rays only, so you can increase the sample
count without nuking your performance since the complexity is heavily reduced
allowing you to reach equilibrium faster since it's essentially less entropy
you're dealing with."*

**E4 made half of this bet already, from the other direction** — it chose a coarse
volume because per-pixel gathering was priced out at 2.25–3.55 ms *per marginal
ray* (ledger 2.18). The finding adds the part E4 never tested: at coarse
resolution you can afford **many** rays, and the low entropy is what makes the
result settle. It is also the exact operational form of the ledger's governing
principle — spend coarseness on the indirect term, where it is masked.

**Priced with our own numbers.** E1 measured `2.25–3.55 ms per marginal full-res
short ray` at 2560×1440 (3.7 M pixels). Scaling by sample count:

| Sampling resolution | Cost per ray | Rays affordable in ~1.4 ms |
|---|---|---|
| Full res (3.7 M px) | 2.25–3.55 ms | 0 |
| ½ linear | ~0.6–0.9 ms | 2 |
| ¼ linear | ~0.15–0.22 ms | 6–9 |
| **E4's 181,928 propagating cells** | **~0.11–0.18 ms** | **8–12** |

The coarse structure **already exists and is already the right shape**: the E4
CAGI volume is a world-space probe grid with ping-pong double buffering, vertical
clamping (3.2), incremental cell attributes (4.7d), the emitter-index attribute
(E5) and trilinear sampling with solid-tap rejection (2.13e) all built. The
hybrid keeps the storage and the sampling and **swaps only the update rule** —
fill cells from a few traced rays instead of, or alongside, the 6-neighbour
integer diffusion. ~0.9–1.4 ms lands inside the slot CAGI already occupies
(1.4–2.0 ms all-in at Balanced).

**It has two concrete targets already on the board, which is why it earns a slot
rather than a note:**

1. **E1d shipped with a grain problem this fixes.** Directional miss radiance is
   ✅ USED on Beautiful, and its recorded catch is: *"ambient becomes Monte Carlo,
   so E1's 2-ray crosshatch now lands in ambient colour — grain in dark
   foreground. 4 rays would cost ≈ +6.8 ms."* That is Pascal's problem statement
   verbatim — needing more samples, at full res, unaffordably. Moving the extra
   samples to probe resolution is the affordable version of "4 rays".
2. **E10c will have the same problem in a second term.** One spp of a 20% GGX
   lobe is noise, and E10c's answer is a temporal history with rejection and a
   neighborhood clamp. Probe-rate sampling is the *other* axis, and the two
   compose.

And it attacks all three weaknesses E4 recorded as **structural**, not tunable:
long-distance transport deliberately weak (12.8 m reach, which is why 25% of the
E1c hemisphere ambient stays in as a readability floor); anisotropy structurally
real (invisible at 1–3 cell transport distances, expected to show now that E5 has
placed point lights); and directional detail intentionally absent — *"rays own
it"*, and this is rays.

**The tension that must be decided, not glossed.** E4's noiselessness is an
*integer identity*: `cagi::propagate_reference` predicts every propagating cell
with 0 mismatches over 181,928 cells, and re-floods are bit-identical. Monte
Carlo directions break that property. The version worth measuring first is a
**fixed deterministic direction set per cell** — e.g. 8 cone directions, no
jitter — which keeps determinism and buys directional storage, trading *noise*
for *banding*. Note the non-goal amendment of 2026-07-31 makes the jittered form
merely a decision now rather than a rule violation, but it would extend temporal
accumulation from E10's reflection buffer into the light volume, which is a
larger change than E10 signed up for.

**Relation to B13 F1/F2 (the parked voxel-face cache).** Both are amortization
substrates; they differ in domain. The face cache amortizes over **surfaces**
(N², face-space, exact integer edge-stopping, needs the history E10a builds); the
probe volume amortizes over a **coarse volume** (N³ nominal, but only ~1/15 of
cells are active, and it already exists). The plan's own note that *"surfaces
scale N², volumes N³"* is the argument for the face cache at *sub-voxel* detail;
R2's counter is that at 0.5 m the volume is already 20× cheaper than the pixels,
and it needs no new frame-graph work. **They are complements, not rivals** — and
E4b is the one that can be built today, because B13 waits on E10a.

**Corroboration from R1.** NAADF's GI needs `renderSampleRefine` +
`renderSpatialResampling` + a custom TAA pass — ReSTIR-family resampling **and**
temporal reuse. A shipped, peer-reviewed engine reaching for exactly the two
tools Pascal names is independent support for "there is no cheat past a point".

→ **New ledger 2.19. Proposed slot E4b.**

---

### R3 — ReSTIR: Introduction to Spatiotemporal Reservoir Resampling

[2023 course notes](https://intro-to-restir.cwyman.org/presentations/2023ReSTIR_Course_Notes.pdf)
(Wyman et al.). **Not summarised from source** — the fetch returned PDF binary
and the summariser correctly declined to invent content. A local copy was
retrieved during this batch; read it directly if this reopens.

**Verdict: dead for now — but the reason changed under it yesterday, and that is
the point worth recording.** Twenty-four hours ago ReSTIR was dead because we had
no history, no G-buffer, no reprojection and a standing non-goal. After E10:

- the non-goal was **amended** (2026-07-31) — temporal accumulation is allowed,
  confined to the reflection buffer;
- **E10a** builds the G-buffer and matrix-free reprojection;
- **E10c** builds a history with rejection tests and a neighborhood clamp.

E10c's history is, in ReSTIR terms, an ad-hoc single-sample temporal reuse scheme
with heuristic rejection. ReSTIR is the principled generalization of exactly that
— reservoirs with proper MIS weights, so reuse stays unbiased instead of relying
on clamps to hide error. What still blocks it: **Monte Carlo path tracing remains
a non-goal**, ReSTIR's headline win is many-light sampling (which CAGI owns for
free and integer-exactly), and spatial reservoir reuse across pixels is a second
frame-graph pass.

**Concrete re-read trigger:** if E10c's rejection heuristics prove fiddly — if
ghosting survives the clamp, or the clamp visibly eats the highlight it was meant
to keep — read this and R1's four shaders. R1 is a working MIT-licensed
implementation of the resampling chain **over a voxel DDA**, which is a far
better starting point than the notes.

→ **New ledger 2.16a (dead, with reason and a re-read trigger).**

---

### R4 — Neural supersampling and denoising for real-time path tracing (AMD)

[GPUOpen article](https://gpuopen.com/learn/neural_supersampling_and_denoising_for_real-time_path_tracing/),
research presented at I3D 2025.

Multi-branch, multi-scale U-Net: one feature-extraction path for noisy colour,
another for noise-free guide buffers. Inputs: **1 spp colour**, albedo, normal,
roughness, depth, specular hit distance, motion vectors, and temporally
accumulated noisy input to raise effective spp. Targets 4K on RDNA 2+.

**Verdict: dead — and note that E10 moved this one closer without making it
viable.** E10a supplies normal, depth/world-position, material id and a
reprojection path (and the plan's *"no motion vectors: the world is static
between edits, so camera motion IS the reprojection"* covers that input a
different way), while E5 and E10 make roughness and specular live. So the input
list is no longer the objection it was.

What kills it anyway: 1 spp Monte Carlo remains a non-goal; **no SDK and no
published millisecond cost** ("research is ongoing"); and an ML inference runtime
is not a thing we can carry to the Quest tier (E9), which every other lever in
this engine is required to reach in some rung. Recorded because "we considered ML
denoising and here is the price of entry" is worth more than silence.

→ **New ledger 2.16b (dead, with reason).**

---

### R5 — Real-time Collision Detection between General SDFs

Pengfei Liu, Yuqing Zhang, He Wang, Milo K. Yip, Elvis S. Liu, Xiaogang Jin ·
GMP 2024, **Best Paper** · Zhejiang University / University of Leeds / Tencent
Games · [project page](https://dlpf.github.io/sdf-collision.github.io/).

Interval calculations plus the SDF gradient guide the search for intersection
points; complex objects are segmented into bounded regions, candidate part pairs
are found, then **penetration depth, contact points and contact normals** are
computed. Handles both continuous and discrete SDFs. Paper, video and slides
released; **no source code**.

**Verdict, corrected against the current state: this is no longer a B3
candidate.** My first read filed it as "player/entity collision from the
clearance field we already own", but **B3 is done** — E2b shipped a CPU swept-box
character collider (ledger 4.11) at **0.62–0.96 µs per movement step**, 0.01–0.05%
of an 8 ms frame, with a per-axis anti-tunneling guarantee. Nothing that costs
more than a microsecond can win that comparison, and E2b already rejected the
*cheaper* alternative (4.11b, the sandbox heightfield) on capability rather than
speed.

**Where it stays live: B5 (entity voxel splatting), which the dossier describes as
an OBB/SDF + local-DDA path.** Entity-vs-entity is the case our brickmap does not
answer, because two moving entities are not in the world grid. That is the
problem this paper actually solves, and it handles **discrete** SDFs, which is
what a voxelized Blockbench entity is.

Two notes to carry into B5: (a) our chebyshev field is per-*brick*, so it can
serve a broadphase and "am I inside geometry" but not contact normals — the same
1 m resolution wall that killed 2.11; (b) E2b's inverted-cost finding (4.11a — a
query's cost tracks the **empty** volume it scans, not the occupied one) is the
sizing rule for any interval/gradient search over our field, and it is
counter-intuitive enough to be worth restating there.

→ **New ledger 4.12 (open, B5).**

---

### R6 — Fast Octree Neighborhood Search for SPH Simulations

Jose Antonio Fernandez-Fernandez, Lukas Westhofen, Fabian Löschner, Stefan Rhys
Jeske, Andreas Longva, Jan Bender · SIGGRAPH Asia 2022 · RWTH Aachen ·
[PDF](https://animation.rwth-aachen.de/media/papers/79/2022-SA-NeighborhoodSearch.pdf).
**Not read in full — the PDF exceeds the fetch size limit.** Attribution is from
the title, venue and group; any speedup figures are **unverified**.

**Verdict: marginal.** Our fluid arc is CA / heightfield
(`docs/transparent-voxels-plan.md`, backlog B6), not SPH, so the planned design
has no particles needing neighbour lists.

One transferable lesson, for **B4 (GPU particles + collision)**: in particle
simulations the **neighbour search dominates**, not the integration — and we
already own the accelerator. Bucketing particles by brick index reuses the
brickmap as a uniform grid for free, which is the "spatial bucketing" A/B that B4
already names. If B6 ever goes particle-based instead of CA, read this first.

→ **New ledger 4.13 (open, B4).**

---

### R7 — Exploring ways to mipmap alpha-tested textures

[lisyarus](https://lisyarus.github.io/blog/posts/exploring-ways-to-mipmap-alpha-tested-textures.html).
Nine methods compared for the classic problem: averaging alpha during mip
generation dilutes it below the 0.5 cutoff, so foliage fades out with distance.
Winner: keep premultiplied-colour averaging, but from level 1 onward **replace
the alpha channel with the maximum of a signed distance field over each 2×2
quad**.

**Not applicable directly** — no albedo textures anywhere, no alpha testing
(ledger 5.14); colour is a palette lookup.

**One rule is transferable, and it is a real one:** when building a coarse level
over a fine occupancy/coverage signal, **max-downsample the distance field; never
average the coverage.** Averaging is precisely what makes thin geometry evaporate
at coarse levels. That is a design constraint on **ledger 1.9 (brick-grid mip
pyramid)** before it is built, and it names the mechanism behind E4's recorded
thin-geometry leaks (a cell absorbs at quarter fill, so sparse canopy transmits —
which reads as light-through-foliage and is *wanted* there, but would read as
disappearing canopy in a traversal pyramid). It compounds with R1: a *directional*
field mipped by max is the conservative form of ledger 1.9.

→ **Note added to ledger 1.9.**

---

### R8 — Handling input in a fixed timestep

[jakubtomsu](https://jakubtomsu.github.io/posts/input_in_fixed_timestep/).
Three failure modes when one input snapshot is handed to N ticks: inputs between
ticks are lost; transient flags ("pressed"/"released") fire once per tick instead
of once; mouse and scroll deltas are applied N times. Fixes: keep frame-level and
tick-level input state separate, divide deltas by tick count
(`tick_input.cursor_delta /= num_ticks`), clear transient flags after each tick
(keep only the held bit), reset deltas after all ticks.

**Verdict: keep, cheap, applies at E9 and B6.** Relevant the moment a fixed-rate
simulation and a variable-rate frame coexist — B6's CA, and E9's fixed-refresh VR
(72/90 Hz with reprojection). Note E2b deliberately needs **no** fixed timestep
today (its collision is sub-microsecond and runs per frame), so this is not a
retrofit of existing code; it is a rule for when the first real tick loop appears.
The duplicated-transient bug is the shape that would make one voxel-place click
place several, and E2 already ships hold-to-repeat editing.

---

### R9 — Fixed timestep without interpolation

[jakubtomsu](https://jakubtomsu.github.io/posts/fixed_timestep_without_interpolation/).
Instead of interpolating between the previous and current sim state, **memcpy the
current state and simulate the copy forward by the leftover accumulator time**,
then render the copy. No added latency, matches real time, easy to implement —
but requires trivially copyable, pointer-free, fixed-size state, and degrades
below ~30 TPS.

**Verdict: the method is already priced dead, by a number we measured ourselves.**
Its precondition is a per-frame copy of the simulation state, and E2 measured a
deep copy of the brickmap at **4.9 ms for 46.4 MB** (ledger 4.2a, which killed
`Arc<Brickmap>` snapshot swapping for the same reason). One render-tick would cost
more than half our entire ~8 ms frame budget. Closed in one lookup, no experiment
— a demonstration that the ledger pays for itself.

**But it names a real bug we own, in two places, and that is the most valuable
thing in the batch after R1/R2.** E4's convergence is quoted in **frames**: 32
frames to bit-exact, 16 to max delta 1, at N iterations *per frame*. So light
propagation speed is a function of frame rate:

| Frame rate | Time to bit-exact (32 frames) |
|---|---|
| 30 fps (Potato) | 1.07 s |
| 60 fps (recorded) | 0.53 s |
| 72 Hz (Quest 3) | 0.44 s |
| 90 Hz (Quest 3) | 0.36 s |
| 120 fps | 0.27 s |

The CA is a fixed-timestep simulation clocked off the render loop. **The plan
already found the same class of bug independently in E10** — the Quest risk note
reads *"a 10-frame window at 72 Hz is 140 ms"*, i.e. the reflection history's
convergence time is frame-rate-bound too. Two instances of one lesson is what
makes it a ledger row rather than a footnote.

Scope, honestly: E5 records that Pascal ran the current 32-frame lag and was
**not bothered**, so this is not urgent for GI. It becomes urgent where a rate is
*perceptible as a speed* — **B6 falling sand would literally fall faster on a
faster machine** — and where a tier switch changes the rate (Potato↔Beautiful,
desktop↔Quest). Logged as an open observation, **not fixed, not in scope for E5 or
E10**. The fix shape is an iteration/alpha budget driven by elapsed time rather
than frame count, which interacts with E5's parked per-region budget and E10c's
history alpha, so all three should be designed together when one of them is
scheduled.

→ **New ledger 7.11 (open observation).**

---

### R10 — LinkedIn post (Annalena Ulschmid)

[Post](https://www.linkedin.com/posts/annalena-ulschmid-437683277_voxel-computergraphics-raytracing-ugcPost-7462894081405476864-lHUi/)
— **not retrievable, authentication wall.** Assumed to be the R1 announcement /
demo video, given the author and the `#voxel #computergraphics #raytracing` tags.
**Open question for Pascal:** does the post contain anything the repo and abstract
don't — frame times, hardware, comparison footage against a brickmap?

---

## Cross-cutting observations from this batch

1. **Two of the strongest items attack the same weakness from opposite ends:
   scalar isotropy.** R1 attacks it in *visibility* (directional distances let a
   grazing ray see far), R2 in *transport* (traced directions let a cell see far).
   Ledger 2.11 died of it, 2.13b keeps a 2.7×-cost lever for it, E4 lists it as a
   structural compromise. It is the engine's most-cited single limitation. R1 now
   also supplies the measured qualifier: **fixing the isotropy is not the same as
   winning** — 1.13 improved reach (9.10 → 10.82 mean) and still lost the frame.
2. **The governing principle sorted this batch faster than any per-item argument —
   and then a measurement found the cost it does not model.** Direct terms need
   accurate visibility, indirect terms mask error; so R1's accuracy belongs in the
   direct term and R2's cheap samples in the indirect one. That prediction held
   for *where the value is* (2.11 is now R1's live consumer). What it does not
   model is **load count**: 1.13's field is more accurate and still slower,
   because our scalar byte answers two questions per load. Add that as a second
   axis before trusting the principle alone on a data-layout change.
3. **The non-goals list is load-bearing in a new way.** It was amended
   2026-07-31 to let temporal accumulation into E10's reflection buffer, and that
   single amendment changed the verdict *reasoning* for R3 and R4 within a day.
   Both are still dead — now on dependencies and Quest reachability rather than on
   principle — so the re-read triggers matter more than the verdicts.
4. **The ledger paid for itself three times, before any code ran.** R9's method
   closed against E2's measured 4.9 ms deep copy; R1's memory was sized from E2's
   recorded 500 KB clearance rebuild (~1.5 MB estimated vs 2 MB built); R5 was
   downgraded a whole slot by E2b's 0.62–0.96 µs swept box.
5. **The one proposal left standing is E4b (R2).** R1's slot was consumed by
   measurement the same day. E4b needs no new data structure and no frame-graph
   change — it swaps the update rule inside a volume that already exists — so
   unlike R1 it does not have to be sequenced before E3.
6. **Process note: this batch was triaged twice.** The first pass was written
   against a snapshot of the docs that was hours stale — E1d, E2b, E6 and E10 had
   landed, the temporal non-goal had been amended, and R1 was implemented while
   the entry proposing it was being written. Three verdicts changed (R1, R3, R5).
   **Re-read the ledger and plan immediately before recording a verdict**, not at
   the start of the session that records it.
