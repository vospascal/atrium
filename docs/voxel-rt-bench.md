# voxel-rt DDA Benchmark — Regression Gate

The permanent perf harness for the voxel-rt renderer. Run it **before and
after any change that could touch traversal cost** — shader edits, brickmap
layout changes, new ray types, world-content changes — and compare against
the recorded baseline below.

```
cargo run -p voxel-rt --example bench_dda --release
```

Runtime ≈ 15–20 min (world gen ~0.5 s, then four sections: 8 traversal
variants, 10 ray-traced-AO variants, 14 E1b/E1c variants and 4 quality presets,
each × 4 camera/sun scenarios × 12 timed batches). No window; needs a real GPU.
Trailing section numbers run a subset — `... --release -- 3` measures only the
E1b section, `-- 4` only the preset table — and because sections are independent
(isolation rule) a subset run yields exactly the rows a full run would print for
it.

## What it measures

Four independent sections, each with its own variant table and pixel compare
(isolation rule — an experiment's numbers never contaminate the gate for the
layer below it):

1. **Traversal levers, AO forced off** — the Stage 2 regression gate. Every
   column here has `AO_MODE = AO_MODE_OFF`, so the medians stay directly
   comparable with the pre-E1 baseline recorded below.
2. **E1 ray-traced AO variants** — the ray-count / distance / direction /
   falloff contenders, built through `passes::dda::build_shader_source`.
3. **E1b cheap occlusion + soft shadows** — the analytic AO estimators, the
   three AO cost-cutting levers, the hard-vs-soft shadow sweep and E1c's
   const-vs-uniform A/B, against E1's `ao-2ray-d8` reference row and the
   `ao-off` floor.
4. **E1c quality presets** — Potato / Quest / Balanced / Beautiful, each
   dispatched at ITS OWN render scale. The headline table (below); future gates
   quote it.

**The variant tables are DERIVED from the lever registry (E1c).** Every lever
has one row in `crates/voxel-rt/src/variants.rs::REGISTRY` carrying its kind
(compile-time const vs runtime uniform), its default, its measured verdict and
the `BenchPoint`s that sweep it; each section collects its own bench points and
applies them to that section's baseline `RenderQuality`. Adding a lever row
therefore adds a bench column forever after — the harness holds no parallel
list, only the *anchors* (a section's baseline and its compare reference).
`registry_defaults_match_shader_source`,
`every_lever_shaped_shader_const_has_a_registry_row` and
`every_compile_time_lever_is_swept_by_the_bench` fail if the shader, the
settings defaults, the registry or the sweep drift apart.

- **Scenarios** (fixed, deterministic poses — seed 1, season 0.0):
  - `A` top-down over the island center, 60 m altitude, default sun
  - `B` same view, sun at 5° elevation — worst case for shadow rays
  - `C` ground level at spawn looking across the island, default sun
  - `D` same view, low sun
- **Variants**: `current` = the shipped shader exactly as in `dda.wgsl`
  (with AO patched off in section 1). Every other traversal column applies one
  registry bench point, which patches one `ENABLE_*` lever (the "A/B benchmark
  levers" block at the top of `shaders/dda.wgsl`), so each optimization is
  measured in isolation. `stage2-baseline` = all traversal aids off. A variant
  carries a whole `RenderQuality`, so runtime levers (AO strength, penumbra
  scale, fade ramp) and the render scale are swept exactly the way the app
  applies them — the preset section dispatches at each tier's real pixel count.
- **Timing**: 25 dispatches encoded back-to-back per command buffer,
  wall-clock per batch / 25, 12 batches, median + p95. Variants rotate
  round-robin inside each scenario so GPU clock/thermal drift hits all
  columns equally. (GPU timestamps are NOT used: Metal resolves
  pass-boundary counters to zero once a command buffer holds more than one
  compute pass.)
- **Correctness gate**: section 1 renders the low-sun scenarios (B, D) per
  variant and pixel-compares them against `stage2-baseline`; section 2
  renders the default-sun scenarios (A, C) per AO variant and sections 3–4
  render ALL FOUR, reporting differing pixels vs the section's reference as a
  *coverage* number (how much of the frame the variant touches — the images
  differ by design, so this is not a correctness gate). Section 4 skips the
  compare for tiers rendering at another resolution than Balanced (no pixel
  correspondence exists), so only Beautiful is compared there. All PNGs land in
  `target/bench_dda/` for eyeballing.
- **Timing before capture** (fixed during E1b): every scenario in a section is
  timed *before* any image readback or PNG encode happens. Interleaving them —
  encoding ten 2560x1440 PNGs between two timed scenarios — measurably
  inflated whichever scenario followed a capture, which is why the E1 table's
  C and D rows read ~1.6 ms (AO off) to ~3.4 ms (AO on) high while its A and B
  rows, which no capture precedes, are reproducible to ~1%. See the correction
  note in the E1 section.

## Recorded baseline — Apple M3 Max, 2560x1440, 2026-07-30

Commit state: Stage 2 traversal optimization round (branchless `dda_step`,
chebyshev distance skip; column-ff / descend-ff / any-hit / bit-grid
defaulted OFF as measured losses). These are the **section 1** (AO off)
numbers; the E1 AO table lives in its own section below.

Per-dispatch **median ms**, `current` column:

| scenario | current | stage2-baseline |
|---|---|---|
| A top-down, default sun | **4.73** | 6.43 |
| B top-down, low sun 5°  | **6.51** | 7.92 |
| C ground, default sun   | **4.38** | 5.29 |
| D ground, low sun 5°    | **4.95** | 5.94 |

Expected correctness output: B shows **19 / 3 686 400 differing pixels
(0.0005%, max channel delta 97)** for every distance-skip variant — known,
accepted float-tie divergence at empty-cube exits; D shows **0**. `no-dist-skip`
and `stage2-baseline` are always bit-identical to each other. One refinement
recorded in E1c: `with-descend-ff` reads **12** rather than 19 on B, because the
descend jump reorders coarse steps and 7 of those float-tie pixels then resolve
toward the baseline. Not a regression — the pixels are the same known tie set,
and `current` still reads exactly 19 / 0.

Cross-run noise on an idle machine is well under ±2% on medians; p95 should
sit within a few percent of the median. Two independent runs on the dev
MacBook reproduced every median within ~0.5%.

**Warmup caveat (measured twice during E1):** the FIRST scenario row of a
section (`A`) inflates by 5–70% when the run starts on a busy machine — a
`cargo test`/`cargo build` finishing just before, or the compile of the run
itself, leaves the GPU clocked down through scenario A's warmup batches.
Scenarios B/C/D are unaffected. Start the bench on an idle machine, and if
row A looks high while B/C/D match, re-run before believing row A.

## Regression protocol when adding features

1. **Before starting**, run the bench once on an idle machine and stash the
   table (or trust this file if the tree is clean).
2. Build the feature. If it adds a lever, the whole wiring is **one registry
   row** (`src/variants.rs::REGISTRY`): identifier, subsystem, kind
   (`ShaderConst` / `Runtime`), the WGSL const it patches, its default, its
   range, its measured verdict, and the `BenchPoint`s that sweep it. That row
   gives you the bench column, the overlay control and the pinning tests at
   once; `patch_shader_const` panics if the const name drifts, and
   `every_lever_shaped_shader_const_has_a_registry_row` fails if a lever is
   added to the shader without a row. A compile-time lever also needs its field
   on the matching mirror struct (`traversal.rs` / `ao.rs` / `shadows.rs`) whose
   `default_settings_match_shader_source` test pins it to the shader's own
   defaults; a runtime lever needs a `ShadingParams` component instead, and is
   then swept with no rebuild at all.
3. **After**, run again and compare medians per scenario:
   - within ±2%: noise, fine;
   - one scenario regressed >2%: understand why before shipping — the
     scenarios are chosen so each stresses a different path (B ≈ shadow
     rays, A ≈ empty-air descent, C/D ≈ near-ground fine-DDA density);
   - all scenarios regressed: the change is a real cost — decide
     deliberately, then update the baseline table in this file.
4. Check the correctness lines: any *new* differing pixels vs baseline
   (beyond the recorded 19 on B) mean the change altered ray results —
   inspect the PNGs in `target/bench_dda/` before accepting.
5. If defaults change (a lever flips), update: the lever comment in
   `dda.wgsl`, the mirror struct's `Default`, the registry row's
   `default_value` + `verdict`, and this file's baseline (the pinning tests
   force the first three to move together). Defaults are ALWAYS the fastest
   measured combination — never flip one without a fresh table.

## Standing verdicts (M3 Max — re-measure on other GPUs)

**These verdicts also live in the code**: since E1c every one of them is the
`verdict` string of a `REGISTRY` row, which the overlay shows as hover text on
the lever's control — "why is this off?" is answerable in-app, and the numbers
below and the numbers in the panel cannot drift apart.

Measured this round, kept as default-off levers because the trade-offs are
architecture-specific (re-run everything on Quest 3 in Stage 6):

- `ENABLE_DISTANCE_SKIP` **on** — the engine of the current numbers
  (17–27% under baseline). Its byte also serves as the occupancy test.
- `ENABLE_GLOBAL_MAX_TERMINATE` **on** — cheap, exact sky-out for upward rays.
- `ENABLE_COLUMN_FAST_FORWARD` / `ENABLE_DESCEND_FAST_FORWARD` **off** —
  superseded by the distance field in all directions (+9–17% if re-enabled).
- `ENABLE_ANY_HIT_SHADOW` **off** — the specialized any-hit loop lost 1–3%
  to plain `trace()` in three separate rounds.
- `ENABLE_BRICK_BIT_GRID` **off** — redundant next to the distance byte for the
  traversal; its data is read by E1b's AO brick early-out. Retry where caches
  are small.

Quality levers with recorded verdicts (details in the E1 / E1b / E1c sections):

- `AO_MODE` **1 = analytic corner** is the shipped default since E1c (E1b's
  winner: 20x cheaper than rays, noiseless). `0 = ray-traced` is the Beautiful
  tier, kept for its reach. `2 = analytic 3x3x3` **off** (over-darkens).
- `SHADOW_MODE` **0 = hard**. Soft-from-distance-field is free but prints the
  1 m brick lattice into the frame at every penumbra scale — documented
  negative result, needs voxel-level clearance data to become viable.
- `AO_BRICK_EARLY_OUT` **off** — measured 0% firing rate on terrain.
- `AO_DISTANCE_FADE` **off** — 0.6–2.9% at ground level; the big aerial saving
  is the effect itself being removed. Aerial-camera / Potato knob (Potato ships
  it at 15→30 m).
- `AO_SUN_AWARE_RAY_BUDGET` **off** — ≤7.5% for the 1-ray crosshatch on exactly
  the surfaces that show it.
- The fade ramp bounds are **runtime** since E1c (`shading_params.z/w`, measured
  free) — the two `AO_FADE_*_VOXELS` consts no longer exist. Everything inside
  the traversal loops and every mode selector stays a compile-time const; the
  preset permutations are precompiled at startup instead (E1c section).

---

## E1 — Ray-traced ambient occlusion (M3 Max, 2560x1440, 2026-07-30)

Short occlusion rays from each primary hit attenuate the hemisphere-ambient
term only (the sun keeps its own shadow ray). Levers: the "E1 AO levers"
block in `dda.wgsl`; Rust mirror + shader patching in `src/ao.rs`; overlay
section "AO".

### No-regression check (AO off)

`current` with AO off (`ENABLE_AO = false` at the time; `AO_MODE =
AO_MODE_OFF` since E1b) measured against a control run of the
pre-E1 tree on the same idle machine:

| scenario | pre-E1 control | E1 tree, AO off | delta |
|---|---|---|---|
| A top-down, default sun | 4.732 | 4.717 | -0.3% |
| B top-down, low sun 5°  | 6.548 | 6.545 | -0.0% |
| C ground, default sun   | 4.397 | 4.399 | +0.0% |
| D ground, low sun 5°    | 4.940 | 4.943 | +0.1% |

All within noise, and the B/D PNGs are **byte-identical** to the pre-E1
renders (`cmp` on the files, not just the pixel counter). Threading
`max_distance` through `trace()` costs nothing — an equivalent build with the
old constant `t_limit` measured 4.937 vs 4.931 ms on D. The AO experiment is
therefore fully excludable: with the lever off the renderer is the Stage 2
renderer, bit for bit.

### CORRECTION (measured during E1b, 2026-07-30)

The C and D rows of the table below are **too high**, and so are the AO costs
derived from them. The harness used to encode the captured scenario's PNGs
between timed scenarios; scenario C's timing therefore started right after
scenario A's ten-PNG burst, and D's after C's. Section 3's rewritten harness
times everything first, and a clean re-run of this exact variant table gives:

| variant | A | B | C | D |
|---|---|---|---|---|
| `ao-off` | 4.738 | 6.584 | **4.434** | **5.236** |
| `ao-2ray-d8` | 11.804 | 14.781 | **8.642** | **10.219** |
| `ao-1ray-d8` | 8.970 | 11.921 | 6.833 | 8.282 |
| `ao-2ray-d16` | 13.165 | 16.118 | 9.958 | 11.214 |
| `ao-4ray-d16` | 19.984 | 22.993 | 14.629 | 16.315 |

A and B reproduce the recorded numbers to ~1%. **Corrected AO cost of the
shipped default: +7.1 (A) / +8.2 (B) / +4.2 (C) / +5.0 (D) ms**, not the
+5.8–8.1 originally recorded — the ground-level views are cheaper than E1
believed. Every *ranking* verdict below is unaffected (the inflation hit all
columns of a row equally). The corrected **secondary-ray budget** from the
clean ladder: 0→1 ray costs 4.89/5.99/3.18/3.73 ms, 1→2 costs
3.54/3.55/2.35/2.25, 2→4 costs 3.41/3.44/2.34/2.55 per ray — i.e. **≈2.25–3.55
ms per marginal full-res short ray at 16 voxels, ≈1.8–2.9 ms at 8** (2ray-d8
minus 1ray-d8). Lower than E1's ≈3.4–4.3 but the same order, so E4's
conclusion stands: a per-pixel CAGI gather of more than ~1 ray is
unaffordable.

### Variant table — per-dispatch median ms

Grid center = 2 rays / 16 voxels / cosine-weighted / distance falloff. The
ray-count, direction and falloff contenders vary one factor around it; the
distance ladder spans 8/16/32 at 2 rays. `ao-2ray-d8` is the shipped default.
Strength 0.8 throughout (runtime uniform, not a compile-time variant).

| variant | A top-down | B top-down low sun | C ground | D ground low sun |
|---|---|---|---|---|
| `ao-off` | 4.78 | 6.67 | 6.03 | 7.50 |
| `ao-1ray-d16` | 9.71 | 12.72 | 9.85 | 12.08 |
| `ao-2ray-d16` | 13.34 | 16.65 | 13.09 | 16.09 |
| `ao-4ray-d16` | 20.24 | 23.37 | 21.63 | 23.39 |
| **`ao-2ray-d8` (default)** | **11.94** | **14.77** | **12.00** | **13.27** |
| `ao-1ray-d8` | 9.08 | 11.95 | 9.00 | 10.49 |
| `ao-2ray-d32` | 15.91 | 18.80 | 18.60 | 21.15 |
| `ao-uniform-d16` | 13.98 | 17.02 | 14.64 | 16.83 |
| `ao-bent-d16` | 12.16 | 15.09 | 11.89 | 13.29 |
| `ao-binary-d16` | 13.26 | 16.28 | 13.69 | 15.97 |

Coverage (differing pixels vs `ao-off`, scenario A / C): 1ray-d16 23.6/19.7%,
2ray-d16 39.2/34.1%, 4ray-d16 52.2/44.4%, 2ray-d8 37.6/30.2%, 1ray-d8
22.6/17.4%, 2ray-d32 41.8/45.6%, uniform 51.1/40.3%, bent 21.0/14.5%, binary
39.8/37.6%. Max channel delta stays ≤ 64 everywhere — AO never blows out to
black.

### Verdicts

- **Ray count 2** (default). 1 ray leaves a *stable but visible* interleaved-
  gradient crosshatch on large flat ground planes (worst on the near ground
  in C/D); 2 rays resolve it into a smooth gradient at +3.0–3.9 ms; 4 rays
  add ~7 ms more for a difference not visible at 1:1 in the PNGs. 2 is the
  knee.
- **Max distance 8 voxels (1 m)** (default). 8 vs 16 is 10–17% cheaper
  (D: 13.27 vs 16.09) with visually equivalent grounding, because the
  distance falloff already discounts far occluders to near-nothing. 32 costs
  +30–60% and mostly adds a flat scene-wide dimming (its coverage jumps
  while contact contrast does not) — the classic AO over-darkening failure.
- **Cosine-weighted hemisphere** (default). Uniform costs +4.7% (A) to +14.7%
  (C) over cosine at equal ray count — its extra grazing rays hit sooner, so
  it walks more occupied bricks — and it over-darkens: 51% coverage vs 39% at
  the same strength, visibly greying open flat ground that should stay
  unoccluded. Cosine also matches the Lambert weighting of the ambient term
  it multiplies, so binary hits average to the correct visibility integral.
- **Bent-up** — cheapest AO-on variant (12.16 A) and pleasantly noise-free,
  but it is a sky-visibility proxy, not occlusion: 21%/14.5% coverage means
  it *misses* most lateral contact darkening (voxel sides against each other
  barely change). Kept as a documented off-lever for the Quest tier, where
  "cheap and clean" may beat "correct".
- **Distance-weighted falloff** (default). Binary costs *more* than falloff at
  the same distance (16.28 vs 16.65 on B is a wash, but binary at d16 vs
  falloff at d8 is 16.28 vs 14.77) and looks worse: uniform mid-grey patches
  wherever anything is within range instead of a contact gradient. Falloff is
  both the better look and, paired with the short distance it enables, the
  cheaper configuration.
- **Half-res AO — REJECTED, not implemented.** Not cleanly separable inside
  this pass: one thread per pixel owns its own primary hit, so "half-res"
  means either a separate pass (needs a G-buffer of hit position/normal +
  a bilateral upsample — a whole new pass, buffer and blur, i.e. E7-scale
  work) or quad-level sharing via subgroup ops (wgpu-portable subgroups are
  not available here yet). Revisit if AO ever needs to survive on Quest at
  full ray count; the render-scale lever already covers the crude version.

### Chosen defaults

AO on (`AO_MODE = 0` since E1b), `AO_RAY_COUNT = 2`, `AO_MAX_DISTANCE = 8.0`,
`AO_DIRECTION_MODE = 0` (cosine), `AO_DISTANCE_FALLOFF = true`, strength 0.8.
Cost: **+7.2 ms (A) / +8.1 ms (B) / +6.0 ms (C) / +5.8 ms (D)** over AO off,
i.e. 11.9–14.8 ms total for the DDA pass. That is over the plan's ~8 ms
desktop target, so AO is the first feature whose default may have to be
re-tiered (render scale 0.75 brings the default variant back under 8 ms);
Pascal gates the look/cost trade visually.

### Secondary-ray budget (the number E4/CAGI sizing needs)

From the ray-count ladder at fixed distance 16, per AO ray per full-res pixel
at 2560x1440 (3.69 M pixels):

| step | A | B | C | D |
|---|---|---|---|---|
| 0 → 1 ray | 4.93 | 6.05 | 3.82 | 4.58 |
| 1 → 2 rays | 3.63 | 3.93 | 3.24 | 4.01 |
| 2 → 4 rays | 3.45 | 3.36 | 4.27 | 3.65 |

**≈ 3.4–4.3 ms per marginal short secondary ray per full-res pixel** (the
first ray is dearer — it pays the shading path's branch and the origin
reconstruction). Shortening the ray to 8 voxels brings the marginal cost to
≈ 2.6–3.6 ms (2ray-d8 minus 1ray-d8). Sizing consequence for E4: a
per-pixel CAGI *gather* of more than ~1 ray is unaffordable at full res on
this GPU, which is exactly the argument for the dossier's CA light volume —
pay the transport once per voxel cell per frame and sample it with **zero**
extra rays, using AO (this experiment) as the only per-pixel ray budget:
`indirect = cagi_sample * ambient_occlusion(...)`.

---

## E1b — Cheap occlusion + soft shadows (M3 Max, 2560x1440, 2026-07-30)

E1's ray-traced AO passed its visual gate but cost +4.2–8.2 ms, so E1b shops
for *techniques*, not ray counts. Levers: the "E1/E1b: ambient occlusion
levers" and "E1b: shadow levers" blocks in `dda.wgsl`; Rust mirrors in
`src/ao.rs` (`AoMode`, three cost-cutting levers) and `src/shadows.rs`
(`ShadowMode`, penumbra scale); composed for every consumer by
`passes::dda::build_shader_source`. `ENABLE_AO` is gone — `AO_MODE` with an
explicit `AO_MODE_OFF` is the single source of truth, and section 1 patches it
to keep the Stage 2 gate comparable.

### No-regression check

Section 1 re-run with `AO_MODE = AO_MODE_OFF` and `SHADOW_MODE = 0`:
**4.723 / 6.509 / 4.391 / 4.943 ms** (A/B/C/D) against the recorded baseline's
4.73 / 6.51 / 4.38 / 4.95, and the shadow-correctness compare reproduces
**19 differing pixels on B, 0 on D** exactly. The E1b levers are fully
excludable: with AO off and shadows hard the renderer is still the Stage 2
renderer.

### Variant table — per-dispatch median ms

All rows from one clean run (idle machine, p95 within 1–3% of every median).

| variant | A top-down | B top-down low sun | C ground | D ground low sun |
|---|---|---|---|---|
| `ao-off` (floor) | 4.739 | 6.523 | 4.400 | 4.957 |
| `ao-2ray-d8` (E1 reference) | 11.758 | 14.642 | 8.561 | 9.399 |
| **`ao-corner`** | **5.039** | **6.837** | **4.649** | **5.208** |
| `ao-neighborhood` | 6.256 | 8.095 | 5.684 | 6.237 |
| `ao-2ray-brickskip` | 11.606 | 14.472 | 8.492 | 9.320 |
| `ao-2ray-fade30-60` | 10.337 | 13.281 | 8.509 | 9.338 |
| `ao-2ray-fade15-30` | 5.978 | 8.900 | 8.314 | 9.170 |
| `ao-2ray-sunbudget` | 10.587 | 14.657 | 8.437 | 9.041 |
| `soft-k4` | 4.915 | 6.874 | 4.501 | 5.137 |
| `soft-k16` | 4.932 | 6.881 | 4.497 | 5.136 |
| `soft-k64` | 4.936 | 6.905 | 4.496 | 5.120 |
| `soft-k115` | 4.929 | 6.892 | 4.499 | 5.113 |
| **`corner+soft-k115`** | **5.233** | **7.186** | **4.753** | **5.363** |

Cost over the `ao-off` floor (the number that matters):

| variant | A | B | C | D |
|---|---|---|---|---|
| `ao-2ray-d8` | +7.02 | +8.12 | +4.16 | +4.44 |
| `ao-corner` | **+0.30** | **+0.31** | **+0.25** | **+0.25** |
| `ao-neighborhood` | +1.52 | +1.57 | +1.28 | +1.28 |
| soft shadows (any scale) | **+0.18** | **+0.35** | **+0.10** | **+0.17** |

Coverage (differing pixels vs `ao-off`, A / B / C / D %, max channel delta):
`ao-2ray-d8` 37.6/37.7/30.2/30.0 (≤63) · `ao-corner` 35.8/38.1/24.8/24.6 (≤64)
· `ao-neighborhood` 79.8/81.6/71.0/68.0 (≤43) · `ao-2ray-brickskip`
37.6/37.7/30.2/30.0 (≤63, **byte-identical to `ao-2ray-d8`**) ·
`ao-2ray-fade30-60` 16.8/19.4/29.7/29.5 · `ao-2ray-fade15-30`
0.0/0.0/28.2/28.1 · `ao-2ray-sunbudget` 29.6/37.3/29.0/26.4 · `soft-k115`
22.9/14.1/11.2/22.3 (≤106).

### Verdict A — analytic corner AO is the headline win

**Analytic corner AO costs +0.25–0.31 ms — 20x less than E1's rays — at
35.8/24.8% coverage against RT-AO's 37.6/30.2%.** It reaches ~82% of RT-AO's
frame coverage for ~4% of the cost. PNG findings (`scenario_c_*`, near-ground
crops at 3x):

- **It looks better than RT-AO on large near surfaces.** RT-AO at 2 rays leaves
  a dense interleaved-gradient crosshatch across big flat faces — plainly
  visible at 1:1 on the near ground, and unmissable in a 4x-amplified diff
  against `ao-off`. Corner AO's diff is *smooth bilinear ramps with zero
  speckle*. E1 chose 2 rays because 1 ray crosshatched; 2 rays reduced the
  crosshatch, it did not remove it. Analytic AO has none by construction —
  which matters more than usual here, because noiselessness is this engine's
  stated identity.
- **Where it visibly falls short: reach.** Corner AO darkens only where voxels
  actually touch (one voxel out from the hit face). RT-AO additionally applies
  a soft general dimming to any surface with geometry within 8 voxels, so
  recessed-but-not-touching areas — the inside of a rock cleft, ground under a
  canopy a metre up, the gap between two boulders — read a step flatter with
  corner AO. Measured as the mean luminance gap: 1.1/255 frame-wide, peaking at
  ~5.4/255 in the densest near-ground tiles. Subtle, and *exactly* the
  medium-scale band the dossier expects CAGI to own at E4.
- Faces of thin cover (grass tufts, flowers) get the same contact treatment as
  in RT-AO — the art style's tell — because this is the signal voxel-sandbox
  bakes into mesh vertex colors.

**`ao-neighborhood` (3x3x3, 26 neighbours) loses.** 5x corner AO's cost
(+1.3–1.6 ms) for 68–82% coverage at ≤43 max delta: a broad low-amplitude
dimming, not contact contrast — the classic analytic over-darkening failure,
and per-voxel flat (visible facets, no sub-voxel gradient) where corner AO is
smooth. Kept as a documented off-lever only.

Implementation note: the 3x3x3 estimator is centered on the FACE-FRONT voxel,
not on the hit voxel as first specified. Centered on the hit voxel, the
surface's own in-plane neighbours (always solid on any flat ground) sit at
cos = 0 and darken open terrain by a computed ~45%; one voxel out they sit at
cos < 0 and the same ground reads ~9%. Flagged as a deliberate deviation.

### Verdict B — soft shadows from the distance field: RECOMMEND AGAINST

Cost is a non-issue (**+0.10–0.35 ms, no extra rays**, exactly as T1 promised).
The look is the problem, and it is structural, not a tuning failure.

**The granularity finding.** `penumbra_scale` is the reciprocal of the light's
angular radius, so the physically correct value for the sun's 0.5° disc is
~115; 4 would model a 14°-wide source. The sweep k = 4 / 16 / 64 / 115 walks
coverage from 56.1% down to 22.9% (scenario A) — the term responds — but at
*every* scale the per-BRICK field stamps its own structure into the frame:

- **The 1 m brick lattice is directly visible.** On the water surface — a
  perfectly flat plane, clean under hard shadows — soft mode draws a hard
  grid of darker lines on 8-voxel centres plus diagonal streaks aligned with
  the sun azimuth (`scenario_a_soft_k115.png`, water crop at 4x). The streaks
  come from `min()` tracking: the clearance estimate steps as the ray crosses
  bricks, and the running minimum locks the worst step in for the rest of the
  ray, smearing it along the sun direction.
- **No penumbra ramp exists to trade against it.** At ground level with a low
  sun (D) the result is not a soft shadow edge but a broad dulling: crisply lit
  surface tops go blotchy and mid-grey, with no gradient at the shadow
  boundary.
- **Cheap fine refinement does not rescue it.** Two refinements were
  implemented and measured. (1) Evaluating the clearance as the L∞ distance
  from the sample point to the guaranteed-empty *cube* boundary rather than the
  flat `(skip - 1) * 8` floor — this adds the point's own sub-brick offset and
  is what makes the term continuous at all. (2) Sampling at the MIDPOINT of the
  ray's segment through each brick rather than at its entry face — mandatory,
  not optional: a face point sits exactly on the cube boundary of a
  distance-1 brick, evaluates to clearance 0, and blacked out 55% of the frame
  at every penumbra scale. Both together still leave the lattice, because the
  remaining error is the field's 8-voxel resolution itself.
- **Why no brick-level trick can fix it:** every brick adjacent to an occupied
  brick has chebyshev distance 1, whose conservative clearance is bounded by
  half a brick; and every shadow ray that grazes terrain — i.e. every ray that
  could produce a penumbra — passes through such bricks. The signal a penumbra
  needs lives *below* the field's resolution.

**Recommendation: keep hard shadows.** `SHADOW_MODE = 0` stays the default;
soft stays as a documented off-lever, because it becomes viable the moment a
voxel-level clearance byte exists (a brickmap data change: 512 B per occupied
brick ≈ 37 MB at today's brick count — an E2/E3-scale decision, not an E1b
one). Filed as the reason, not as a maybe.

### Verdict C — the three AO cost-cutting levers (Pascal's addendum)

1. **Brick-neighbourhood early-out (`AO_BRICK_EARLY_OUT`) — NEGATIVE, it never
   fires.** Byte-identical output to `ao-2ray-d8` in all four scenarios
   (identical differing-pixel counts against `ao-off`, and the fallback
   estimate differs everywhere it would fire, so identity proves a 0% firing
   rate), and −0.6% to −1.4% ms, i.e. noise. Two independent reasons, both
   structural: (a) the chebyshev field cannot drive the test at all, because
   any neighbour of the hit's occupied brick has distance ≤ 1 — so the test has
   to read the 1-bit brick grid directly (implemented, 27 bit reads, and cheap
   enough to be invisible in the timings); (b) on terrain the answer is always
   "occupied" — the brick under a surface brick is solid ground and the ring
   bricks hold the neighbouring surface. Restricting the test to the normal
   hemisphere does not help: those ring bricks are exactly the occupied ones.
   The mechanism needs per-brick *voxel-level* clearance to ever fire, i.e. the
   same missing data as the soft shadows. Kept as an off-lever with this note;
   it is also the only current consumer of binding 9.
2. **Distance level of detail (`AO_DISTANCE_FADE`) — WEAK, and not free.** At
   ground level (C/D) it saves 0.6% at 30→60 m and 2.4–2.9% at 15→30 m,
   because a ground-level frame's hits are mostly *within* 30 m — AO cost is
   dominated by near pixels, which is exactly where the fade must not apply. In
   the 60 m top-down view it saves 12% (30→60 m) and 49% (15→30 m), but the
   whole frame is ~50 m out, so those savings ARE the effect being removed:
   coverage falls 37.6% → 16.8% → 0.0%, and the 30→60 m PNG is visibly flatter
   than `ao-2ray-d8` (AO grounding around trees and rocks measurably weaker,
   max channel delta 18 over 17% of the frame). It is a legitimate knob for
   aerial/map cameras and the Potato tier, not a quality-neutral win. Default
   stays off; the shipped range (30 m → 60 m) is the conservative rung.
3. **Sun-aware ray budget (`AO_SUN_AWARE_RAY_BUDGET`) — REJECT for the
   Balanced/Beautiful tiers.** 0–7.5% saving (A −7.5%, C −6.4%, D −0.7%,
   B +0.1% = noise), for a coverage drop of 37.6% → 29.6% (A). No *new* seam
   appears — the `sun_term > 0.5` threshold coincides with the hard shadow
   boundary, which is already a discontinuity, so the transition hides inside
   it — but the pixels it cheapens are precisely the bright, flat, sunlit ground
   where E1 measured the 1-ray crosshatch. Paying a known noise artifact for
   ≤7% is the wrong trade when analytic AO offers 96% for none of it. Kept as
   an off-lever for a Quest re-measure.

### Per-tier recommendation for E1c

Numbers are scenario A / C (the two the player actually sees), full DDA pass at
render scale 1.0:

| tier | AO | shadows | other | DDA pass ms (A / C) |
|---|---|---|---|---|
| **Potato** | analytic corner | hard | render scale 0.7, distance fade 15→30 m | ~2.5 / ~2.3 (scale 0.7 on 5.0 / 4.6) |
| **Quest** | analytic corner | hard | render scale to taste; re-measure every lever on device | 5.04 / 4.65 at scale 1.0 |
| **Balanced** | analytic corner | hard | — | **5.04 / 4.65** |
| **Beautiful** | RT-AO 2 rays / 8 voxels / cosine / falloff | hard | — | 11.76 / 8.56 |

- **Analytic corner AO is the default-tier winner up to and including
  Balanced.** It is 20x cheaper than rays, noiseless where rays are not, and
  brings the whole stack in at **5.0–7.2 ms across all four scenarios** —
  under the plan's ~8 ms target at render scale 1.0, which no RT-AO
  configuration achieves (11.8–14.6 ms).
- **Beautiful keeps RT-AO**, but on the strength of its *reach*, not its
  cleanliness: it is the only variant that dims recessed-but-not-touching
  geometry. Once CAGI lands (E4) that job moves to the light volume, and the
  expected end state is analytic corner AO in every tier with RT-AO demoted to
  a "no CAGI" fallback — the dossier's inference, now with numbers behind it.
- **Hard shadows in every tier.** Soft is free but broken (verdict B); there is
  no tier where 1 m lattice artifacts are the right trade.
- **`ao-neighborhood`, the brick early-out, the sun-aware budget and the
  distance fade appear in no tier.** All four stay as documented off-levers
  with the verdicts above; the fade is the one to reach for first if an
  aerial/map camera ever ships.

### Chosen defaults (unchanged pending Pascal's visual gate)

`AO_MODE = 0` (ray-traced, E1's winner), `SHADOW_MODE = 0` (hard), all three
cost-cutting levers off, penumbra scale 115 (the sun's true angular radius, so
the lever reads correctly if it is ever switched on). The E1b winner —
`AO_MODE = 1` — is one radio button away in the overlay's AO section and is
what E1c should install as the Balanced default once the look is gated in-app.

---

## E1c — Variant registry, quality presets & the headline table (M3 Max, 2026-07-30)

E1c turns every measured lever into data: one `REGISTRY` row per lever in
`crates/voxel-rt/src/variants.rs` (kind, default, range, measured verdict, bench
points) that the **bench sweep**, the **overlay panel** and the **pinning tests**
all read, plus a `RenderQuality` struct with the named tiers. Pascal's
requirement — "keep most of them just to be able to test more things in the
future… but separate them out or make them selectable. I don't like dead code but
I also don't like to remove all the nice research we did" — is met structurally:
losers stay compiled-out but selectable, documented and swept, and they no longer
sprawl through the hot loop.

### Headline table — the quality presets

Per-dispatch **median ms**, each preset dispatched at ITS OWN render scale
(base 2560×1440; the tier knob is a resolution, so a preset table measured at one
size would be fiction). Clean isolated run, p95 within 1–2% of every median.

| preset | render size | A top-down | B top-down low sun | C ground | D ground low sun |
|---|---|---|---|---|---|
| **Potato** (corner AO, hard, fade 15→30 m) | 1792×1008 (0.7) | **2.68** | **3.83** | **2.49** | **2.80** |
| **Quest** (corner AO, hard) | 2048×1152 (0.8) | **3.46** | **4.84** | **3.12** | **3.52** |
| **Balanced** (corner AO, hard — shipped default) | 2560×1440 (1.0) | **5.01** | **6.82** | **4.62** | **5.17** |
| **Beautiful** (RT-AO 2 rays / 8 vox / cosine / falloff, hard) | 2560×1440 (1.0) | **11.69** | **14.59** | **8.52** | **9.37** |

- Balanced reproduces E1b's `ao-corner` row (5.04 / 6.84 / 4.65 / 5.21) within
  0.7%, and Beautiful reproduces `ao-2ray-d8` (11.76 / 14.64 / 8.56 / 9.40)
  within 0.6% — the presets are exactly the configurations E1b measured, now
  named.
- **Potato is 2.2–2.7 ms** and **Quest 3.1–4.8 ms**, i.e. 47–57% of Balanced at
  0.7 scale and 68–74% at 0.8 — close to the pixel-count ratios (0.49 / 0.64)
  plus the fixed per-frame cost, so the render scale behaves as the clean tier
  knob E9 needs.
- Beautiful is the only tier over the plan's ~8 ms target (8.5–14.6). It buys
  RT-AO's *reach*, and the compare confirms the reach: 34–47% of the frame
  differs from Balanced at max channel delta 38–48.
- A second, back-to-back run of this table (immediately after section 2) read
  5–7% high on every column. Run the preset section on an idle machine, as the
  warmup caveat above says.

### No-regression check after the hot-loop extraction

Task 3 moved the loser-variant bodies out of the two coarse DDA loops into named
WGSL functions (`coarse_height_levers` for the column-max refresh + global-max
sky-out + both fast-forwards, `soft_penumbra_update` for the T1 penumbra term,
`ao_distance_fade` for the fade ramp), so the loop body now reads as the
algorithm. **It was free** — with the shipped defaults naga folds the helper down
to the same single compare:

| scenario | recorded (E1b, AO off) | E1c, AO off | delta |
|---|---|---|---|
| A top-down, default sun | 4.723 | 4.723 | 0.0% |
| B top-down, low sun 5° | 6.509 | 6.530 | +0.3% |
| C ground, default sun | 4.391 | 4.379 | −0.3% |
| D ground, low sun 5° | 4.943 | 4.918 | −0.5% |

Section 3's AO rows reproduce equally well (`ao-corner` 5.036 / 6.837 / 4.675 vs
5.039 / 6.837 / 4.649; `ao-2ray-d8` 11.760 / 14.692 / 8.615 vs
11.758 / 14.642 / 8.561), as does section 2's whole ladder (`ao-off`
4.753 / 6.556 / 4.432 / 5.038, `ao-2ray-d8` 11.866 / 14.664 / 8.612 / 9.728,
`ao-4ray-d16` 20.093 / 22.832 / 14.606 / 15.681). **Pixel gate intact: B shows
19 differing pixels, D shows 0**, exactly as recorded. Nothing was reverted.

### Compile-time vs runtime: measured, not assumed

The split now recorded per lever in the registry's `kind` column:

- **Compile-time (unchanged):** all six traversal levers and both mode selectors
  (`AO_MODE`, `SHADOW_MODE`), plus the RT-AO knobs `AO_RAY_COUNT`,
  `AO_MAX_DISTANCE`, `AO_DIRECTION_MODE`, `AO_DISTANCE_FALLOFF`, and the three
  cost-cutting flags. These sit inside the traversal loops or select an
  estimator; folding them away is what the S2 round bought, and E1c does not
  touch it.
- **Runtime (already):** AO strength, penumbra scale, sun azimuth/elevation,
  render scale.
- **Runtime (moved in E1c):** the AO distance-fade ramp bounds, from two shader
  consts to `shading_params.z/w`. **Measured free:**

  | variant | A | B | C |
  |---|---|---|---|
  | `ao-2ray-fade15-30` (runtime uniform) | 5.988 | 8.867 | 8.361 |
  | `ao-2ray-fade15-30-const` (folded literals) | 5.998 | 8.873 | 8.347 |
  | delta | −0.17% | −0.07% | +0.17% |

  Well inside the ±2% rule (the uniform build is nominally *faster* on two of
  three clean scenarios), and the two builds' differing-pixel counts against
  `ao-off` are identical (1 036 316 on D, max delta 47), so the uniform path
  computes the same term. The `-const` row stays in section 3 as the permanent
  evidence for this decision. Consequence: the fade range is a preset field that
  needs no pipeline rebuild — Potato's 15→30 m ramp costs nothing extra to dial.
- **Not moved:** nothing else was even attempted, because the remaining
  compile-time levers are all inside the loops the S2 round optimized. `AO_MODE`
  and `SHADOW_MODE` stay consts on purpose; instant switching is bought with a
  pipeline cache instead (below).

### Preset pipeline cache — startup cost and memory

`DdaPass` keys compiled pipelines by a hash of their shader source and the app
prewarms every preset's permutation in `AppState::new`, so a preset switch is a
hash lookup rather than a mid-frame shader compile.

```
== preset pipeline cache ==
  Potato       2.07ms  (cache holds 2)
  Quest       16.92µs  (cache holds 2)
  Balanced    16.71µs  (cache holds 2)
  Beautiful    1.94ms  (cache holds 3)
  re-prewarm of all 4 presets: 66.83µs (cache holds 3 distinct pipelines)
```

- **Startup cost ≈ 4.0 ms** for the whole preset set: the four named tiers need
  only **3 distinct pipelines** (Quest and Balanced differ by render scale, which
  is not a shader const, and Balanced is already the pipeline `DdaPass::new`
  built), so exactly two extra compiles happen at ~2 ms each. Against 0.5 s of
  world generation this is invisible, and re-prewarming is 67 µs of hashing.
- **Memory:** two extra Metal compute pipelines from the same ~69 KB WGSL
  source; the cache stores only pipelines — the bind group, the bind group
  layout and every brickmap buffer are shared, so nothing per-permutation
  besides the pipeline object itself (not introspectable through wgpu; it is
  dwarfed by the ~30 MB of brickmap buffers). A Custom combination the presets
  do not cover compiles once, then stays cached for the session.

### Verdict

Registry + presets shipped: **Balanced (analytic corner AO + hard shadows at
render scale 1.0) is the default**, and it is byte-for-byte the unpatched
`dda.wgsl` (`balanced_preset_is_the_shipped_baseline`). Every loser stays
selectable with its verdict one hover away, every compile-time lever is swept by
the harness forever, and the hot-loop extraction cost nothing. 54 tests green
(was 30): the new ones pin registry ↔ shader ↔ typed defaults in both
directions, force every settings field to have a row (the destructuring in
`every_settings_field_has_a_registry_lever` stops compiling otherwise), and
compile every lever combination and every preset headlessly.
