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
with used / lever / open / dead status and the number that decided it)
· `voxel-rt-research-dossier.md` (published papers and released engines, R-numbers,
each triaged to a verdict + ledger row — batch 1 landed the NAADF work and Pascal's
coarse-probe finding).

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
- **To the EDITOR, water IS air (Pascal, 2026-07-31):** *"we need to treat water
  as air basically when adding and removing blocks"* — and *"not on the surface,
  under water"*: a click into a pond belongs on the bed, not floating on the
  skin. This is ONE predicate, not a pile of liquid special cases: the edit path
  has a single notion of "empty" in which every liquid is empty, and both edit
  directions use it. Consequences fall out rather than being decided one by one —
  an edit ray passes through water like air; the placement cell may be a water
  cell (the placed solid *displaces* the water, overwritten today, B6's CA owning
  displacement properly later); and clicking a pond's surface places nothing
  there, because there is nothing there to click. Water still cannot be
  *removed* — it is not a block. Implement it as one shared emptiness test, so
  the next transparent fluid (oil, honey) inherits the behaviour instead of
  needing its own branch; an `is air` check anywhere on the edit path is a bug.

- **A medium has no colour of its own (Pascal, 2026-07-31):** *"water shouldnt
  have ehh an color realy .. water blocks light coming in .. so if a ray comes in
  water the distance it travels the less light comes down."* A participating
  medium is described by **per-channel absorption and scattering coefficients**;
  its colour is a CONSEQUENCE of those and the light, never a painted value. Using
  a material's `albedo` (a surface quantity) as a volume colour is the specific
  bug this rule forbids — it was why E6's water read teal regardless of the
  lighting, and why the flat field outside Snell's window was a flat *teal* field.
  Absorption alone is equally wrong: it sends the depths black, and scattering is
  why deep clear water is blue with no bottom in sight. The coefficient PAIR is
  also the only form that reaches the dossier's target set — clouds are
  scattering-dominated with almost no absorption, honey and oil the reverse — so
  a paint-based model cannot express clouds at all. Expose the coefficients as
  levers, never a tint multiplier.

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
- **Open observation, logged 2026-07-31 (ledger 7.11) — NOT fixed, not in scope
  here.** Those figures are in *frames*, and the CA runs N iterations per frame, so
  **light propagation speed is a function of frame rate**: 1.07 s at 30 fps, 0.53 s
  at 60, 0.44 s at 72 Hz, 0.36 s at 90, 0.27 s at 120. A preset switch or the Quest
  port silently changes the physics rate. E5 records Pascal running the current lag
  and not being bothered, so this is not urgent for GI — it becomes urgent where a
  rate reads as a **speed** (B6 falling sand would fall faster on a faster machine)
  and it is the same class of bug E10's Quest risk note already found for the
  reflection history (*"a 10-frame window at 72 Hz is 140 ms"*). Fix shape: an
  iteration/alpha budget driven by elapsed time rather than frame count — which
  touches E5's parked per-region budget and E10c's history alpha, so all three want
  designing together. Surfaced while pricing a fixed-timestep article (dossier R9)
  whose actual method was already dead by E2's 4.9 ms deep-copy number.
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

### E5 — CAGI v1: emissives + live editing 🟡 (injection DONE, flood PARKED)

**Landed 2026-07-31:** the emission table (M1/M1b — `material.rs`, with
`GlowBlock` + `GlowBerry`), placement (**L** places a glow block through E2's
existing edit path), and the CA injection itself. Emission is stored as a **3-bit
emitter INDEX** in cell-attribute bits 29-31, not a colour — emission is a
material constant, so a cell only says *which* emitter it is and the CA looks the
radiance up in an 8-slot palette riding in the CAGI uniform. It therefore cost
the attribute volume nothing: no second array, no wider word, no new binding.
Slot 0 is black, so a non-emitter needs no branch. An emissive SOLID pins its own
radiance each iteration (so neighbours diffuse it outward); emissive THIN COVER
injects on the air path like the sky term. `gi_params.w` became `emissive_scale`
(runtime, 0-16, linear so 0 can mean lights-off), scaling the palette and the
surface's own emission together. Levers: `CAGI_EMISSIVE` (default ON without a
measured verdict — worldgen places no emitter, so it changes nothing until one
is placed) and the scale.

**PARKED 2026-07-31 at Pascal's request — on the list, not scheduled:** the
dirty-region flood below. Edit→light latency is still E4's ~32-frame global
re-flood; Pascal ran it and was **not bothered** by the lag, so the perf half
waits for a reason to exist. Everything in the paragraph that follows is the
still-valid measured design for when it does. Note the stage gate is only half
met: "light bleeds around corners, zero noise" ✅, "edit→light latency number" ⬜.

**Also found while tuning it:** a bright emitter **clips to flat white** rather
than blooming, because the tonemap is Reinhard straight to an 8-bit sRGB target.
Raising `emissive_scale` makes a light flatter, not brighter. This is a concrete
new driver for **E7b's HDR intermediate** — the emissive scale cannot mean
anything physical until it lands.


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

### E4b — Ray-fed CAGI: trace the indirect rays at PROBE resolution ⬜ **PROPOSED, NOT APPROVED** (Pascal's own finding, 2026-07-31)
From the research dossier's **R2** — Pascal's result from his earlier RT-GI
experiments: *"as soon as you add indirect bounces … you can't really cheat at
some point and need more samples … what works relatively well is to use a much
coarser probe resolution for indirect rays only, so you can increase the sample
count without nuking your performance … allowing you to reach equilibrium faster
since it's essentially less entropy."* Ledger row **2.19**.

E4 made half this bet already, from the other direction — it went to a coarse
volume because a per-pixel gather was priced out (2.25–3.55 ms *per marginal
ray*). The untested half is that coarse resolution makes **many** rays affordable.
Priced on E1's own ladder: E4's **181,928 propagating cells cost ~0.11–0.18 ms per
ray, so 8–12 rays fit in ~1.4 ms** — inside the slot CAGI already occupies
(1.4–2.0 ms all-in at Balanced).

- **Scope is deliberately small: keep E4's storage, ping-pong, vertical clamp,
  incremental attributes (4.7d), emitter index (E5) and trilinear solid-tap
  sampling (2.13e); swap ONLY the update rule.** Traced directions instead of, or
  alongside, the 6-neighbour integer diffusion. No new buffer, no frame-graph
  change, no G-buffer dependency — which is why this does not need to be
  sequenced before E3, unlike a world-data change.
- **Two consumers already waiting.** (1) **E1d's shipped catch** — directional miss
  radiance made ambient Monte Carlo, so E1's 2-ray crosshatch now lands in ambient
  *colour*, and the recorded fix was *"4 rays would cost ≈ +6.8 ms"* at full res.
  Probe-rate sampling is the affordable form of that. (2) **E10c's 1-spp lobe
  noise**, where this is the other axis from the temporal history and the two
  compose.
- **Attacks all three of E4's *structural* compromises**, not its tunable ones:
  12.8 m transport reach (which is why 25% of the E1c hemisphere ambient stays as
  a readability floor), anisotropy (2.13b's 2.7× lever exists precisely because
  diffusion cannot fix it), and *"directional detail intentionally absent — rays
  own it"*. This is rays.
- **The decision the A/B must force, stated up front:** jittered directions destroy
  E4's integer identity (`propagate_reference`: 0 mismatches over 181,928 cells,
  bit-identical re-floods). **Measure the fixed deterministic 8-cone set first** —
  banding instead of noise, determinism kept, and `propagate_reference` still
  applies. The jittered variant is now merely a decision rather than a non-goal
  violation (the 2026-07-31 amendment), but it would extend temporal accumulation
  from E10's reflection buffer into the light volume, which is a bigger change
  than E10 signed up for and should be its own approval.
- **Complement to B13's parked face cache, not a rival:** the face cache amortizes
  over surfaces (N², face space, exact integer edge-stopping, needs E10a); this
  amortizes over a coarse volume that already exists and needs nothing new. E4b is
  the one buildable today.
- **Variants to bench:** rule = diffusion (shipped) / traced / traced+diffusion
  hybrid; 1 / 4 / 8 / 12 fixed cones; round-robin cell subsets vs every active
  cell per frame; against E4's recorded per-scenario CA times and the
  `propagate_reference` cross-check.

**Gate (proposed):** does shaded indirect light gain reach and directionality that
diffusion cannot produce, at ≤ E4's current CA cost — and does the deterministic
variant still pass the CPU cross-check bit for bit?

### E6 — Water: reflections + refraction 🔶 (implemented 2026-07-31, PULLED AHEAD of E5; **look gate FAILED, steps 1-3 landed**; being taken one step at a time at Pascal's request)

**Gate result and the step plan.** Pascal's verdict on the first build: *"the
looking up out of the water part is completely broken :)"*, plus *"snells is too
strong for me personally"*, *"water shouldn't have a colour really"* and *"lets do
one step at the time.. so i can test it and stop after every step"*. E6 is
therefore being finished in numbered steps, each gated on its own:

- **Step 1 — the flat field outside Snell's window. ✅ DONE, awaiting gate.** Past
  the critical angle the ray totally internally reflects and nothing was added, so
  with one interface of budget the whole mirrored region was ONE FLAT COLOUR — most
  of the screen at any tilted upward view. Fixed with a cheap mirrored stand-in
  (one march, cheap shading, no shadow ray / AO / light volume) at **40% of the
  cost of a second full interface for a near-identical frame** — which also
  demoted Beautiful from 2 interfaces to 1. Full tables and frames in the bench
  doc's **E6 step 1** section. It also explains both taste complaints: the flat
  field *is* the in-scatter colour, so the view read as uniformly tinted, and a
  bright cone against a featureless surround reads as harsh.
- **Step 2 — the medium model** (*"water blocks light coming in ... it self should
  probably have a color it self really"*): replace albedo-as-volume-colour with a
  per-material absorption/scattering PAIR so the colour is derived. **Already in
  the tree** (it was written before the step split arrived) — see below.
- **Step 3 — the surface from below becomes plainly transparent. ✅ DONE, awaiting
  gate.** *"lets disable the fesel like camera looking up out of water for now
  should be just transparent looking out and in .. only top should have the
  reflection"*. New lever `WATER_UNDERWATER_INTERFACE`, shipped at `transparent`:
  from inside the medium the interface is fully transmissive and **unbent**, with
  only the path's absorption and scattering applied. The above-water side is
  untouched. **Unbent is the only coherent reading**: total internal reflection is
  not a separable effect, it *is* what Snell's law yields when
  `sin(theta_transmitted) > 1`, so past the 48.607-degree critical angle there is no
  transmitted direction to bend toward — keep the bend and there is nothing to draw
  beyond the window. Accepted consequences: Snell's window disappears from below and
  the surface becomes invisible from underneath; no substitute rim was added. It is
  **cheaper as well as simpler** (−39% / −45% / −11% on the three views where the
  mirrored stand-in used to fire, noise on the other two), and it makes
  `WATER_BOUNCES` and `WATER_TIR_FALLBACK` inert — documented and greyed out rather
  than left as dead dials. The physical `fresnel` interface stays a documented
  off-lever, because the objection was to Snell's window *dominating* the view, not
  to it existing, and wave normals or Quest may flip the verdict.
- **Step 4 — the look levers** Pascal cannot dial today: the index of refraction
  (the window-width dial, half-angle `asin(1/n)`) and the coefficient scales. Not
  started as levers; the uniform slot exists and is pinned at the physical value.
- **Step 5 — bench evidence hygiene**: fix the crop rectangles and add a guard that
  fails the bench when a variant crop set is byte-identical. Not started.
- **Owed regardless: a section-8 re-record.** A concurrent generation workstream
  changed the world's dimensions mid-flight (`WORLD_SIZE_*` now reads 125/32/125
  where every recorded baseline was taken against a 1000-voxel axis), so section 8's
  absolute numbers no longer compare with the E6 or step-1 tables. Every step-3
  verdict rests on within-run comparisons, which are unaffected; the tables want
  re-recording once the generator settles, per the baseline-versioning rule.

**A process failure worth recording, because it is the reason the gate failed on
something the harness said was fine.** E6's underwater PNG claims were written from
crop rectangles that do not contain what they are named for — all eight `f` crops
are byte-identical across variants including `water_off`, and the `g` crop is
identical across six of eight. Worse, the two underwater poses (straight up, dead
sideways) **structurally cannot show the region that was broken**: at a 68° vertical
FOV the frame reaches 34° off-axis vertically against a 48.6° critical angle, so
looking up is almost entirely *inside* the window and looking sideways entirely
outside it. Neither puts the rim in frame. A scenario at 45° of pitch (`I`) now
does. **A crop is not evidence until it is confirmed to contain the thing being
judged, and a scenario is not coverage until it is confirmed to contain the failure
mode.**

---

#### E6 as first built (superseded in part by step 1 above)

**Why it jumped the queue** (Pascal): *"until we have actual opacity in water I
can't really debug and swim"* — E2b shipped walk/swim and the 5 m debug pool, and
opaque water made both unjudgeable. Full tables, every verdict and the PNG
findings in the bench doc's **E6** section; five registry rows under the new
`Water` subsystem; new code `src/water.rs` + `shaders/water.wgsl`, with the
composition in `dda.wgsl` and one new shared helper each in `world.wgsl`.

- **The model, and every constant derived rather than tuned.** Fresnel via
  Schlick with **`F0` = 0.0204 computed from the two indices of refraction**;
  Snell in vector form with total internal reflection as the failure case
  (**critical angle 48.607°**); Beer–Lambert extinction **per channel, per metre
  (0.45, 0.12, 0.06)**, integrated over the path travelled *inside* the water, so
  transmittance at the pool's 5 m is (0.105, 0.549, 0.741) — red gone, blue
  intact, i.e. depth reads as colour. Each is checked on the CPU against a hand
  computation (`src/water.rs` tests) rather than by eye.
- **Index of refraction became a per-MATERIAL column, for zero bytes.** The
  dossier records xima's own transparency target as *"water, oil, clouds and
  honey"* — a material class, not a water special case — so `material.rs` grew
  `index_of_refraction` in the GPU row's former pad word. Retrofitting it after
  the Snell code was written would have been far more work.
- **Cost (bench section 8, over the island + the debug pool):** full optics is
  **+2.40 ms** on a grazing shore view and **+4.64 ms** on the aerial view with
  the most water (14.4% of the frame). The **zero-ray Fresnel tint** tier is
  **+0.36–0.74 ms** and still reads as water with a sun glint, which is what
  Potato and Quest ship. Reflection is the expensive half at grazing angles,
  refraction at steep ones — both track the screen share of what they have to
  *shade*, since every secondary hit goes through the full path (sun, shadow ray,
  AO, CAGI) as the experiment required.
- **The finding that changed the design: liquids must not shadow the sun.** With
  water drawn as a medium but shadow rays still stopping on it, every submerged
  surface is in shadow, so the top-down lakes came out **dark navy where opaque
  water had been bright cyan** — refraction made shallow water *worse*.
  `WATER_SUN_THROUGH_LIQUID` fixes it in the shared `trace_shadow_visibility`, so
  **E4's CA pass inherits it too** and the light volume lights under water. It is
  the most expensive thing in E6 and the cost is concentrated in one view:
  **+77% on a horizontal underwater view** (a shadow ray that no longer stops at
  the surface walks metres of water voxel by voxel), +8% aerial, noise elsewhere.
  Hence a lever, on where water is drawn properly.
- **Recursion budget: 1 interface.** Above water a second interface is **free**
  (inside noise, because a refracted ray that reaches the bed never asks for one);
  from inside the water it is expensive. **Step 1 corrected the conclusion drawn
  from that:** the second interface only looked necessary because the region outside
  Snell's window had nothing but a flat constant in it, and the cheap stand-in puts
  geometry there for 40% of the price — so Beautiful dropped back to 1 too.
- **One optimization worth its row: the Fresnel ray cutoff** (runtime,
  `water_params.z` = 0.04). Fresnel already says what each half of a water pixel
  is worth, so a term below the threshold takes its analytic stand-in instead of a
  ray: **−7.1%** on the steep aerial view, where `full` then costs the same as
  `refraction-only`.
- **In-scatter needed a correction, recorded because it is the difference between
  water and a black hole.** The absorbed share is replaced by the liquid's albedo
  lit by the **downwelling irradiance** (sun × elevation cosine + sky). With the
  sky term alone the body radiance was (0.003, 0.044, 0.134) against a sunlit
  surface's 2.2 and a horizontal underwater view rendered near-black.
- **Underwater is one condition, not two.** The shader tests the primary ray's own
  origin, so it is true for E2b's submerged head *and* for a fly camera under the
  surface; `water::eye_is_submerged` is the CPU mirror, and
  `the_two_underwater_predicates_agree` pins it against
  `CharacterController::head_submerged` through a dive. That test **found a real
  one-substep lag** in E2b's published flag (each substep sampled at its start,
  so the flag described the pose the last substep began at) — fixed by
  re-sampling after the substep loop, three voxel reads per frame.
- **What the PNGs show:** Snell's window reads correctly (the whole upper
  hemisphere — sky, sun glow, the shore's trees — compressed into a cone with the
  trees crowding its rim); a grazing pool returns a sharp mirror of trees, rocks
  and sky and goes see-through toward its far edge; the horizontal underwater view
  is a blue-green fog thickening with distance. **Known flatness with a known
  cause:** E4 marks a cell absorbing at a quarter fill, so cells inside a body of
  water hold zero light and a submerged surface gets only the 25% GI ambient
  floor — the in-scatter is what keeps it readable, and fixing it properly is a
  CAGI-transport question (E5/B6).
- **Balanced's cost, stated plainly:** the ground views hold (7.59 ms default sun,
  9.06 low sun) but the two AERIAL views are now **11.1 / 14.9 ms**, so E6 is the
  first experiment to put Balanced clearly over the ~8 ms target from 60 m. It is
  screen share, not inefficiency, and all three levers to close it are measured.
- **No baseline below E6 moved.** `trace`/`trace_brick` grew a `skip_liquids`
  parameter and it measured **free** (section 1: 4.709 / 6.609 / 4.385 / 4.937 vs
  the recorded 4.723 / 6.530 / 4.379 / 4.918, pixel gate still **19 / 0**), and
  section 8 runs over its own carved brickmap so the island every other section
  measures is untouched. Section 4 *did* move (Balanced ships water) and is
  re-recorded.
- **Also shipped here, at Pascal's direction — "to the EDITOR, water IS air".**
  One shared predicate (`material::material_is_empty_for_edits`) that the whole
  edit path routes through, so a click into a pond lands on the bed either way:
  removing takes the bed voxel, placing puts the block in the water cell against
  it and displaces the water. That is what makes *"place a light in a submerged
  niche"* the same click as placing one in a cave. Water itself is still never
  removable — it is not a block.
**Gate (Pascal, in-app):** mirror at grazing angles, see-through when steep,
Snell's window from below — and does the pool now read as something you can judge
a swim in? *(First build: FAILED on the upward view — see the step plan at the top
of this entry.)*

### Materials arc — the multi-scale material model 🔄 (Pascal, 2026-07-31)
*S0, S0b, S1 and S2 landed and gated; S3-S6 open.*
*Ladder position: a new arc, not an E-slot, sitting **before E7**. E7's fog,
grading and bloom want surfaces with material detail to act on, and E10's
roughness re-author wants an editor to do it in. Full plan at
`.claude/plans/the-way-we-make-eventual-deer.md`.*

Pascal's framing: *"the way we make materials in our voxel engine is suppar to say
the least in any measure or way … this is our basis for getting real nice look and
feel"*. He was right about the state of it: a voxel's entire appearance was **one
flat sRGB triple**, every stone voxel byte-identical, every face of it identical to
every other face — and the table buffer was created without `COPY_DST`, so a live
material edit was not unimplemented, it was *impossible*. Half the columns were
therefore authored blind (roughness a uniform `0.60` on every solid).

**The model: materials act at four scales, and three of them are one mechanism.** A
pattern layer carries a **sampling frame** (world / voxel / face) and a **period in
metres**, and that pair is the whole difference between within-face grain
(`0.02 m`), per-voxel tone (`0.125 m`) and a multi-voxel band (`1 m`). World-framed
layers *cannot* tile per voxel, which is why cross-voxel continuity is a property of
the default rather than a fix bolted on. Only sub-voxel **geometry** needs its own
architecture.

**Two invariants held throughout:** per-voxel storage stays ONE BYTE (nothing here
adds per-voxel state), and the face frame was already built (E1's analytic corner AO
computes an integer face frame at every hit, so everything above sub-voxel geometry
needs **no traversal change at all** — pure ALU on an existing hit).

Landed so far, each gated in-app before the next:

- **S0** — the studio (one voxel, orbit camera, neutral plate, its own brickmap and
  fully excludable), a **writable** material table, and the authored row turned into
  a tagged union (`MaterialKind`: Air / Solid / Cover / Medium, with `emission` an
  orthogonal `Option` because it composes with any kind). This killed the
  `NOT_A_MEDIUM` sentinel on 24 of 26 rows and made `MaterialFlags` *derived* rather
  than hand-written beside the data it describes. **The GPU row stays flat**: a
  sentinel is correct in a wire format and wrong in an authoring format, and the
  shading path must not branch to find out whether a column applies.
- **S0b** — the `.vox` layer, for the tool ecosystem rather than the format (*"there
  are lot of external tools to create .vox files.. edit them .. would be nice if we
  can use that"*). The parser was **lifted into `voxel-core`** so voxel-sandbox and
  voxel-rt share one, and it does the three traps once: the Z-up/Y-up swap, `IMAP`
  index-space resolution (VoxelChain's own reader reads `IMAP` then `throw`s), and
  `_ior` being refractive index **minus one**. Imports **seed existing rows and never
  change kind**, and re-import is non-destructive by storing the post-import row as a
  baseline — a field differing from it was hand-tuned and is kept, a repainted
  palette slot is a reported *conflict* that applies nothing.
- **S1** — face roles (top / side / bottom). Grass is the demonstration case: earth
  sides, green top, which is what stops a cut bank reading as green rock. **+0.5–0.8%,
  i.e. free.** All three roles are explicit overrides including the sides, because an
  implicit-side design forces the base to become dirt and then grass renders BROWN
  with the feature switched off — and the off state has to be the pre-S1 look.
- **S2** — the layer model, built whole at Pascal's direction, with generators in the
  order he asked for (grain/speckle → per-voxel tone → coursing). Three generators
  after the gate (`Flat`, `Noise`, `Speckle`), three frames, three targets, three
  blends, a face-role mask, four slots per row. **Coursing was built and then cut**
  on Pascal's look judgement (*"only mortor brick like thing we dont need thats
  meh"*) — a mortar mask plus a per-brick tone over the same tessellation, working
  and measured, but not a look this world wants. Deliberately *not* kept as a lever:
  variant hygiene exists because Quest may flip a measured PERFORMANCE verdict, and
  no hardware flips a taste verdict. Recoverable from git; section 9's saturated
  stack was re-authored and re-recorded rather than left describing dead code.
  `src/pattern.rs`
  carries a **full CPU reference implementation** of what `shaders/pattern.wgsl` does,
  down to the integer hash, because the WGSL is hand-mirrored and cannot be
  unit-tested — so the tests pin the Rust against hand-computed values and the shader
  against the Rust. The studio gained the **wall (16×16)** and **cube (4×4×4)** poses,
  because continuity and any period over one voxel are invisible on a single voxel.
  Section 9 of the bench is new (S1 had registered a column and nothing ran it) and
  uploads a **saturated** table, since a sweep over the shipped table would report
  four layers as free.

**The texel grid, and the gap it filled.** Every frame and period the model shipped
with was **continuous**, so `Noise` gave smooth mottle and `Speckle` gave round dots —
and neither is what a voxel surface wants. Pascal, looking at a reference frame at the
gate: *"for most cases you want even with spekles and things to keep to the 8x8
sizing"*. So a layer carries **texels per voxel edge**, and the sample position is
snapped to the centre of its texel before the generator runs.

A snap on the *coordinate* rather than a blocky variant of each generator, which is
what makes it one field instead of five: noise becomes blocky noise, speckles become
square specks, and it stays orthogonal to the period (8 texels with a 1 m period is a
large soft field rendered in 1.5 cm squares). Default **8, on** — a default you have to
switch on to look right is in the wrong place.

**The grid is a shared engine lattice, not a pattern setting**, which is the part worth
scheduling around (Pascal: *"if you make .vox and assign material it will nicely snap
to one of the 8x8"*). It is anchored to the world and its size divides `VOXEL_SIZE`
exactly, so it lines up across neighbours and a texel never straddles a voxel edge —
and the same lattice is what a `.vox` model drawn at `n` cells per engine voxel lands
on. **Two consequences, both scheduling rather than work.** *S5:* its "resolution per
material" should be a rung of `TEXEL_RUNGS`, not a second independent number, so a
hand-drawn mask and a generated field share cells and compose instead of one being
resampled onto the other. *B13:* its persistent `(voxel, face)` cache is the dossier's
*"voxel-native equivalent of a surfel/texel-space denoiser"*, and if it ever wants
sub-face resolution — the plan's own CAGI sub-voxel gather does — that subdivision is
this grid. A cache cell and a texel being the same thing is what lets an amortized term
and a procedural term be filtered together. (Note for the record: this is B13's
face-space filter, **not** B5's entity *voxel* splatting, which is a different
technique about getting dynamic objects into the grid.)

Two follow-on notes. Snapping is an **anti-aliasing win**, which inverts the intuition
that hard edges alias worse: a piecewise-constant signal box-filters toward its local
mean, where continuous noise at a sub-pixel period keeps producing new values per
pixel. And the fade still keys off the **period**, not the texel size — fading on texel
size would erase a 1 m band because its texels are small.

**In the studio, the selected ROW is the subject on screen.** Found at the S2 gate:
the panel seeded its selection from the sample once at startup and then the two
drifted, so picking `stone` in the dropdown edited stone's row while the camera went
on showing grass and every slider appeared dead (*"it doesnt re apply i only ever see
grass"*). Two things called "selected" that were not the same thing. The subject now
follows the selection (and the eyedropper already covers the reverse: click a voxel,
select its row). Confined to the studio by a tested predicate — in the island the
selection is the *placement* material, and rebuilding the world because you picked a
different block to place would be absurd.

**`opacity` is deliberately not a pattern target**, and it is the one narrowing worth
stating: it is a *traversal* input, decided before any shading runs, so patterning it
would mean evaluating the stack inside the innermost traversal loop — precisely the
cost this stage avoids, since every layer here is paid once per HIT and never once per
step. A dissolve effect wants it and is a named follow-on with its own cost argument.

**S2 gate: PASSED** (Pascal, 2026-07-31, on a stone wall with a snapped 4-octave noise
grain plus red emissive specks: *"looks SO GOOD! now i can make a stone with emission
specles thats awesome!"*). Worth recording *what* passed, because it was not a case
anyone designed: **emissive specks are `target: emission` x `blend: add` x the 8-texel
snap composing into a material no code path was written for.** That is the argument for
keeping generator, frame, texel grid, target and blend orthogonal rather than shipping a
list of named effects — the combinations outnumber what can be anticipated.

**S2c — patterned emitters cast light, and a bug in S0's tier story.** Pascal, on
seeing the specks: *"i do think it makes sense to be able to let them emit light"*.
Agreed, and getting there exposed something worse than the missing feature.

**The correction first, because it invalidates something written during S0.** The panel
claimed a two-tier model — albedo/transmittance/emission live in direct shading, stale in
the GI bounce "until a re-pack". The tiers are real, but **the re-pack read the COMPILED
material table**, so it recomputed the attributes it already had: a material edit could
never reach the bounce at all, and the button was a no-op for exactly the case it was
added for. That was wrong when written and is now fixed.

**The fix is a seam, not a plumbing change.** Passing `&[Material]` down to the attribute
builders does not work — the incremental per-edit path runs on the world thread and would
need an 8 KB copy per 2.8 us edit. But the builders do not want materials, they want
*"material id -> packed attribute word"*: `MaterialAttributes` is that, 104 bytes and
`Copy`, so it rides in a `VoxelEdit` across the thread boundary for free. The emitter
palette still needs the rows (it carries f32 radiances) and is rebuilt per uniform upload
on the render thread, where the live table is at hand. The volume uniform gained
`COPY_DST` for the same reason — a live palette had no way to reach the GPU either.

**What a 0.5 m GI cell can represent, and why a mean is the right answer rather than a
compromise.** CAGI holds a 3-bit emitter index per half-metre cell; it cannot carry
per-texel structure and never could. It does not need to: the light arriving *somewhere
else* from a speckled emissive surface is that surface's **average** emission times its
area. So the two tiers are not one model approximated twice, they are the **near field
and the far field**, and each now gets the right quantity — full detail to the pixel,
`Material::mean_emitted_radiance` to the volume. The mean is obtained by evaluating the
CPU reference over a grid on each of the six faces, weighted by face count so a
top-masked layer contributes its real share; numerically rather than analytically, so a
future generator cannot silently inherit a wrong closed form. It is the pattern
evaluator's second real use after the WGSL cross-check.

Two consequences worth knowing: `is_emissive()` widened, so **a patterned emitter claims
one of the seven palette slots** — that is the real budget this spends. And brightness
reaches the volume as you drag (the palette re-uploads on a dirty table), while a row
becoming emissive for the *first* time still needs the explicit re-pack, because which
CELLS hold an emitter lives in the attribute volume.

**S2d — two reasons the emitter still could not be SEEN, both found by trying.** The
mechanism worked and the result was invisible, and neither cause was in the material
model (Pascal: *"i cant realy see it emiting .. might be as well that we dont have the
right sky or light conditions .. we have pretty crude over head light … we can also not
controll emition amount like real radiate"* — both halves correct):

1. **The light could not be turned down.** `SUN_INTENSITY` was a hardcoded `2.2` and
   `AMBIENT_STRENGTH` a hardcoded `0.4`, with `SunSettings` carrying direction only, so
   every emitter was judged against fixed daylight. **An emitter cannot be judged against
   a light you cannot dim** — that is a general point about this renderer, not a material
   one. `SunSettings` gained `intensity_scale` and `ambient_scale`; both to zero is a
   genuine night with only GI and emitters left. The uniform already carried both fields,
   so this was exposure rather than plumbing.
2. **A patterned emitter could not be bright.** The emission target's value was drawn
   with an `egui` colour picker, which clamps to 0..1 — and with `amount` also ≤1 the
   maximum authorable radiance was **1.0**, against `glow_block`'s **3.0**. So a
   patterned emitter was structurally incapable of matching the dimmest existing one.
   Emission is *radiance*, not a colour, and is allowed above 1 because a source may be
   brighter than any surface can reflect. The first fix replaced the picker with three
   raw 0..16 channels, and that was the wrong trade (*"why not the picker we had before?
   why 3 fields?"*): **picking a hue and exceeding 1.0 are separate problems**, so they
   are separate controls — the picker keeps the colour, and `emission_intensity` (0..16)
   scales it. `target_value()` multiplies them into the one value everything downstream
   reads, which is why the field **costs no bytes and reaches no shader**: `to_gpu` folds
   it into the uploaded `target_color`, so the WGSL needed no change at all. Recorded on
   `PatternLayer::target_color`: the field is in **whatever space its target is in** —
   sRGB 0..1 for albedo (it mixes before `srgb_decode`), linear for emission (nothing
   decodes it).

Both are levers-in-the-panel rather than new machinery, and together they are what makes
the S2c gate performable at all.

**One hole the gate found and closed: the face frame repeated.** It is voxel-local, so
it drew the *identical* pattern on every face in the world — a visible repeat rather
than detail (*"the face within the face .. this part should still have a randomizer so we
dont have a repeating patern .. doesnt have to seamless like world"*). Fixed with a
per-`(voxel, face)` **hash salt**, on by default, and the salt-not-offset choice is the
load-bearing part: an offset would slide the pattern within the face and break both the
texel grid's alignment to it and any future positional generator's relationship to the
edge it sits on, whereas a salt re-rolls the draw and moves nothing. Zero salt is
mixed with `^`, so "variation off" is exactly the unvaried pattern — which is the
deliberate-motif case, the classic voxel look where every face of a block type is
identical. Confined to the face frame: the world frame must not have it (it would
destroy the continuity that frame exists for) and the voxel frame does not need it.

Still to come: **S3** animation (value oscillation + pattern drift, with `t = 0`
pinned in the harness so the noiseless-identity pixel gates keep working), **S4**
templates, **S5** sub-voxel models (the only stage that touches traversal, so it lands
last, with a three-way A/B), **S6** apply to real materials and re-author roughness
and specular.

**The next blocker this arc creates and does not solve:** material ids are welded 1:1
to `voxel-core`'s 26-variant `Voxel` enum through two hand-mirrored 26-arm matches. A
`.vox` palette holds up to 256 entries, so the moment palettes become how materials
arrive, the weld is the binding constraint — a 40-colour file cannot land in 26 rows.
Out of scope here because this arc adds richness *per row*; VoxelChain budgets
`id: 0-1024` for a reason.

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

### E10 — Specular reflections + temporal reprojection ⬜ (Pascal, 2026-07-31)
*Ladder position: after E7, before E8/E9. Numbered 10 because 8 and 9 were
scheduled first and are referenced elsewhere (same convention as B3 → E2b).*

Pascal's ask, in his own comparison order: **roughness 0% / roughness 20% /
roughness 80% / reflections disabled**, with *stable* temporal reprojection.
Today none of that exists: `roughness` and `specular` are authored on all 26
material rows and read by nothing (`material.rs`'s table calls this slot F2),
the only mirror in the engine is E6's water branch, and there is **zero**
temporal machinery — no history texture, no previous-frame camera, no jitter.
`dda.wgsl`'s "noiseless identity (no temporal accumulation, no per-frame
randomness)" is a deliberate property of the shading pass, and this stage is the
first thing that spends it. It also **reverses a Non-goal** (see below).

**What E6 already gives us, for free.** `shade_secondary` is the terminal for
every secondary hit — full sun + shadow ray + AO + CAGI — with the recursion
stop for liquids, and `shadow_ray_origin`'s integer-anchored reconstruction is
what keeps a secondary ray acne-free at large `t`. A general specular ray reuses
both verbatim. The ray half of this stage is therefore nearly written; what E10
actually adds is **which direction to trace** and **how one sample per pixel
becomes a readable image**.

**Ordering: this stage wants E7b's HDR intermediate first.** The storage texture
is `Rgba8Unorm` holding *tonemapped, sRGB-encoded* bytes, and accumulating a
history in that space bands the tail of the exponential filter and clamps
exactly the blown sun glint that makes a mirror read as a mirror. Two ways out:
land E10 after E7b (recommended), or have E10 carry its own linear
`rgba16float` reflection + history pair, which is a slice of E7b done early.
**E10b (mirror) is exempt** — it composites inside the shading pass before the
tonemap and needs neither HDR nor history, so it can be pulled forward next to
E6 if the look is wanted sooner.

**E10a — G-buffer + reprojection infrastructure (pulls backlog B12 in).**
The DDA pass writes a second storage texture: hit world position (or view
depth), packed normal, material id. Reprojection is **matrix-free in our camera
model** — with the previous frame's `CameraUniform` (position, `forward`,
`right_scaled`, `up_scaled`) a world point `p` maps to the previous NDC by
solving `p - position = forward + x*right_scaled + y*up_scaled`, i.e. one 3x3
inverse of the basis `[right_scaled, up_scaled, forward]` uploaded per frame. No
projection matrices need to be introduced and the camera stays
windowing-independent (VR: one inverse per eye). **No motion vectors**: the
world is static between edits, so camera motion IS the reprojection, and an edit
invalidates history in its region through the seam `WorldDelta` already
provides (the same touched-cell list E5's regional flood uses). Unlocks E7b's
depth of field and E1's rejected half-res AO as a side effect.
Estimate: +0.1–0.3 ms for the extra store; measured as its own bench row before
anything reads it.

**E10b — mirror (roughness 0), inside the shading pass, no history.**
One `trace` along `reflect(direction, normal)` per pixel whose material passes
the specular mask, terminal `shade_secondary`, composited with a Schlick Fresnel
built from the material's own `specular`/F0 — the identical weight water already
computes. Deterministic and noiseless, so the 0% row of Pascal's comparison
needs no reprojection at all. **E6's `water_surface_radiance` mirror branch
becomes a call into this shared `specular_radiance`**, so there is one specular
path in the engine rather than two that drift.
Cost anchor: E1's clean ladder measured **2.25–3.55 ms per marginal full-res
*short* (16-voxel) secondary ray**; a reflection ray is unbounded, so budget
worse per traced pixel and buy it back with (i) the **specular mask** — on
today's authored table that is water alone (roughness 0.05 / specular 0.02),
every solid row being a placeholder 0.60 / 0.03–0.04 — and (ii) a
`ReflectionMaxDistance` lever, with the chebyshev distance-skip doing the rest.
Bench rows: full-res masked, half-res masked, and **unmasked (the ceiling)**.

**E10c — the roughness lobe + temporal accumulation (the actual ask).**
One sample per pixel from a GGX/cosine-power lobe around the mirror direction,
widened by `materials[hit].roughness`, drawn from a low-discrepancy sequence
(R2/Halton) offset by frame index and hashed per pixel. One spp of a 20% lobe is
noise; the history is what makes it an image:
- exponential accumulation into the reprojected history, alpha as a lever
  (start ~0.1, a ~10-frame window = ~80 ms at 120 fps);
- **rejection** on: reprojected pixel off-screen; world-position mismatch beyond
  a depth-relative threshold (disocclusion); normal dot below threshold;
  material id changed; a world edit touching the region; a camera cut
  (teleport, walk/fly switch);
- **neighborhood clamp** to the current frame's 3x3 mean ± k·sigma, which is what
  kills the ghosting the rejection tests miss;
- accumulation lives **in the reflection buffer only**, never over the whole
  frame. The primary shading path keeps its noiseless identity, a still camera
  stays bit-stable outside reflective pixels, and the S2/E1 pixel-diff gates keep
  working.
- **`ReflectionRoughness` override lever with exactly Pascal's four rungs —
  0% / 20% / 80% / off** — forcing one roughness globally so the A/B compares the
  *technique* and not the authored table; the shipped path reads the material.

**E10d — spatial filter + half-res.** Roughness-scaled cross-bilateral blur
guided by the E10a G-buffer, so 80% resolves without a 100-frame window; plus a
half-res reflection buffer with bilateral upsample (the machinery B12 wanted).
Both levers, both with numbers.

**E10e — the honesty pass.** Amend `dda.wgsl`'s noiseless-identity comment, the
Non-goals line, and `material.rs`'s table (`roughness`/`specular`: "authored,
unread" → live). Then **re-author the roughness column**: a uniform 0.60 across
every solid was a placeholder written when nothing read it, and E10 is the first
stage that can see it is wrong.

**Levers** (new `LeverSubsystem::Reflections`, one registry row each):
`ReflectionMode` (off / mirror / glossy), `ReflectionRoughnessOverride`
(0 / 20 / 80 / material), `ReflectionMaxDistance`, `ReflectionSpecularThreshold`
(the mask), `ReflectionHalfRes`, `ReflectionSpatialFilter` — all `ShaderConst`;
`ReflectionHistoryAlpha`, `ReflectionRejectNormal`, `ReflectionRejectPosition`,
`ReflectionNeighborhoodClamp` — all `Runtime`, riding a new `reflection_params`
vec4 on `LightingUniform`, same shape as `water_params`.

**Bench section 9** (own scenario: the E2b debug pool plus a placed reflective
block set, so its numbers deliberately do not compare with sections 1–8). The
four roughness rows x {full-res, half-res} x {temporal on, off}, the
reprojection pass measured on its own, and **two metrics that are not
milliseconds**, because "stable" is the requirement and the overlay cannot judge
it: frame-to-frame mean absolute pixel delta over a *scripted* camera orbit
(crawl/shimmer), and frames-to-converge after a forced disocclusion.

**Gate:** (1) the four rows switched in-app in Pascal's order; (2) scripted
orbit: temporal MAD under threshold with no visible crawl, and ghosting behind
the walking body's silhouette gone within N frames; (3) `ReflectionMode::Off`
**pixel-identical** to E7's output (isolation rule); (4) Balanced stays ≤ ~8 ms
at render scale 1.0 with the shipped reflection tier — or the shipped tier is
half-res and that is what gets recorded.

**Risks, written down now:**
- **Quest (E9).** Reprojection is per-eye at head motion rates; ghosting is far
  more visible in an HMD and a 10-frame window at 72 Hz is 140 ms. Expect the
  Quest verdict to be *mirror only, history off* — which is why E10b is built to
  stand without E10c.
- **Budget.** Balanced is 5.0–7.2 ms today. A masked full-res mirror is
  affordable; an unmasked one is not, and the bench must show both.
- **Water double-count.** E6's Fresnel mix and E10's specular weight must be ONE
  weight, resolved in E10b, or water reflects twice.
- **Every later look judgement** is made against a renderer that now has a
  temporal component — E7's gate should land first for exactly the reason E7
  itself gives.

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
- ~~**B12 SSAO + G-buffer**~~ (deferred out of E1b 2026-07-30) → **pulled into
  E10a** (2026-07-31): the depth+normal G-buffer and the bilateral upsample are
  E10's reprojection infrastructure, so they get built there and the SSAO
  question is what remains of this slot once E10 has paid for the buffer.
- **B13 Voxel-face filtering / face cache** (parked 2026-07-31 at Pascal's
  request — on the list, deliberately not scheduled). From the dossier's
  second-hand intel: xima denoises AO and reflections with a **voxel-face
  blurring filter**, i.e. filtering in face space rather than screen space, so
  the blur cannot cross a silhouette and needs no depth/normal edge-stopping
  weights. Two sizes:
  - **F0 — face-id edge-stopping.** A screen-space blur whose taps are rejected
    unless they share the hit's `(voxel, face)`. Exact integer edge-stopping
    instead of heuristics. The key is already free: `dda.wgsl`'s integer face
    frame (built for analytic corner AO) IS the id.
  - **F1/F2 — persistent `(voxel, face)` cache.** The real prize, and NOT an AO
    feature: it is an *amortization substrate*. Every stochastic term in
    voxel-rt is currently forced to be noise-free per pixel per frame because
    there is no history — which is why analytic AO won E1 and why shadows are
    one hard ray. Best consumers, in order: **soft shadows** (the sun is static
    between re-floods, so accumulation converges and stays valid, and the
    `CAGI_SUN_CACHE` invariant is the same trick one level coarser);
    **CAGI sub-voxel gather** (surfaces scale N², volumes N³ — the right way
    past the 0.5 m / 33 MB vs 0.25 m / 258 MB wall, and probably what xima's
    "voxel scale, not sub-voxel — at least for now" hedge is dancing around);
    then **glossy reflections**.
  - **Scheduling note:** F0's stated blocker was that the DDA pass writes final
    sRGB with no G-buffer and no history — but **E10a builds exactly that**, so
    this rides E10a rather than paying for its own frame-graph change. Do not
    schedule it before E10a.
  - **Lattice note (added by the materials arc, 2026-07-31):** if F1/F2 ever wants
    resolution *below* one face — and the CAGI sub-voxel gather above is exactly
    that case — the subdivision should be a rung of S2's `TEXEL_RUNGS` rather than a
    number of its own. The texel grid is already anchored to the world, divides
    `VOXEL_SIZE` exactly, and is the grid `.vox` art and S5's models land on; a
    cache cell and a texel being the same cell is what would let an amortized term
    and a procedural term be filtered together instead of resampled onto each other.
  - **Honest caveat:** AO is the *weakest* consumer despite being the headline.
    Analytic corner AO is ~20x cheaper and noiseless, and now that E4 CAGI does
    the medium-scale occlusion, ray AO's remaining job keeps shrinking. The
    filter buys quality on terms we have not built yet, not cost on the one we
    have.

## Non-goals (still)
Monte Carlo path tracing, meshes of any kind, Bevy interop in voxel-rt,
hardware RTX. voxel-sandbox stays untouched.

**Amended 2026-07-31 (Pascal):** *temporal denoisers* left this list. E10 asks
for glossy reflections with stable temporal reprojection, and a roughness lobe
at one sample per pixel is unreadable without a history. The amendment is
narrow: temporal accumulation is confined to E10's reflection buffer, and the
primary shading path keeps the noiseless, per-frame-deterministic identity that
made the rule worth having.
