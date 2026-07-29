# Transparent Voxels → Volumetric Cellular-Automata Fluid

Generalize transparent-block rendering (today: water only, special-cased), then
put a bounded 3D cellular-automata fluid on top of it so vertical flow —
waterfalls, pours, honey — becomes real simulated volume instead of a faked
visual. Inspired by xima's devlog
["GPU-Driven Voxel Engine: Transparency and Underwater"](https://www.youtube.com/watch?v=6f-rzok4Mok),
which sequences it the same way: transparency foundation first, fluid sim second.

**Status: PLANNED, not started. Each stage needs explicit approval + an app-run
gate before the next begins.**

## Relation to existing work

- The shallow-water arc (`docs/fluid-water-plan.md`, F1–F5) is DONE (GPU compute
  node deferred). Its virtual-pipes heightfield **stays the workhorse for open
  water bodies** — volume-conserving, smooth, cheap. Nothing here replaces it.
- This arc is the item that plan explicitly deferred: *"true volumetric falling
  water needs a 3D representation (post-F4)"* — and it answers open water issue
  #1 (spill curtains never read as flow; removed in F4, rim spill currently has
  no visual).
- The underwater stack (Snell window, per-channel Beer–Lambert fog, screen
  tint) is done and gets *reused*, parameterized per fluid — not rebuilt.
- The transparency foundation pays off even if the CA fluid stalls: glass, ice,
  and voxel clouds ride the same rails.

## Current state / the two structural gaps

1. **One transparent type, special-cased.** `MeshGroup::Water` is a hardcoded
   branch in the mesher: water faces are emitted only against Air/Cover
   (`mesh.rs` face rules), transparency rides vertex-color alpha, and one
   `WaterMaterial` (AlphaMode::Blend, cull off, no depth prepass) renders it.
   There is no notion of "a transparent voxel class" — oil next to water has no
   defined face or blend behavior.
2. **No volumetric fluid representation.** The dynamic water surface is a
   static-topology heightfield grid displaced by a GPU height buffer (F4) — by
   design it cannot represent water over air (falls, pours, overhangs).

## Model choices

**Transparency: mesh-level classes + per-chunk sort, no OIT (yet).** Each voxel
type gets an opacity class (Opaque / Transparent(class)). Face rules: cull
between same-class transparent neighbors, emit at different-class boundaries,
emit against Air/Cover; opaque faces against transparent stay as today. Each
class gets its own per-chunk submesh entity so Bevy's per-entity distance sort
orders chunks back-to-front; within-chunk order artifacts are accepted at first
(Minecraft ships this way). OIT / sorted index buffers only if the artifacts
prove ugly on real scenes — decided at the T2 gate, not preemptively.

**Fluid: hybrid — heightfield for bodies, bounded CA boxes for vertical flow.**
A full-grid 3D CA is off the table (1000×1000×256 = 256M cells; the catalog's
Video 10 does this on GPU compute — that remains the eventual scale path, same
posture as the deferred F4 compute node). Instead, small **active CA regions**
(falling-sand rules over per-cell fill levels) spawn only where the heightfield
can't represent the flow: at spill lips (detection already exists from F3) and
at player pours over dry ground. Volume is handed between the two sims,
conservation preserved end-to-end. CA regions retire when settled.

## Staged plan (each stage gated: tests + clippy + fmt → user runs the app)

### Part A — transparency foundation

- [ ] **T1 — Transparency classes in the mesher.** Replace the `MeshGroup::Water`
  special case with a general opacity-class table + the face rules above.
  Multiple transparent submeshes per chunk (island + streamed paths). Class
  params must fit the packed 12-byte vertex format (water alpha currently rides
  packed vertex color). **Gate:** face-count unit tests on hand-built voxel
  patterns (same-class cull, cross-class boundary, opaque adjacency) + the
  island renders *identical to today* (water is the only transparent class —
  regression, not change).
- [ ] **T2 — Second transparent material end-to-end.** Add one test fluid voxel
  (honey — visually distinct, sets up viscosity later). Decide material
  strategy: one shared "transparent voxel" material with per-vertex
  tint/absorption vs. per-class materials (prefer shared — one pipeline, fewer
  draw groups). Build a test pool scene: honey pool beside + behind water.
  **Gate:** app run — cross-class boundary faces render, sight lines through
  stacked transparents blend acceptably; OIT go/no-go decided here.
- [ ] **T3 — Inside-volume view, generalized.** Camera-in-volume detection per
  class; the underwater stack (Beer–Lambert `from_visibility_colors`, screen
  tint, fog-sea/cloud suppression) becomes per-fluid parameters. Snell window
  stays water-only. **Gate:** dive into the honey pool (amber murk, short
  visibility) + underwater-in-water is pixel-unchanged.

### Part B — volumetric cellular-automata fluid

- [ ] **C1 — CPU cellular-automata core (`voxel-core`).** Falling-sand rules
  over a bounded box: per-cell fill 0..=MAX, pass order fall → spread → settle,
  viscosity as a flow-rate divisor, strict volume conservation. Generic over
  its region like `WaterSim`, fully unit-testable headless. **Gate:** physics
  tests — closed-box conservation, column collapses and spreads flat, flows off
  a ledge, honey spreads measurably slower than water.
- [ ] **C2 — Hybrid coupling.** CA boxes spawn at heightfield spill lips and at
  pours (`G`) over dry ground; heightfield outflow (`outflow_at`) feeds the box
  as inflow, CA volume settling onto a wet heightfield footprint hands volume
  back. Boxes retire when still. **Gate:** conservation test across the
  handoff + app run — pour on the dry plateau, watch volume flow downhill and
  join a pool; rim spill steady state (recharge) unchanged.
- [ ] **C3 — Dynamic transparent meshing of CA regions.** Dirty CA boxes remesh
  into T1-class transparent meshes per tick; fill level renders as a lowered
  top face (Minecraft-style partial cells) so surfaces aren't full-voxel
  stair-steps. Measure remesh cost (geometry census + P overlay budget line).
  **Gate:** fps within budget on the M3 Max (no drop below ~60 with an active
  fall), remesh time logged and sane.
- [ ] **C4 — Waterfalls v2 (the payoff).** Permanent CA boxes at the rim spill
  lips render the actual falling volume — replaces the deleted curtain hack
  with sim-driven falls. **Gate:** side-on app run (`VOXEL_ORBIT_PITCH`), falls
  read as *flowing water*, not stripes; this was the original complaint, so the
  user's eye is the gate.
- [ ] **C5 — More fluids (stretch, optional).** Honey pour interaction; oil
  with density layering (floats on water); voxel clouds as drifting
  low-density gas. Each is its own approval — none is assumed.

## Notes

- **Perf discipline:** never simulate or remesh beyond active boxes; retire
  settled regions; remesh dirty regions only. GPU compute port of the CA is
  the known scale lever if CPU boxes ever bind — measure first.
- **Conservation stays the correctness anchor** across BOTH sims and the
  handoff between them: closed world → constant total volume.
- **Look > greedy:** every Part B gate is a visual read (falls, pours), not
  just green tests.
- **Process:** follow stages in order; any deviation (reorder, split, approach
  change) is flagged and approved before acting.
