# Cave world-generation arc — findings repair + two-pass generation

**Status:** ACTIVE PLAN
**Recorded:** 2026-08-08
**Parent research:** [Hytale-informed world-generation integration plan](./hytale-worldgen-integration-plan.md) — this arc is the concrete
execution of its S3/S4/S5 slices, scoped to what the CPU generator can run today.

## Why this arc exists

A code review of the working tree found 32 issues in the new chunked
generator, the biome graph lowering, and the flare/runtime code. Three of them
break generation outright (an authored value panics the app, cave ramps punch
shafts to the sky, water can never generate), and a cluster of them break
reproducibility (rebuild uses a different seed than startup, the biome graph is
applied only in memory). Repairing those is the foundation; on top of it this
arc delivers what the world actually needs:

1. **Pass one — terrain shape and caves.** Terrain that straddles sea level so
   oceans exist, caves inside an authored band, and fluids *in* the caves:
   aquifer water and lava lakes.
2. **Pass two — scatter.** Vegetation and cave decoration placed from a cheap
   deterministic RNG over the pass-one result, never by re-deriving terrain.

Both passes are authored as nodes and visually debuggable. Mostly cellular
automata for structure; hash noise only where it belongs — per-voxel scatter
chance, jitter, and variant choice.

## Principles this arc holds

- **One lowering.** A node record becomes a program in exactly one function,
  shared by the runtime, the viewport preview, and the node-card thumbnail. The
  previous three copies are why the preview showed caves the world did not have.
- **One seed path.** `manifest.world_seed` is the only seed. Startup and every
  rebuild go through the same call.
- **Authored means persisted.** A graph that changes the world is registered in
  the manifest and applied at load, or it is not a source of truth.
- **A declared field is a live field.** Every field on a node declaration must
  reach the program. A slider that does nothing is a bug, not a placeholder.
- **Dummy materials are fine, invisible materials are not.** A new voxel class
  gets a real row in the table *and* a project JSON, because the app renders the
  project's table. Its look can be crude; its presence cannot be optional.
- **Debuggable by construction.** Every pass publishes counts and a slice view
  the debug window can render, in its own toggled window.

## Stage roadmap

- [x] **W0 — Generation core repair.** No-panic validation (`ChunkGenerator::sanitized`),
      ramps bounded by the cave band and the surface, `sea_level` from the named
      constant, `BiomeKind` single home (`voxel-rt`'s `BiomeType` deleted),
      `with_caves` and the constructor wrapper stack deleted, NaN guard, one-hash
      Worley, region-aligned `CellularFieldCache`.
- [x] **W1 — Terrain shape (pass one, part A).** Authored base height, relief,
      and sea level so the surface straddles water; a default world now generates
      ocean and beach, pinned by a test.
- [x] **W2 — Cave fluids (pass one, part B).** `CaveFluids` carried by
      `world.cave_carver`'s fluid fields; carved voids below a level fill with
      water or lava, and a test pins that every fluid voxel has a solid bed.
- [x] **W3 — Cave surface classification.** `Chunk::cave_surface` publishes
      floor/ceiling/wall/interior per open voxel — the input pass two reads.
- [x] **W4 — Scatter (pass two).** A `world.scatter` node driving
      `ScatterProgram`: coarse patch field for clumping, one coordinate hash per
      candidate. Surface grass and flowers; cave moss, mushrooms, vines, glowing
      crystals. Five new material rows, synced into the project so they render.
- [x] **W5 — One lowering, live fields.** `graph::lowering` is the single
      record-to-program mapping used by the runtime, both previews and the node
      thumbnail; defaults read from the DECLARATION; `cave_carver` declares every
      field it reads; the dead birth/survival fallbacks are gone and
      `cellular_openness` replaced the threshold that never opened.
- [x] **W6 — Persistence and seed.** Biome graph registered in the manifest and
      applied at load; save goes through the store (atomic, schema-validated);
      one seed from `manifest.world_seed` for startup and every rebuild.
- [x] **W7 — Visual debug.** `voxel-game-ui::worldgen_panel` (**G**): measured
      per-pass counts, fluid volumes, scatter tallies, surface range against sea
      level, carved range against the authored band, and a cave slice view with
      corridors. Studio previews and the node thumbnail on the shared lowering,
      with a memoised field.
- [x] **W8 — Runtime/flare repair.** A dedicated `FlareEmber` row, attach in and
      under water, no key auto-repeat, no half-flare at the border, camera ejected
      after a rebuild, no world lock with zero flares, frame-constant shader work
      hoisted.
- [x] **W9 — Cleanup and gates.** `fmt` + `clippy` clean, full test run, in-app run.

## Second pass: smaller nodes, and a pipeline you can see

The first pass left one node carrying 14 fields, three of them dead, and a preview
that drew the authored mask as coloured Y-bands — it showed where rooms *could* be,
never what the pipeline actually built.

- [x] **W10 — Decompose.** `biome.cellular_automata` (14 fields) deleted, replaced
      by four one-decision nodes: `cellular_seed` (fill, cell size),
      `cellular_smooth` (iterations, R1, R2), `cellular_cleanup`, and
      `cellular_connect` (minimum room, corridor radius — two values that were
      hardcoded inside the field builder). On the world side, `cave_carver` dropped
      from 22 fields to 7 (it now owns only WHERE caves are and the value-noise
      family), and `cave_chambers`, `cave_detail`, `cave_fluids` and per-class
      `scatter_rule` became their own nodes. This also ends the duplicate authoring
      the review flagged: the world graph owns the band, the biome graph owns the
      recipe, and neither restates the other.
- [x] **W10b — Chain-walking lowering.** `apply_cave_node` dispatches on node type
      and reads only that node's keys, so chain ORDER cannot change the result — the
      property that makes the nodes composable. Tests pin that every declared field
      reaches the program and that a node ignores its neighbour's keys.
- [x] **W11 — Stage report.** Generation records, per stage, how many voxels it was
      offered and how many it changed, plus the finished material composition and a
      per-Y layer breakdown. Measured while generating, never re-derived.
- [x] **W12 — Layered views.** The G window gained a stage pipeline strip (hit rate
      per stage), a material composition list with percentages, and a vertical layer
      chart with the sea level and cave band marked. The Biomes-tab 3D preview now
      generates a real chunk and colours every voxel by the STAGE that produced it,
      with per-stage visibility so the rock can be hidden to look at the caves alone.
- [x] **W13 — Gates.** 41 test suites green, `fmt` and `clippy` clean, in-app run at
      120 FPS with a visible coastline and scattered decoration.

### Not part of this arc, found while working

The uncommitted world-height change (`WORLD_VOXELS_Y` 32 → 64, `WATER_LEVEL_WORLD`
10 → 32, `PLATEAU_FLOOR_WORLD` 6 → 4) left nine `voxel-rt` test fixtures stale —
they hardcoded heights from the 32-tall world, most visibly a test pool with its
water 22 m below the real water plane. Verified against a clean `HEAD` worktree:
`HEAD` is green, `HEAD` plus those three constants is not. The fixtures now derive
their heights from the constants, so the next time one moves they follow.

## Pass one: terrain, caves, fluids

```text
world.generated_terrain   base height + relief + sea level  -> surface_y per column
        |
        v
world.cave_carver         CA region field + 3D worley/billow -> carve mask
        |                 ramps clamped to band and surface
        v
world.cave_fluids         aquifer water level, lava level    -> fluid fill in voids
        |
        v
(published)               BiomeKind + CaveSurface per voxel
```

The CA field is region-aligned and deterministic, so it is computed once per
`(region origin, Y band)` and shared by every chunk in that region — the review
measured 64x redundant rebuilds without the cache.

Fluids are a *fill* of already-carved voids, not a second carve: a cave void
whose voxel is at or below the authored water level becomes water, below the
lava level becomes lava, and the level's own floor stays solid so the lake has a
bed. That keeps the fluid pass independent of the carve mask's internals.

## Pass two: scatter

```text
pass-one voxels + CaveSurface + BiomeKind
        |
        v
world.scatter    per class:  patch CA mask  x  hash chance  x  surface role
        |
        v
decoration voxels (grass, flowers, moss, mushrooms, vines, crystals)
```

Structure comes from a coarse cellular patch mask so decoration appears in
clumps rather than uniform static; the per-voxel decision is one hash compare.
Each class declares which surface role it accepts (floor, ceiling, wall), which
biome it belongs to, and its rate. Nothing in pass two re-samples terrain noise.

New dummy material rows, crude on purpose:

| Voxel | Role | Note |
| --- | --- | --- |
| `CaveMoss` | cave floor cover | dull green, walk-through |
| `CaveMushroom` | cave floor cover | pale cap, walk-through |
| `CaveVine` | cave ceiling cover | hanging, walk-through |
| `CaveCrystal` | cave wall/floor | emissive, the readable cave light |
| `FlareEmber` | thrown flare | replaces the flare's `Lava` masquerade |

## Debugging

The worldgen debug window (`P`-style toggle, own module) shows, for the last
build: per-pass voxel counts, water and lava volumes, scatter tallies per class,
the authored cave band, and a horizontal slice of the CA field at a chosen Y with
corridors marked. The Studio Biomes tab previews the *same* program the world
built, from the shared lowering, with fluid and scatter layers drawn on top.

## Gates

- Determinism: same seed and same graph produce identical chunks regardless of
  generation order; a headless test pins it.
- No authored value from any declared field range can panic generation.
- Caves never open to the sky except where a ramp is explicitly allowed to.
- Water exists in a default world, and cave lakes have solid beds.
- Scatter is reproducible and bounded: a tally test pins counts per class.
- The preview and the built world agree — one lowering, asserted by a test that
  lowers the same record twice through both call sites.
- `cargo fmt`, `cargo clippy`, full test suite, and a real app run.
