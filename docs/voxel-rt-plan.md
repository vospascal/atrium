# voxel-rt — Ray-Traced Voxel Engine (Master Plan v2)

New workspace crate `crates/voxel-rt`, built **next to** `voxel-sandbox` (untouched).
Thesis (validated by `xima-engine-dossier.md`): **one shared discrete voxel world
serves rendering, GI, simulation, collision, entities and sound**, with the GPU
owning almost all fine-grained work — and atrium as the far-deeper audio half.
No meshes, no rasterized geometry: everything flows through DDA traversal.

Stack: `winit` + `wgpu` (Metal on macOS dev, Vulkan on Quest 3 later) + `egui`.
World data from `voxel-core` today (CPU island); GPU-side generation is an early
experiment (E3), not an afterthought.

Reference docs: `xima-engine-dossier.md` (what xima's engine does, evidence-tiered)
· `voxel-rt-technique-bank.md` (Shadertoy/IQ cheap-beauty techniques, tagged by
experiment slot) · `voxel-rt-bench.md` (harness protocol, baselines, verdicts)
· `voxel-rt-optimization-ledger.md` (the scoreboard: every optimization idea
with used / lever / open / dead status and the number that decided it).

**Targets (the three axes every gate scores): FAST (desktop: full stack ≤ ~8 ms
@ 2560×1440; Quest tier reachable via levers), LOW-MEMORY (world + light budgets
tracked per experiment; billions-of-logical-voxels thinking, never
billions-of-resident-bytes), BEAUTIFUL (the Voxile screenshot mood: warm GI,
noiseless, readable shadows).**

## Process rules

- Experiments in ladder order; any deviation flagged and approved first.
- Optimize aggressively at every step; Pascal gates each experiment in the app.
- Checkbox roadmap shown every turn.
- **Benchmark rule:** all gate numbers come from the headless harness
  (`cargo run -p voxel-rt --example bench_dda --release`; protocol + baselines in
  `docs/voxel-rt-bench.md`). Never from eyeballing the overlay at an unknown
  window size. ±2% = noise. Pixel-diff gates guard correctness.
- **A/B(/C/D/…) rule (Pascal, 2026-07-30):** architectural choices are decided by
  measured variants, not taste. The bench harness owns variant patching
  (`ENABLE_*` levers / shader-source substitution / alternative pipelines); every
  contender gets a number and the verdict is recorded in `voxel-rt-bench.md`
  before the loser is deleted. Losers with plausible future value stay as
  documented off-levers; everything else dies (no dead code).
- **Isolation rule (Pascal, 2026-07-30):** every experiment is self-contained —
  its own pass module / shader function / bench scenario — and excludable, so the
  app can run without it, a stuck experiment can be shelved without blocking the
  ladder, and an older experiment can be re-opened to look for bigger wins.
- **Architecture-first rule (Pascal, 2026-07-30):** decisions that are hard to
  reverse later — data-flow/authority, threading, world generation locus,
  memory layout — get their own early experiments with real A/B prototypes and
  written verdicts. We take the time now.
- **Baseline-versioning rule (Pascal, 2026-07-30):** world generation is NOT
  frozen to protect benchmark baselines. Deeper water and caves are wanted and
  approved, and they will invalidate every recorded median and pixel-tie set.
  The protocol is therefore: change generation deliberately, **re-record the
  baselines in the same commit, and tag every baseline table with the generator
  version it belongs to** (seed + a generation revision), so a stale comparison
  is impossible rather than merely discouraged. Never silently compare across
  generator versions.
- **Water is a fluid, not a material (Pascal, 2026-07-30):** *"you expect to be
  able to dig the earth under it but not the water itself."* Digging must skip
  water; the E2b pool tool *writing* water voxels is an acknowledged stopgap
  until backlog B6's fluid CA gives water mass and flow. Until then, treat any
  edit that paints or removes water as debug-only.

**Modularity rule (Pascal, 2026-07-29):** hard seams between platform/windowing ↔
GPU device ↔ render passes ↔ world data ↔ camera ↔ overlay. Pass convention:
self-contained struct, `new`/`rebind`/`encode`. Brickmap stays
renderer-independent (doubles as the audio-ray structure). Camera stays
windowing-independent (doubles as the VR head pose). Platform layer stays thin
(replaced by OpenXR on Quest). Clean seams, never compat shims.

## Threading & data-flow model (SETTLED in E2, 2026-07-30 — numbers in the bench doc)

**The world is CPU-authoritative and lives on its own thread; the GPU gets
deltas; the audio side gets the authority itself.**

- **Main/platform thread:** window, input, egui, surface present — plus the
  *uploads* (`write_buffer` is frame-thread work by definition) and the pass
  encoding. Cost of the edit pipeline here: **0.000 ms idle, 0.065 ms median /
  0.123 ms max** while a wall is being built 4 voxels per frame.
- **World thread (`src/world_host.rs`):** owns the CPU `Brickmap` and is its only
  writer. Applies edits (`Brickmap::set_voxel` → every derived structure repaired:
  occupancy bits, material bytes, brick allocation/free, the chebyshev clearance
  field, column + global heights, the E4 CAGI cell attributes) and publishes
  **owned `WorldDelta`s** — 14 bytes for a typical edit — through a channel the
  render thread drains. Also where the ~50 ms CAGI attribute rebuild goes when the
  GI resolution lever moves, so that stops being a frame hitch, and where **bulk
  edits** run: a `BulkEdit` shape is expanded into voxel spans and applied here as
  ONE coalesced delta (E2b's pool carve: 130 634 voxels, 116.9 ms off-frame,
  0.001 ms on the frame thread meanwhile). Future off-frame work (E3 generation,
  B6 CA simulation, B8 streaming) plugs into the same seam.
- **NOT `Arc<Brickmap>` snapshot swapping** (the pre-E2 sketch): a deep copy is
  **4.9 ms for 46.4 MB** per published edit, against a 14-byte delta. One brickmap
  behind an `RwLock` + owned deltas instead — the render thread never locks, and
  readers hold the read lock only for the microseconds a ray takes. Snapshots stay
  the right answer for a *stable* view (a save, a network frame), not for "what is
  the world right now".
- **Rayon: not needed, deliberately.** The only remaining multi-millisecond CPU
  jobs (the 62 ms build, the 50 ms attribute rebuild, the 31 ms full clearance
  rebuild) all became *off-frame* rather than *fast*, and off-frame was the whole
  requirement. Parallelizing them is available if a future job is latency-critical.
- **Frame-thread readers of the authority (E2b):** the walking body takes the read
  lock once per frame and holds it for **0.62–0.96 µs** (bench section 7) — the
  same shape as voxel picking, and the reason a reader does not need its own
  snapshot. It is also the pattern E8's resolver follows from a *background*
  thread.
- **Audio thread (exists in atrium):** rtrb commands in, audio out; E8's
  `VoxelDdaResolver` runs on a *background* thread, takes `WorldHost::shared()`
  (an `Arc<RwLock<Brickmap>>`) and queries `src/voxel_dda.rs` — **0.94 µs per
  occlusion ray, 0.96 µs per reflection cast**, over a mirror that is never stale
  because it IS the authority.
- **GPU:** all per-voxel/per-pixel work — passes composed by the renderer. Its
  world buffers are patched in place (`COPY_DST` since E2) with the touched words
  only; 4096 spare brick slots (2.4 MB) make brick materialization a patch too.
- **GPU-authoritative bricks: rejected, and the reason generalizes.** A GPU→CPU
  readback costs **1.29 ms round trip regardless of size** (64 B and 43.8 MB
  measure the same), so no delta scheme can amortize it, and non-blocking mapping
  converts the cost into **7–10 submit/poll cycles of staleness**. Since the audio
  mirror is not optional, C pays for two copies plus synchronization where B pays
  for one that is authoritative. GPU-authoritative *derived* data whose only
  consumer is the GPU (E3's generated bricks, E4's light volume) is a different
  question and stays open.
- **Async readbacks:** timers today; the sound-queue readback idea now has a price
  tag (1.29 ms per round trip, size-independent) and must therefore be
  amortized over many frames or dropped.

---

## Done (the engine core)

### S0 — Scaffold ✅ (2026-07-29: 8 ms / 120 fps vsync-capped)
winit + wgpu compute→storage-texture→blit + egui FPS overlay.

### S1 — Brickmap + primary-ray DDA ✅ (2026-07-29: ~4 ms @ 2560×1440)
Two-level sparse brickmap from `VoxelWorld` (61.7 ms build, 71,941 bricks,
sandbox-parity seed 1 / season 0.0), per-pixel two-level DDA, palette from
mesh.rs hues, fly camera, round-trip tests.

### S2 — Direct light ✅ (2026-07-30: 4.71 ms top-down / 6.53 ms low-sun, harness)
Sun shadow ray (integer-reconstructed origins — no acne), hemisphere ambient,
linear-space light + Reinhard, sun az/el sliders. **Key traversal win: Pascal's
chebyshev distance-field skip (bindings 9/10).** Column fast-forward + any-hit
measured as losses → off-levers. Permanent bench harness + GPU pass timers.

---

## Experiment ladder (one at a time; every E = lever + bench scenario + gate)

### E1 — Ray-traced ambient occlusion ✅ (gate passed 2026-07-30)
Short occlusion rays from primary hits (reuse `trace` with a `max_distance`
argument — no forked DDA). Implemented: an AO master lever (`ENABLE_AO`, since
replaced by E1b's `AO_MODE`) + 4 variant levers in
`dda.wgsl`, `src/ao.rs` (Rust mirror + shader-const patching), overlay "AO"
section, 10-variant bench section. **Verdict (bench doc, E1 section):
2 rays / 8 voxels / cosine-weighted / distance falloff, strength 0.8** —
+4.2 to +8.2 ms at 2560x1440 (8.6–14.6 ms total DDA pass; the originally
recorded +5.8–8.1 overstated the ground-level views, corrected during E1b),
so the default needs re-tiering against the ~8 ms target — E1b found the
cheaper technique. Half-res AO rejected (not
separable without a G-buffer + bilateral pass); bent-up kept as a cheap
off-lever for Quest. **Secondary-ray budget for E4: ≈2.25–3.55 ms per marginal
full-res short ray** (corrected in E1b's clean re-run from the recorded 3.4–4.3) → per-pixel GI gathering is unaffordable, which is the
quantitative case for the CA light volume; composition contract is
`indirect = CAGI_sample * AO`. **Gate (Pascal, in-app):** crevices/overhangs
ground the scene without shimmer or over-darkening.

### E1b — Cheap occlusion + soft shadows shootout ✅ (measured 2026-07-30; soft-shadow negative confirmed in-app by Pascal; **analytic corner AO promoted to the shipped default**; AO look gate still open)
Triggered by E1's verdict: ray-traced AO costs +4.2–8.2 ms (E1's C/D numbers
were inflated by a harness artifact, corrected in the bench doc), over the ~8 ms
target, so the *technique* — not just the ray count — needed options. Everything
below is a lever with bench numbers and PNG comparisons against E1's
`ao-2ray-d8` reference; full tables, per-tier recommendation and PNG findings in
the bench doc's **E1b** section.

- (A) **Analytic AO — WINNER.** Classic voxel *corner* occlusion (8 occupancy
  bits around the hit face, bilinearly interpolated across it with the DDA's
  exact face-local UV): **+0.25–0.31 ms vs RT-AO's +4.2–8.2**, at 82% of its
  frame coverage, and *noiseless* where 2-ray RT-AO still crosshatches large
  near surfaces. Falls short only in reach (contact-only — recessed-but-not-
  touching areas read a step flatter), which is precisely the band CAGI owns at
  E4. The wider 3×3×3/26-neighbour variant lost: 5× the cost for a broad
  over-darkening (68–82% coverage) and per-voxel flat facets.
- (B) ~~SSAO~~ **deferred to backlog B12** (approved 2026-07-30).
- (C) **RT-AO** kept as the Beautiful-tier lever, on the strength of its reach.
- (D) **Soft shadows from the chebyshev distance field — DOCUMENTED NEGATIVE.**
  Free as promised (+0.10–0.35 ms, no extra rays) but the per-BRICK field
  stamps a visible **1 m lattice plus sun-aligned streaks** into flat surfaces
  at *every* penumbra scale (swept k = 4/16/64/115; 115 = the sun's true
  angular radius). Both cheap refinements were implemented and measured —
  cube-boundary clearance instead of the per-brick floor, and midpoint instead
  of face sampling (mandatory: a face point reads clearance 0 and blacked out
  55% of the frame) — and the artifact survives both, because every ray that
  could form a penumbra grazes distance-1 bricks whose clearance is bounded by
  half a brick. Needs voxel-level clearance data (≈37 MB, an E2/E3 decision).
  Hard shadows stay the default; the Stage 2 pixel gate still reads 19/0.
- **Pascal's addendum (3 extra cost-cutting ideas), all measured, all off:**
  brick-neighbourhood early-out **fires 0% of the time** on terrain (byte-
  identical output; the distance field structurally cannot drive it and the
  bricks under/beside a surface are solid ground); distance LOD saves only
  0.6–2.9% at ground level because AO cost is dominated by *near* pixels, and
  its large aerial saving is the effect itself being removed; sun-aware ray
  budget saves ≤7.5% by putting the known 1-ray crosshatch on exactly the
  bright flat ground that shows it.

**Result: analytic corner AO + hard shadows puts the whole DDA pass at
5.0–7.2 ms across all four scenarios at render scale 1.0** — under the ~8 ms
target, which no RT-AO configuration reaches (11.8–14.6 ms). Per-tier picks
(Potato/Quest/Balanced = analytic corner + hard; Beautiful = RT-AO + hard) are
tabulated in the bench doc, ready for E1c to install.
**Gate (Pascal, in-app):** does analytic corner AO ground the scene as well as
E1's rays did — and is losing the medium-scale dimming acceptable before CAGI?

### E1c — Variant registry, quality presets & settings panel ✅ (gate passed 2026-07-30: presets 2.68 / 3.46 / 5.01 / 11.69 ms scenario A; 54 tests)
Pascal's requirement — keep the measured losers *runnable* (an M3 Max loss can
be a Quest win) without dead code or hot-loop clutter — is met with **one lever
registry** (`src/variants.rs::REGISTRY`): a row per lever carrying kind
(compile-time const / runtime uniform), default, range, the measured verdict
with its numbers, and the bench points that sweep it. The **bench derives its
variant tables from it** (adding a row adds a bench column forever), the
**overlay generates the Quality panel from it** (verdict as hover text — "why is
this off?" answered in-app), and **pinning tests close the drift gate in both
directions** (registry ↔ `dda.wgsl` ↔ typed `Default`s; a settings field without
a row stops the test compiling). 20 levers, 4 subsystems.
- **Presets are a table, not if/else:** sparse override lists over the shipped
  baseline, so a future field (E4 CAGI iterations, E6 reflection depth, E7 post
  effects) needs a registry row and only the tiers that differ grow a line.
  **Potato 2.5–3.8 ms · Quest 3.1–4.8 · Balanced 5.0–6.8 · Beautiful 8.5–14.6**
  (each dispatched at its own render scale; headline table in the bench doc —
  superseded by E4's, which adds the CAGI pass to give frame totals).
  Balanced is byte-for-byte the unpatched shader.
- **Compile-time vs runtime, measured:** traversal levers and both mode
  selectors stay consts (that folding IS the S2 win); the AO fade ramp moved to
  the lighting uniform and measured **free** (−0.17% to +0.17%, byte-identical
  output). Instant preset switching comes from **precompiling the permutations**
  instead: 4 tiers = 3 distinct pipelines, ≈4.0 ms total at startup, 67 µs to
  re-prewarm.
- **Hot-loop extraction was free:** the column fast-forwards, the global-max
  sky-out and the T1 penumbra term moved out of both coarse loops into named
  functions (`coarse_height_levers`, `soft_penumbra_update`, `ao_distance_fade`);
  AO-off reads 4.723 / 6.530 / 4.379 / 4.918 vs the recorded
  4.723 / 6.509 / 4.391 / 4.943 and the pixel gate still reads 19 / 0. Nothing
  reverted. 54 tests green (was 30).
**Gate (Pascal, in-app):** switching presets visibly changes look and frame time,
and the Quality panel's verdicts read usefully.

### E1d — Directional miss radiance, from VGI ✅ (gate passed 2026-07-30; shipped on Beautiful; 113 tests)
Out-of-slot, approved by Pascal on the source he brought: **Thiedemann, Henrich,
Grosch & Müller, "Voxel-based Global Illumination", I3D 2011**, §5.1 / Fig. 7
point C. An occlusion ray that escapes samples the hemisphere lobes **in its own
direction** instead of the pixel falling back to a flat constant, so the ambient
term becomes a visibility-weighted environment integral. Full tables in the bench
doc's **E1d** section; one registry row (`AO_MISS_RADIANCE`) under
`AmbientOcclusion`.
- **Free: +0.18–0.41%** vs `ao-2ray-d16` (inside the noise band) because it reuses
  rays the RT-AO path already traces — one lobe mix per *escaped* ray, no new
  traversal. Beautiful totals **10.86–18.14 ms** against 10.8–18.1 before, i.e.
  unmoved; the other three tiers run analytic AO, so the lever cannot fire there
  and it ships on Beautiful only.
- **Reach:** 72.5% frame coverage at max delta 116 (scenario C) vs baseline RT-AO's
  34.1% at 55 — the medium-scale directional band E1b's analytic corner AO gives
  up and CAGI only partly covers.
- **Structural consequence:** the integral is already visibility-weighted, so it
  *replaces* the hemisphere term instead of being multiplied by the AO factor
  (multiplying would double-count occlusion), and the `strength` knob stops
  applying to that term while still scaling the CAGI volume. The occlusion multiply
  therefore moved into `indirect_light`, with the lever-off branch preserving the
  original arithmetic **order** — section 1 reads 4.709 / 6.518 / 4.383 / 4.931 and
  the pixel gate still reads 19 / 0.
- **Catch, ships with it:** ambient becomes Monte Carlo, so E1's 2-ray crosshatch
  now lands in ambient *colour* — grain in dark foreground. 4 rays would cost
  ≈ +6.8 ms; the cheap fix is backlog **B12**, which this makes the third
  independent argument for.
- **Documented negative:** sampling the raw sky function (luminance-normalized, so
  the level matched) turned shadowed grass teal and rock purple — those constants
  are emitted radiance through inverse Reinhard, so their chromaticity cannot serve
  as an ambient tint. Sampling `ambient_light` needs no calibration constant.
- **Rejected from the same paper:** its per-pixel hemisphere gather architecture,
  on E1's 2.25–3.55 ms per marginal full-res ray and the paper's own 123 ms
  full-res figure. Its ε back-projection trick is parked for E5 (a lantern's RSM is
  tiny → zero-ray emissive fill).
**Gate (Pascal, in-app):** passed on the scenario-C pair — no degradation, grain
acceptable at this tier.

### E2 — World authority, threading & edit pipeline (ARCHITECTURE) ✅ (measured 2026-07-30: **variant B wins**; 90 tests)
The hard-to-reverse one, done early on purpose. Full tables, the C readback
numbers and the reasoning in the bench doc's **E2** section; the settled model is
the "Threading & data-flow model" section above. New code: `src/world_edit.rs`
(levers + the delta), `src/world_host.rs` (the authority + its thread),
`src/voxel_dda.rs` (the CPU traversal picking and E8's audio rays share),
`Brickmap::set_voxel` + `BrickmapEdit` in `src/brickmap.rs`, delta uploads in
`passes/world_bindings.rs` + `passes/cagi.rs`, 4 registry rows under a new
`WorldEdit` subsystem.
- **Verdict: (B) CPU-authoritative + world thread.** (A) inline stays an off-lever
  (better latency — 0.04–0.11 ms vs one frame — and the right Quest tier if a
  second core is unaffordable), but it cannot bound its worst frame. (C)
  GPU-authoritative is **dead for the world's authority**: a GPU→CPU readback costs
  **1.29 ms regardless of size** (64 B ≈ 43.8 MB), so no delta scheme amortizes it,
  and non-blocking mapping turns the cost into **7–10 submit/poll cycles of
  staleness** for a mirror E8 needs exact.
- **The threading argument is one row pair, not a general slowdown:** with the
  shipped clearance strategy A and B are indistinguishable (max 0.42 vs 0.25 ms);
  force the expensive one and the same work is a **33.3 ms frame hitch inline
  (41.1 ms whole frame) versus 1.4 ms + 8 frames of latency threaded**. The world
  thread does not make edits cheaper, it makes the worst edit not a frame problem —
  which is exactly what E3/B6/B8 will need.
- **Gate met with margin:** holding the place button costs **0.3 µs of CPU and 14
  bytes of upload per voxel**; a 4-edits-per-frame storm adds 0.065 ms median /
  0.123 ms max to the frame thread, and the `idle` row is 0.000 ms (no shader file
  changed; the section-1 pixel gate still reads 19/0).
- **Distance field, the asymmetry:** adding solid only shrinks clearance (exact
  chebyshev shell walk with an exact early-out); removing it can grow clearance
  arbitrarily far, so the shipped strategy is a **bounded local recompute
  (radius 8, 258 µs)** whose error is provably ≤ the freed brick's own new
  clearance *independent of the radius* — i.e. **exact for every edit into
  terrain** — against the full rebuild's 31.5 ms and 500 KB. Never overestimates,
  which is the property the traversal's empty-cube skip requires.
- **Memory:** 46.4 MB CPU / 46.4 MB GPU, of which **2.4 MB per side is edit
  headroom** (4096 spare brick slots so materializing a brick is a word patch).
  Fixed-size slots + a LIFO free list mean **no fragmentation to manage**.
- **CAGI response: a global re-flood, 32 frames (0.53 s) to bit-exact** and free
  per frame — fine for a block, wrong for a lamp, so E5's dirty-region flood has a
  concrete hand-over list (dirty AABB from the deltas, dispatch-size uniform,
  dilated boundary, per-region iteration budget).
- **E8 seam delivered early:** `voxel_dda::cast` / `path_is_clear` over the
  authority itself — **0.94 µs per occlusion ray, 0.96 µs per reflection cast**.
**Gate (Pascal, in-app):** hold-to-place and hold-to-dig at 60fps+ with no visible
hitch, and the light settles within ~half a second.

### E2b — Player walk mode + voxel collision ✅ (backlog B3, PULLED FORWARD on request 2026-07-30; measured: **0.62–0.96 µs per movement step, 4.04 µs through a 1 s hitch**; 119 tests incl. the pool tool below)

**Why this is here and not in the backlog.** Pascal asked for B3 the moment E2
landed, out of ladder order, and the two reasons both pay off *before* E3:
**presence / VR groundwork** — E9's player is a body, not a flying camera, and
E8's HRTF listener is that body's *head*, so the sooner the body exists the
sooner both have something real to attach to — and **judging the look from eye
level**: every lighting gate so far (S2, E1, E1b, E4) was flown, and a 60 m
top-down view is not what this engine is for. E2 also made it cheap: the
authority (`WorldHost::shared()`) and the CPU query path already existed, so the
controller is a *consumer* of E2's seam rather than new infrastructure.
New code: `src/character.rs` (the whole controller, pure math), two material
predicates in `src/material.rs`, the mode toggle + readout in `main.rs` /
`overlay.rs`, and bench **section 7**.

- **Modularity, same rule as the camera:** `src/character.rs` has no winit, no
  wgpu and no renderer type. Its only world input is `&Brickmap` — the
  `Arc<RwLock<Brickmap>>` read guard derefs straight into it — and it speaks
  world meters. **The seam it exists for is stated in its module docs:**
  `eye_position()` at head height is the pose atrium's HRTF listener should take
  at **E8** (the same `&Brickmap` resolves occlusion via `voxel_dda`), the same
  body is the **E9** VR player with yaw/pitch coming from a tracked head pose
  instead of a mouse, and `head_submerged()` is the flag **E6** will read.
- **Body + collision.** A 0.6 x 1.8 m axis-aligned box (4.8 x 14.4 voxels) with
  the eye at 1.65 m, resolved **per axis (X, then Y, then Z)** and **swept**: each
  axis move tests the body's cross-section against *every voxel layer the leading
  face crosses*, not just the layer it lands in, and stops 1 mm short of the
  first blocked one. Blocking is `Voxel::is_solid()` via a new
  `material::material_blocks_movement`, so water and thin cover (tall grass,
  flowers, reeds, lily pads, weeds) are walked *through* — sandbox parity. Note
  the consequence: `is_solid` counts **leaves**, so a canopy is standable here
  where sandbox drops you through it (one predicate away if it reads wrong, and
  it makes a tree an obstacle with no special case — sandbox needed a separate
  trunk mask).
- **Gravity 22 m/s² · jump apex 1.2 m · step-up 0.375 m.** Gravity is sandbox's
  snappier-than-real 22.0; the jump *speed* (7.27 m/s) is derived from the apex so
  the tunable is what you can see. Step-up is **exactly 3 voxels**, not the round
  0.35 m: natural terrain steps are 1–3 voxels and 0.35 would miss the 3-voxel
  case by 2.5 cm. It is implemented as lift → retry → settle from the *pre-move*
  position, so a 4-voxel step still stops the body dead (tested both ways), and a
  matching **step-down snap** keeps the feet on the ground walking downhill
  instead of hopping.
- **Anti-tunneling, two independent mechanisms.** (1) `delta_seconds` is clamped
  to 100 ms and split so no substep moves more than **0.25 m — strictly less than
  half the body's smallest dimension**, which is the condition that makes per-axis
  resolution safe against corner cutting. (2) A sweep **refuses to move past a
  layer it has not tested**, so even a call that escaped (1) stops at the last
  verified voxel boundary. Proof:
  `absurd_frame_deltas_never_end_inside_solid` fires the body in 24 directions
  from 4 start points at maximum speed with 40 ms / 250 ms / **1000 ms** deltas —
  288 runs, 8 steps each — and asserts it is never inside solid *and* that a
  quarter of the runs were actually obstructed (so the fan still exercises
  collision).
- **Water v0, per-voxel and cheap** (three point samples per substep, no global
  water plane): feet in water = `Wading` (horizontal x0.55), water over the
  shoulders (0.8 x body height) = `Swimming` — ±2 m/s vertical cap, jump = up /
  crouch = dive, **no ground requirement**. Vertical drag (4/s) is applied whenever
  *any* part of the body is wet, not only while swimming, which is what stops the
  swim/wade boundary dithering at the surface: the test asserts the float line
  moves **< 0.1 m over a whole second** and that a resting swimmer's eye is above
  water while holding dive submerges it.
- **Buoyancy is a spring to the SURFACE, not a lift** (corrected 2026-07-30 once
  the pool below made the swim feelable; Pascal, from voxel-sandbox: *"we were
  floating when we couldn't stand, and I needed to press down mostly when near the
  surface; if deeper we didn't float"*). The shipped model is one restoring force
  whose strength **fades with depth** — `stiffness 12 /s² x displacement x (1 - t)
  - 0.5 m/s² x t`, `t = min(displacement / 0.75 m, 1)`, displacement measured from
  the float line half a voxel under the local surface (a bounded upward probe, ≤ 10
  voxel reads). Near the surface the body is pinned head-out and must actively
  dive; past the 0.75 m band it is neutral and drifts down at 0.125 m/s, so a
  swimmer can hold a depth. The first version was a *constant* +8 m/s² against
  quarter gravity, i.e. +2.5 m/s² at every depth, which corked the body to the
  surface from anywhere in the column — the numbers, the regimes and the
  regression test that catches the old shape are in the bench doc's **E2b
  correction**.
- **Toggle: `F`** (the only free key near WASD; announced in the overlay, which
  reads `mode: WALK (F = fly) | 4.5 m/s` and `body: grounded | wading | 0.7 us`).
  Fly stays the default — bench and dev work want a camera that goes anywhere.
  Entering walk mode snaps the body to the ground under the camera and **lifts it
  clear if the camera was inside terrain**; entering fly mode keeps the eye
  exactly where the head was. Mouse-look, the mouse-wheel speed knob (it tunes
  whichever mode is active) and E2's click-to-dig/place all work in both.
- **Cost: 0.08 µs idle, 0.62 µs walking, 0.96 µs sprinting, 1.58 µs through a
  40 ms hitch, 4.04 µs through a 1 s hitch, 6.17 µs to enter walk mode** (bench
  section 7, M3 Max, medians). That is **0.01–0.05% of an 8 ms frame** — the
  movement step is not a budget item, and the interesting finding is *which way* it is
  expensive: `any_blocking_voxel` early-outs on the first hit, so **open air is
  the costly case and dense terrain the cheap one**, the opposite of the usual
  intuition.
- **Registry: deliberately no rows.** `CharacterSettings` is its own type outside
  `RenderQuality`, because the registry's rows carry *measured frame-time
  verdicts* and drive shader permutations, and movement feel has neither. The
  pinning tests (`every_settings_field_has_a_registry_lever`) are untouched.
- **Deferred, explicitly:** no underwater rendering / Snell's window (**E6** owns
  it — this ships only the `head_submerged` flag), no swim-along-view-direction
  (horizontal + explicit up/down keys), no crouch, no head bob, no
  push-out-if-stuck beyond the walk-mode entry lift, no fluid-flow coupling
  (**B6**), no gamepad.
**Gate (Pascal, in-app):** press F — does the island read at eye level, does the
auto-step make walking terrain pleasant rather than a jump puzzle, and does
falling into water wade/swim/float the way it should?

#### E2b test tool — the swim-test pool (`P`) — a TOOL, not a stage (2026-07-30)

The island's own water is **0.6–1.75 m** deep, every column of it under the 1.44 m
swim threshold, so the swim states above were testable in code and unreachable in
the app. `src/debug_pool.rs` carves one on demand: **8 m of water across, 5 m
deep, with a 4 m graded shore**, centred 10 m in front of the eye, triggered by
**`P`** or the overlay's **Debug tools → "Carve 5 m water pool ahead (P)"** button
(worded to say it modifies the world). Both flanks are `smoothstep` ramps, so the
worst gradient stays inside the 3-voxel auto-step and you **walk** in and back out
— crossing wade → swim on foot is the point.

- **Through E2's pipeline, never through generation.** Every voxel is a
  `Brickmap::set_voxel` behind the authority. Changing `voxel-core` would
  invalidate every recorded bench baseline and pixel gate (they are tied to the
  seed-1 island) and would move voxel-sandbox's world too.
- **It writes water, it does not just dig.** Nothing makes water flow yet (**B6**),
  so removing bed voxels alone would leave a dry pit: each column gets a water span
  up to `WATER_LEVEL` and an air span above it.
- **New capability it needed: a bulk-edit entry point** —
  `world_edit::BulkEdit` (a shape the authority expands into `VoxelSpan`s on the
  world thread) + `WorldHost::request_bulk_edit`, publishing ONE delta whose dirty
  word ranges are coalesced across every voxel. **130 634 voxels, 116.9 ms on the
  world thread, 503 KB in 696 uploads, 0.001 ms worst on the frame thread while it
  works and 4.7 ms on the frame that uploads** (bench section 6). E3's generation,
  B6's fluid CA and B8's streaming are the same shape and inherit it.
- **The finding worth carrying to E5:** the upload frame first measured **93 ms**,
  all of it `write_buffer` call overhead for 28 672 four-byte CAGI attribute
  writes; grouping consecutive cells took it to 4.7 ms. At ~3 µs of fixed cost per
  call, *calls* are the budget, not bytes — E5's dirty-region flood and B6 must
  publish region-shaped uploads.
- **CAGI needed nothing new:** the existing global re-flood invalidation handles a
  130 k-voxel change on the same curve as a 256-voxel wall (32 frames to 18
  differing cells, 64 to bit-exact).
- **No registry rows** (a one-shot world action has no frame-time verdict and no
  shader permutation), and the acceptance test is
  `character.rs::the_test_pool_makes_swimming_reachable`: carve, wade in, reach
  `Swimming`, float head-out.

### E3 — GPU world generation ⬜ (pulled early per Pascal — optimize from the start)
Port generation to compute and A/B/C it: (A) CPU voxel-core (baseline),
(B) WGSL port of the island column stack (noise → columns → bricks written
directly into brick buffers on GPU), (C) VoxelChain-style subdivision + CA
rule-table generator (public-source reference; discrete rules, no smooth
noise), (D) cave variants on top (density carving vs CA growth). Measure:
full-world gen ms, bytes moved CPU↔GPU, brick count/memory, visual verdict
per variant (bench PNGs). Feeds infinite-world later; must respect E2's
authority verdict (occupancy mirror for audio either way). **Gate:** regenerate
the island from a seed slider in-app; gen time readout; pick the keeper.

### E4 — CAGI v0: sun + sky flood ✅ (gate passed 2026-07-30: "looks very good"; 33 MB, ~1.2 ms, 0.53 s convergence, 72 tests)
Integer RGB light volume (10:10:10 in one u32 + 2 flag bits — 8:8:8 would cost
the same bytes, so the wider channel is free headroom for the integer division),
**ping-pong double buffer** (xima's stated preference, and the only way the CA
stays deterministic), sun + sky injection only, N iterations/frame. Full tables,
per-lever verdicts and the compromise-checklist findings in the bench doc's **E4**
section; 10 registry rows under the new `Gi` subsystem; new pass module
`src/passes/cagi.rs` + `shaders/cagi.wgsl`, with `shaders/world.wgsl` split out of
`dda.wgsl` so the CA pass compiles the SAME traversal core it traces its sun rays
through (no copied DDA), and `src/passes/world_bindings.rs` owning the brickmap
buffers both passes bind.
- **Resolution: 0.5 m cells (4 voxels) shipped, 33 MB total** (2×11 MB ping-pong +
  11 MB static attributes), vertically clamped to the occupied height + 2 cells
  (44 of 64 rows = −31%). 1 m cells = 4.3 MB and 5.8× cheaper = **the Quest tier**;
  0.25 m cells = 258 MB and 6× the cost = **dead**.
- **Cost: +0.40–0.51 ms sampling in the shading pass + 0.92–1.52 ms for the CA
  pass** at 2 iterations. Frame totals: Potato 2.5–3.9 (GI off) · Quest 3.6–5.5 ·
  **Balanced 5.96–8.81** · Beautiful 10.8–18.1 ms. Balanced holds the ~8 ms target
  on the three player-facing scenarios and is 10% over on the aerial low-sun view.
- **Rule A/B: 6-neighbour diffusion beats the max-decrement flood for free** (same
  6 loads, the pass is bandwidth-bound; 66% of the frame differs, mean 8.8/255, the
  flood reads flatter). 26-neighbour diffusion — the anisotropy fix — **rejected on
  terrain**: 2.1–2.7× the cost for a mean 0.5/255, because sky light is injected
  everywhere above the terrain so transport distances are 1–3 cells. Kept for E5's
  point lights.
- **Sky injection needs no ray: the per-column max-brick-Y buffer (binding 8) is
  sufficient** and free; the exact upward trace costs +33–53% and disagrees on 33%
  of the frame at mean 2.1/255 (the 1 m brick column makes cells beside a trunk read
  "covered" until diffusion fills them back in). **Sun injection is one shadow ray
  per candidate cell** through the shared `trace_shadow_visibility`, tinted by the
  neighbour cell's albedo — and its **ray RESULT is cached in bit 30** (−10 to −19%
  of the CA pass at byte-identical output; caching the cell's *value* instead was
  measured as a real defect and reverted).
- **Convergence: 32 frames (0.53 s) to bit-exact, 16 frames (0.27 s) to max delta 1**
  after a cold start or a sun change; the sun sliders re-flood every frame of the
  drag.
- **Correctness: the CPU cross-check predicts every propagating cell of all three
  rules with zero mismatches, a re-flood reproduces the volume bit for bit, no
  absorber holds light, no channel saturates.** Noiseless is an integer identity
  here, not an impression.
- **Compromise checklist:** latency present but cheap while the world is static;
  anisotropy structurally real but invisible at these transport distances (re-test at
  E5); glowing walls structurally prevented (absorbers are written to 0 and dropped
  from the trilinear taps); **thin-geometry leaks present and expected** (a cell
  absorbs at quarter fill, so sparse canopy transmits — reads as light-through-foliage,
  and it is resolution-bound); long-distance transport deliberately weak (12.8 m
  reach, calibrated per meter so the resolution lever cannot change the physics),
  which is why 25% of the E1c hemisphere ambient stays as a readability floor;
  directional detail intentionally absent (rays own it).
- **No traversal regression:** with the lever off, 4.706 / 6.517 / 4.359 / 4.913 ms
  against the recorded 4.723 / 6.530 / 4.379 / 4.918 and the pixel gate still 19 / 0.
**Gate (Pascal, in-app):** are shadowed areas lit by directional, colour-bled bounce
light — noiselessly — and does a sun drag re-flood cleanly inside a second?

### E5 — CAGI v1: emissives + live editing ⬜
Emission table per material; lantern place/remove (**E2's edit API + input are
done** — `Brickmap::set_voxel`, the world thread, delta uploads, left/right click
with hold-to-repeat, and the incremental distance-field update; E5 only adds the
emissive material and the regional flood); dirty-region re-flood. E2's hand-over
list for that flood, measured: a dirty cell AABB accumulated from the edit deltas
(the deltas already report touched cell indices, so it is free), the CA dispatch
size as a per-frame uniform instead of the grid size, a boundary dilated by the
transport reach, and a per-region iteration budget — 0.44–0.76 ms buys the WHOLE
grid, so a 16³-cell region can be flooded to convergence inside one frame instead
of E2's 32-frame global re-flood. E4 hands
over three measured levers to reach for here: **`CAGI_RULE = 2` (26-neighbour
diffusion)**, whose isotropy is worth nothing on sky-lit terrain but is the whole
look of a point light; **`CAGI_SKY_TEST = 1`**, if canopy interiors need the exact
test; and the **`gi_params.w` slot**, reserved for the emissive scale. The dirty
flood must be regional — E4's global re-flood takes 32 frames, which is fine for a
sun slider and not for a placed lamp.
**Gate:** place a lantern → warm light bleeds around corners, zero noise;
edit→light latency number.

### E6 — Water: reflections + refraction ⬜
Fresnel-weighted reflect + Snell refract continuation rays on water voxels
(reuse `trace`), absorption tint by traveled distance, underwater camera +
Snell's window (sandbox reference look). After CAGI so secondary rays see GI.
**Gate:** mirror at grazing angles, see-through steep, Snell's window from below.

### E7 — Look pass ⬜ (Pascal's wishlist, 2026-07-30, after the E4 gate)
His words: *"things that are missing to judge better are for sure things like
bloom and depth of field (+ bokeh / gaussian mode picker) with the better sky
day night time from the bevy version, we also had clouds and fog kinda things
… they do change the light a lot."* Note the reasoning: these are not garnish,
they change how all lighting reads, so **every later lighting judgement is made
against a less complete baseline until this lands.** Split by whether it needs a
pipeline change:

**E7a — cheap look pass (no pipeline change, all IQ technique-bank tricks):**
three-channel exponential atmosphere/fog (T2), sun glare, vignette, smoothstep
contrast + per-channel grading, rim/silhouette highlights, foliage normal
blending (T3), distance-faded local tweaks. All a handful of ALU inside the
existing shading path; each a registry row. Biggest look-per-ms in the plan.

**E7b — HDR + post chain (real pipeline restructure):**
- **HDR intermediate** (rgba16float) replacing today's tonemap-then-store-sRGB,
  because **bloom is impossible without it** — you cannot recover blown
  highlights after Reinhard + 8-bit sRGB. Tonemap moves to the end of the chain.
- **Auto exposure** (histogram or log-average reduction) — mandatory once E5's
  emissives and dark interiors coexist.
- **Bloom**: threshold → downsample chain → blur → composite.
- **Depth of field** with a **bokeh / gaussian mode picker** (Pascal's ask) —
  needs a depth output from the DDA pass, which also **unlocks backlog B12
  (SSAO/G-buffer)** and E1's rejected half-res AO.
- **Day/night sky + sun colour ramp**, ported in look (not code) from
  voxel-sandbox's Bevy sky; drives the existing sun az/el uniform, so it
  exercises CAGI's re-flood path continuously — a functional test as well as a
  look feature.
- **Clouds**: volumetric density accumulation + low-pass-gradient lighting +
  near-free cloud shadows via plane-intersect (technique bank).

**Gate:** screenshot-worthy still — the Voxile mood; day/night cycle runs
without hitching CAGI.

### E8 — Audio bridge (can be pulled earlier on request) ⬜
`VoxelDdaResolver` in atrium: CPU DDA over the occupancy mirror → direct +
occlusion + early-reflection `PathContribution`s → background thread → rtrb.
**E2 already built the traversal half**: `voxel_rt::voxel_dda::{cast,
path_is_clear}` takes `&Brickmap` and nothing else, speaks world meters, and
returns hit voxel + face voxel + integer normal + material — **0.94 µs per
occlusion ray, 0.96 µs per reflection cast** on the M3 Max, over
`WorldHost::shared()` (an `Arc<RwLock<Brickmap>>`) which is the *authority*, not a
readback mirror, so it is never stale. E8's remaining work is the atrium side: the
resolver thread, the material→absorption table and the `PathContribution` mapping.
Environment metrics (enclosedness, sky fraction — cheap ray stats) reserved
for the ambience coupling (backlog B7). **E2b delivered the listener half**: the
listener pose is `character::CharacterController::eye_position()` at 1.65 m on a
body that stands, wades and submerges (`head_submerged()`), not a free-flying
point — which is what makes occlusion and early reflections read as *being
there* rather than as a camera effect. **Gate:** fly behind a hill → source
muffles; enclosed space → reverb tightens.

### E9 — Quest spike ⬜
aarch64-linux-android cross-compile, OpenXR loader, single-eye DDA on device
(wgpu↔OpenXR interop per bevy_mod_openxr). Render-scale + iteration levers =
the tier knobs. **E2b delivered the player half**: `src/character.rs` is the VR
body already — gravity, swept collision, step-up and the water states are
winit-free, and the platform layer only has to feed yaw/pitch (and later the
room-scale offset) from the tracked head pose into the same `CameraPose` slot the
fly camera fills today. **Gate:** runs on Quest 3.

---

## Backlog (dossier-sourced menu — scheduled only when an E-slot opens)

- **B1 Sub-voxel emission** (glowing berries; needs E5 + E7 exposure range).
- **B2 Wind by sky occlusion** — mask `voxel_core::wind` by sky visibility from
  the CAGI volume/AO data; couples visuals AND audio wind (our unique three-way
  link; his engine has the visual half only).
- ~~**B3 Player walk mode + voxel collision**~~ → **promoted to E2b** (pulled
  forward on Pascal's request, 2026-07-30, right after E2: presence/VR
  groundwork and judging the look from eye level). Landed on the CPU over E2's
  authority; the numbering of the other B-slots is left alone because other
  entries reference it.
- **B4 GPU particles + collision** (cascaded, spatial bucketing A/B; needs E2).
- **B5 Entity GPU voxel splatting** (Blockbench, OBB/SDF + local DDA, indirect
  passes — dossier §3 is the blueprint; far future).
- **B6 CA material simulation** (programmable materials → falling sand, then
  **compressible CA fluids**; pairs with our F1 shallow-water work; needs E2+E3).
- **B7 Environment-driven ambience** — darkness/enclosedness/sky-fraction
  metrics driving atrium's wind/rain/ambience synths (his cave-horror idea, done
  with a real audio engine; needs E4+E8).
- **B8 Infinite world / streaming** (needs E3 GPU gen + E2 authority; the
  sandbox's streaming lessons apply: entity-bound not geometry-bound).
- **B9 RT-AO upgrades / bent normals** (E1 follow-on if it wins hard).
- **B10 Procedural vegetation growth** (vines/berries as growth rules;
  VoxelChain rule-table style; needs E3).
- **B11 Gameplay layer** (inventory/items — out of engine scope for now).
- **B12 SSAO + G-buffer** (deferred out of E1b 2026-07-30): depth+normal
  G-buffer, bilateral upsample — unlocks half-res effects generally. Revisit
  after E4 once CAGI's own occlusion contribution is measured.

## Non-goals (still)
Monte Carlo path tracing, meshes of any kind, Bevy interop in voxel-rt,
hardware RTX, temporal denoisers. voxel-sandbox stays untouched.
