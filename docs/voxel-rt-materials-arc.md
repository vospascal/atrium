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
| **S2d** | Sun/ambient dimming + emission colour x intensity, so S2c is judgeable | ⏳ built, awaiting the S2c gate |
| — | Embedded-emitter fix, **step 1**: sticky emitter index in the cell sweep | ✅ landed (CAGI, not this arc) |
| — | Step 2 (area-weighted per-cell radiance) → became **E5b**, see the plan | ✅ implemented + CPU-gated; visual/GPU gate pending |
| **S3** | Animation — clock, oscillators, world events, pattern drift | ✅ landed, **visual gate pending** |
| **S4** | Template library | ⬜ not started |
| **S5** | Sub-voxel models (the only stage that touches traversal) | ⬜ not started |
| **S6** | Apply to real materials, re-author roughness/specular | ⬜ not started |

**359 library tests** in voxel-rt (+5 bin), **51** in voxel-core. `cargo fmt` and
`cargo clippy --all-targets` clean.

### S3 — animation, as landed

Four node types in `NodeCategory::Animation`, plus the scalar multiply that
makes gating expressible at all:

| node | outputs | what it is for |
|---|---|---|
| `material.time` | `value` | Monotone seconds. Never steps backwards. |
| `material.oscillator` | `value` | sine / triangle / saw / pulse / flicker, with `low`..`high`, an `enabled` toggle, and a **sync** source: `global`, `per_voxel`, `per_face`, `per_material`. |
| `material.event_sensor` | `signal`, `nearness`, `envelope` | "Did something happen within X metres of me, and how long ago?" Attack / hold / release. |
| `material.direction` | `vector` | Speed + azimuth + elevation to a velocity vector, so a flow is dialled as an angle rather than three components. Every input connectable: an oscillator on the azimuth swirls, one on the speed surges. |
| `material.multiply_scalar` | `value` | Gating. `sensor.signal x oscillator` is the whole "pulse only when something is near" mechanism. |

Pattern layers gained two input sockets: `animation_gain` (multiplies the
authored `amount`, identity at 1.0) and `drift_velocity` (metres per second).
Drift is quantised to the texel grid, so a pattern MARCHES a whole texel at a
time. There is **no** smooth-drift flag — there was briefly, and it did nothing:
`pattern_coordinate` snaps after subtracting the offset, so an un-quantised
offset gives byte-identical output and only costs the grid its alignment to
world voxel boundaries. Continuous motion is what `texels_per_voxel = 0` already
means.

Four things worth knowing before touching it:

- **A disabled oscillator is removed, not frozen.** `enabled: false` makes the
  lowering ignore the link, so the consumer falls through to its own socket
  default exactly as if nothing were connected — a layer gain returns to 1.0 and
  the material looks as it did before the node existed. There is deliberately no
  authored value-while-disabled: the neutral value belongs to the consumer, and
  a layer gain (1.0) and a mix factor (0.0) do not share one. The bypass happens
  at compile time, so a disabled node emits no shader code at all.
- **`drift_velocity` is a velocity, not an offset.** The shader applies the
  clock, so a bare `Vector3` wired in is a flow. An offset socket would have
  made the obvious graph a static displacement that merely looked animated.
- **The event field is entity-shaped, not camera-shaped.** The camera raises a
  presence event like anything else would; `crates/voxel-rt/src/world_event.rs`
  is where a mob system plugs in, and no shader or node changes when it does.
  Re-raising an open event PRESERVES its start timestamp — that one rule is what
  lets an envelope exist without per-voxel history.
- **GI does not follow the animation.** The material table bakes one still
  sample for the light volume, so a pulsing emitter's *surface* pulses while the
  light it throws stays at the resting value. Closing that loop is its own arc:
  the volume has no bounded re-flood (`LightVolume::mark_dirty` clears both
  buffers and floods the whole grid), so an event-driven light needs either
  regional CAGI propagation or a separate transient-light mechanism. Measure
  before choosing.
- **Determinism narrowed.** `MaterialAnimationSpeed = 0` freezes the clock but
  NOT event sensors, whose inputs still move with the camera.
  `MaterialAnimationDeterministic` freezes both and is what the bench sets. It
  buys frame-to-frame stability, not equality with an un-animated material — a
  frozen oscillator still returns a value, so animated scenes carry their own
  pixel baselines.

### Next action

**Run the S2c gate.** Everything is built and nobody has yet confirmed that an emitter
visibly lights a neighbouring surface. Procedure below.

Step 1 of the embedded-emitter fix is in, so an emitter now always injects rather than
sometimes injecting nothing — the `wall + glow block` prop should light up. E5b now applies
the exposed-area weighting, so the remaining visual question is whether the patterned source
reads like the authored texture rather than a block-sized light.

If nothing lights at all, the suspects in order are: the `emissive scale` lever under GI
(global, defaults 1.0), whether the attribute re-pack was pressed, the ~32-frame GI
convergence, and `ambient` sitting at 0 so there is no floor to see a change against.

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

1. **Quality → Materials → `pattern layers`** is ON by default on Balanced, Quest and
   Beautiful (four-layer cap); Potato deliberately patches it off and caps at zero.
2. **`row`** dropdown — picks the row being edited **and, in the studio, the voxel on
   screen**. Row 6 is stone, 24 is `glow_block` (already a 3.0 emitter).
3. **Studio subject** — `single voxel` / `wall (16x16)` / `cube (4x4x4)` /
   `wall + glow block`. The wall is for continuity and any period over one voxel; the cube
   is for corners; the last is a **diagnostic prop**, see below.
4. **Pattern layers → add layer**, then dial. A new layer starts fully on at `amount: 1`,
   with the shared 8×8 texel and 0.02 m feature defaults; dial it down if needed.

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

### The `wall + glow block` prop, and the bug it shows

One `GlowBlock` embedded at the centre of the wall. Not a pose for judging a material —
it exists because **an emitter embedded in a surface behaves arbitrarily today**, and
that stayed invisible until someone tried it.

`build_cell_attributes` writes every occupied voxel's material into its cell
*unconditionally*, ascending Y outermost, so the last write wins: **one voxel represents
all 64 of a half-metre cell** (highest Y, then furthest Z, then furthest X). So an
embedded emitter either:

- **is not the elected voxel → it lights nothing at all.** Not dimly. It glows (that is
  the per-hit material read) and injects zero. 1-in-16 for a thin wall, 1-in-64 inside
  solid.
- **is the elected voxel → the whole 0.5 m cell blazes** at the block's full radiance, so
  one 12.5 cm block lights like a 50 cm cube.

Move it one voxel and it flips between those. **E5 never caught it because its gate
placed glow blocks in open air**, where the block is its cell's only occupant and always
wins — so this is a pre-existing E5 bug the materials arc exposed, not one it caused.

The prop places the block where its cell does **not** elect it as the albedo voxel, which
is exactly the case that used to fail.

**Step 1 is DONE.** The pre-E5b sticky-source fix ensured an embedded source could not be
discarded by the albedo election. It is now superseded for emission by the area
accumulator below; albedo/transmittance still take the last voxel visited.

**Step 2 / E5b is IMPLEMENTED.** The cell stores the mean radiance of exposed emitting
faces divided by all exposed faces, so a buried source contributes zero and a small source
no longer lights like the whole half-metre cell. CPU tests pin the embedded reduction and
incremental/full rebuild agreement; the studio/GPU gate remains.

**That step became its own stage: `E5b` in `docs/voxel-rt-plan.md`.** It is CAGI's work,
not this arc's — per-cell mean radiance weighted by exposed emitting area, retiring the
3-bit emitter index and the 8-slot palette. One thing from it constrains **S5** and is
recorded there: the per-material question must be *"how much light leaves one exposed face,
per unit area"*, so that S5's sub-voxel masks refine the answer instead of computing a
second one.

A separate finding from the same frame, and also not a material problem: **Reinhard
compresses an emitter's hue away.** `glow_block`'s authored `(3.0, 2.8, 2.4)` tonemaps to
`(225, 223, 219)` — a 1.25 red:blue ratio becomes 1.03, so a bright emitter reads neutral
white however warm it was authored. Which means **an emissive material cannot currently
look like lava even when the transport works.** That is E7's HDR intermediate plus bloom,
and it is the strongest concrete argument for scheduling them.

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

Remaining decisions and gates:

- **`Voxel::Lava` as its own row — RESOLVED.** Lava now has its own enum/material id,
  solid semantics, sandbox colour, authored warm patterned emission, and upload pin.
  The enum weld was mechanical and deliberately crossed rather than overloading
  `glow_block`.
- **`CAGI_TRANSMISSION`** still ships off pending an app-run verdict (predates this arc).
- **`per-face roles` and `pattern layers` defaults — RESOLVED separately from E5b.**
  Normal tiers enable both; Potato keeps them off (and keeps zero pattern layers) for
  its low-cost path. This is a visual-baseline change, not part of the E5b data model.
- **E5b implementation gate:** emission at every scale now uses an area-weighted per-cell
  mean radiance, with the 3-bit emitter index and palette retired. CPU tests cover buried
  zero, embedded reduction, and incremental/full rebuild agreement. The remaining gate is
  the studio visual check and GPU sweep benchmark. The release bench now builds and reports
  its CPU/memory sections before attempting the GPU run; this machine simply cannot acquire
  a Metal adapter.

## Known limits (by decision, not by accident)

- **No HDR intermediate.** Two consequences, both E7's. Emission past ~3–4 barely changes
  the *surface* while the light it casts keeps growing — judge intensity by what it lights.
  And **Reinhard compresses an emitter's hue away**: `glow_block`'s authored
  `(3.0, 2.8, 2.4)` tonemaps to `(225, 223, 219)`, so a 1.25 red:blue ratio becomes 1.03.
  **A bright emitter reads neutral white however warm it was authored, so lava cannot look
  like lava** until HDR + bloom land. Nothing in the material model can fix that.
- **`opacity` is not a pattern target.** It is a traversal input, decided before shading,
  so patterning it would move the layer stack into the innermost traversal loop — the one
  cost this stage is built to avoid. A dissolve effect wants it; named follow-on.
- **Per-cell emission buffer.** E5b adds one packed 10:10:10 word beside the attribute
  word (16 bytes/cell total including ping-pong at the shipped 0.5 m rung), trading the
  old palette field for the range needed by voxel- and texel-scale sources.
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
