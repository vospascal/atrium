# voxel-rt DDA Benchmark — Regression Gate

The permanent perf harness for the voxel-rt renderer. Run it **before and
after any change that could touch traversal cost** — shader edits, brickmap
layout changes, new ray types, world-content changes — and compare against
the recorded baseline below.

```
cargo run -p voxel-rt --example bench_dda --release
```

Runtime ≈ 12–15 min (world gen ~0.5 s, then two sections: 7 traversal
variants and 10 AO variants, each × 4 camera/sun scenarios × 12 timed
batches). No window; needs a real GPU.

## What it measures

Two independent sections, each with its own variant table and pixel compare
(isolation rule — an experiment's numbers never contaminate the gate for the
layer below it):

1. **Traversal levers, AO forced off** — the Stage 2 regression gate. Every
   column here has `ENABLE_AO = false`, so the medians stay directly
   comparable with the pre-E1 baseline recorded below.
2. **E1 AO variants** — the ray-traced ambient-occlusion contenders, built
   through the app's own `AoSettings::shader_source`.

- **Scenarios** (fixed, deterministic poses — seed 1, season 0.0):
  - `A` top-down over the island center, 60 m altitude, default sun
  - `B` same view, sun at 5° elevation — worst case for shadow rays
  - `C` ground level at spawn looking across the island, default sun
  - `D` same view, low sun
- **Variants**: `current` = the shipped shader exactly as in `dda.wgsl`
  (with AO patched off in section 1). Every other traversal column
  string-patches one `ENABLE_*` lever (the "A/B benchmark levers" block at
  the top of `shaders/dda.wgsl`) so each optimization is measured in
  isolation. `stage2-baseline` = all traversal aids off.
- **Timing**: 25 dispatches encoded back-to-back per command buffer,
  wall-clock per batch / 25, 12 batches, median + p95. Variants rotate
  round-robin inside each scenario so GPU clock/thermal drift hits all
  columns equally. (GPU timestamps are NOT used: Metal resolves
  pass-boundary counters to zero once a command buffer holds more than one
  compute pass.)
- **Correctness gate**: section 1 renders the low-sun scenarios (B, D) per
  variant and pixel-compares them against `stage2-baseline`; section 2
  renders the default-sun scenarios (A, C) per AO variant and reports
  differing pixels vs `ao-off` as a *coverage* number (how much of the frame
  AO touches — the images differ by design, so this is not a correctness
  gate). All PNGs land in `target/bench_dda/` for eyeballing.

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
and `stage2-baseline` are always bit-identical to each other.

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
2. Build the feature. If it adds a traversal path, give it an `ENABLE_*`
   lever const in `dda.wgsl` and a matching variant in
   `bench_dda.rs::build_variants()` — `patch_flag` panics if the lever
   string drifts, so the bench and shader cannot silently diverge.
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
   `dda.wgsl`, `build_variants()` patch directions, and this file's
   baseline. Defaults are ALWAYS the fastest measured combination — never
   flip one without a fresh table.

## Standing verdicts (M3 Max — re-measure on other GPUs)

Measured this round, kept as default-off levers because the trade-offs are
architecture-specific (re-run everything on Quest 3 in Stage 6):

- `ENABLE_DISTANCE_SKIP` **on** — the engine of the current numbers
  (17–27% under baseline). Its byte also serves as the occupancy test.
- `ENABLE_GLOBAL_MAX_TERMINATE` **on** — cheap, exact sky-out for upward rays.
- `ENABLE_COLUMN_FAST_FORWARD` / `ENABLE_DESCEND_FAST_FORWARD` **off** —
  superseded by the distance field in all directions (+9–17% if re-enabled).
- `ENABLE_ANY_HIT_SHADOW` **off** — the specialized any-hit loop lost 1–3%
  to plain `trace()` in three separate rounds.
- `ENABLE_BRICK_BIT_GRID` **off** — redundant next to the distance byte;
  standalone it only matched pointer reads. Retry where caches are small.

---

## E1 — Ray-traced ambient occlusion (M3 Max, 2560x1440, 2026-07-30)

Short occlusion rays from each primary hit attenuate the hemisphere-ambient
term only (the sun keeps its own shadow ray). Levers: the "E1 AO levers"
block in `dda.wgsl`; Rust mirror + shader patching in `src/ao.rs`; overlay
section "AO".

### No-regression check (AO off)

`current` with `ENABLE_AO = false` measured against a control run of the
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

`ENABLE_AO = true`, `AO_RAY_COUNT = 2`, `AO_MAX_DISTANCE = 8.0`,
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
