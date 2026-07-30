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
experiment slot) · `voxel-rt-bench.md` (harness protocol, baselines, verdicts).

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

**Modularity rule (Pascal, 2026-07-29):** hard seams between platform/windowing ↔
GPU device ↔ render passes ↔ world data ↔ camera ↔ overlay. Pass convention:
self-contained struct, `new`/`rebind`/`encode`. Brickmap stays
renderer-independent (doubles as the audio-ray structure). Camera stays
windowing-independent (doubles as the VR head pose). Platform layer stays thin
(replaced by OpenXR on Quest). Clean seams, never compat shims.

## Threading & data-flow model (to be settled in E2 — current sketch)

- **Main/platform thread:** window, input, egui, surface present.
- **World thread (new in E2):** generation, brickmap/distance-field builds,
  edit application — rayon-parallel internally; publishes immutable snapshots
  (`Arc<Brickmap>` swap) + GPU delta uploads. Never blocks the frame.
- **Audio thread (exists in atrium):** rtrb commands in, audio out; consumes the
  same occupancy snapshots via `VoxelDdaResolver` (E8) — CPU mirror is therefore
  load-bearing even if generation moves to GPU (authority question, E2).
- **GPU:** all per-voxel/per-pixel work — passes composed by the renderer.
- **Async readbacks:** timers today; sound-queue / occupancy readback candidates
  later (GPU-authority variant, E2/E3).

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
  (each dispatched at its own render scale; headline table in the bench doc).
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

### E2 — World authority, threading & edit pipeline (ARCHITECTURE) ⬜
The hard-to-reverse one, done early on purpose. Variants to build + measure:
(A) CPU-authoritative (today): CPU world → brickmap → full upload; edits =
CPU patch + delta upload. (B) CPU-auth + world thread: builds/edits off-frame,
snapshot swap, delta uploads; rayon inside. (C) GPU-authoritative: bricks
live only on GPU; CPU keeps occupancy-only mirror (audio needs it) via delta
readback. Measure: edit latency (place/remove → visible), build times, frame
hitches, memory (CPU+GPU), audio-mirror freshness. **Deliverable: written
verdict in bench doc + implemented winner + `Brickmap::set_voxel` edit API
(the seam building/audio/CAGI-dirty all share).** **Gate:** hold-to-place
blocks at 60fps+ with zero frame hitches; numbers table.

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

### E4 — CAGI v0: sun + sky flood ⬜
Integer RGB light volume (resolution/format A/B: 2×2×2-voxel cells vs 4×4×4;
8-bit vs 10-bit channels), **ping-pong double buffer** (xima's stated
preference), N iterations/frame slider, sun + sky injection only. Probe the
dossier compromise checklist: propagation latency, axis anisotropy, glowing
walls, thin-wall leaks, long-distance falloff — one bench scene per symptom.
**Gate:** shadowed areas lit by directional bounce color; noiseless; sun-drag
re-floods ~1 s; ms + MB table.

### E5 — CAGI v1: emissives + live editing ⬜
Emission table per material; lantern place/remove (uses E2's edit API);
dirty-region re-flood; incremental distance-field update on placement.
**Gate:** place a lantern → warm light bleeds around corners, zero noise;
edit→light latency number.

### E6 — Water: reflections + refraction ⬜
Fresnel-weighted reflect + Snell refract continuation rays on water voxels
(reuse `trace`), absorption tint by traveled distance, underwater camera +
Snell's window (sandbox reference look). After CAGI so secondary rays see GI.
**Gate:** mirror at grazing angles, see-through steep, Snell's window from below.

### E7 — HDR, auto exposure & look pass ⬜
HDR accumulation + auto exposure FIRST (emissives + dark caves = huge luminance
range; dossier shows xima needed exactly this), then fog, DOF (lens-sample ray
origins), tonemap curve, palette polish. **Gate:** screenshot-worthy still —
the Voxile mood.

### E8 — Audio bridge (can be pulled earlier on request) ⬜
`VoxelDdaResolver` in atrium: CPU DDA over the occupancy mirror → direct +
occlusion + early-reflection `PathContribution`s → background thread → rtrb.
Environment metrics (enclosedness, sky fraction — cheap ray stats) reserved
for the ambience coupling (backlog B7). **Gate:** fly behind a hill → source
muffles; enclosed space → reverb tightens.

### E9 — Quest spike ⬜
aarch64-linux-android cross-compile, OpenXR loader, single-eye DDA on device
(wgpu↔OpenXR interop per bevy_mod_openxr). Render-scale + iteration levers =
the tier knobs. **Gate:** runs on Quest 3.

---

## Backlog (dossier-sourced menu — scheduled only when an E-slot opens)

- **B1 Sub-voxel emission** (glowing berries; needs E5 + E7 exposure range).
- **B2 Wind by sky occlusion** — mask `voxel_core::wind` by sky visibility from
  the CAGI volume/AO data; couples visuals AND audio wind (our unique three-way
  link; his engine has the visual half only).
- **B3 Player walk mode + voxel collision** (shader or CPU vs sandbox
  controller as reference; prerequisite for "being there" in VR).
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
