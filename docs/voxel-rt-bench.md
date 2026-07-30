# voxel-rt DDA Benchmark — Regression Gate

The permanent perf harness for the voxel-rt renderer. Run it **before and
after any change that could touch traversal cost** — shader edits, brickmap
layout changes, new ray types, world-content changes — and compare against
the recorded baseline below.

```
cargo run -p voxel-rt --example bench_dda --release
```

Runtime ≈ 1–2 min (world gen ~0.5 s, then 7 shader variants × 4 camera/sun
scenarios × 12 timed batches). No window; needs a real GPU.

## What it measures

- **Scenarios** (fixed, deterministic poses — seed 1, season 0.0):
  - `A` top-down over the island center, 60 m altitude, default sun
  - `B` same view, sun at 5° elevation — worst case for shadow rays
  - `C` ground level at spawn looking across the island, default sun
  - `D` same view, low sun
- **Variants**: `current` = the shipped shader exactly as in `dda.wgsl`.
  Every other column string-patches one `ENABLE_*` lever (the "A/B benchmark
  levers" block at the top of `shaders/dda.wgsl`) so each optimization is
  measured in isolation. `stage2-baseline` = all traversal aids off.
- **Timing**: 25 dispatches encoded back-to-back per command buffer,
  wall-clock per batch / 25, 12 batches, median + p95. Variants rotate
  round-robin inside each scenario so GPU clock/thermal drift hits all
  columns equally. (GPU timestamps are NOT used: Metal resolves
  pass-boundary counters to zero once a command buffer holds more than one
  compute pass.)
- **Correctness gate**: the low-sun scenarios (B, D) are rendered per
  variant and pixel-compared against `stage2-baseline`; PNGs land in
  `target/bench_dda/` for eyeballing.

## Recorded baseline — Apple M3 Max, 2560x1440, 2026-07-30

Commit state: Stage 2 traversal optimization round (branchless `dda_step`,
chebyshev distance skip; column-ff / descend-ff / any-hit / bit-grid
defaulted OFF as measured losses).

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
