# Materials arc — working state

**Read this first when picking the arc up in a new session.** It is the *status and
how-to-resume* doc, deliberately not a second copy of the design or the measurements:

| What you want | Where it lives |
|---|---|
| The full design, stage by stage | `.claude/plans/the-way-we-make-eventual-deer.md` |
| Ladder position + what each stage landed | `docs/voxel-rt-plan.md`, "Materials arc" |
| Every measurement and verdict | `docs/voxel-rt-bench.md`, section 9 |
| Transferable lessons | `docs/voxel-rt-optimization-ledger.md`, rows 6.20–6.28, 7.14–7.15 |
| **Status, how to run it, what is next** | **this file** |

Last updated 2026-07-31. Keep the status table, "next action" and "open decisions"
current; leave the rest alone unless it stops being true.

---

## Status

| Stage | What it is | State |
|---|---|---|
| **S0** | Studio, writable material table, `MaterialKind` union | ✅ landed + gated |
| **S0b** | `.vox` import layer (parser in `voxel-core`, provenance merge) | ✅ landed + gated |
| **S1** | Face roles (top / side / bottom) | ✅ landed + gated |
| **S2** | Layer model — generators, frames, periods, blends | ✅ landed + gated |
| **S2b** | Texel snap + per-face variation | ✅ landed + gated |
| **S2c** | Patterned emitters cast light (mean → GI volume) | ⏳ built, **gate not passed** |
| **S2d** | Sun/ambient dimming + emission intensity, so S2c is judgeable | ⏳ built, awaiting the S2c gate |
| **S3** | Animation — value oscillation + pattern drift | ⬜ not started |
| **S4** | Template library | ⬜ not started |
| **S5** | Sub-voxel models (the only stage that touches traversal) | ⬜ not started |
| **S6** | Apply to real materials, re-author roughness/specular | ⬜ not started |

**252 tests** in voxel-rt (+5 bin), **51** in voxel-core. `cargo fmt` and
`cargo clippy --all-targets` clean.

### Next action

**Run the S2c gate.** Everything is built; nobody has yet confirmed that a patterned
emitter visibly lights a neighbouring surface. Procedure below. If it fails, the
suspects in order are: the `emissive scale` lever under GI (global, defaults 1.0), then
whether the attribute re-pack ran, then the ~32-frame GI convergence.

---

## How to run it

```
cargo build -p voxel-rt --release
./target/release/voxel-rt --studio
```

`--studio` skips world generation entirely and builds its own scene: one voxel, a plate
under it, an orbit camera (drag to turn, wheel to zoom). World editing is disabled there
on purpose.

### The panel path for a pattern

All under **Materials** in the overlay:

1. **Quality → Materials → `pattern layers`** must be ON. It ships off, because no
   compiled row authors a layer and turning it on is S6's decision.
2. **`row`** dropdown — picks the row being edited **and, in the studio, the voxel on
   screen**. Row 6 is stone, 24 is `glow_block` (already a 3.0 emitter).
3. **Studio subject** — `single voxel` / `wall (16x16)` / `cube (4x4x4)`. The wall is for
   continuity and any period over one voxel; the cube is for corners.
4. **Pattern layers → add layer**, then dial. A new layer starts at `amount: 0`, which is
   the exact identity, so it is safe to add before configuring.

### To see an emitter at all

**Sun → `sun intensity` 0, `ambient` ~0.1.** An emitter cannot be judged against
daylight, and until S2d the sun was a hardcoded constant with no control.

Then an emission layer: target `emission`, blend `add`, pick a `glow colour`, and raise
`glow intensity` (0..16 — above 1 is normal for a source).

### The S2c gate specifically

1. Sun down as above, **Studio subject → cube**, row 6 (stone).
2. An `add`/`emission` layer, orange, intensity ~8, amount 1.0.
3. Press **`re-pack GI attributes`** once.
4. Watch the snow plate beside the cube for warm bounce, over ~0.5 s (GI converges in
   ~32 frames).

The re-pack is needed exactly once, because *which cells hold an emitter* lives in the
attribute volume. After that, dragging the colour or intensity changes the injected
light with no re-pack — the palette re-uploads on every dirty table.

### Bench

```
cargo run -p voxel-rt --example bench_dda --release -- 9    # the materials section
cargo run -p voxel-rt --example bench_dda --release -- 1    # the traversal no-regression gate
```

Section 9 uploads a **deliberately saturated** table (four layers on every visible row),
because the shipped table authors none and a sweep over it would report four layers as
free. Numbers and verdicts in `docs/voxel-rt-bench.md`.

---

## Open decisions

These are waiting on Pascal, not on work:

- **`Voxel::Lava` as its own row?** It crosses the **enum weld** — material ids are 1:1
  with `voxel-core`'s 26-variant `Voxel` enum through two hand-mirrored matches, so one
  variant touches ~10 sites across three crates (`world.rs`, `terrain_chunk.rs`,
  `material.rs` ×4 including `MATERIAL_COUNT` and the upload pin, and four matches in
  `voxel-sandbox/mesh.rs`, which the plan's non-goals currently protect). Mechanical, but
  a deliberate crossing. The alternative is to keep authoring lava-like looks on
  `glow_block`.
- **`CAGI_TRANSMISSION`** still ships off pending an app-run verdict (predates this arc).
- **When to turn `per-face roles` and `pattern layers` on by default** — that is S6, with
  a re-recorded baseline, because it changes how the island looks.

## Known limits (by decision, not by accident)

- **No HDR intermediate**, so emission past ~3–4 clips the *surface* to flat white while
  the light it casts keeps growing. Judge intensity by what it lights.
- **`opacity` is not a pattern target.** It is a traversal input, decided before shading,
  so patterning it would move the layer stack into the innermost traversal loop — the one
  cost this stage is built to avoid. A dissolve effect wants it; named follow-on.
- **7 emitter palette slots.** A patterned emitter claims one, and CAGI's per-cell field
  is 3 bits. Two rows emit today.
- **The GI volume gets a MEAN, not the pattern.** Deliberate and physically right: cells
  are 0.5 m and the light reaching elsewhere from a speckled surface *is* its average.
  Near field vs far field, not one model approximated twice.
- **Roughness is authored but unread** until a reflection stage exists. A roughness
  pattern layer works and shows nothing.
- **The face frame repeats unless `vary per face` is on** (it is, by default). Off is a
  feature: identical detail on every face of a block type is the classic voxel look.

## Where the code is

| File | Owns |
|---|---|
| `crates/voxel-rt/src/material.rs` | The authored row (`Material`, `MaterialKind`), the flat `GpuMaterial`, `mean_emitted_radiance` |
| `crates/voxel-rt/src/pattern.rs` | The layer model **and a full CPU reference evaluator** the WGSL is checked against |
| `crates/voxel-rt/shaders/pattern.wgsl` | The shader half — a hand-mirror of the above |
| `crates/voxel-rt/shaders/world.wgsl` | The uploaded row's layout (shared with the CAGI pass) |
| `crates/voxel-rt/src/material_table.rs` | The live-editable rows + dirty tracking |
| `crates/voxel-rt/src/material_edit.rs` | The panel |
| `crates/voxel-rt/src/studio.rs` | The studio scene and its poses |
| `crates/voxel-rt/src/material_tune.rs`, `vox_material.rs` | `.vox` import + non-destructive provenance merge |
| `crates/voxel-core/src/vox.rs` | The shared `.vox` parser (voxel-sandbox uses it too) |
| `crates/voxel-rt/src/cagi.rs` | `MaterialAttributes` — the 104-byte reduction that lets a live row reach the GI |

**If you change `pattern.rs`, change `pattern.wgsl` to match.** The mirroring is by hand
and is the likeliest thing to drift; `pattern.rs`'s tests pin the Rust against
hand-computed values, and the shader is only checked against the Rust by eye.

## Traps found the hard way

Each of these cost real time and is now pinned by a test. Do not re-learn them:

1. **A panic reachable from registry DATA needs a registry TEST.** A `Count` lever given
   `Discrete` instead of `Rungs` compiled, passed every other pinning test, shipped, and
   panicked when its panel opened. Now `every_lever_declares_bounds_the_overlay_can_draw`.
2. **Documenting a mechanism is not testing it.** The panel described a two-tier liveness
   model in which the second tier did not work: the GI re-pack read the *compiled*
   material table, so a live edit could never reach the bounce. Now
   `a_live_edited_albedo_reaches_the_cell_attributes`.
3. **The lever-off state must be the pre-stage look.** Putting earth in grass's *base*
   albedo and green in its `top` role renders the island brown with face roles OFF. All
   three roles are explicit overrides for this reason.
4. **Two things called "selected" were not the same thing.** The panel's row and the
   studio's subject drifted, so editing stone while looking at grass made every slider
   appear dead. The studio's subject now follows the selection.
5. **A feature can be correct and invisible.** Patterned emitters worked and could not be
   seen: the sun could not be dimmed, and the emission target's colour picker capped
   radiance at 1.0 against `glow_block`'s 3.0. Check the *observation conditions* and that
   every quantity is authorable in the range it needs.
6. **When a widget's range is wrong, widen the range — do not throw away the widget.**
   Replacing the colour picker with three raw channels lost the thing a picker is good at.
   Picker + intensity, product folded at the upload boundary.
