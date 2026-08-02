# voxel-rt DDA Benchmark — Regression Gate

The permanent perf harness for the voxel-rt renderer. Run it **before and
after any change that could touch traversal cost** — shader edits, brickmap
layout changes, new ray types, world-content changes — and compare against
the recorded baseline below.

```
cargo run -p voxel-rt --example bench_dda --release
```

Runtime ≈ 30–40 min (world gen ~0.5 s, then eight sections: 8 traversal
variants, 10 ray-traced-AO variants, 14 E1b/E1c variants, 4 quality presets,
11 E4 CAGI variants — each × 4 camera/sun scenarios × 12 timed batches — E2's
edit storm, 6 variants × 4 patterns, ~2 min, E2b's movement rows, ~5 s, and E6's
8 water variants × 4 water scenarios, ~3 min).
No window; needs a real GPU (except section 7, which is CPU-only).
Trailing section numbers run a subset — `... --release -- 3` measures only the E1b
section, `-- 4` only the preset table, `-- 5` only the CAGI section, `-- 6` only
E2's edit pipeline, `-- 7` only E2b's movement rows, `-- 8` only E6's water
section — and because sections are independent (isolation rule) a subset run
yields exactly the rows a full run would print for it.

## What it measures

Eight independent sections, each with its own variant table and pixel compare
(isolation rule — an experiment's numbers never contaminate the gate for the
layer below it). **Sections 1–3 force CAGI off and E6's water optics off** as
well as pinning AO, so every number recorded before E4/E6 stays directly
comparable:

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
   quote it. Since E4 it also prints the CAGI pass's per-frame cost per tier, so
   the frame total is visible.
5. **E4 CAGI light volume** — the propagation-rule / resolution / sky-test /
   sampling / amortization contenders, plus the memory table, the convergence
   tables (cold start and sun change) and the CPU cross-check of the transport
   rule. Two timing tables: the shading pass's sampling cost and the CA pass's
   own per-frame cost.
6. **E2 world authority, threading & the edit pipeline** — the odd one out: it
   measures a *pipeline*, not a shader, so it reports per-frame cost
   **distributions (median / p99 / max)** instead of medians. Four edit-storm
   patterns (idle / scattered placements / a dense wall / digging whole bricks) ×
   the authority variants, plus build and snapshot costs, the CAGI re-flood
   convergence after an edit, the GPU→CPU readback numbers that decide against a
   GPU-authoritative world, and what one audio-style ray over the CPU mirror
   costs. Runs at `-- 6`; ~2 minutes.
7. **E2b character movement & voxel collision** — the other CPU pipeline, and
   also reported as **distributions**: what one movement + collision step costs
   the frame thread, across the axes its cost actually has (how much open air the
   body's cross-section scans, how often the auto-step fires, how many substeps
   the frame delta forces), plus the ground search that entering walk mode runs.
   No GPU at all, so `-- 7` finishes in ~5 seconds — which is why it is a section
   rather than a number quoted from a test: it is a permanent gate that costs
   nothing to re-run.
8. **E6 water optics** — the four cost tiers (opaque / zero-ray Fresnel tint /
   reflection only / refraction only / both), the bounce budget, the Fresnel ray
   cutoff and the sun-through-liquid lever. It is the one section with its OWN
   world and its OWN scenarios: the island plus a carved debug pool (the natural
   water is 0.6–1.75 m deep, too shallow for extinction or an underwater camera
   to mean anything), and four poses of which two put the camera INSIDE the
   water. Its numbers therefore do not compare with sections 1–5 by
   construction, and the carve leaves those sections untouched.

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

## S3 materials — pattern path re-record (M3 Max, 2560x1440, 2026-08-03)

**Shipped defaults moved.** The pattern field cache and the texel LOD are now **on**
(ledger 6.34), and two fixes landed on top: the `log2` in `pattern_texels_at` became
an exponent-bit read (6.36) and the generator mask compiles out generators the
material table cannot reach (6.35). Every S3 number recorded before this date
describes a renderer that no longer exists.

Read the pattern path as **rung 0 minus rung 11** of the entry probe, not as a raw
column: it subtracts the layers-off floor, so machine drift between runs cancels.

| scenario | pattern path, before | after `log2` fix | after generator mask |
|---|---|---|---|
| A top-down, default sun | 2.218 | 2.073 | **2.003** |
| B top-down, low sun 5° | 2.261 | 2.141 | **1.991** |
| C ground, default sun | 2.215 | 2.061 | **1.999** |
| D ground, low sun 5° | — | 2.057 | **1.939** |

The generator-mask column is **bit-identical** to the unmasked one in all four
scenarios — same differing-pixel count, same max channel delta — because a cleared
bit is a generator no row authors.

### The entry-cost probe (`MATERIAL_PATTERN_ENTRY_PROBE`)

Twelve cumulative rungs, innermost first; `entry-N-<name>` columns in section 9.
Scenario C, before the two fixes:

| rung | ms | Δ | stage |
|---|---|---|---|
| 0 shipped | 4.352 | | |
| 1 no-generator | 3.879 | 0.473 | generator + cache |
| 2 no-fade | 3.865 | 0.014 | `pattern_fade` |
| 3 no-salt | 3.705 | 0.160 | `pattern_variation_salt` |
| 4 no-snap | 3.585 | 0.120 | `pattern_snap_to_texels` |
| 5 no-period | 3.614 | ~0 | the period divide |
| 6 no-tile-frame | 3.468 | 0.146 | the tile branch's **presence** |
| 7 no-frames | 3.165 | 0.303 | voxel/face branches + the integer divide |
| 8 no-drift | 2.399 | 0.766 | see the attribution warning below |
| 9 no-coordinate | 2.406 | ~0 | the hit position |
| 10 no-strength | 2.349 | 0.057 | face mask + gain |
| 11 no-layers | 2.137 | 0.212 | row load + target branch + blend + loop |

Rung 11 landed on the independent `material-patterns-0-layers` column at 2.135 —
**0.001 ms apart**. That closure is what makes the decomposition trustworthy.

> **⚠ Rungs 4 and 8 are ONE item, not two.** A cumulative ladder charges shared work
> to whichever rung removes its last consumer. With the snap stubbed at rung 4, drift
> became the final reader of `pattern_texels_at`, so rung 8 collected the whole
> texel-grid computation. Drift itself is cheap. See ledger 7.20.

Every rung above 0 renders **deliberately wrong output**; only rung 0 passes the
pixel gates, and the default is 0, pinned by `registry_defaults_match_shader_source`.
The rungs are shader consts, so they fold away entirely — the shipped pipeline is
byte-identical to one built without them. Kept in the tree deliberately: they are the
only instrument that can find the next finding of this kind.

### ⚠ Section 5 cannot be fully recorded on this world

`gi-cells2` fails validation — the CAGI volume at 2-voxel cells needs **188 MB**
against wgpu's `max_storage_buffer_binding_size` of **128 MiB** on this adapter. The
run does not stop: the invalid bind group's dispatches are dropped and the column
times at **0.005 ms**, reading as 700× faster than the coarser grid rather than as
broken. The harness now counts GPU errors and prints a banner on stdout (ledger
7.22), but the underlying sweep needs re-sizing before section 5's finest rung means
anything. Pre-existing, and a consequence of the larger post-lattice world.

## Recorded baseline — Apple M3 Max, 2560x1440, 2026-07-30

> **⚠ STALE BY ~5× AS OF 2026-08-02 — DO NOT COMPARE AGAINST THIS TABLE.**
> The generated world changed to one material per authored 1 m block, and each such
> block is exactly one uniform 8³ brick, so the uniform collapse now fires on **100%**
> of occupied bricks and level-1 descent has disappeared from the island. Section 1
> `current` now measures **0.915 / 1.472 / 0.968 / 1.020 ms** against the
> 4.709 / 6.609 / 4.385 / 4.937 below, and scenario B's pixel gate reads **125**
> differing pixels rather than 19. Nothing regressed and nothing was optimized — it
> is a different world. **Every section in this file recorded before that change is
> on the old world and every section after it is on the new one; a full re-record is
> owed.** Details and the diagnosis in the S3 section at the end.

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
- `ENABLE_DIRECTIONAL_SKIP` **off** — AADF: jump the box spanned by six per-axis
  bounds instead of the chebyshev cube. Slower in every scenario (A 5.011 vs
  4.748, B 6.791 vs 6.550, C 4.565 vs 4.408, D 5.055 vs 4.973 ms). The FIELD is
  better — 27,578 empty cells where chebyshev grants reach 0 get a mean 5.19
  cells, mean reach overall 9.10 → 10.82 — but reading it costs more than the
  reach returns: the chebyshev byte doubles as the occupancy test (one load, two
  answers) where a bound is a second load, 2 MB stops being cache-resident where
  500 KB was, and six 5-bit fields cost shifts where a byte costs a compare.
  Retry on Quest: the reach win is hardware-independent, the cache cost is not.
  Adapted from NAADF (Ulschmid et al., CGF 2026, MIT).

Not a lever, and deliberately so:

- **Uniform-brick tag** — a brick that is one material in all 512 cells is hit at
  its entry face with no descent and no level-1 fetch. **100,865 of 100,865 occupied
  bricks qualify — 100%, re-measured 2026-08-02 — taking the CPU brickmap from
  65.1 MB to 7.0 MB (9.3x).** It is no longer a fast path taken often; it is the only
  path the generated island takes, because the world authors one material per 1 m
  block and a block is exactly one 8³ brick. (Previously 57.9% and 45.2 → 21.9 MB, on
  a world that had sub-block detail.) There is
  no `ENABLE_` flag because a collapsed brick has no level-1 slot at all, so a
  shader compiled without the fast path would read its material id as a slot
  index — tag and fast path are one data format, not a toggle, and the only way
  to turn it off is to build different DATA. That is what `bench_dda
  --no-collapse` does (a whole separate run, since every variant in a section
  shares one uploaded brickmap): **scenario A 4.744 ms collapsed against 5.069
  uncollapsed (6.4% faster), scenario C 4.402 against 4.899 (10.1% faster)**,
  taking the minimum of three runs each because the uncollapsed build carries
  more variance and noise only adds. Adapted from NAADF (Ulschmid et al., CGF
  2026, MIT).

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
E4 CAGI levers (details in the E4 section):

- `CAGI_ENABLED` **on** — +0.40–0.51 ms sampling + 0.92–1.52 ms of CA pass at the
  shipped tier, against 2.25–3.55 ms *per ray* for a per-pixel gather. Off is
  byte-identical to E1c.
- **Cell size 4 voxels** (0.5 m, 33 MB) — 8 voxels is the Quest tier (4.3 MB,
  5.8× cheaper), 2 voxels is dead (258 MB, 6× the cost).
- `CAGI_RULE` **1 = diffusion 6** — same cost as the max-decrement flood (the pass
  is bandwidth-bound), better gradient; `2 = diffusion 26` off (2.1–2.7× for a
  mean 0.5/255 — the isotropy fix buys nothing at this world's transport
  distances, keep for E5's point lights).
- `CAGI_SKY_TEST` **0 = column max** — free (reuses binding 8) vs +33–53% for the
  exact upward trace, which disagrees on 33% of the frame at mean 2.1/255.
- `CAGI_SUN_CACHE` **on** — caches the shadow-ray RESULT (not the value): −10 to
  −19% of the CA pass at byte-identical output.
- `CAGI_SAMPLE_MODE` **1 = trilinear** — +0.28–0.35 ms over nearest, which
  otherwise stamps flat 0.5 m patches over 36% of the frame.
- **2 iterations/frame** — 0.44–0.76 ms each; 32 frames (0.53 s) to bit-exact
  convergence after a sun change.
E6 water levers (details in the E6 section):

- `WATER_MODE` **4 = full** (Fresnel reflection + refracted march) — +2.4 ms
  grazing / +4.6 ms on the aerial view with the most water. `1 = fresnel tint`
  (zero secondary rays, analytic sky over the diffuse surface) is the
  Potato/Quest tier at **+0.36–0.74 ms**; the two half-modes exist to attribute
  the cost and stay as documented off-levers.
- `WATER_BOUNCES` **1** — the second interface is FREE above water and **2.35×**
  from inside it looking up, which is also the only place it changes the picture
  (the bed mirrored outside Snell's window). Beautiful ships 2.
- `WATER_SUN_THROUGH_LIQUID` **on** — not optional for the look: off, every
  submerged surface is in shadow and shallow water reads DARKER than the opaque
  water it replaced. Costs +77% on a horizontal underwater view, +8% aerial, ~0
  elsewhere. Off on the zero-ray tiers.
- **Fresnel ray cutoff 0.04** (runtime, `water_params.z`) — −7.1% on the steep
  aerial view for a term worth ≤4% of the pixel; noise elsewhere.
- **Extinction (0.45, 0.12, 0.06) per metre, `F0` = 0.0204, critical angle
  48.607°** — all derived, all pinned against hand computations by test.

E2 world-edit levers (details in the E2 section, all RUNTIME — an edit changes
buffer contents, never a shader):

- `world thread` **on** — the authority owns the brickmap on its own thread. The
  same rare cost (a full clearance rebuild) is a **33 ms frame hitch** inline and
  **1.4 ms of frame work plus 8 frames of latency** threaded.
- `clearance update` **0 = local box** — 258 µs (r=8) against the full rebuild's
  **31.5 ms**, never overestimating and never underestimating by more than the
  freed brick's own new clearance (= 1 for any edit into terrain, i.e. exact).
- `clearance radius` **8 bricks** — 25–48 µs (r=2) / 258–270 µs (r=8) / 1.6–1.7 ms (r=16);
  the radius buys how many cells become *exact*, not safety.
- `re-flood GI on edit` **on** — free per frame, 32 frames (0.53 s) to bit-exact.
  E5 replaces it with a dirty-region flood.
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

**Superseded by the E4 section's preset table (below), which adds the CAGI pass and
therefore the frame TOTAL.** The rows here are the pre-E4 stack — still the right
comparison for the shading pass alone.

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

---

## E4 — CAGI v0: sun + sky flood (M3 Max, 2560x1440, 2026-07-30)

The integer light volume and the cellular automaton that floods it. Levers: the
"E4: CAGI levers" blocks in `shaders/cagi_volume.wgsl` (the half both pass shaders
include) and `shaders/cagi.wgsl` (the CA pass alone); Rust mirrors in `src/cagi.rs`;
GPU resources + dispatch in `src/passes/cagi.rs`; registry rows under the new `Gi`
subsystem. **Section 5** of the harness measures it on TWO axes, because it is two
passes: the shading pass's *sampling* cost (the usual DDA table) and the CA pass's
own *per-frame* cost (a second table). Sections 1-3 now force CAGI **off** so their
numbers stay comparable with everything recorded above (isolation rule).

### No-regression check (CAGI off) — the isolation anchor

Section 1 re-run with `AO_MODE = AO_MODE_OFF` and `CAGI_ENABLED = false`, on an
idle machine:

| scenario | recorded (E1c, AO off) | E4 tree, AO+GI off | delta |
|---|---|---|---|
| A top-down, default sun | 4.723 | **4.706** | -0.4% |
| B top-down, low sun 5°  | 6.530 | **6.517** | -0.2% |
| C ground, default sun   | 4.379 | **4.359** | -0.5% |
| D ground, low sun 5°    | 4.918 | **4.913** | -0.1% |

**Pixel gate intact: B shows 19 differing pixels (max channel delta 97), D shows 0,
`with-descend-ff` still 12** — exactly the recorded tie set. So splitting `dda.wgsl`
into `world.wgsl` (the shared traversal core the CA pass also compiles) +
`cagi_volume.wgsl` + `dda.wgsl`, adding three bindings and raising the
storage-buffer limit changed neither the math nor the cost. With the lever off the
renderer is the E1c renderer, bit for bit, and the volume shrinks to a 12-byte
placeholder buffer. The only measurable residue is **0.06 ms/frame** for the empty
CAGI compute pass, which is opened even at zero iterations so the overlay's `CAGI:`
readout cannot go stale.

### Design decisions and their numbers

**Volume resolution — 4 voxels (0.5 m cells) shipped, 8 voxels for Quest, 2 voxels
dead.** The vertical extent is clamped to the world's occupied height + 2 cells
(the island's tallest brick is row 21, so 44 cells instead of 64 at 4 voxels — a
31% saving on every buffer, and everything above is open sky by definition, which
the sampler and the CA both treat as a constant).

| cell voxels | grid | cells | one buffer | ping-pong | + attributes = total | CA ms/frame (2 it) | CPU attr build | look vs 0.5 m |
|---|---|---|---|---|---|---|---|---|
| 2 (0.25 m) | 500x86x500 | 21.5 M | 86.0 MB | 172.0 MB | **258.0 MB** | 5.83-7.94 | 53 ms | 71% of frame, mean 7.8/255 |
| **4 (0.50 m)** | 250x44x250 | 2.75 M | 11.0 MB | 22.0 MB | **33.0 MB** | **0.92-1.52** | 48 ms | — (shipped) |
| 8 (1.00 m) | 125x23x125 | 359 K | 1.4 MB | 2.9 MB | **4.3 MB** | 0.26-0.42 | 79 ms | 46% of frame, mean 4.2/255 |

17.5-17.8% of cells are absorbers at every resolution — the surface shell of the
island, which is also the share of cells that can become sun-bounce candidates. The
CPU attribute build is bounded by OCCUPIED BRICKS x 512 voxels, not by the cell
count, which is why it barely moves with resolution.

> **⚠ OPEN REGRESSION, measured 2026-08-02 — this cost is now ~10-17x what is
> recorded below and the "~50 ms hitch" claim is no longer true.**
>
> | cell voxels | recorded | 2026-08-02 |
> |---|---|---|
> | 2 (0.25 m) | 53 ms | **946 ms** |
> | 4 (0.50 m), shipped | 48 ms | **827 ms** |
> | 8 (1.00 m) | 79 ms | **801 ms** |
>
> A preset switch between Quest and Balanced is therefore a **~0.8 s stall**, not a
> one-off 50 ms hitch. The absorbing share moved with it, 17.5-17.8% -> 25.8-27.5%.
>
> **It is not the uniform collapse.** `bench_dda --no-collapse 5` is *slower* still
> (1.06 s / 947 ms / 923 ms), so synthesising the uniform brick's words is helping,
> not hurting. What changed is the world: every occupied brick is now fully solid, so
> the loop body runs on **100,865 x 512 = 51.6 M** occupied voxels with no partial
> bricks to `continue` past, and each one calls `exposed_face_weight`, which probes
> six neighbours through `brickmap.get`. That accounts for a large factor but not
> obviously all of it, so **the cause is localised, not concluded.**
>
> Owed: profile the loop body, and check whether `exposed_face_weight` can be
> answered from the brick's own occupancy words for interior voxels instead of six
> full `get` calls — on a fully-solid world nearly every probe returns "solid" and
> the answer is zero. This is a startup/preset cost, not a per-frame one, which is
> why it has not been visible in any frame-time table.
>
> **And it is single-threaded, on a loop that needs no synchronisation to
> parallelise.** The engine has exactly one worker thread (the E2 world authority in
> `world_host.rs`) and **no data parallelism anywhere** — no `rayon`, no `par_iter`,
> not in the dependency list. This loop writes cells indexed by
> `(brick * BRICK_SIZE + local) / cell_voxels`, and every shipped `cell_voxels`
> (2, 4, 8) divides `BRICK_SIZE = 8`, so **each brick maps to a disjoint set of
> cells** — at the shipped tier brick_x writes only cells `brick_x*2` and
> `brick_x*2 + 1`. No two bricks touch the same cell, and `exposed_face_weight` only
> reads neighbours. So it parallelises over bricks with **no locks, no atomics and no
> per-thread merge**.
>
> Two caveats before anyone builds it: the disjointness above is established by
> reading the indexing, **not** by a test, so it needs one; and it breaks the moment
> `cell_voxels` exceeds `BRICK_SIZE`, which the assertion should say out loud.

2 voxels is rejected on both axes at once: 258 MB against the brickmap's own ~30 MB,
and 5.8-7.9 ms per frame *for the CA alone*. 8 voxels is the Quest configuration —
5.8x cheaper than the shipped tier and 7.7x smaller, at a mean 4.2/255 coarser look
— which answers the "the Quest tier must be able to run *some* configuration"
requirement with room to spare.

**Format — RGB 10:10:10 in one u32, saturating, with two flag bits.** `bits 0..9`
red, `10..19` green, `20..29` blue, `bit 30` = "this cell's shadow ray already found
the sun" (the amortization flag), `bit 31` free. 1023 = linear radiance 1.0.
10:10:10 over 8:8:8 is free — both fit one u32, so the byte cost is identical — and
the extra two bits per channel give the diffusion rule's integer division 4x the
headroom before rounding shows up as banding over a long flood. All transport is
`u32`: no float accumulates anywhere in the volume, which is what makes it
deterministic and noiseless (verified below).

**Propagation rule — 6-neighbour diffusion shipped; the A/B is a look choice, not a
cost choice.** Both contenders read 6 neighbours and the pass is bandwidth-bound, so
they measure **the same**:

| rule | CA ms/frame (A/B/C/D, 2 iterations) | vs shipped rule (scenario C) |
|---|---|---|
| **diffusion 6 (shipped)** | **0.97 / 1.52 / 0.92 / 1.52** | — |
| max-decrement flood | 0.92 / 1.53 / 0.95 / 1.52 | 66% of frame, mean 8.8/255, max 71 |
| diffusion 26 | 2.64 / 3.25 / 2.67 / 3.27 | 26% of frame, mean 0.5/255, max 9 |

- **Diffusion wins on look for free.** The max-decrement flood is visibly flatter
  and brighter in shade — a straight-line falloff parks many cells at the same
  level — where diffusion's equilibrium is a discrete Laplace solution and gives a
  gradient. Both exclude the cell's own previous value, so both can *darken*, which
  is what lets a sun change converge instead of leaving stale light behind.
- **The 26-neighbour isotropy fix is REJECTED on terrain: 2.1-2.7x the cost for a
  mean 0.5/255.** Worth stating why, because the dossier lists anisotropy as a known
  compromise: the max-decrement rule genuinely is anisotropic (its iso-surfaces are
  L1 balls, so a diagonal loses sqrt(3)x more reach than an axis), and 6-neighbour
  diffusion is only isotropic asymptotically — but on this world sky light is
  injected *everywhere above the terrain*, so transport distances are 1-3 cells,
  far too short for the front's shape to be visible. Kept as a documented off-lever
  for E5, where a lantern makes transport distance large and the front shape is the
  whole look.

**Injection — sky by the column-height buffer, sun by one cached shadow ray.**

- **Sky: the existing per-column max-brick-Y buffer (binding 8) is sufficient, and
  it is free.** A cell is a sky source when its brick row sits above its XZ column's
  highest occupied brick — one load of data the traversal already owns, no ray. The
  exact alternative (a real upward trace per cell) costs **+33-53% of the CA pass**
  (1.47-2.07 vs 0.92-1.52 ms) and the two disagree on **33% of the frame at a mean
  of 2.1/255, max 59**: the cheap test is quantized to the 1 m brick column, so a
  cell beside a tree trunk shares the trunk's column and reads "covered" until
  diffusion carries light back in. Verdict: keep the free test (the diffusion fills
  most of it back), with the exact trace as a documented lever for dense-canopy
  scenes.
- **Sun: one shadow ray per candidate cell, through the shared
  `trace_shadow_visibility`.** Candidates are air cells touching a solid cell whose
  face normal has a positive Lambert term toward the sun — the surface shell of the
  world, a few percent of the volume — and the injected colour is the mean albedo of
  those solid neighbours (from the static attribute buffer) times the sun radiance
  times `sun_bounce`. This is what makes the bounce *coloured*: green under grass,
  warm over sand.
- **Amortization: the flag caches the ray RESULT, not the value.** Bit 30 says "this
  cell already proved it sees the sun"; on later iterations the ray is skipped while
  the cell still propagates and still recomputes its (cheap) bounce colour. Saves
  **10% (default sun) to 19% (low sun)** of the CA pass at **byte-identical output —
  0 differing pixels against re-tracing, in both scenarios**. The first
  implementation cached the cell's *value* instead and measured a real defect: source
  cells froze at their injected level and lost the diffusion they should also
  receive, on 26% of the frame (mean 0.6/255, **max 38**). Recorded because it is the
  kind of "obvious" CA optimization that quietly costs quality.

**Iterations per frame — 2 shipped, cost is linear.** 1 it = 0.52-0.77 ms, 2 =
0.92-1.52, 8 = 3.59-5.87 (≈0.44 ms per iteration at default sun, 0.76 at 5°: the
low sun leaves more candidate cells un-lit, and an un-lit candidate cannot cache its
ray, so it re-traces every iteration — the one asymmetry left in the amortization).

**Shading integration — `indirect = CAGI_sample * ambient_occlusion`, per the
documented contract.** The sample position walks out along the hit normal to the
first non-solid cell (up to 3), so nothing is ever sampled inside an absorber;
trilinear then renormalizes its weights over the non-solid taps so a wall's interior
(always 0) cannot bleed darkness onto the surface in front of it. The direct sun
term and its shadow ray are untouched.

| sampling | DDA pass ms (A/B/C/D) | cost over CAGI off | vs trilinear (C) |
|---|---|---|---|
| off | 5.011 / 6.783 / 4.623 / 5.156 | — | — |
| nearest | 5.159 / 6.952 / 4.742 / 5.278 | +0.12 to +0.17 | 36% of frame, mean 2.9/255, max 76 |
| **trilinear (shipped)** | **5.509 / 7.293 / 5.019 / 5.580** | **+0.40 to +0.51** | — |

Nearest sampling stamps flat 0.5 m patches of indirect light onto surfaces — plainly
visible in the crops — so trilinear's +0.28-0.35 ms is the price of the volume not
advertising its own grid. Nearest stays as the Quest lever.

### Steady-state and cold-start cost

Per-frame totals at the shipped Balanced tier (shading pass + CA pass, both from the
clean preset run):

| preset | shading ms (A/B/C/D) | CAGI ms (A/B/C/D) | **frame total** |
|---|---|---|---|
| Potato (GI off) | 2.688 / 3.803 / 2.443 / 2.787 | 0.06 (empty pass) | **2.75 / 3.86 / 2.50 / 2.85** |
| Quest (1 m cells, 2 it) | 3.798 / 5.113 / 3.337 / 3.717 | 0.263 / 0.414 / 0.264 / 0.414 | **4.06 / 5.53 / 3.60 / 4.13** |
| **Balanced (0.5 m cells, 2 it)** | 5.467 / 7.282 / 5.002 / 5.563 | 0.957 / 1.524 / 0.962 / 1.524 | **6.42 / 8.81 / 5.96 / 7.09** |
| Beautiful (RT-AO, 0.5 m, 4 it) | 12.233 / 15.116 / 8.998 / 9.815 | 1.833 / 2.994 / 1.829 / 2.990 | **14.07 / 18.11 / 10.83 / 12.81** |

- **Balanced is 5.96-7.09 ms on the three player-facing scenarios and 8.81 ms on B**
  (top-down + 5° sun, the shadow-ray worst case) — i.e. the ~8 ms target holds
  everywhere except the aerial low-sun view, which is 10% over. The levers to close
  it are already measured: 1 iteration (-0.7 ms), nearest sampling (-0.34), or the
  Quest cell size (-1.1).
- **Cold start is not a spike.** A full flood from an empty volume is the same
  0.46-0.76 ms per iteration as the steady state (the per-iteration cost is dominated
  by the 6-neighbour bandwidth, not by injection), so a re-flood costs nothing extra
  per frame — it costs FRAMES, which is the convergence table.
- Beautiful is now 10.8-18.1 ms. RT-AO remains its dominant cost; E4's arrival is
  the argument for demoting it (see the plan's E1b note), which the look gate decides.

### Convergence (scenario C/D, 2 iterations per frame = 60 fps)

Differing pixels against the fully converged image of the same scenario:

| iterations | frames @2/frame | cold start: differing (%) / max delta | sun change: differing (%) / max delta |
|---|---|---|---|
| 0 | 0 | 0 (empty volume renders the floor only) | 20.10% / 71 |
| 1 | 0.5 | 32.38% / 44 | 32.34% / 59 |
| 2 | 1 | 31.61% / 38 | 31.50% / 29 |
| 4 | 2 | 28.62% / 26 | 28.51% / 18 |
| 8 | 4 | 21.04% / 11 | 20.71% / 9 |
| 16 | 8 | 10.13% / 3 | 8.09% / 3 |
| 32 | 16 | 0.22% / **1** | 0.20% / **1** |
| 64 | 32 | **0** | **0** |
| 128 | 64 | 0 | 0 |

**Full convergence at 64 iterations = 32 frames = 0.53 s at 60 fps; visual
convergence (max channel delta 1) at 16 frames = 0.27 s.** The gate's "sun-drag
re-floods in ~1 s" is met with margin, and dragging the slider re-floods every frame
of the drag, so the GI tracks the drag rather than lagging behind it. The high early
percentages are the *whole indirect term* fading in at once (a cleared volume is
black everywhere), not a localized artifact — the max delta collapses from 44 to 3
within 8 frames.

### Correctness: the CPU cross-check

The harness reads the volume back, runs one more GPU iteration, and predicts every
purely-propagating cell (source cells excluded exactly as the shader identifies them:
sky by the column test, sun by the pinned flag) with `cagi::propagate_reference` —
the same integer arithmetic reimplemented on the CPU:

```
Diffusion6     181928 propagating cells checked, 0 mismatches; deterministic re-flood: yes
MaxDecrement   181928 propagating cells checked, 0 mismatches; deterministic re-flood: yes
Diffusion26    181928 propagating cells checked, 0 mismatches; deterministic re-flood: yes
  solid cells holding light: 0 (must be 0); channels over 1023: 0 (must be 0)
```

All three rules match the CPU reference exactly, a re-flood from scratch reproduces
the volume **bit for bit** (the "same inputs give the same volume" requirement), no
absorber holds light, and no channel ever saturates. This is the evidence for the
"noiseless and deterministic" claim: not a visual impression but an integer identity.

### The dossier's compromise checklist — findings

Crops in `target/bench_dda/` (`crop_{region}_{scenario}_{variant}.png`, 3x nearest
zoom over three fixed regions of the ground-level views: `near-ground`,
`mid-terrain`, `canopy-shade`).

1. **Multi-frame propagation latency — PRESENT, and it is the cheapest compromise to
   live with.** 32 frames to bit-exact convergence, 16 to visually converged. Because
   E4's world is static, latency is only ever paid on a sun move, and the sun is a
   slider. It becomes a real design question at E5 (a placed lantern must not take
   half a second to light its room), where the answer is a dirty-region flood at a
   higher local iteration count rather than a global one.
2. **Grid-direction anisotropy — NOT VISIBLE on this world, but structurally real.**
   No axis-aligned crosses or diamonds appear in any crop, and the 26-neighbour
   stencil (the fix) changes the image by a mean 0.5/255. Reason: sky injection
   covers every cell above the terrain, so light travels 1-3 cells before it is
   sampled — an anisotropic *front* never gets far enough to form. The max-decrement
   rule's L1 iso-surfaces are the honest negative here: they are octahedral by
   construction, and the rule is only free of visible artifacts for the same reason.
   **Re-test at E5 with a point source**; that is where anisotropy will show, and
   where `diffusion 26` earns its 2.7x.
3. **Over-diffusion / "glowing walls" — NOT PRESENT, structurally prevented, at the
   cost of a documented approximation.** Solid cells are written to 0 every iteration
   and the trilinear sampler drops solid taps, so a wall never brightens its own
   surface. The approximation that buys this is the absorption model: a cell absorbs
   completely once a quarter of its voxels are occupied (16 of 64 at 0.5 m cells,
   i.e. exactly one voxel layer), and passes light freely below that. That threshold
   is deliberate — with "any occupied voxel" a single grass tuft would seal a cell
   and the flood would never reach the ground it is supposed to light.
4. **Thin-wall / thin-geometry leaks — PRESENT AND EXPECTED, bounded by the same
   threshold.** A one-voxel wall that straddles a cell boundary puts 8 of 64 voxels
   in each of two cells, i.e. 12.5% each, and both cells then transmit: light passes
   through. On the island this shows as canopy that is more translucent than the
   hard-shadowed ground under it implies (`crop_canopy-shade_*`) — leaves are sparse
   per cell, so the volume treats a crown as a partial absorber while the sun's own
   shadow ray treats every leaf voxel as opaque. The result reads as soft
   light-through-foliage rather than as an artifact, which is why it ships, but it IS
   the leak the dossier predicts, and it is resolution-bound: 1 m cells leak more,
   0.25 m less.
5. **Weak long-distance transport — PRESENT BY DESIGN, and calibrated per meter.**
   Both rules are tuned in per-METER units so the resolution lever cannot change the
   physics: the max-decrement reach is 1023/80 ≈ 12.8 m at every cell size, and the
   diffusion transmission is 0.884/m. Beyond that a cell's light comes only from its
   own sky visibility, so deep interiors go black — which is why the shipped
   configuration keeps 25% of the E1c hemisphere ambient as a readability floor
   (`gi_ambient_floor`). Honest framing: the volume is a *local* light transport, and
   the floor is the admission of that, not a bug fix.
6. **Loss of high-frequency directional information — PRESENT, and it is exactly the
   division of labour the dossier describes.** The volume is direction-less (one RGB
   per cell), so it contributes no directional shading; sharpness comes from the
   sun's own ray and contact detail from analytic corner AO. Where nearest sampling
   exposes the volume's resolution (36% of the frame, max 76), trilinear hides it.

Net: **CA GI is viable for us.** Two of the six compromises do not appear at this
world's transport distances, three are bounded and documented, one (latency) is
inherent and cheap while the world is static. The cost is 0.92-1.52 ms of CA plus
0.40-0.51 ms of sampling for a term that would cost 2.25-3.55 ms *per ray* to gather
per pixel — and it is noiseless, which per-pixel gathering at any affordable ray
count is not.

### Chosen defaults (pending Pascal's visual gate)

`CAGI_ENABLED = true`, 4-voxel cells, `CAGI_RULE = 1` (diffusion 6),
`CAGI_SAMPLE_MODE = 1` (trilinear), `CAGI_SKY_TEST = 0` (column max),
`CAGI_SUN_CACHE = true`, 2 iterations/frame, strength 1.0, ambient floor 0.25, sun
bounce 0.35. Per-tier: **Potato off · Quest 1 m cells x 2 it · Balanced 0.5 m x 2 it
· Beautiful 0.5 m x 4 it.**

---

## E2 — World authority, threading & the edit pipeline (M3 Max, 2026-07-30)

The ARCHITECTURE experiment, so the deliverable is a verdict, not a number: which
side owns the voxel world, on which thread, and how the GPU and the audio mirror
learn what changed. Levers: the four `WorldEdit` rows in `src/variants.rs`
(all runtime). Code: `src/brickmap.rs` (`set_voxel` + the derived-structure
repairs), `src/world_edit.rs` (the delta), `src/world_host.rs` (the authority and
its thread), `src/voxel_dda.rs` (the CPU traversal picking and E8's audio rays
share), `passes/world_bindings.rs` + `passes/cagi.rs` (the delta upload).
**Section 6** of the harness measures it.

**Machine caveat for this section:** the storm was run on a laptop under real load
(load average ~7, a VM at 57% CPU). That inflates absolute frame times a few
percent, but the E2 verdict rests on *within-run* comparisons — variant A and
variant B measured back to back on the same machine — and on *maxima*, which the
load can only push in the direction that makes the winner look worse.

### The three variants, and what actually decided it

| | (A) CPU-authoritative, synchronous | (B) CPU-authoritative + world thread | (C) GPU-authoritative |
|---|---|---|---|
| edit latency (input → uploaded) | **0.04–0.11 ms** median | 6.5–7.9 ms median (**1 frame**) | ≥ 1 frame to dispatch, plus a readback per mirror refresh |
| worst frame the pipeline can cause | **33.3 ms** (full clearance rebuild) / 0.5 ms (shipped strategy) | **1.4 ms** | unmeasured for edits, but every mirror refresh costs 1.29 ms of blocked readback |
| CPU mirror freshness for E8 | exact, always | exact, always (one `RwLock` read) | **7–10 submit/poll cycles behind**, or 1.29 ms of stall per refresh |
| CPU / GPU memory | 46.4 / 46.4 MB | 46.4 / 46.4 MB | GPU 46.4 MB + a mirror the CPU still has to hold |
| complexity | least | one thread, one channel, owned deltas | edit compute shader + free-list on GPU + delta readback + epoch tracking |

**Verdict: (B) CPU-authoritative + world thread ships.** (A) stays as the
`world thread` off-lever — it is *better* on latency and is the right variant for
a Quest tier that cannot afford a second core, but it cannot bound its worst
frame, and every future off-frame job (E3's generation, B8's streaming, B6's CA
simulation) would land inside a frame there. (C) is rejected on numbers below.

### Why (C) — GPU-authoritative — loses, in one table

The decisive quantity is not bandwidth, it is **round-trip latency**, and it is
flat:

| mirror | bytes | blocked round trip | effective GB/s |
|---|---|---|---|
| one brick's occupancy words (an edit's delta) | 64 | **1.295 ms** | 0.00 |
| brick occupancy bit grid (1 bit/brick) | 62 500 | **1.296 ms** | 0.05 |
| voxel occupancy words (1 bit/voxel, occupied bricks) | 4 866 368 | **1.293 ms** | 3.76 |
| occupancy + materials (the level-1 mirror audio wants) | 43 797 312 | **1.299 ms** | 33.72 |

- **A 64-byte readback costs the same 1.29 ms as a 43.8 MB one.** The cost is the
  submit → map → poll round trip, not the copy. So "read back only the delta" —
  the obvious way to keep a GPU-authoritative mirror cheap — buys *nothing*: the
  per-edit readback is as expensive as re-reading the entire world.
- **1.29 ms is 16–22% of the shipped frame budget** (5.96–8.81 ms), paid per
  refresh, forever, on a thread that must not stall.
- **Non-blocking mapping does not fix it, it converts it into staleness:** with
  the CPU never blocking, the 4.9 MB mirror became readable after **7–10
  submit-and-poll cycles** (two runs). So a GPU-authoritative world hands atrium's resolver a
  mirror that is structurally several frames behind — where B hands it the
  authority itself.
- And the mirror is not optional: E8's `VoxelDdaResolver` needs occupancy on the
  CPU, so C pays for the CPU copy *and* the GPU copy *and* the synchronization,
  while B pays for one copy that is authoritative by construction.
- For scale on the other side of the trade: one CPU occlusion ray over the mirror
  costs **0.94 µs** and a full reflection cast (hit voxel + face + material)
  **0.96 µs** — 4096 rays in 3.9 ms. The CPU mirror is not a burden to query; it
  is the cheapest part of the audio bridge.

C is therefore **dead for the world's authority**, and the reason is worth
keeping: it is not "readback is slow", it is "readback latency is
size-independent, so no delta scheme can amortize it". GPU-authoritative
*derived* data whose only consumer is the GPU (E3's generation writing bricks,
E4's light volume) is unaffected — that is a different question and E3 owns it.

### Deviation from the plan's threading sketch, with the number

The plan proposed publishing **immutable `Arc<Brickmap>` snapshots**. Measured, a
deep copy of the brickmap is **4.59–4.95 ms for 46.4 MB** — per published edit,
because a snapshot cannot be mutated in place while a reader holds it. Against
that, the *whole delta of a typical edit is 14 bytes of upload and 0.3 µs of CPU*.

So the shipped design keeps ONE brickmap behind an `RwLock` and publishes owned
**deltas** instead:

- the **render thread never locks** — it drains `WorldDelta`s (owned word
  payloads) from a channel and writes them into the GPU buffers;
- **readers** (picking today, E8's resolver on its background thread) take a read
  lock, uncontended except for the microseconds an edit holds the write lock;
- the mirror is the authority, so freshness is not a design problem at all.

Snapshot swapping stays the right answer if a reader ever needs a *stable* world
across many milliseconds (a save, a network snapshot); it is the wrong answer for
"the audio thread wants to know what the world is now".

### Memory

```
CPU brickmap (the authority AND the audio mirror):     46.4 MB
GPU world buffers:                                     46.4 MB
GPU CAGI light volume:                                 33.0 MB
of which edit headroom (4096 spare brick slots):        2.4 MB (CPU and GPU each)
71941 occupied bricks, 0 free slots, capacity 76037
```

**Edit headroom is the memory price of the whole experiment: 2.4 MB per side,
5.2% of the brickmap.** The level-1 arrays carry 4096 spare brick slots so
materializing a brick patches words that already exist instead of reallocating
46 MB of buffers. Outgrowing it is handled (`BrickmapEdit::arrays_grew` → a full
re-upload) and never happened in any storm.

**Fragmentation: there is none to manage.** Every slot is exactly 16 + 128 words,
so a freed slot fits any future brick exactly — no compaction, no coalescing, no
best-fit search can help. Freed slots are reused LIFO before the headroom is
touched, so a dig-and-rebuild session consumes no headroom at all; the only waste
is slack at the end of the arrays, bounded by the peak simultaneous brick count.

### Build and repair costs (CPU)

```
full brickmap build (world -> every derived structure):    89.5 ms
DEEP COPY of the brickmap (the snapshot-swap alternative):  5.4 ms for 46.4 MB
clearance repair on a FREED brick, local box r=2           24.7 us  (592 B delta)
clearance repair on a FREED brick, local box r=8          269.9 us  (592 B delta)
clearance repair on a FREED brick, local box r=16           1.72 ms (592 B delta)
clearance repair on a FREED brick, full rebuild            32.8 ms  (500 000 cells, 500 588 B delta)
```

The ~86–90 ms build is the bench's *second* build with another 46 MB brickmap
already resident (first-touch page faults on fresh arrays, on a loaded machine);
the app's startup build is the recorded 61.7 ms. Either way it is a startup cost,
and on the world thread it stops being a frame cost.

### The distance field: the add/remove asymmetry, and the strategy chosen

This is the part of an edit that is genuinely hard, and it is asymmetric:

- **Adding solid only SHRINKS clearance**, and the new field is exactly
  `min(old, chebyshev distance to the new brick)`. Implemented as an expanding
  chebyshev shell walk that stops at the first shell it fails to improve — an
  **exact** early-out, not a heuristic (a cell at shell k+1 can only improve if
  its neighbour toward the new brick, at shell k, also improved). Measured cost:
  ~106 cells written per materialized brick, inside a 2.8 µs median edit.
- **Removing solid can GROW clearance arbitrarily far away** (a lone brick in open
  air is the nearest occupied brick for a huge region), so there is a real choice.

**Chosen: bounded local recompute, radius 8 bricks.** An exact chamfer transform
over the box around the freed brick, seeded from the ring one cell outside it.
The reason it is safe at any radius, and the reason it is *exact where it matters*:

- the seeds are the OLD distances, which after a removal are ≤ the new exact
  distances, and the chamfer can only produce `min(seed + path, occupied inside
  the box)` — so **every value written is ≤ the exact new value**. An
  underestimate is harmless by construction (a cell at distance d claims a
  guaranteed-empty cube of half-width d−1; a smaller d claims less and costs only
  DDA steps). An overestimate would tunnel through geometry and cannot happen.
- the error is bounded by **D, the freed brick's own new clearance**, uniformly and
  *independently of the radius*: `old ≤ local ≤ exact ≤ old + D`. For any edit into
  terrain the freed brick still has occupied neighbours, so **D = 1 and the update
  is exact**. A large D only happens for an isolated brick in open air, where a
  one-cell-low clearance costs one extra step.
- both properties are unit-tested against a brute-force full transform
  (`an_isolated_brick_recycles_its_slot_and_leaves_a_safe_clearance_field` asserts
  the safety direction AND the D bound;
  `edits_keep_every_derived_structure_consistent` asserts *exactness* for terrain
  edits).

**The full rebuild was implemented and measured, and it is the losing strategy for
a reason worth recording: 31.5 ms and 500 KB of upload per freed brick, i.e.
122× the cost of the bounded box for a difference that is invisible on terrain.**
It stays as an off-lever because it is the correctness reference, and because it is
the exact case that proves the threading verdict (below).

### Edit storm — the hitch table

Four patterns: `idle` (no edits — the regression anchor), `scatter-place` (256
stone voxels 3 m above the terrain at scattered positions, so almost every edit
MATERIALIZES a brick), `wall-place` (a 16×16 wall built voxel by voxel — the
gate's "hold-to-place blocks" case), `dig-bricks` (1092 removals that clear four
brick-aligned surface bricks completely — the only pattern that FREES bricks and
therefore the only one the clearance lever can move). 4 edits/frame (16 for the
dig) against a human's 8 per *second*.

**Frame cost the pipeline adds on the frame thread (ms).** One representative run
(the harness convention); the section was run three times and the *sub-millisecond*
frame-thread numbers move by up to ±0.4 ms with machine load, while the two figures
the verdict rests on — the **33.3 ms** inline hitch and the **1.36 ms** threaded
residual — reproduced to 0.1% every time:

| variant / pattern | idle | scatter-place | wall-place | dig-bricks |
|---|---|---|---|---|
| **edit-shipped** (B, local box) median | 0.000 | 0.130 | 0.065 | 0.127 |
| **edit-shipped** p99 / **max** | 0.003 | 0.254 / **0.254** | 0.123 / **0.123** | 0.246 / **0.246** |
| edit-inline (A, local box) median | 0.000 | 0.170 | 0.050 | 0.115 |
| edit-inline p99 / **max** | 0.000 | 0.513 / **0.513** | 0.162 / **0.162** | 0.423 / **0.423** |
| edit-clearance-rebuild (B, full) p99 / **max** | 0.001 | 0.249 / 0.249 | 0.177 / 0.177 | 1.356 / **1.356** |
| **edit-inline-clearance-rebuild (A, full) p99 / max** | 0.000 | 0.505 / 0.505 | 0.286 / 0.286 | **33.3 / 33.3** |
| edit-clearance-radius16 (B, r=16) max | 0.001 | 0.335 | 0.284 | 0.349 |
| edit-no-reflood (B, no GI response) max | 0.000 | 0.291 | 0.081 | 0.185 |

**Whole frame, blocked (edit pipeline + this frame's CAGI iterations + one shading
dispatch at 2560×1440), ms:**

| variant / pattern | idle median / max | scatter median / max | wall median / max | dig median / max |
|---|---|---|---|---|
| **edit-shipped** | 6.41 / 14.64¹ | 7.78 / 7.99 | 6.44 / 7.73 | 6.54 / 7.90 |
| edit-inline | 6.35 / 7.75 | 7.83 / 8.20 | 6.41 / 7.97 | 6.50 / 7.94 |
| edit-clearance-rebuild (B) | 6.41 / 7.66 | 7.80 / 7.99 | 7.49 / 7.91 | 6.58 / **9.34** |
| **edit-inline-clearance-rebuild (A)** | 6.40 / 7.72 | 7.74 / 8.18 | 7.55 / 7.85 | 7.64 / **41.07** |

¹ the first timed frame of the whole section, i.e. shader/pipeline warmup — every
other `idle` row reads 7.7 ms max.

**Per-edit CPU cost and upload bytes:**

| pattern | apply median | apply p99 | apply max | upload / edit | brick allocs | brick frees | clearance cells |
|---|---|---|---|---|---|---|---|
| scatter-place | 2.8 µs | 220 µs | 337 µs | **1689 B** | 237 | 0 | 25 148 |
| wall-place | **0.3 µs** | 3.5 µs | 21.8 µs | **14 B** | 6 | 0 | 15 |
| dig-bricks (local box) | 0.3 µs | 2.0 µs | 349 µs | **14 B** | 0 | 4 | 4 |
| dig-bricks (full rebuild) | 0.3 µs | 1.8 µs | **47 970 µs** | 1846 B | 0 | 4 | 2 000 000 |

**Edit → uploaded latency:**

| variant | median | max | worst case in frames | tail frames |
|---|---|---|---|---|
| edit-shipped (B) | 6.5–7.9 ms | 8.1–8.2 ms | **1** | 1 |
| edit-inline (A) | 0.04–0.11 ms | 0.16–0.40 ms | **0** | 0 |
| edit-clearance-rebuild (B), dig | 7.7 ms | **51.4 ms** | 8 | 8 |
| edit-inline-clearance-rebuild (A), dig | 0.10 ms | 32.7 ms | 0 | 0 |

### What those numbers mean

1. **The gate is met with two orders of magnitude of margin.** Building a wall by
   holding the button costs **0.3 µs of CPU and 14 bytes of upload per voxel**, and
   the frame-thread cost of a 4-edits-per-frame storm is 0.065 ms median /
   0.123 ms max — 1.5% of a 60 fps frame. There is no hitch to find at human edit
   rates; the storm had to run 30–120× faster than a human to produce a
   measurable line at all.
2. **The threading verdict is the `dig-bricks` row pair, and nothing else.** With
   the cheap clearance strategy, A and B are indistinguishable (max 0.42 vs
   0.25 ms). Force the expensive one and the SAME work becomes a **33.3 ms frame
   hitch inline (41.1 ms whole frame — two and a half dropped frames)** versus
   **1.356 ms of frame work plus 8 frames of latency** threaded. That is the whole
   architecture argument: the world thread does not make edits cheaper, it makes
   the *worst* edit not a frame problem. Since E3 (GPU/CPU generation), B6 (CA
   simulation) and B8 (streaming) all add work of exactly that shape, the seam has
   to exist before they do.
3. **Threading's residual cost is 1.356 ms, and it is the upload, not the edit.**
   The full-rebuild delta is 500 KB of clearance bytes, and `write_buffer` runs on
   the frame thread by definition. Uploads are the one part of an edit that cannot
   move off-frame — which is another argument for the bounded local update, whose
   delta is 592 bytes.
4. **Latency is A's win, and it is small enough to ignore.** B costs exactly one
   frame (median 6.5–7.9 ms = one frame time, max 8.2 ms, worst case 1 frame): the
   edit is applied while the frame it was requested in is rendering, and uploaded at
   the top of the next one. A places it in the same frame (0.04–0.11 ms). One frame
   of placement latency is not perceptible; a 33 ms hitch is.
5. **Scattered mid-air placements are ~6× more expensive than wall building
   (2.8 µs, 1689 B) because they materialize bricks** — a new brick costs a level-0
   pointer, a bit-grid word, a column-height word, the clearance shell (~106 cells)
   and 592 bytes of zeroed slot words. Still nothing, but it is the right worst
   case to quote: 237 brick materializations in 64 frames cost a 0.254 ms worst
   frame.
6. **The `idle` anchor is exactly 0.000 ms.** An edit-capable renderer with no
   edits costs nothing: the section-1 gate re-run below is unchanged, and no shader
   file was touched by E2 at all.

### CAGI's response to an edit: the global re-flood, measured

E5 owns dirty-region re-flooding; E2 does the only thing E4 offers, a global
re-flood, and measures it. After 256 edits (the 16×16 wall), the volume is thrown
away and re-flooded, compared **cell by cell against the converged volume** (the
volume itself, not the image — a stricter test than E4's pixel compare):

| iterations | frames @2/frame | differing cells | % |
|---|---|---|---|
| 0 | 0 | 2 185 354 | 79.47% |
| 2 | 1 | 128 485 | 4.67% |
| 4 | 2 | 128 323 | 4.67% |
| 8 | 4 | 127 302 | 4.63% |
| 16 | 8 | 108 495 | 3.95% |
| 32 | 16 | 57 648 | 2.10% |
| 64 | **32** | **18** | 0.0007% |
| 128 | 64 | **0** | 0 |

**32 frames (0.53 s at 60 fps) to essentially bit-exact, 64 to exactly bit-exact —
identical to E4's sun-change convergence, and for the same reason: a re-flood costs
FRAMES, not milliseconds** (a cold iteration is the same 0.46–0.76 ms as a steady
one, which the storm's whole-frame medians confirm: 6.4–7.8 ms with a re-flood
every frame of the storm). The `edit-no-reflood` variant is 0.03–0.06 ms cheaper on
the frame thread and visibly wrong (lit air where a block now stands), so it exists
only to isolate the edit pipeline's own cost.

**What a dirty-region version needs (the E5 hand-over).** The global re-flood is
acceptable for a placed *block* — geometry appears immediately, its shading settles
over half a second — and unacceptable for a placed *lamp*, whose light IS the
feedback. A regional flood needs, concretely: (1) a dirty AABB in cell space
accumulated from the edit deltas (the edit API already reports the touched cell
indices, so the AABB is free); (2) a dispatch over that AABB instead of the whole
grid, which means the CA pass's dispatch dimensions become a per-frame uniform
rather than the grid size; (3) a decision about the region's boundary — the cells
just outside must keep feeding light in, so the AABB has to be dilated by the
transport reach the flood is expected to cover (a few cells per iteration), and
either re-flooded from the surviving values (cheap, converges from a good guess) or
cleared (correct but reintroduces the fade-in); (4) a per-region iteration budget
so a small region can be flooded to convergence in one frame — 0.44–0.76 ms buys
the *whole* grid, so a 16³-cell region is essentially free and can afford 30
iterations in a frame.

### No-regression check (section 1, the S2 gate)

Section 1 re-run with `AO_MODE = AO_MODE_OFF` and `CAGI_ENABLED = false`, on the
same machine:

| scenario | recorded (E4 tree) | E2 tree | delta |
|---|---|---|---|
| A top-down, default sun | 4.706 | **4.708** | +0.0% |
| B top-down, low sun 5° | 6.517 | **6.491** | −0.4% |
| C ground, default sun | 4.359 | **4.395** | +0.8% |
| D ground, low sun 5° | 4.913 | **4.945** | +0.7% |

**Pixel gate intact: B shows 19 differing pixels (max channel delta 97), D shows
0, `with-descend-ff` still 12** — exactly the recorded tie set. The output is
byte-identical *by construction*: **E2 changed no shader file**, only Rust, and
the world buffers it can now patch are uploaded from the same arrays as before
(the headroom is appended past the end of the data the shader reads). The one
measurable residue is nothing: the `idle` storm row's frame pipeline cost is
0.000 ms.

Section 4 (the preset table) was re-run and read 40–80% high with p95 up to
14 ms — the machine was not idle (load average 7.5, a VM at 57% CPU). That is the
documented warmup/idle caveat, not a regression: nothing in E2 touches the shading
or CA passes, and section 1 measured on the same machine matched to 0.8%. Re-take
the preset table on an idle machine before quoting it again.

### The CPU traversal, and why it is shaped for E8

`src/voxel_dda.rs` is the picking ray *and* the seed of atrium's
`VoxelDdaResolver`. It takes `&Brickmap` and nothing else (no wgpu, no winit, no
renderer type), speaks world METERS on both ends, and reports a hit the way a
reflection needs it — voxel, face voxel, integer face normal, distance, material
id — not the way a pixel needs it. Two entry points: `cast` (first hit) and
`path_is_clear` (the occlusion query an audio direct path asks). It accelerates on
the same chebyshev clearance field the shader uses (S2's win, on the CPU), which is
why one ray is under a microsecond:

```
4096 listener->source occlusion rays over the island: 3.87 ms total, 0.94 us per ray (1902 blocked)
4096 full reflection casts (hit + face + material):   3.93 ms total, 0.96 us per ray
```

For E8 sizing: a 32-source scene asking for one direct ray plus 8 early-reflection
casts per source is **~0.28 ms per update** on one background thread, over a mirror
that is never stale. Correctness is pinned by tests against a fine-step brute-force
walk (one-sided on purpose: a discrete sampler can miss a sliver the true ray
clips, so it is used only in the direction where it is evidence), plus the
face/normal/adjacency invariants and an edit that flips an occlusion answer.

### Chosen defaults

`world thread` **on**, `clearance update` **local box**, `clearance radius`
**8 bricks**, `re-flood GI on edit` **on**. Left mouse removes the aimed voxel,
right mouse places `Voxel::Stone` against the aimed face, both repeating at 8 Hz
while held (a platform-layer constant, not a lever). Emissive materials are E5.

---

## E1d — Directional miss radiance, from VGI (M3 Max, 2560x1440, 2026-07-30)

Out-of-slot lever, approved by Pascal after he brought the source. Not from the
dossier: **Thiedemann, Henrich, Grosch & Müller, "Voxel-based Global
Illumination", I3D 2011**, §5.1 / Fig. 7 point C. Their near-field gather reads
an *environment map* when an occlusion ray finds nothing inside its search
radius, instead of falling back to a scalar ambient. We already trace those rays
on the RT-AO path, so the upgrade is one lobe mix per **escaped** ray and no new
traversal.

The rest of that paper stays rejected — its per-pixel hemisphere gather is the
architecture E1 already priced out (2.25–3.55 ms per marginal full-res short
ray). Their own numbers agree: 20 directions/fragment at 1/4x1/4 cost them
13.6 ms + 7.7 ms of upsample, and **123 ms at full resolution**, on a GTX 295.

### What it changes, structurally

`ambient_light(normal)` is a function of `normal.y` alone, so it cannot tell a
crevice open upward from an overhang open sideways. Sampling per **ray** couples
direction to **visibility**: the upward-open crevice gets the cool sky lobe, the
sideways-open overhang the warm ground lobe, a sealed pocket nothing.

Two consequences that are not cosmetic:

- The integral is **already visibility-weighted** (occluded rays contribute
  zero), so it *replaces* the hemisphere term rather than being multiplied by
  the AO factor — multiplying would double-count occlusion. Therefore the
  artistic `strength` knob stops applying to the hemisphere term when the lever
  is on; it still scales the CAGI volume term. This is why the occlusion multiply
  moved into `indirect_light`, with the lever-off branch keeping the original
  arithmetic **order** (not merely the algebra) so reassociation cannot move the
  S2 pixel gate.
- Unbiased only under cosine-weighted directions (`AO_DIRECTION_MODE = 0`, the
  shipped RT-AO default), since the ray density *is* the cosine factor of the
  irradiance integral. The uniform and bent-up modes reweight it.

### Cost — free

Against `ao-2ray-d16`, the matching baseline (section 2):

| scenario | ao-2ray-d16 | + miss radiance | delta |
|---|---|---|---|
| A top-down, default sun | 13.107 | 13.144 | +0.28% |
| B top-down, low sun | 16.117 | 16.183 | +0.41% |

Inside the ±2% noise band. A first run on an unloaded machine read +0.18% /
+0.18% / +0.94% / −1.19% (A/B/C/D). C and D were re-run under build load
(`ao-off` itself drifted 4.40→6.01 and 5.62→7.09) and are **not** reported as
measurements — A and B were load-stable in both runs and agree.

### Reach — the point of it

Coverage vs `ao-off`, scenario C: **72.5% of frame at max channel delta 116**,
against the baseline RT-AO's 34.1% at 55. It reaches into the medium-scale
directional band analytic corner AO gives up (E1b) and CAGI only partly covers.

### The catch — variance, and it is visible

The lever makes the ambient term **itself** Monte Carlo. Before, ambient was a
smooth analytic function times a 2-sample occlusion scalar; now the whole term is
a 2-sample estimate, so E1's known 2-ray crosshatch lands in ambient **colour**
and reads as grain in dark foreground (`scenario_c_ao_2ray_missradiance.png`).
Options priced: 4 rays (`ao-4ray-d16` = 19.9 ms vs 13.1, so ≈ +6.8 ms) or B12's
bilateral filter. **This is the third time the ladder has pointed at B12.**

### DOCUMENTED NEGATIVE — sampling the raw sky function

The first implementation sampled `sky_color`'s gradient, luminance-normalized so
the overall level matched exactly. It **turned shadowed grass teal and rock
outcrops purple**. Cause is exact and worth keeping: those sky constants are
*emitted radiance* pushed through inverse Reinhard, so their chromaticity —
normalized zenith ≈ `(0.19, 0.73, 6.03)` — is nowhere near usable as an ambient
tint. Luminance-normalizing preserves the level and imports the chroma. Fixed by
sampling `ambient_light` itself, i.e. the already art-directed hemisphere lobes,
which bounds every colour to the tuned range and needs **no new calibration
constant**. Do not retry the raw-sky form without a chroma-desaturation knob.

### No-regression check (lever off) — the isolation anchor

Section 1, AO off: **4.709 / 6.518 / 4.383 / 4.931 ms** against E1c's recorded
4.723 / 6.530 / 4.379 / 4.918, and the shadow pixel gate still reads **19 / 0**.
So folding the AO multiply into `indirect_light` moved nothing.

### Preset totals after promotion (DDA + CAGI, per frame)

| tier | A | B | C | D |
|---|---|---|---|---|
| Potato @1792x1008 (GI off) | 2.702 | 3.821 | 2.484 | 2.785 |
| Quest @2048x1152 | 4.054 | 5.557 | 3.614 | 4.160 |
| Balanced @2560x1440 | 6.458 | 8.849 | 5.934 | 7.104 |
| **Beautiful @2560x1440** | **14.083** | **18.142** | **10.855** | **12.878** |

Beautiful was 10.8–18.1 ms before the promotion and is 10.86–18.14 after — the
lever is free at tier level too. Potato/Quest/Balanced are unchanged (they run
analytic AO, so there is no miss direction to sample and the lever cannot fire).

### Chosen defaults

`AO_MISS_RADIANCE` **on for Beautiful only**, at 2 rays. Off everywhere else, and
structurally inert wherever `AO_MODE != rays`. Look gate passed by Pascal on the
scenario-C pair; the grain was judged acceptable at this tier.

---

## E2b — Character movement & voxel collision (M3 Max, CPU only, 2026-07-30)

**Section 7** of the harness, and the odd one out twice over: it measures a CPU
pipeline (so it reports median / p99 / **max**, like E2's storm — the question is
whether a movement step can ever hitch a frame, and the median is the least
interesting column) and it needs no GPU at all, so `-- 7` runs in ~5 seconds
including world generation. Code: `crates/voxel-rt/src/character.rs`
(renderer-independent, `&Brickmap` in, world meters out — the same purity rule as
`camera.rs`, because E8's audio listener and E9's VR body both consume it).

Body: **0.60 x 1.80 m** (4.8 x 14.4 voxels), eye **1.65 m**, step-up **0.375 m**
(exactly 3 voxels), gravity **22 m/s²**, jump **7.27 m/s → 1.20 m apex**.

### Per-step cost

| scenario | median | p99 | max |
|---|---|---|---|
| idle on terrain, 60 fps | **0.08 µs** | 0.08 | 0.71 |
| walk over terrain, 60 fps | **0.62 µs** | 0.92 | 1.75 |
| sprint over terrain, 60 fps | **0.96 µs** | 1.38 | 5.67 |
| sprint into a rise (auto-step firing) | 0.75 µs | 1.42 | 1.75 |
| free fall through open air, 60 fps | 0.79 µs | 1.38 | 2.04 |
| sprint + fall through a **40 ms** hitch | 1.58 µs | 2.38 | 3.04 |
| sprint + fall through a **250 ms** hitch | 4.04 µs | 5.71 | 6.08 |
| sprint + fall through a **1000 ms** hitch | **4.04 µs** | 5.88 | 14.62 |
| enter walk mode (ground search from ~17 m up) | **6.17 µs** | 8.83 | 15.25 |

**Read the median and p99, not the max.** At this scale a single OS scheduling
slice dwarfs the work: two consecutive runs put the 250 ms row's maximum at
19.62 µs and 6.08 µs while its median moved 4.08 → 4.04 µs. The medians reproduce
run to run within a few percent.

- **The whole feature is 0.01–0.05% of an 8 ms frame.** Movement is not a budget
  item, and the sub-microsecond typical step is why the controller needs no
  amortization, no fixed timestep and no thread of its own.
- **The cost axis is open air, not dense geometry** — the opposite of the usual
  intuition. `any_blocking_voxel` returns on the *first* blocking voxel, so a box
  test in solid terrain early-outs after one lookup while a test in clear air
  scans the full 5 x 15 (or 5 x 5) cross-section. That is also why the "sprint
  into a rise" row is *cheaper* than plain sprinting: the auto-step's extra
  sweeps run against geometry that answers immediately.
- **The pathological deltas are bounded by construction, and the numbers show
  it.** A 1000 ms delta costs 4.04 µs because it is clamped to 100 ms and then
  split into at most `TERMINAL_VELOCITY x MAX_STEP_SECONDS / 0.25 m` = 22
  substeps — so the cost of a hitch is bounded by the *clamp*, not by how long the
  machine actually stalled. 4 µs is three orders of magnitude away from being a
  frame problem.
- **Entering walk mode is a one-off 6 µs**, because the ground search walks voxel
  layers down from the camera testing the body's 5 x 5 footprint (≈140 layers
  from 17 m up), then lifts the body clear if the camera was inside terrain.

### Correctness evidence (tests, not timings)

The guarantee that matters here is not a millisecond, it is "the body is never
inside solid geometry", so it lives in `cargo test -p voxel-rt --release`
(12 new tests in `character.rs`, 3 in `material.rs`; **113 total, was 98**):

- `absurd_frame_deltas_never_end_inside_solid` — 24 directions x 4 start points x
  {40 ms, 250 ms, 1000 ms} deltas x 8 steps at maximum speed, asserting the body
  is outside solid after *every* step, plus that ≥ 25% of the runs were actually
  obstructed (so the fan still exercises collision rather than passing vacuously).
- `a_three_voxel_step_is_climbed_without_jumping` /
  `a_step_above_the_step_height_is_not_climbed` — the auto-step in both
  directions, on a **carved flat walkway**: the first version of these tests
  measured the island's natural slope instead of the controller and had to be
  rebuilt, which is the recurring lesson that a terrain test needs deterministic
  terrain.
- `deep_water_wades_then_swims_and_floats_without_chatter` — the float line moves
  **< 0.1 m over a whole second** at rest, a resting swimmer's eye is above water,
  and holding dive submerges it. (Water drag is applied while *wading* too, not
  only while swimming; that is what keeps the state boundary from dithering.)
- `substeps_bound_the_per_step_motion`, `the_body_stays_inside_the_world_box`,
  `vegetation_and_water_are_walked_through`, `a_jump_reaches_the_configured_apex`,
  `snapping_to_ground_lands_on_the_surface` (including a camera buried in a hill).

### No rendering regression

Nothing in E2b touches a shader, and the section-1 anchors confirm it on an idle
machine:

| scenario | recorded (E1c/E4, AO off) | E2b tree | delta |
|---|---|---|---|
| A top-down, default sun | 4.723 | **4.702** | −0.4% |
| B top-down, low sun 5° | 6.530 | **6.570** | +0.6% |
| C ground, default sun | 4.379 | **4.390** | +0.3% |
| D ground, low sun 5° | 4.918 | **4.912** | −0.1% |

All inside the ±2% noise rule, and the pixel gate is byte-for-byte the recorded
tie set: **B 19 / 3 686 400 (max channel delta 97), D 0, `with-descend-ff` 12**.

### Registry note

E2b adds **no** registry rows. `CharacterSettings` is its own type outside
`RenderQuality`, because a registry row carries a *measured frame-time verdict*
and drives a shader permutation, and movement feel has neither — the pinning
tests (`every_settings_field_has_a_registry_lever`) are untouched. The mode
toggle (`F`) and the walk-speed wheel are platform-layer input feel, the same
call E2 made for its 8 Hz edit repeat.

### E2b addendum — the swim-test pool carve, i.e. E2's first BULK edit (M3 Max, 2026-07-30)

The island's own water is **0.6–1.75 m** deep, all of it under the 1.44 m the body
needs before it swims, so the swim half of E2b was unreachable in the app. The fix
is a debug affordance rather than a generation change (`src/debug_pool.rs`, the
`P` key / overlay **Debug tools** button): carve a **8 m wide, 5 m deep pool with
a 4 m graded shore**, 10 m in front of the eye, through E2's own edit pipeline.
Deliberately not a generation change — `voxel-core` is what every recorded
baseline and pixel gate in this document is tied to, and voxel-sandbox shares it.

That makes it the largest single world change the engine can be asked for today,
which is exactly why it earned a bench row: **section 6, `report_pool_carve`**.

| what | number |
|---|---|
| voxels touched, in **one** delta | **130 634** |
| world-thread apply (expand + 130 k `set_voxel` + coalesce) | **116.9 ms** |
| request → delta available on the frame thread | 128.0 ms (17 rendered frames) |
| frame-thread cost during those 17 frames | **0.001 ms worst** |
| frame-thread cost on the frame that uploads | **4.7 ms** |
| upload | **503 KB in 696 `write_buffer` calls** (3.9 B/voxel) |
| clearance cells rewritten | 5 607 |
| CAGI cells re-attributed | 28 672 |
| CAGI convergence after the carve | **32 frames** to 18 cells, 64 to bit-exact |

- **Coalescing is the whole reason this is affordable.** 130 634 voxels published
  per-voxel would be ~8 MB and ~130 k `write_buffer` calls; coalescing the dirty
  word ranges across every `set_voxel` (`brickmap::coalesce_dirty_words`, the same
  merge `DirtyRanges::finish` already used per edit) leaves **503 KB in 696
  calls** — 3.9 bytes per voxel against a single click's 64.
- **The 93 ms trap: the CAGI attribute upload, not the edit.** The first
  measurement of this row read **93.2 ms** on the upload frame. None of it was the
  brickmap: `LightVolume::write_cell_attributes` issued one 4-byte
  `write_buffer` per touched cell, and 28 672 of them at ~3.2 µs of driver
  overhead each *is* 93 ms. Grouping consecutive cell indices into one call
  (the cells of a box are contiguous along X) took the frame to **4.7 ms**. The
  lesson generalizes past this tool: at ~3 µs of fixed cost per `write_buffer`,
  **the number of upload calls is the budget, not the bytes** — 1 592 calls carry
  503 KB in 4.7 ms, which is 0.1 GB/s of *effective* bandwidth on hardware that
  does 33 GB/s.
- **Hand-over to E5 (dirty-region flood) and B6 (fluid CA):** both will publish
  region-shaped changes every frame rather than once on a keypress, so both need
  the *call* count bounded — one buffer write per dirty region, not per cell or
  per brick. E5's dirty AABB is already the right shape for it; this row is the
  measurement that says why it matters.
- **CAGI needed no new invalidation path.** The existing global re-flood handles
  it: the carve's delta marks the volume dirty and the volume walks back to
  bit-exact on the same curve a 256-voxel wall does (32 frames to 18 differing
  cells, 64 to zero — the residual 18 are the pre-existing convergence artifact
  the wall table shows too). Costing 0.53 s of settle for a one-off debug carve is
  fine; it is the same reason E5 exists for lamps.
- **Not a registry lever, still.** A one-shot world edit has neither a frame-time
  verdict nor a shader permutation, so the pinning tests are untouched — same call
  E2b made for `CharacterSettings`.

### E2b correction — buoyancy is a spring to the surface, not a lift

Judging the swim in the pool (Pascal, from voxel-sandbox's feel: *"we were
floating basically when we couldn't stand, and I needed to press down mostly when
near the surface; if deeper we didn't float"*) rejected E2b's first water model.
That model was a **constant** +8 m/s² against quarter gravity, i.e. a net +2.5 m/s²
*at every depth*, so the body was corked to the surface from anywhere in the water
column and could not hold a depth.

Replaced by a **depth-faded restoring force** (`character.rs`,
`buoyant_acceleration`) — one expression, two regimes:

```text
a = stiffness * displacement * (1 - t) - deep_sink * t,   t = min(displacement / band, 1)
```

with `displacement` = how far the shoulders are below the float line (which sits
half a voxel under the local surface, found by a bounded upward probe — ≤ 10
`Brickmap::get` calls, never a global water plane), **stiffness 12 /s²**, **band
0.75 m**, **deep sink 0.5 m/s²**, and E2b's existing water drag (4 /s) and ±2 m/s
swim cap unchanged.

| depth below the float line | net vertical acceleration | behaviour |
|---|---|---|
| 0 m (the float line) | 0 | stable equilibrium: eye 0.15 m clear of the water, head out |
| 0.2 m | +2.4 m/s² (≈ 0.6 m/s against the drag) | bobs back up to the line |
| 0.71 m | 0 | the watershed — the unstable divide between the two regimes |
| ≥ 0.75 m | −0.5 m/s² (0.125 m/s terminal) | neutral: holds depth, drifts down slowly |

So near the surface you float head-out and must actively hold dive (12 m/s² of
thrust, 5x the strongest buoyant term — dive always wins), and at depth you stay
where you are. Two new tests pin it, both in `character.rs`
(**119 total, was 113**): `the_test_pool_makes_swimming_reachable` (wade in from
the pool's shallows on foot, reach `Swimming`, then float within **0.1 m over a
second** with the eye above water) and
`a_swimmer_holds_its_depth_instead_of_being_pushed_back_to_the_surface` (dive
1.5 m+ under, release, and drift **< 0.05 m up** in a second and none over three
more). Re-checked with the old constant model patched back in, the second test
fails at *0.449 m of lift per second* — the regression it exists for is real.
Section 1 after both changes: **B 19, D 0, `with-descend-ff` 12**, unchanged.

---

## E6 — Water: reflection, refraction, extinction and the underwater view (M3 Max, 2560x1440, 2026-07-31)

Fresnel-weighted reflection + Snell refraction on water voxels, Beer-Lambert
extinction along the path travelled *inside* the water, and a camera that can be
inside it. Levers: the "E6: water levers" block in `shaders/water.wgsl` (the
optics mode and the bounce budget) plus `WATER_SUN_THROUGH_LIQUID` in the shared
`shaders/world.wgsl`; Rust mirror + patching in `src/water.rs`; composition in
`shaders/dda.wgsl`; five registry rows under the new `Water` subsystem.
**Section 8** of the harness measures it.

### The world this section runs on, and why it is its own

Section 8 builds its **own brickmap**: the seed-1 island plus ONE debug pool
carved through E2's bulk-edit path (`WaterPool::in_front_of` at the ground
scenario's pose — voxel (500, 780), 82 376 voxels written, 8 m across, 5 m deep,
water surface at 10.62 m). The island's natural water is **0.6-1.75 m** deep,
which is too shallow for extinction to say anything and leaves an underwater
camera nowhere to stand. Sections 1-7 still measure the untouched island, so
**this is a carve, not a generation change, and no baseline above had to move for
it** (the plan's baseline-versioning rule is satisfied without a re-record).

### Scenarios (section 8 only)

Two look AT water from the air — the two ends of the Fresnel curve — and two look
at it from inside:

- `E` **shore -> pool, grazing**: eye 1.7 m above the waterline, pitched 0.16 rad
  down at the pool 10 m ahead. The surface is met ~10 degrees off grazing, where
  Fresnel is ~0.4 and the mirror term dominates. The "mirror at grazing angles"
  half of the gate.
- `F` **top-down over the lakes, steep**: the section-1 pose A. The natural lakes
  are met almost head-on, Fresnel ~0.02 — the "see-through when steep" half, and
  the scenario with the most water pixels in frame (14.4% of it).
- `G` **underwater, looking up**: inside the pool, 2 m under the surface, looking
  straight up. Snell's window.
- `H` **underwater, looking sideways**: the same eye, horizontal. Pure extinction.

### Variant table — per-dispatch median ms (clean run, p95 within 1% of every median)

| variant | E grazing | F steep | G under, up | H under, sideways |
|---|---|---|---|---|
| `water-off` (opaque — the anchor) | 5.249 | 5.521 | 1.213¹ | 3.797¹ |
| `water-tint` (zero secondary rays) | 5.606 | 6.264 | 4.150 | 8.385 |
| `water-reflect` (mirror ray only) | 7.308 | 8.388 | 4.160 | 10.433 |
| `water-refract` (medium march only) | 7.256 | 10.019 | 7.508 | 11.112 |
| **`water-full` (shipped)** | **7.649** | **10.156** | **7.509** | **11.055** |
| `water-full-2bounce` (Beautiful) | 7.689 | 10.184 | **17.672** | 11.392 |
| `water-full-nocutoff` | 7.663 | 10.931 | 7.506 | 11.067 |
| `water-full-sunblocked` | 7.506 | 9.393 | 7.498 | 6.240 |

¹ **The underwater `water-off` rows are not a cost baseline, they are a degenerate
image.** With water opaque the eye sits *inside* an opaque voxel, so the primary
ray terminates at t = 0 and the whole frame is one voxel face. The delta against
them is "the cost of having a view at all", not "the cost of water". The
meaningful underwater comparison is tint vs full.

Cost over the opaque anchor on the two above-water scenarios (E / F ms):
tint **+0.36 / +0.74** · reflection-only +2.06 / +2.87 · refraction-only
+2.01 / +4.50 · **full +2.40 / +4.64**.

Coverage (differing pixels vs `water-off`): E **2.34%** (max channel delta 169),
F **14.36%** (172), G and H **100%** (213 / 148). Note that the coverage number
is the water's *screen share*, not a mode-vs-mode difference: every mode changes
every water pixel, which is why the four rows report the same count and differ
only in max delta.

### Verdict A — the model, and its constants

- **Fresnel: Schlick, with `F0` DERIVED from the two indices of refraction**,
  `((n1 - n2) / (n1 + n2))^2` = **0.0204** for air/water. Straight down water
  reflects 2% and transmits 98%; at grazing angles the term goes to 1.0 and the
  surface is a mirror. Not a tuned constant — `fresnel_f0_is_derived_from_the_indices_of_refraction`
  pins it.
- **Snell: the vector form, with total internal reflection as the failure case.**
  Critical angle `asin(1 / 1.333)` = **48.607 degrees**, checked against a hand
  computation, with 48 deg inside the window and 49 deg outside it.
- **Beer-Lambert, per channel, per METER: (0.45, 0.12, 0.06).** Inside the
  measured range for natural water (red 0.35-0.5, green 0.05-0.15, blue
  0.01-0.07 /m), at the turbid end for green/blue so the pool's 5 m reads as a
  legible gradient: transmittance at 5 m is **(0.105, 0.549, 0.741)**. The
  conversion from voxel units goes through the brickmap's own voxel size, so the
  physics cannot drift if the world resolution changes.
- **Index of refraction is a per-MATERIAL column** (`material.rs`), not a water
  constant, because the dossier records xima's own transparency target as *"water,
  oil, clouds and honey"* — a material class, each member with its own index
  (water 1.333, oil ~1.47, honey ~1.50). It cost **zero bytes**: the value took
  the GPU row's former pad word, so the row is still 48 bytes.
- **In-scatter, and the correction it needed.** The absorbed share is replaced by
  the liquid's albedo lit by the **downwelling irradiance** (sun radiance times
  its elevation cosine, plus the sky hemisphere). The first implementation used
  the sky term alone and measured a body radiance of (0.003, 0.044, 0.134)
  against a sunlit surface's ~2.2 — the horizontal underwater view came out
  **near-black** rather than blue-green. With the sun included it is
  (0.036, 0.32, 0.64), a legible blue-green fog. Two documented simplifications:
  the downwelling term is uniform inside the body (no attenuation with depth) and
  unshadowed (one evaluation per pixel).

### Verdict B — `WATER_SUN_THROUGH_LIQUID`: the correctness fix that costs the most

**The finding that changed the design.** With water drawn as a medium but shadow
rays still stopping on it, every submerged surface is in shadow — so the top-down
lakes came out **dark navy where opaque water had been bright cyan**, i.e.
refraction made shallow water *worse*. A pool bed one metre down is sunlit, and
the fix is to let the sun's ray pass through liquids. It applies to the CA pass's
per-cell sun test through the same shared `trace_shadow_visibility`, so the light
volume lights under water too — one flag, both passes.

The cost is real and **concentrated in one view**: sideways underwater
**6.240 -> 11.055 ms (+77%)**, because a shadow ray that no longer stops at the
surface walks metres of water voxel by voxel (water bricks are occupied, so the
chebyshev skip cannot help). The steep aerial view pays **+8%** (9.393 ->
10.156); grazing and looking-up are inside noise (+1.9%, +0.1%). It therefore
ships as a lever, ON where water is drawn properly and OFF on the zero-ray tiers,
which cannot see under the surface anyway.

Ships with a documented simplification: the SUN's own path through the water is
not attenuated, so a deep bed is lit as brightly as a shallow one. Correcting it
needs a second medium march per shaded point.

### Verdict C — the bounce budget is not a global dial

**1 interface ships on Balanced, 2 on Beautiful, and the asymmetry is the whole
verdict.** Above water the second interface is **free** (7.649 -> 7.689 grazing,
10.156 -> 10.184 steep — inside noise), because a refracted ray that reaches the
bed never asks for another. From inside the water looking up it costs
**7.509 -> 17.672 ms, 2.35x**, because that is exactly where it fires: every ray
past the critical angle mirrors off the underside of the surface and marches the
body a second time. What it buys there is the bed *mirrored* outside Snell's
window instead of the flat body colour. Sideways underwater: +3%.

So the budget is free until the player dives and looks up — which is the one place
it also changes the picture.

### Verdict D — the Fresnel ray cutoff (`water_params.z` = 0.04)

Fresnel already says how much each half of a water pixel is worth, so a term below
the threshold takes its analytic stand-in (the sky function for the mirror, the
diffuse surface for the transmission) instead of a secondary ray. **-7.1% on the
steep aerial view** (10.156 vs 10.931 with it disabled) and inside noise on the
other three — exactly where the reasoning put it: a steep view is where the mirror
carries 2% and gets cut on almost every water pixel, so `full` costs the same as
`refraction-only` there (10.156 vs 10.019). It does not help underwater, where the
expensive term (the march) is the one carrying the weight. Runtime, so it needs no
rebuild; 0 restores "always trace".

### Verdict E — the two half-modes, and which half is expensive where

**Reflection is the expensive half at grazing angles, refraction at steep ones**,
and both follow the screen share of what they have to shade:

- the mirror ray is a full-length trace through the open scene plus a full
  shading of whatever it finds (sun ray, AO, CAGI), so it costs +2.06 / +2.87 ms;
- the refracted march is short in shallow water but the *bed* it finds gets the
  same full shading, and on the top-down view nearly every water pixel refracts
  (Fresnel 0.02), so it costs +2.01 / **+4.50** ms.

`water-tint` — zero secondary rays, the analytic sky in the mirror direction over
the diffuse surface, mixed by the same Fresnel curve — is **0.36-0.74 ms** and
reads as a recognisable water surface with a sun glint but no scene reflection and
no visible depth. That is the Potato/Quest pick, and it is the row that proves the
Fresnel weighting alone carries most of the "this is water" impression.

### PNG evidence (`target/bench_dda/`)

`scenario_{e,f,g,h}_water_{off,tint,reflect,refract,full,full_2bounce}.png`, plus
three crops per pair at 3x nearest zoom (`snells-window`, `water-near`,
`water-far`).

- **Snell's window reads correctly** (`scenario_g_water_full.png`): the entire
  180-degree hemisphere above the water — sky, sun glow, the shore's trees — is
  compressed into a cone, with the trees crowding the cone's elliptical rim and
  the frame's corners falling outside it. That IS the physics: the whole upper
  hemisphere squeezed into 97.2 degrees.
- **The grazing mirror reads correctly** (`scenario_e_water_full.png`): the pool
  returns a sharp mirror of the trees, the rocks and the sky, and the water goes
  see-through toward its far edge where the angle steepens. The reflection is
  perfectly sharp because a voxel water surface is a flat plane — there is no
  wave normal yet, which is E7/B6 work, not a defect here.
- **Extinction reads correctly** (`scenario_h_water_full.png`): a blue-green fog
  that thickens with distance, the bright surface overhead, and the bed faintly
  legible through it.
- **The tint tier reads as water, not as glass** (`scenario_e_water_tint.png`):
  a flat sky-coloured pool. Clearly cheaper, not broken.
- **Known flatness, and its cause**: the bed under water is dimmer than it should
  be even with the sun reaching it, because E4 marks a cell absorbing at a
  quarter fill — so cells inside a body of water hold zero light and a submerged
  surface receives only the 25% GI ambient floor. The in-scatter term is what
  keeps it readable. Fixing it properly is a CAGI-transport question (E5/B6), not
  an E6 one.

### Re-recorded baselines, and why

Balanced now ships water optics, so **section 4's preset table moved and is
re-recorded below**. Nothing else did:

- **Section 1 (the Stage 2 traversal gate) is UNCHANGED and re-verified.**
  `trace` and `trace_brick` grew a `skip_liquids` parameter for the sun ray, and
  threading it through measured **free**, exactly as E1's `max_distance` did:
  **4.709 / 6.609 / 4.385 / 4.937 ms** against the recorded
  4.723 / 6.530 / 4.379 / 4.918 (-0.3% / +1.2% / +0.1% / +0.4%, all inside the
  +-2% band), and the **pixel gate still reads 19 differing pixels on B, 0 on D,
  `with-descend-ff` 12** — the same known float-tie set. Sections 1-3 force water
  to `opaque` AND `sun_through_liquid` off (the `water_off` helper), so the layers
  below E6 still describe the renderer every baseline above was recorded against.
- **Sections 6 and 7** (the CPU pipelines) are untouched by water.

#### Section 4 — the quality presets, re-recorded with E6

Per-dispatch median ms, each preset at ITS OWN render scale (base 2560x1440):

| preset | render size | A top-down | B top-down low sun | C ground | D ground low sun |
|---|---|---|---|---|---|
| **Potato** (tint water, no GI, no sun-through) | 1792x1008 | **2.943** | **4.151** | **2.564** | **2.889** |
| **Quest** (tint water, 1 m GI cells) | 2048x1152 | **4.296** | **5.766** | **3.536** | **3.950** |
| **Balanced** (full water, 1 interface, sun-through) | 2560x1440 | **10.150** | **13.278** | **6.617** | **7.500** |
| **Beautiful** (full water, 2 interfaces, RT-AO) | 2560x1440 | **16.355** | **19.502** | **10.057** | **10.972** |

Frame totals (shading pass + CAGI pass, the CAGI column unchanged at
0.06 / 0.27 / 0.97 / 1.88 ms per tier):

| preset | A | B | C | D | vs E4 |
|---|---|---|---|---|---|
| Potato | 3.01 | 4.21 | 2.63 | 2.95 | 2.75 / 3.86 / 2.50 / 2.85 |
| Quest | 4.56 | 6.19 | 3.80 | 4.37 | 4.06 / 5.53 / 3.60 / 4.13 |
| **Balanced** | **11.12** | **14.85** | **7.59** | **9.06** | 6.42 / 8.81 / 5.96 / 7.09 |
| Beautiful | 18.23 | 22.54 | 11.94 | 14.01 | 14.07 / 18.11 / 10.83 / 12.81 |

Read honestly:

- **Potato and Quest barely move** (+0.1-0.7 ms): the zero-ray water mode is what
  it says it is, and those tiers stay comfortably inside budget.
- **Balanced holds the ~8 ms target on the ground default-sun view (7.59 ms) and
  is 13% over on the low-sun one (9.06)** — but the two AERIAL views are now
  11.1 and 14.9 ms, i.e. E6 is the first experiment to put Balanced clearly over
  target from 60 m. The cause is screen share, not inefficiency: 14.4% of that
  frame is water, and every one of those pixels buys a refracted march plus a full
  shading of the bed. The levers to close it are all measured above — the tint mode
  (-3.9 ms on the steep view), the cutoff (already on, -7%), and
  `sun_through_liquid` off (-8% aerial, -77% underwater).
- **Beautiful is 11.9-22.5 ms**, unchanged in character: RT-AO still dominates it,
  and the second water interface is free except underwater.
- Balanced remains byte-for-byte the unpatched shader
  (`balanced_preset_is_the_shipped_baseline`), and the preset set now needs
  **four** distinct pipelines rather than three, because Quest's water mode differs
  from Balanced's — ~2 ms more of startup compile.

### Chosen defaults

`WATER_MODE = 4` (full), `WATER_BOUNCES = 1`, `WATER_SUN_THROUGH_LIQUID = true`,
extinction scale 1.0, in-scatter 0.7, ray cutoff 0.04. Per tier: **Potato
tint + no sun-through · Quest tint + no sun-through · Balanced full x 1 interface
+ sun-through · Beautiful full x 2 interfaces + sun-through.**

### Machine caveat

Section 8's recorded table and the section 4 re-record are from runs on an idle
machine (the `water-off` grazing row reproduced to 0.2% across two independent
runs). Two intermediate runs during development read 10-40% high across every
column with a VM at ~80% CPU and load average 5-7; every *verdict* above rests on
**within-run** comparisons, which the round-robin interleaving makes load-neutral,
and never on a cross-run absolute.

---

## E6 step 1 — the flat field outside Snell's window (M3 Max, 2026-07-31)

Pascal's look gate on E6: **"the looking up out of the water part is completely
broken :)"**, plus *"snells is too strong for me personally"*. This section is the
diagnosis, the fix and its price. It supersedes the E6 section's `WATER_BOUNCES`
verdict and its underwater PNG claims.

### Correction to the E6 section's evidence — read this first

The E6 section's underwater visual claims were **written from crops that do not
contain what they are named for**, and one of them was wrong as a result. The
facts, verified by md5 and by opening the files:

- All **eight** `crop_snells-window_f_water_*.png` are **byte-identical** across
  variants, `water_off` included. That rectangle contains no water at all, so the
  entire `f` crop set is non-evidence — while `f`'s *timings* differ by 5.5 → 10.9
  ms, i.e. the water is in the frame, just outside the crop.
- `crop_snells-window_g_*` is identical across six of the eight variants, because
  the rectangle sits in the middle of the window where every mode renders the same
  sky. It shows nothing about the modes it was supposed to compare.
- The **full frames** (`scenario_g_water_full.png` and friends) do exist and DO
  show the cone — that part of the E6 report was read off a real frame — but the
  crop paths quoted alongside it were not opened. **A crop is only evidence once
  it is confirmed to contain the thing being judged.** Fixing the rectangles and
  adding a bench guard that fails when a variant crop set is all-identical is
  step 4.

### The scenario error that hid the bug

E6's two underwater poses **structurally cannot show the region outside Snell's
window**, which is exactly the region that was broken:

| | half-angle from the view axis |
|---|---|
| vertical half-FOV (68° vertical FOV) | 34.0° |
| horizontal half-FOV at 16:9 | 50.2° |
| diagonal (corners) | 54.0° |
| **Snell's window half-angle** | **48.61°** |

Looking straight **up** (`G`), the frame reaches 34° vertically — the whole
vertical extent is *inside* the window, and only the extreme left/right edges fall
outside. Looking **sideways** (`H`), every ray that reaches the surface does so at
56–90° from its normal, so that half of the frame is *entirely* outside. Neither
pose puts the rim in frame, which is why `full x1` and `full x2` looked identical
and why the failure was invisible in the harness while being unmissable in the
app, where the head tilts to any angle.

**Added: scenario `I` — underwater, up 45°.** At that pitch the rim crosses the
middle of the frame, so the cone, its edge and the mirrored world beyond it are all
in one picture. This is the view the gate was failed on.

### Cause — confirmed, and it is the prime suspect

Past the critical angle `refract_at` reports total internal reflection, so Fresnel
is 1 and **no radiance term is added**; with `WATER_BOUNCES = 1` the loop then
broke immediately and paid the whole remaining throughput out as the in-scatter
constant. The region outside the window was therefore **one flat colour**, over
most of the screen at any tilted upward view.

It also explains both taste complaints, which is why they were not tuned first:
the flat field *is* the in-scatter colour, so the view read as uniformly tinted;
and a bright cone against a featureless surround has nothing to sit against, so
its rim reads as harsh.

### Fix — the cheap mirrored stand-in

`WATER_TIR_FALLBACK`, in the same spirit as the above-water half-modes (substitute
a cheap analytic stand-in for a term you cannot afford to trace properly, **never a
constant**):

- `0 = flat` — the in-scatter constant. Documented negative, kept selectable only
  so the bench can price the fix and the failure stays reproducible.
- `1 = cheap mirror` (**shipped**) — one more medium march, shaded *cheaply*:
  albedo × downwelling × the face's own up-facing share, with **no shadow ray, no
  ambient occlusion and no light-volume sample**. That keeps the geometry, which is
  the entire point, and drops the term that actually costs — underwater the
  dominant cost of a full bounce is the sun shadow ray, which has to walk metres of
  water.

### Measured — per-dispatch median ms

| scenario | `tir-flat` | **cheap mirror (shipped)** | full 2nd interface |
|---|---|---|---|
| E shore → pool, grazing | 8.55 | **8.18** | 8.29 |
| F top-down over the lakes | 10.69 | **10.86** | 10.90 |
| G underwater, up | 7.92 | **13.96** | 23.82 |
| H underwater, sideways | 14.15 | **18.57** | 18.43 |
| **I underwater, up 45° (the rim)** | **10.33** | **14.46** | **20.60** |

- **Free above water** (E, F inside noise): the fallback cannot fire there, which
  is the isolation property the lever needed.
- On the rim view the stand-in costs **+4.13 ms** over the flat constant; a second
  full interface costs **+10.27 ms**. The stand-in is therefore **40% of the price**
  — and the two frames are near-identical, the full bounce adding only a little
  extra shading detail inside the mirrored region.
- **Consequence: Beautiful dropped from 2 interfaces to 1.** The second interface
  was only ever compensating for the missing fallback; with the stand-in in place it
  buys a near-identical frame for 2.5×. No tier ships `flat`
  (`preset_table_tiers_the_water_optics_by_ray_budget` asserts it).

### Full frames — the artifact, since the failure is about what fills the screen

`target/bench_dda/scenario_{g,h,i}_water_{tir_flat,full,full_2bounce}.png`, at
2560×1440. Crops are deliberately **not** the evidence here.

- `scenario_i_water_tir_flat.png` — the failure: beyond the window arc, a smooth
  featureless wash. The rim is a hard edge against nothing.
- `scenario_i_water_full.png` — the fix: the same region now shows the **mirrored
  pool bank and bed** with full voxel structure and a depth gradient, and the rim
  reads as a boundary between sky and a mirrored world, which is what Snell's
  window actually looks like.
- `scenario_i_water_full_2bounce.png` — near-identical to the above; this is the
  comparison that demoted the second interface.
- `scenario_g_water_full.png` — unchanged by the fix, and that is the point: at 34°
  of vertical reach the frame is almost entirely *inside* the window.

### Machine caveat

Load average ~4.7 with a VM resident, so absolute numbers are a few percent high
and scenario E's p95 is polluted (20–25 ms) by the first-row warmup on top of it.
Every verdict rests on **within-run** comparisons, which the round-robin
interleaving makes load-neutral; the flat-vs-standin-vs-2bounce ordering reproduced
across two independent runs.

---

## E6 step 3 — the surface seen from below becomes plainly transparent (M3 Max, 2026-07-31)

Pascal, after gating steps 1–2: *"lets disable the fesel like camera looking up out
of water for now should be just transparent looking out and in .. only top should
have the relfextion"*, and separately *"snells is too strong for me personally"*.

### What changed

A new lever, `WATER_UNDERWATER_INTERFACE`, shipped at `transparent`:

- **`transparent` (shipped)** — from inside the medium the surface is fully
  transmissive and the ray continues **straight through, unbent**. No Fresnel
  weighting, no mirror, no total internal reflection. Only the absorption and
  scattering along the path still apply.
- **`fresnel` (off-lever)** — the physical interface E6 shipped through step 1:
  Snell's bend, a Fresnel-weighted split, and TIR past the critical angle whose
  mirrored region `WATER_TIR_FALLBACK` fills.

The **above-water** side is untouched — it keeps its Fresnel-weighted mirror
("only top should have the reflection") and its refracted march inward.

### Why "just transparent" has to mean UNBENT

Total internal reflection is **not a separable effect that can be switched off on
its own — it *is* what Snell's law yields when `sin(theta_transmitted) > 1`**. Past
the 48.607-degree critical angle there is no transmitted direction to bend toward,
so a build that kept the bend and dropped the mirror would have nothing to draw
beyond the window. Dropping the bend removes the critical angle along with it and
the interface becomes a plain window. There is no coherent middle option, and the
request picked the consistent one.

Accepted consequences, deliberately not treated as defects: **Snell's window
disappears from below, and with it every cue that the surface is there at all.** No
substitute rim or edge hint was added.

### Kept as a lever, not deleted

Per the variant/lever hygiene rule the `fresnel` interface remains selectable with
its step-1 numbers intact, because (a) it is the correct physics, (b) Snell's window
is a genuinely striking effect and the objection was to it *dominating* the
underwater view rather than to it existing, and (c) it is the mode that will want
re-judging once wave normals exist and on Quest.

**Why a separate lever rather than a third rung of `WATER_TIR_FALLBACK`:** that
lever answers "what fills the region outside Snell's window". Transparency removes
the *existence* of that region, so it is a different question and would have made
the fallback's name a lie for one of its values. As a separate lever it *gates* the
other two instead, which is the honest relationship.

### Two levers are now inert from below — documented, not left dead

Under `transparent` every branch of the medium loop returns inside its **first**
iteration (solid, murk limit, or straight out through the surface), so:

- **`WATER_BOUNCES` has no effect** — there is no mirror to bounce off. It is not
  merely a no-op from below: since the above-water refracted ray enters the same
  loop, the bounce budget is inert **everywhere** under this default.
- **`WATER_TIR_FALLBACK` has no effect** — there is no region outside a window.

Both stay levered because they are exactly what the `fresnel` interface needs, and
`lever_is_relevant` greys them out in the Quality panel rather than offering dead
dials (`the_transparent_interface_makes_the_bounce_levers_inert` pins the predicate).

### Measured — it is cheaper, as predicted

Per-dispatch median ms, within one run. The shipped `transparent` against the
`fresnel` interface it replaces:

| scenario | `fresnel` (was shipped) | **`transparent` (shipped)** | delta |
|---|---|---|---|
| E shore → pool, grazing | 7.684 | **7.723** | +0.5% (noise) |
| F top-down over the lakes | 8.359 | **5.082** | **−39%** |
| G underwater, looking up | 4.712 | **2.587** | **−45%** |
| H underwater, sideways | 7.004 | **6.866** | −2% |
| I underwater, up 45° (rim) | 5.141 | **4.551** | **−11%** |

**Cheaper on every scenario where the mirrored stand-in march used to fire, and
noise where it did not** — which is the expected shape: the +4.13 ms rim-view cost
that step 1 paid for the stand-in is gone, because there is no longer a mirrored
region to fill. E and H barely move because their frames are dominated by rays that
end on geometry rather than at the interface. F moves a lot despite being an
above-water view, because the island's water is shallow: many refracted rays reach
the far surface from below and used to TIR there.

### Numbers caveat — a clean re-record is owed

**These absolute figures are NOT comparable with the E6 or step-1 tables.** The
world's dimensions changed underneath this run (a concurrent generation workstream;
`WORLD_SIZE_*` currently reads 125/32/125 where the recorded baselines were taken
against a 1000-voxel axis), so section 8 is measuring a much smaller world — hence
G falling from 13.96 ms to 2.59. The run was also contended (p95 up to 2x median on
E and F). **Only the within-run column-to-column comparison above is load- and
world-neutral**, and it is the comparison the verdict rests on. Section 8 wants a
full re-record once the generator settles, under the baseline-versioning rule.

The same caveat means the `water-tir-flat` column cannot be used to confirm
inertness by timing (it wanders by more than the effect). Inertness is established
from the code path instead — every branch of the loop returns in the first
iteration — and by the greying-out predicate's test.

### Full frames — the artifact (crops are not evidence for this)

`target/bench_dda/scenario_{g,i}_water_{full,fresnel_from_below}.png`, same world
and same pose, so the pair is directly comparable:

- **`scenario_i_water_fresnel_from_below.png`** — the effect Pascal found too
  strong: the sky compressed into a circular window with the shoreline crowded
  around its rim, and everything outside it the mirrored underwater world. A strong
  fisheye.
- **`scenario_i_water_full.png`** — the shipped result: the sky is a plain,
  undistorted expanse, the shoreline sits where it actually is, and there is no rim
  and no mirror. Looking out through a window, tinted and dimmed by the water the
  ray travelled. The surface is invisible from below, as intended.

### Presets

**Every tier ships `transparent`**, and the preset table asserts it
(`preset_table_tiers_the_water_optics_by_ray_budget`). It is a look decision, so it
should not vary by tier — and since it is also the cheaper option, no tier has a
cost reason to differ either. Nothing else in the preset table moved.

---

## Materials arc S1+S2 — face roles and pattern layers (M3 Max, 2560x1440, 2026-07-31)

Section 9, and it is a **new section**: S1 registered a bench point and nothing ran
it, so `BenchSection::Materials` had a column and no table. S2 closed that and
brought the layer sweep with it.

### The table this section runs on, and why it is its own

Section 9 builds its own `WorldBindings` over the shared island brickmap and uploads
a **saturated material table**: every non-Air row carries four pattern layers.

That is not a convenience, it is the only way the sweep means anything. **No row in
the compiled table authors a layer** — that is S6's step — so a sweep over the
shipped table would find the `MATERIAL_FLAG_PATTERNS` bit test short-circuiting on
every hit and would report four layers as free. The number this section exists to
produce is the **per-layer slope**, and only a table that authors layers produces it.

The four are the realistic saturated stack rather than four copies of the cheapest
generator, because understating the slope is the one failure mode that matters here:

| slot | generator | period | what it costs |
|---|---|---|---|
| 1 | `Coursing` (mortar mask) | 0.5 m | the tessellation walk, two divides, two eased edge masks |
| 2 | `CoursingTone` | 0.5 m | the same walk, one cell hash |
| 3 | `Speckle` | 0.05 m | four cell hashes and a `length` |
| 4 | `Noise` x3 octaves | 0.02 m | **24 cell hashes** — by far the dearest of the four |

Sections 1-8 keep the shared bindings and the compiled table, so no baseline above
moves for this.

### Variant table — per-dispatch median ms

Re-recorded after the S2 gate cut the coursing generators and added the texel snap, so
these describe the shipped stack. **Read C and D**: A and B were contended in this run
(p95 6.8% and 6.5% above median across every column, including ones this arc cannot
touch), while C sits at 1.6% and D at 0.5%.

| variant | A top-down | B top-down low sun | **C ground** | **D ground low sun** |
|---|---|---|---|---|
| **material-flat** (the anchor: both levers off) | 10.772 | 14.387 | **6.745** | **7.608** |
| material-face-roles | 10.873 | 14.520 | **6.805** | **7.663** |
| material-patterns-0-layers | 10.777 | 14.433 | **6.761** | **7.627** |
| material-patterns-1-layer | 12.256 | 15.963 | **7.616** | **8.428** |
| material-patterns-2-layers | 12.332 | 16.006 | **7.819** | **8.617** |
| **material-patterns** (all four) | 13.434 | 17.323 | **8.771** | **9.574** |
| material-patterns-half-strength | 13.453 | 17.260 | **8.776** | **9.578** |

The CAGI pass does not move at all (0.98 / 1.58 ms flat across all seven columns),
which is the expected result and worth recording: the light volume bakes its own cell
attributes and never reads the material table, so nothing in this arc can reach it.
That is also exactly why the panel has a "re-pack GI attributes" button.

### Verdict A — face roles are free, as S1 predicted

**+0.9% (C), +0.7% (D).** Inside the +-2% band, and the mechanism says why: the
DDA already records the stepped axis and the ray's sign along it for E1's analytic
corner AO, so the face costs a flag test and a `select` on data the hit already has.
No traversal change, no extra fetch.

Coverage is **1.33% of the frame top-down, 4.66% from the ground** — and that ratio
is the feature working rather than a weak effect. Top-down you are looking at tops,
which keep the row's base colour; from the ground you see the SIDES, which is where
the earth colour lives. Grass is still the only row authoring roles.

### Verdict B — the flag test itself is genuinely free

`material-patterns-0-layers` runs the lever ON with the cap at zero: **+0.2% (C and
D)** and **0 differing pixels in all four scenarios**. So the cost of *having* the mechanism
compiled in, on a row that authors nothing, is nothing — which is what makes it safe
which is why the normal tiers ship the lever on; S6 can author more rows without a
second table. Potato remains the measured zero-layer fallback.

### Verdict C — **the entry cost exceeds the dearest generator**, and that is the finding

The stack is ordered cheapest-first and the cap drops the tail, so the four deltas are
a per-generator cost as well as a per-layer slope. Taking C against the 0-layer column:

| layers | slot added | C ms | delta | that slot's cost |
|---|---|---|---|---|
| 0 | — | 6.761 | — | — |
| 1 | `Flat`, voxel frame, **1 hash** | 7.616 | +0.855 | **+0.855** |
| 2 | `Speckle` + snap, 4 hashes | 7.819 | +1.058 | +0.203 |
| 4 | `Noise` x2 and x3 + snap, 16 and 24 hashes | 8.771 | +2.010 | +0.476 each |

Slot 1 is the *cheapest generator the model has* — one cell hash, no interpolation, no
snap — and it costs **+0.855 ms**, while the dearest (three octaves of value noise, 24
lattice hashes) costs roughly **+0.55**. So almost all of slot 1 is fixed entry cost:
building the `PatternSample` (the clamped position reconstruction, two scalings into
metres), fetching the row, and the register pressure the block adds. **Paying to run the
mechanism at all costs more than the most expensive thing it can run.**

That is a sharper statement than the first recording could make — its stack led with
brick coursing, which is not cheap, so the entry cost and the generator cost were mixed
together in slot 1.

Three consequences, all actionable:

- **A material that wants detail should use two or three layers, not one.** The
  authoring instinct "keep it to one layer for performance" is backwards: once you have
  paid the entry cost, layers 2-4 are a quarter to a half its price.
- **`MATERIAL_PATTERN_MAX_LAYERS` is a weak tier knob.** Dropping 4 to 1 recovers
  1.16 ms of the 2.01 ms; dropping to 0 recovers all of it but removes the feature. For
  a Quest tier the honest lever is **per-material opt-in** — pattern the surfaces you
  stand on and look at, leave the rest flat — not a global cap.
- **The entry cost is where an optimization would pay**, if S2 ever needs one. Sharing
  one `PatternSample` across the albedo/roughness/emission entry points already happens;
  the remaining candidate is hoisting it out of the per-target functions entirely.

The full worst case is **+30% (C) / +26% (D)**: every visible surface in frame carrying
the four-layer stack, three of them snapped. Nothing will ever author that, and it is
the right number to have measured before authoring anything.

### Verdict D — `MATERIAL_PATTERN_STRENGTH` costs nothing, exactly as claimed

Half strength measures **0.06% slower** than full (8.776 vs 8.771 on C) and 0.04% on
D — i.e. identical, well inside run-to-run noise, in all four scenarios. The generator runs
regardless; strength only scales the result. The registry row says so, and this is
the row that shows it: it is the taste knob, not a performance knob, and the bench
column exists to prove the negative rather than to find a saving.

Coverage confirms it is doing something: **86.8% of the frame differs from flat at
half strength vs 86.6% at full, with the max channel delta halved (78 vs 145)**. Same
pixels touched, half as hard — which is what a strength scale should mean.

### Bit-identity, and what actually establishes it

`MATERIAL_PATTERNS=off` reproducing pre-S2 frames is not established by a timing
column. It is established two ways, both stronger:

- **0 differing pixels** on `material-patterns-0-layers` vs `material-flat`, in all
  four scenarios, over a table where every row authors four layers. The uploaded
  slots are read and the result is byte-identical.
- **`no_row_authors_a_pattern_layer_yet`** pins that the shipped table has no layers
  at all, so on the shipped table the flag test fails and the code path is the S1
  path. That test is also the tripwire for S6: when a row gains a layer, it fails,
  and that is the moment to check the layer was a decision rather than a demo left
  behind.

### The GPU row doubled and it costs nothing

`GpuMaterial` went 128 -> 256 bytes (four 32-byte slots), so the table is
**6656 bytes for 26 rows**, up from 3328. Section 1 re-verified unchanged
(see below), which is the answer to the only real question a doubled row raises:
whether the wider stride costs a cache miss in the hottest read in the renderer. It
does not, and it should not — 6.6 KB fits anywhere.

### PNG evidence (`target/bench_dda/`)

`scenario_{a,b,c,d}_material_*.png`. The pair worth looking at is
`material_flat` against `material_patterns`: same world, same light, one flat
palette and one where every surface has per-voxel tone, square specks and blocky grain
on the 8-texel grid.

### Section 1 re-verification — the doubled row does not cost a cache miss

Re-ran the Stage 2 traversal gate against the recorded
**4.709 / 6.609 / 4.385 / 4.937 ms**:

| scenario | recorded | now | delta | p95 spread this run |
|---|---|---|---|---|
| A top-down | 4.709 | **4.734** | +0.5% | 0.7% |
| B top-down low sun | 6.609 | **6.571** | -0.6% | 0.4% |
| C ground | 4.385 | **4.412** | +0.6% | 0.4% |
| D ground low sun | 4.937 | **5.242** | +6.2% | **5.4%** |

**A, B and C are inside the +-2% band; D is not, and it is a contended
measurement rather than a regression.** Every column of scenario D in this run shows
a p95 5-13% above its median (`with-column-ff` 6.252 / 6.911, `with-anyhit-shadow`
5.121 / 5.960), where A, B and C all sit under 1% — including the unrelated columns
that this arc cannot have touched. D wants a re-check on a quiet machine; it is not
claimed as clean here.

**The pixel gate is unchanged, which is the load-independent evidence:** 19
differing pixels on B, 0 on D, `with-descend-ff` 12 — the same known float-tie set
recorded through E1, E2 and E6. The traversal core is bit-identical with a 256-byte
material row.

---

## Materials arc S3 — the scratch spill, generator prices, tessellation (M3 Max, 2560x1440, 2026-08-02)

Three things landed together: a **-56% optimization of the pattern path**, a new
**section 11** that prices every generator, and a new **section 12** that shows what
each one looks like. All numbers here are scenario C (ground, default sun) at
2560x1440 on an Apple M3 Max, on the saturated four-layer bench table.

### The optimization — 6.713 -> 2.972 ms of pattern cost

Measured back to back on one tree, so the deltas are the trustworthy part:

| step | DDA pass | pattern cost | delta |
|---|---|---|---|
| start (with the anchor fixed) | 8.792 | 6.713 | — |
| drift index -> if-chain | 7.430 | 5.357 | **-1.36** |
| stop copying the material row | ~5.5 (contended) | ~3.3 | **-1.9** |
| fuse three surface loops into one | **5.002** | **2.972** | **-0.51** |

**Pattern cost -56%, whole DDA pass -43%.** The entry cost — the 0 -> 1 layer step
that S2's verdict C is about — went **5.55 -> 1.32 ms**.

### One root cause wearing three hats: arrays copied by value, then indexed dynamically

Every one of the three wins is the same defect. A **by-value array that is then
indexed by a non-constant** cannot live in registers, so the backend spills the whole
thing to thread-local scratch and every access becomes a memory round trip.

1. **`PatternAnimation.drift_velocity`** — an `array<vec4<f32>, 4>` indexed by the
   loop variable. 64 bytes per invocation, for a value that is almost always zero.
   Replaced by an if-chain over the four constant indices.
2. **`let row = materials[material]`** — copied the entire **256-byte** `Material`
   struct, including its 128-byte `patterns` array, and then indexed the copy.
   Reading through the storage binding instead (`materials[material].patterns[slot]`)
   is an ordinary buffer access. **This was the single biggest win**, and it is a
   one-line change.
3. **Three separate surface loops** (albedo, roughness, emission), each re-reading the
   row and re-testing the flag: 12 slot iterations to do 4 slots of work. Now one
   loop, one row read.

The call site was also evaluating the whole stack **twice** on graph-active
materials — computing a row-base albedo, discarding it, and recomputing from the
graph base. Fixed with the fuse.

Operationally: **`let row = big_struct_array[i]` is a performance bug in WGSL**, not a
readability preference, whenever the struct contains an array the code then indexes.
Read through the binding.

### Correctness — this optimization changed nothing visible

**All 28 bench PNGs byte-identical to the pre-optimization tree: 0 differing pixels,
max channel delta 0.** `material-patterns-0-layers` still reads 0 differing pixels
against `material-flat`, so S2's verdict-B bit-identity survives intact.

The naga full-source validation test earned its keep here: it caught `target` being a
[WGSL reserved word](https://gpuweb.github.io/gpuweb/wgsl/#reserved-words) before it
reached a driver.

### Three DOCUMENTED NEGATIVES — measured, rejected, recorded so nobody re-litigates

| hypothesis | measured | verdict |
|---|---|---|
| The animation plumbing is the cost. Stub it out entirely | 7.431 vs **7.430** | **0.00 ms.** It was never the animation system |
| Unused generators cost something; specialise the switch | 1-layer column 6.106 vs **6.140** | **0.00 ms.** Dead switch arms are free — naga and the driver already drop them |
| Hoisting the four redundant per-layer salt hashes will pay, by analogy with the row-copy win | **0.002 ms** | Reasoning by analogy from a real win produced a non-win. Not implemented |

The third is the instructive one: the row-copy fix was worth 1.9 ms, and the salt
hoist *looks* like the same shape — redundant work in a loop. It is not. The row copy
was expensive because of **where the data lived** (scratch), not because it was
repeated; four extra ALU hashes repeat but never leave registers.

### Section 11 — what each generator costs, one layer deep

New section, and it is not a variant table: every other section sweeps levers, this
one sweeps a **field of the authored material**. One layer, world frame, 0.5 m period,
8 texels per voxel, measured as a marginal delta **over the `checker` column**, which
stands in for everything the layer mechanism costs *around* a generator.

**Median of three runs** — see the stability note below, it matters.

| generator | ms over checker | band |
|---|---|---|
| `flat` | 0.000 | free |
| `checker` | 0.000 | free |
| `tile-tone` | **0.029** | free |
| `speckle` | 0.041 | free |
| `tile-edge` | **0.052** | cheap (boundary — see below) |
| `wave` | 0.210 | cheap |
| `simplex` | 0.383 | cheap |
| `ridged` | 0.656 | moderate |
| `turbulence` | 0.656 | moderate |
| `noise` | 0.661 | moderate |
| `worley-F1` | 1.003 | expensive |
| `perlin` | 1.012 | expensive |
| `worley-edge` | 1.076 | expensive |
| `worley-smooth` | 1.114 | expensive |
| `warp-noise` (noise + domain warp) | 1.419 | — |
| `warp-worley` (worley + domain warp) | 1.809 | — |
| `face-unsalted-4L` / `face-salted-4L` | 3.153 / 3.175 | — |

Three things fall out of it:

- **The domain warp costs about three octaves, not one.** `warp-noise` at 1.419 against
  `noise` at 0.661 is **+0.758** — the warp evaluates a second full field to perturb
  the first. An earlier comment in the source claimed it was "the same order as an
  octave"; that was wrong and is corrected in `pattern.rs`.
- **The per-face salt is free**: 3.175 vs 3.153 at four layers is 0.7%, inside noise.
- **This doubles as the bake-payoff table.** A stage evaluated at voxel rate and cached
  returns exactly its own row here — so there is nothing to win below `wave`, and
  about a millisecond on each of the Worley three.

#### The bands are only as stable as the run — take three samples

The first version of this table was a single run, and one row's **band** was wrong
because of it. Spread across three runs is +-0.07 ms on the dear rows and +-0.02 on
the cheap ones. That changes no band anywhere except at a threshold, where it changes
the answer:

| run | `tile-edge` | band |
|---|---|---|
| 1 | 0.065 | cheap |
| 2 | 0.052 | cheap |
| 3 | 0.046 | free |

0.05 is the Free/Cheap boundary, and `tile-edge` sits on it. It is recorded as the
median (0.052, cheap) with the instability written on the constant. Every other
generator kept its band across all three runs.

### Tessellation is effectively free, which is the headline of that arc

`tile-tone` at **0.029 ms** lands in the same band as `checker`. The masonry walk is a
floor, a hash and four min/max — and it is the whole difference between a wall of
blocks and a painted slab. `tile-edge` costs roughly twice the tone and is still a
twentieth of one noise layer; the `pow` that sharpens the joint is the entire delta.

### Section 12 — generator swatches

`bench_dda 12` renders one 4x4 m studio wall per generator to
`target/bench_dda/`, so section 11's prices have a picture beside them. Getting a
*readable* swatch took three attempts, all the same class of error — the image was
technically correct and visually useless:

- an 85% mix landed the pattern on near-black;
- the camera sat on the shadowed face (sun `(0.55, 0.8, 0.35)`, viewing -Z, dot -0.35);
- a `MixToColor` toward light grey on a grey wall produced almost no contrast.

Final recipe: **`Multiply` blend at full amount, viewed from +Z.**

### Section 9 re-recorded on this tree

| variant | A | B | C | D |
|---|---|---|---|---|
| `material-flat` | 2.236 | 3.212 | 2.029 | 2.059 |
| `material-face-roles` | 2.277 | 3.247 | 2.031 | 2.075 |
| `material-patterns` | 4.963 | 6.007 | 5.163 | 5.169 |
| `material-patterns-half-strength` | 4.959 | 5.981 | 5.149 | 5.164 |
| `material-patterns-1-layer` | 3.748 | 4.709 | 3.499 | 3.503 |
| `material-patterns-2-layers` | 3.904 | 4.882 | 3.678 | 3.696 |
| `material-patterns-0-layers` | 2.239 | 3.193 | 2.030 | 2.032 |
| `material-patterns-octave-lod` | 4.761 | 5.802 | 5.400 | 5.413 |

- **The flag test is still free and still bit-identical.** `0-layers` matches `flat`
  within 0.1% and at 0 differing pixels in all four scenarios.
- **The layer slope is still front-loaded** (S2's verdict C). At C: layer 1 costs
  **+1.470 ms**, layer 2 only **+0.179**, layers 3-4 **+1.485** together.
- **`MATERIAL_PATTERN_STRENGTH` still costs nothing**: half strength is within 0.3% of
  full everywhere, while changing 56.8% of the frame's pixels.
- Pattern cost at C reads **3.134 ms** here against the **2.972** recorded at the end
  of the optimization above. The gap is run variation plus the material table growing
  a row (`slate tile`), not a regression — but it is why the optimization's own
  deltas, measured back to back, are the number to quote.

#### Octave LOD is a split verdict, and the earlier prediction was backwards

| scenario | `material-patterns` | `octave-lod` | delta |
|---|---|---|---|
| A top-down | 4.963 | 4.761 | **-0.202** |
| B top-down low sun | 6.007 | 5.802 | **-0.205** |
| C ground | 5.163 | 5.400 | **+0.237** |
| D ground low sun | 5.169 | 5.413 | **+0.244** |

It **helps top-down and costs at ground level** — the opposite of the prediction that
it would pay most where surfaces are seen at grazing distance. A plausible reading is
that dropping octaves per-pixel costs more in divergence than it saves in ALU when
neighbouring pixels disagree about the octave count, which is exactly the ground-level
case; that is a **hypothesis, not a measurement**. Ships off.

### The section 1 anchor is dead, and it is not this arc's doing

This arc grew `GpuMaterial` from **256 to 320 bytes** (the pattern row went 32 -> 48 to
carry tessellation), and the S1+S2 section above set the precedent of re-running
section 1 to answer whether a wider stride costs a cache miss. **That check can no
longer be made against the recorded baseline**, because the baseline's world no longer
exists:

| scenario | recorded 2026-07-30 | 2026-08-02 |
|---|---|---|
| A top-down | 4.709 | **0.915** |
| B top-down low sun | 6.609 | **1.472** |
| C ground | 4.385 | **0.968** |
| D ground low sun | 4.937 | **1.020** |

The run prints **`0 occupied bricks`**, against the 71,941 at a 57.9% collapse rate
that the ledger's 1.12 row recorded before this was found (both are now corrected to
100,865 at 100%). That reads like a broken world and is not one:
`brickmap_round_trips_generated_world` already asserts `occupied_brick_count() == 0`,
because **one generated world voxel now maps to exactly one uniform 8³ brick**. Every
occupied brick is fully solid and single-material, the collapse fires on 100% of them,
and level-1 descent has disappeared from the island entirely. That is where the ~5x
went.

Consequences, all owed:

- **The headline baseline table at the top of this file is stale by 5x** and needs a
  full re-record. Every section measured before the lattice change is on a different
  world than every section measured after it.
- **The pixel gate moved too.** Scenario B now reads **125** differing pixels against
  `stage2-baseline`, not the 19 recorded throughout this file, and
  `with-directional-skip` reads 281. `no-dist-skip` is still bit-identical to
  `stage2-baseline`, so the traversal core is consistent — the known float-tie set
  simply got bigger with the new world.
- **`uniform_bricks_collapse_and_the_survivors_are_all_sculpted` had gone vacuous.**
  Its `uniform > unique / 2` predicate is trivially true once `unique == 0`, so it said
  nothing while the world changed underneath it. Now asserts both sides explicitly.
### The 320-byte row costs nothing — answered by a slope, not by an anchor

With the recorded baseline dead, the stride question was settled the way it should
have been in the first place: a **controlled same-tree comparison**. `GpuMaterial` was
temporarily padded **320 -> 384 bytes** (a dummy `[f32; 16]`, mirrored in
`struct Material`, table 8,960 -> 10,752 bytes) and section 9 re-run — section 9, not
section 1, because that is where the row read actually dominates. The probe was
reverted after measuring.

`material-patterns`, per-dispatch median ms:

| scenario | 320 B | 384 B | delta |
|---|---|---|---|
| A top-down | 4.963 | 4.926 | **-0.7%** |
| B top-down low sun | 6.007 | 5.969 | **-0.6%** |
| C ground | 5.163 | 5.177 | **+0.3%** |
| D ground low sun | 5.169 | 5.175 | **+0.1%** |

`material-flat` behaves the same way (0.000 / -0.017 / +0.027 / -0.006).

**A further 20% of row width is free, and the deltas do not even share a sign** — two
scenarios got faster, two slower, all inside the +-1% noise floor. Since +64 bytes on
top of 320 produces no signal, the earlier +64 that took the row from 256 to 320
cannot have produced one either. The row width is not a cache-miss concern at this
table size, and the way to keep it that way is the table size, not the row: 28 rows
fit in any L1.
