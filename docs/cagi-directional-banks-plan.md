# CAGI Directional Banks — staged plan

Rebuild the CAGI light volume to x1m4's reference design: **6 directional banks of
10-bit RGB at 1/8 voxel resolution**, Moore-neighbourhood transport, directional bounce.
Primary sources: his verbatim property list (2026-08-06, `x1m4-architecture-notes.md` §3),
the quoted rule (2025-12-24, `x1m4-graphics-programming-channel.md`), and TooManyLimits'
matching kernel (`docs/cagi-reference-implementation.md`).

## Why (one paragraph)

Our isotropic volume cannot represent a bounce: reflected light deposits directionlessly
and diffuses as glow. His design stores light *per direction*, so a bounce is a direction
reversal tinted by the surface — `bank[opposite(d)] = incoming[d] * solid_color *
bounce_loss` — and surfaces read only the banks facing them. Every independent
implementer named directionality as CAGI's weakness; the banks are his answer.

## The reference rule (what we're building toward)

- **Storage**: 6 banks × 10-bit RGB per cell, one u32 per bank, SoA planes
  (`index = cell_index + bank * cell_count`), double-buffered. `cell_voxels = 8`
  (1/8 res) → 125×32×125 cells ≈ 24 MB both buffers, less after height clamp.
- **Propagation**: per direction, `max(src, upstream_neighbour) * propagation_loss`,
  loss SUBTRACTIVE: `max(LOSS, L) - LOSS` (2024-02-28). Converges to a fixed point, no
  oscillation, never creates energy.
- **Diffusion**: per-axis `mix(bank_pos, bank_neg, k)` — the "heat conduction" term.
- **Bounce**: `light * neighbor_solid_color * bounce_loss` into the REVERSED bank
  (TML: `input[c][(dir+2)%4] * color[c]`).
- **Moore terms**: diagonal *injection* for emissives/bounces (face-only injection gives
  square halos — his 2024-03-20 bug). Face-wise transport + diagonal injection = his
  "Moore neighborhood".
- **Sampling**: ambient-cube evaluation — weight each bank by `max(0, dot(normal,
  -bank_dir))`; jitter lever against banding (his 2026-07-24 tip).

## Stages (each gated by a real app run)

- [x] **D0 — Plan** (this doc)
- [x] **D1 — Layout + plumbing, behind a lever.** *(gate passed 2026-08-07:
  image identical, light volume 45.8 → 160.2 MiB at 4-voxel cells, exactly the
  predicted 2×(6×4)+8 bytes/cell.)* `GiLayout` lever (Isotropic |
  Banks6), default Isotropic. Rust: `CagiSettings.layout`, buffer sizing ×6, meta
  uniform gains `cell_count`, rebuild on change. WGSL: `CAGI_LAYOUT` const, bank
  indexing helpers. Banks mode runs the EXISTING isotropic rule into bank 0 and samples
  bank 0 — plumbing proven by "looks identical with the lever flipped", isotropic
  default bit-identical. Registry row UNMEASURED. Tests: sizing, index round-trip,
  shader-const completeness.
- [x] **D2 — Directional transport.** *(gate passed 2026-08-07: banks at
  8-voxel cells, directional volume confirmed in-app, default coefficients
  kept.)* Banks propagation kernel: per-bank max-flood with
  SUBTRACTIVE losses (direct along the bank's direction; a steeper-loss lateral
  seep as the heat-conduction spread), sky into the downward bank plus a
  horizontal fraction, sun bounce and E5c emitter bounce injected into the bank
  their surface's normal points along (directional by construction — no
  lambert-mean needed). Emissive cells split 1/6 per bank so SUM over banks ~=
  the isotropic word; the sampler and fog sum the banks until D4. Levers:
  `GiBanksLossPerMeter` (8/m), `GiBanksSideLossMultiplier` (4x),
  `GiBanksSkyHorizontal` (0.25) — all UNMEASURED, tuned at this gate. As built:
  diagonal injection deferred to D3 (watch for square halos on emissives at this
  gate); transmission/reflectance stay isotropic-only until D3. App gate: banks
  at 8-voxel cells — open ground reads like isotropic, and a wall's sun side
  floods bright air while the shadow side stays dark (flip iterations up to
  converge faster). Expect surfaces to still shade omnidirectionally (D4 is the
  sampler).
- [x] **D3 — Directional bounce + transmission.** *(gated 2026-08-07 with two
  findings, both fixed: emitter takeover → `GiBanksTransmission`; hard
  terminator + back-face wrap glow → falloff-hierarchy flip + the D4 sampler.
  Re-verify both under the D4 gate.)* A solid cell's bank d now holds what LEAVES it
  travelling d: the reflectance term is TooManyLimits' kernel — the light that
  arrived travelling d's reverse, read from `cell + dir(d)` bank `d^1`, tinted
  by the cell's albedo and cut by the `GiBanksBounce` fraction (0.5,
  UNMEASURED) — and the M2 transmission term forwards bank d straight through
  from the upstream neighbour at the material's transmitted fraction. Combined
  per bank with max, paying the direct step loss so a bounce cannot outrun
  direct light. Air cells need NO direct-read bypass (the isotropic
  `cagi_reflectance_bounce` problem dissolves: max-transport picks the solid's
  banks up at full strength). At the volume top the reflectance term turns the
  sky's downward bank into an upward ground bounce for free. Both terms stay
  behind the existing CAGI_REFLECTANCE / CAGI_TRANSMISSION levers (defaults
  off). App gate: flip reflectance ON with banks6 — colour bleed in the
  corridor, crisp and directional, converging (not a light pipe); optionally
  transmission ON — light seeps through the canopy, still shadowing.
  **Gate finding (2026-08-07): emitter takeover.** Lava/glow-berry emissions
  sit at the HDR ceiling (level 8184), and with purely subtractive transport
  loss, reach is LINEAR in energy — 8184/8 per meter ~= a kilometre, the whole
  scene. Fix: `GiBanksTransmission` (0.884/m, matching the isotropic
  TRANSMISSION_PER_METER) — a multiplicative per-step decay on top of the
  subtractive losses, the term the reference kernel carries as DECAY +
  attenuations. Reach is now logarithmic in emission (8x brighter = +17 m, not
  +900 m). Re-run the gate with an emissive placed.
- [x] **D4 — Sampler + shading.** *(gate passed 2026-08-07 — "this works!":
  shadow core black behind the wall, lava radius convincing, no through-wall
  or wrap glow.)* `cagi_sample_surface` under banks6 reads per bank
  `max(0, dot(-normal, bank_dir))` — at most three banks for any normal,
  arbitrary normals (relief/water/grass) supported, trilinear twin with the
  same dropped-solid-tap renormalization. NO normalization constant: ground
  under open sky reads the downward bank at weight 1 (the full sky value, the
  same exposure anchor as isotropic); walls read the horizon share, so
  `GiBanksSkyHorizontal` is now the wall-brightness knob. Above-the-volume
  reads reconstruct the CA's own boundary injection. Fog and debug reads keep
  the omnidirectional bank sum. `shade()` contract unchanged. As built: the
  sample-jitter banding lever is DEFERRED until the gate shows banding (the
  D2/D3 runs did not). Three gate fixes folded in en route: the falloff
  hierarchy flip (subtractive loss is the epsilon at 1.0/m, multiplicative
  transmission is the falloff), `GiBanksTransmission` itself, and
  `GiBanksDirectionMix` (0.08/m) — the direction-decay term D2 shipped without:
  without it, lava's up-column stayed labelled "upward" after wrapping a wall
  and painted bottom faces behind it orange, because a bottom face correctly
  reads the upward bank. Landed in two iterations, both gate-measured: the
  literal `mix(lightpy, lightny, x)` opposite-bank version manufactured
  backward light along every beam — exactly the bank a wall's dark face
  samples ("comes through everywhere") — so the shipped term scatters into the
  four PERPENDICULAR banks instead (reversal takes two hops; conservative
  across the six banks, so fog and exposure are untouched). This is a
  deliberate, documented deviation from the quote's literal example.
  Fourth gate finding, and the load-bearing one: the lateral seep had no
  occlusion test at all, so the over-the-wall wrap band re-seeded beams into
  the wall's shadow at full strength regardless of the geometry between —
  Pascal's "you need to take the faces into account". Fix:
  `GiBanksSealPartial` (0.25) — the reference kernel's THREE-TIER CORNER SEAL,
  which its porting notes call the difference between "lighting" and "light
  leaks through my walls". A seep from lateral neighbour L into cell C cuts
  the diagonal bracketed by (C-upstream, L): both solid = zero, one solid =
  the partial fraction, open = full. Still pending from the same family:
  per-FACE opacity from sub-cell occupancy (the kernel doc's step 5) — D5+
  material if thin-geometry leaks show up. App gate:
  the dark side of a thick wall stays dark (the wrap-over light travelling
  away from the face no longer registers); a wall's sun side vs shadow side
  shade differently; ground exposure unchanged vs D3.
- [ ] **D5 — Bench + verdicts.** CPU-port the sampler maths, measure face-luminance
  distributions (corridor scene) banks vs isotropic vs (optional lever) the
  trilinear-gradient approximation. Perf ledger. Flip defaults per verdict; verdicts
  into the registry rows.
  **Started early (2026-08-07):** the "sliders are not the issue" leak round
  forced the CPU transport mirror ahead of schedule
  (`scratchpad/banks_leak_probe.rs` — graduate it into a repo test at D5). Its
  first verdict is already in the registry: `GiBanksTransmission` 0.884 → 0.7
  MEASURED (shadow behind a 10-cell wall was 1/4-1/10 of the lit side at
  0.884 — half-life 5.6 m cannot darken a 10 m shadow; 0.7 floors it at level
  <=30 with a soft edge rim while the emitter keeps a convincing 6-7 m
  radius; 0.6 zeroes the shadow but halves the radius). Line-of-sight look =
  air transmission calibration, not a new transport term.
  **Perf ledger (bench section 5, 2026-08-07, M3 Max, post attribute-hoist):**
  CA per frame: banks6@8vox 0.97-1.10 ms < shipped iso@4vox 1.33-1.47 ms
  (iso@8vox 0.27-0.32, iso@2vox 9.6-10.7, iterations8 ~5.1). DDA per dispatch
  @1440p: banks 5.47-6.33 ms vs shipped 4.85-5.68 — the directional trilinear
  is +~0.6 ms after the sampler unroll (a dynamically-indexed local array
  spilled to scratch under naga/Metal; unrolling per axis with static bank
  indices cut the tax from +1.5 ms) and scales with pixels. In-app finding
  (Pascal): the DDA span's big consumer at native res is the CLOUD MARCH on
  sky pixels (`sky_color`, dda.wgsl's own COST note) — the cloud arc's
  cloud-steps lever is that dial, not this arc's. Memory 20.9 vs 45.8 MiB. Look delta vs
  shipped: max channel 11-12 (every isotropic rule variant: 43) — the
  exposure anchor held. All banks lever settings cost the same (0.97-1.05) —
  the levers are look knobs, not perf knobs.
  **Corridor face-luminance comparison DONE (2026-08-07)** — the repo test
  `corridor_faces_read_directionally_under_banks` (cagi.rs) CPU-ports BOTH
  transports and BOTH samplers onto one corridor scene (8 m walls 4 m apart,
  near half open-top, far half roofed, sky only). Four pinned findings, full
  table in `docs/voxel-rt-bench.md` section D5: (1) anchor holds at 0.94 —
  the direction-decay skim, exact at mix 0; (2) a sky-lit wall reads 0.28 of
  ground under banks vs 1.00 under isotropic; (3) orientation contrast at one
  location — roof underside 0.05 of its floor under banks, 1.00 under
  isotropic; (4) beams carry under cover — roofed floor 0.18 of anchor vs
  isotropic's near-black 0.02. Verdicts written into the GiLayout,
  GiBanksSkyHorizontal and GiBanksDirectionMix registry rows. REMAINING for
  the default flip: Pascal's in-app fps confirmation at the reference pairing.
- [ ] **D6 — (separate arc, only if D5 perf demands)** dirty-region tracking +
  checkerboard update cadence — his 256³ 2 ms → 0.4 ms path.

## Non-goals

- No change to the DDA traversal or the shading contract.
- No removal of the isotropic layout — it stays a selectable lever (Quest may want it).
- Constants ship as levers with UNMEASURED verdicts until D5; defaults follow verdicts.

## Open questions (resolve during D2/D3, flag before deviating)

- Exact `propagation_loss` / `bounce_loss` / diffusion `k` values are voxelgamedev-only;
  we tune ours in the app run and record them as OUR verdicts, not his.
- Whether the sun (arbitrary direction) injects into 1–3 banks by dot weights or gets
  the transmittance-LUT treatment per bank — decide against the D2 app run.
- Emissive (E5) and event-light injection move to diagonal+face injection in D2 or stay
  face-only until D3 — decide by whether square halos show.
