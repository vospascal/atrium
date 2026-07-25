# Volumetric Fluid Water (Stage 8)

Replace the static water plane + faked waterfalls with a **simulated** water
body: water has volume, flows downhill, fills basins, and spills over edges — so
waterfalls *emerge* from the sim. Committed to the full arc (user, 2026-07-23).

## Model choice: shallow-water "virtual pipes" (Mei et al.), 2D

The world grid is **1000×1000×256** — a full 3D cellular automaton per frame is
far too much for CPU. Terrain here is a heightfield, so a **2D shallow-water**
sim (one water *column* per (x,z), flux between neighbours through virtual
pipes) is the right fit: cheap (2D, and only the wet region needs simulating),
and it naturally produces rivers, level pools, and rim spill (waterfalls). GPU
3D falling-sand (catalog V10) is the eventual scale path (F4), not the start.

## Staged plan (each stage gated)

- [ ] **F1 — CPU sim core (`voxel-core`)**  *(this stage)*
  `WaterSim`: per-column `terrain` floor + `depth`, 4-way pipe `flux`, `step(dt)`
  (flux update from surface-height differences → outflow scaling for volume
  conservation → depth update), sources (add water) and open/drain cells (spill).
  Generic over a sub-grid so it's unit-testable on small grids and later bounded
  to the island's wet region. **Gate: physics unit tests** — conservation in a
  closed basin, downhill flow, basin fills level, open edge drains.
- [ ] **F2 — Dynamic rendering** — drive the water surface mesh/shader from the
  sim each tick (keep the Beer–Lambert depth-absorption shader for volume).
  Bound the sim to the water bounding-box; fixed timestep (~30 Hz).
- [ ] **F3 — Emergent waterfalls** — delete the faked waterfall meshes; render
  rim-spill (drained columns losing water over an edge) as falling water.
- [ ] **F4 — GPU compute** — port the CA to a compute shader (ping-pong
  buffers) for full-res scale. New render subsystem (first compute in the repo).
- [ ] **F5 — Interaction** — dig channels / displace water (Stage 8 editing).

## Notes
- Perf: only simulate wet columns + their fringe (active set) or the water
  bounding box — never the full 1M columns.
- Conservation is the correctness anchor: closed sim → constant total volume.
- Waterfalls are not authored — they're where the sim spills over the rim.
