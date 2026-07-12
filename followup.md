# Path Architecture — Follow-up Work

The core path-based rendering architecture from `path-architecture-plan.md` is complete
(Phases 1–3 + bonus features). This document captures remaining cleanup and future work.

---

## Phase 4: Legacy Cleanup

### 4.1 Delete dead SourceStage implementations

The old `SourceStage` implementations are no longer used by any pipeline — all 4
path-based modes use `PathEffect` chains instead.

**Dead code:**
- `src/pipeline/stages/air_absorption.rs` — `AirAbsorptionStage` (lines 124–154).
  Note: `AirAbsorptionFilter` (lines 84–117) in the same file is still actively used
  by `AirAbsorptionEffect` and `WorldLockedRenderer` — do NOT delete it.
- `src/pipeline/stages/ground_effect.rs` — `GroundEffectStage` (entire file). Only
  consumer is a test at `src/pipeline/mod.rs:1003` that uses it as a dummy stage.

**Steps:**
1. Delete `GroundEffectStage` and its file
2. Delete `AirAbsorptionStage` from `air_absorption.rs` (keep `AirAbsorptionFilter`)
3. Update `src/pipeline/stages/mod.rs` to remove the `ground_effect` module
4. Replace the test at `mod.rs:1003` with a trivial inline struct implementing `SourceStage`

### 4.2 Remove SourceStageBank (dead machinery)

Every pipeline passes `source_stages: SourceStageBank::new(vec![], ...)` — no
SourceStage factories exist anywhere.

**What touches SourceStageBank:**
- `RenderPipeline.source_stages` field — holds the bank
- `render_pipeline()` — calls `source_stages.ensure_source()`, iterates `stage_refs`,
  calls `process()` and `process_sample()` per source
- `Renderer::render_source()` signature — takes `source_stages: &mut [&mut dyn SourceStage]`
  and `src_out: &SourceOutput` (2 of its 8 params)
- All 5 `build_*` functions — construct with empty vec

**Steps:**
1. Delete `SourceStageBank` struct and its impl
2. Remove `source_stages` field from `RenderPipeline`
3. Remove `stage_refs` plumbing from `render_pipeline()`
4. Simplify `Renderer::render_source()` from 8 params to 6 (drop `source_stages`, `src_out`)
5. Update all 5 renderer implementations to match the new signature

**Renderer-specific cleanup when removing `src_out`:**
- `MultichannelRenderer` (VBAP): reads `src_out.gain_modifier` — drop the multiply (always 1.0)
- `HrtfRenderer`: reads `src_out.gain_modifier` — drop the multiply
- `DbapRenderer`: reads `src_out.gain_modifier` — drop the multiply
- `AmbisonicsRenderer`: reads `src_out.gain_modifier` — drop the multiply
- `WorldLockedRenderer`: already ignores it (`_src_out`)

### 4.3 Delete SourceOutput

Once SourceStageBank is removed, `SourceOutput` in `src/pipeline/source_stage.rs` is
fully dead:
- `gain_modifier` — always 1.0, no stage sets it
- `channel_gains` — never read by any renderer (VBAP computes gains internally)
- `distance_gain` — never read by any renderer

Delete `SourceOutput`, `SourceOutput::default_for()`, and `SourceContext` (only used
by the now-deleted SourceStage trait). The `SourceStage` trait itself can go too.

### 4.4 Relocate AirAbsorptionFilter

`AirAbsorptionFilter` (`src/pipeline/stages/air_absorption.rs:84–117`) is a one-pole
lowpass used by two active consumers:
- `AirAbsorptionEffect` in `path_effects.rs` (per-path DSP)
- `WorldLockedRenderer` in `renderers/world_locked.rs` (per-speaker DSP)

Once `AirAbsorptionStage` is deleted, the only remaining code in
`stages/air_absorption.rs` is the filter itself.

**Options (pick one):**
1. Move `AirAbsorptionFilter` to `src/audio/filters.rs` (new file, general filter home)
2. Inline into `path_effects.rs` and duplicate for WorldLocked (small struct, ~30 lines)
3. Leave in `stages/air_absorption.rs` (rename module to just `filters.rs`)

Option 1 is cleanest if we also consolidate biquad filters (see 4.5).

**Bug to fix during move:** The filter completely recreates state when cutoff changes
by >5%, causing audible clicks. Instead, it should smoothly transition coefficients.
The check is at `air_absorption.rs:103`:
```rust
if (cutoff - self.current_cutoff).abs() > cutoff * 0.05 {
    *self = Self::new(self.sample_rate); // ← resets all state, causes click
    self.update(cutoff);
}
```
Fix: update coefficients without resetting filter state (`y1` accumulator).

### 4.5 Consolidate biquad filter implementations

Three separate biquad-style filter implementations exist in the codebase:
- `LowPassBiquad` in `src/audio/filters.rs` (if it exists) or inline in stages
- `ShelvingFilter` in `src/pipeline/path_effects.rs` (used by `GroundEffectFilter`)
- `ParametricEQ` / biquad in `src/pipeline/path_effects.rs` (used by `WallAbsorptionEffect`)

Consider a single `BiquadFilter` struct with configurable type (lowpass, highshelf,
peaking) to reduce code duplication. This is optional but reduces maintenance burden.

### 4.6 Keep ReflectionCore (actively used)

`src/pipeline/stages/reflections.rs` contains `ReflectionCore` which is **actively
used** by `WorldLockedRenderer` for per-speaker reflection delay taps. Do NOT delete
this during cleanup — it is not dead code.

---

## Phase 4b: Renderer Distance Attenuation Consolidation

**Goal:** Move distance attenuation out of renderers into `DistanceAttenuationEffect`
so renderers only handle spatialization (angular placement).

`DistanceAttenuationEffect` exists in `path_effects.rs:60–119` and is fully tested,
but intentionally not wired in because it would double-count with the renderer's own
distance computation.

### Per-renderer feasibility

| Renderer | Where distance lives | Feasible to extract? |
|---|---|---|
| **VBAP** (`multichannel.rs`) | `compute_gains_with_spread()` includes `DistanceParams` | Yes — pass `DistanceParams::None` and let PathEffect handle it |
| **HRTF** (`binaural.rs`) | `distance_gain_at_model()` as scalar multiplier on direct path | Yes — remove the scalar multiply |
| **Ambisonics** (`ambisonics.rs`) | `distance_gain_at_model()` fed into `foa_encode()` | Yes — encode at gain=1.0, apply distance via PathEffect |
| **DBAP** (`dbap.rs`) | Distance is integral to `dbap_gains()` formula (1/d^rolloff) | **No** — distance IS the spatialization in DBAP |
| **WorldLocked** (`world_locked.rs`) | `distance_gain_at_model()` per-speaker | **No** — per-speaker model, not per-path |

### Reflection paths

`ImageSourceResolver` bakes distance into `path.gain = wall_reflectivity / image_distance`.
To avoid double-counting when `DistanceAttenuationEffect` is wired in:
- Split into `path.gain = wall_reflectivity` (set by resolver)
- Distance attenuation applied by `DistanceAttenuationEffect` using `path.distance`
- Requires `ImageSourceResolver` to set `path.distance = image_distance` (already done)
  and NOT divide gain by distance

### Additional: DirectivityEffect

For clean separation, a `DirectivityEffect` (new `PathEffect`) would also be needed.
Currently directivity gain is computed inline in HRTF and Ambisonics renderers using
`directivity_gain()`. Extracting it to a PathEffect keeps renderers purely angular.

### Migration plan

1. Wire `DistanceAttenuationEffect` into VBAP, HRTF, Ambisonics path_effect_factories
2. Remove distance computation from those 3 renderers' direct-path code
3. Update `ImageSourceResolver` to NOT divide by distance (set gain = reflectivity only)
4. Leave DBAP and WorldLocked unchanged (distance is integral to their algorithms)
5. Optionally create `DirectivityEffect` for further renderer simplification

Do this after Phase 4 cleanup when renderer signatures are already being simplified.

---

## Phase 5: New Features

These are extension points enabled by the path architecture. Each plugs in via
existing traits (`PathResolver`, `PathEffect`, `MixStage`) without modifying core
pipeline code.

### 5.1 RayTracerResolver
- New `PathResolver` implementation consuming ray-traced propagation data
- Replaces or augments `ImageSourceResolver` for complex geometries
- Produces N paths with arbitrary directions, not limited to 6 walls
- `PathSet` already supports up to 12 paths (1 direct + 11 reflections)
- Reference: `raytraced-audio` crate uses persistent rays — see `REFERENCES.md`

### 5.2 TransmissionEffect
- New `PathEffect` for wall transmission loss
- New `PathKind::Transmission` variant (add to enum in `path.rs:25–32`)
- Models sound passing through walls (frequency-dependent TL)
- Resolvers would need to produce transmission paths (through-wall geometry)

### 5.3 LOD System
- Limit path count based on source distance or CPU budget
- Two implementation points:
  - In `PathResolver`: skip reflections for distant sources (cheapest)
  - In `render_pipeline()`: truncate `PathSet` after resolve (most flexible)
- `PathSet.count` field makes truncation trivial

### 5.4 Late Reverb Feeding from Paths
- FDN `MixStage` receives total reflection energy from resolved paths
- Replaces fixed wet gain with physically-derived reverb level
- `path_energy = sum of path.gain for all PathKind::Reflection paths`
- Requires passing path data to MixStage (currently mix stages don't see paths)
- Could add a `reflection_energy: f32` field to a shared pipeline state

### 5.5 Occlusion
- `PathResolver` detects occluded direct paths (ray-geometry test)
- Two approaches:
  - `OcclusionEffect` as `PathEffect` — frequency-dependent attenuation
  - Resolver sets `path.gain` directly — simple broadband occlusion
- Needs geometry data in `ResolveContext` (currently only room AABB)

### 5.6 Dynamic Room Geometry
- `ResolveContext` receives updated room bounds each buffer
- Already partially supported (`room_min`/`room_max` are per-frame in `ResolveContext`)
- Extension: non-rectangular rooms via polygon-based resolver
- Would require new `ResolveContext` fields for wall polygons

---

## Remaining Audit Items (from audit-verification.md)

### Integer-sample reflection delays in ReflectionCore
`src/pipeline/stages/reflections.rs:72` — `(delay_seconds * sample_rate) as usize` truncates
to integer samples. No fractional delay interpolation. A fractional delay pattern exists in
`delay_comp.rs:118-126` (linear interpolation) but is not reused here.

Impact: Moving reflections jump in whole-sample increments. Can produce zipper noise during
source/listener motion, especially for close walls where delay changes rapidly.
Only affects WorldLockedRenderer (the only remaining consumer of ReflectionCore).

### Dead match arms in `compute_gains()`
`crates/core/src/speaker.rs:1022-1028` — `compute_gains()` has arms for
WorldLocked/Hrtf/Dbap/Ambisonics that fall through to stereo panning. Only called
in tests (line 1773-1774). `compute_gains_with_spread()` is the production entry point
and delegates directly to MDAP. Low priority — maintenance trap only.

---

## Phase 6: Research-Derived Improvements

Items extracted from 31-paper synthesis that haven't been implemented yet or are
only partially done.

### 6.1 IRCAM 4-Segment Room Model (Carpentier 2017)

**Status:** Partially implemented — pieces exist, no unified orchestration.

The IRCAM Spat/Panoramix architecture divides room simulation into 4 perceptually
distinct temporal segments, each with its own spatialization strategy:

| Segment | Time Range | Character | Current Implementation |
|---|---|---|---|
| Direct sound | 0 ms | Point source | `DirectPathResolver` → renderer panning |
| Early reflections | ~1–80 ms | 8–16 discrete echoes | `ImageSourceResolver` (6 walls) + `ReflectionCore` |
| Late reflections | ~80–200 ms | Diffuse, decorrelated | `FdnReverbStage` pre-delay (~20ms fixed) |
| Reverb tail | 200+ ms | Exponential decay | `FdnReverbStage` feedback loop |

**What's missing for unified orchestration:**

1. **Configurable segment boundaries** — currently the FDN pre-delay is fixed at 20ms
   (`fdn_reverb.rs:18`), not derived from room geometry or matched to the early
   reflection timing. The crossover between "early reflections" and "late reverb" should
   be configurable per room.

2. **Energy-matched handoff** — the FDN wet level is a fixed parameter, not derived
   from the resolved reflection paths. Section 5.4 (Late Reverb Feeding from Paths)
   describes feeding `path_energy = sum(path.gain for Reflection paths)` into the FDN,
   replacing the fixed wet gain with physically-derived reverb level.

3. **Decorrelation in late segment** — the FDN uses a Hadamard matrix for mixing
   (good), but the delay line outputs aren't spatially distributed to different speaker
   directions. IRCAM distributes late reflections across all speakers for envelopment.

4. **3-band decay control** — the FDN currently uses a single-band one-pole damping
   filter per delay line. IRCAM uses 3-band (lo/mid/hi) decay, allowing different RT60
   at different frequency ranges (e.g., carpet absorbs highs more than lows).

**Implementation approach:**

- Add `pre_delay_ms: f32` and `crossover_ms: f32` to FDN config (segment boundaries)
- Feed reflection path energy into FDN wet gain (requires `MixStage` seeing path data)
- Replace one-pole damping with 3-band shelving filter per delay line
- Add per-delay-line output panning (distribute across speaker channels)

**Reference:** Carpentier 2017, LAC (panoramix_lac2017.pdf). Also Jot & Chaigne 1991
for the original FDN design.

### 6.2 QuickHull + Ray-Triangle VBAP (Choi 2012)

**Status:** Not implemented. Current VBAP uses matrix-inverse method (Pulkki).

Alternative VBAP triangulation that replaces the speaker-pair search + matrix inverse
with computational geometry primitives:

**Algorithm:**
1. Compute convex hull of speaker positions on the unit sphere (Quickhull3D)
2. For each source direction, cast a ray from origin through the source position
3. Find which triangle the ray intersects → those 3 speakers are active
4. Barycentric coordinates of intersection point = gain ratios

**Gain computation for speakers S0, S1, S2 with intersection point I:**
```
g_i = (dist(S_j, I) + dist(S_k, I)) / (dist(S0,I) + dist(S1,I) + dist(S2,I))
```

**Advantages over current matrix-inverse:**
- Standard computational geometry — any convex hull library works
- GPU-acceleratable (ray-triangle is a standard graphics primitive)
- Handles up to 200+ speakers in real-time
- Convex hull computed once at startup unless layout changes
- Natural extension to 3D (current 2D pair-search doesn't extend to height speakers)

**When to implement:** Only relevant if we add height speakers (3D layouts). For the
current 2D 5.1 layout, the matrix-inverse method is simpler and sufficient. The
pre-computed 1° LUT already eliminates the runtime cost for reflection paths.

**Gotcha:** When source direction falls inside a speaker triangle, per-speaker time
delay can become negative (physically impossible). Only matters if combining with
per-speaker delay compensation.

**Reference:** Choi 2012 (an-alternative-implementation-of-vbap.pdf).

### 6.3 SDN Air Absorption Filter (Moorer Approximation)

**Status:** Not implemented. Current air absorption uses full ISO 9613-1.

The Scattering Delay Network (Yeoward 2021) uses Moorer's simplified first-order LPF
for air absorption, which is cheaper than our ISO 9613-1 biquad:

**Moorer's formula:**
```
T(z) = (1 - g) / (1 - g * z^{-1})    (one-pole LPF)
g = (1/5) * ln(d/3 + 1)               (humidity 50%, Fs = 44.1 kHz)
```

Where d = distance in meters. This is a single-pole filter vs our 2nd-order biquad.

**When useful:** If CPU profiling shows air absorption is a bottleneck (7 filter
instances per source with path architecture), the Moorer approximation is ~2x cheaper
per sample (1 multiply + 1 add vs 5 multiply + 4 add). Trade-off: less accurate at
high frequencies and doesn't account for temperature/humidity variation.

**Current implementation note:** `AirAbsorptionFilter` in `air_absorption.rs` uses
`Biquad::set_lowpass()` to update coefficients without resetting state (the click bug
from the old `*self = Self::new()` pattern was fixed). The hysteresis threshold is 5%
relative change in cutoff (`air_absorption.rs:101`).

### 6.4 Flutter Mitigation via Modulated Delay Lines

**Status:** Not implemented. FDN and multi-delay use fixed delays.

Yeoward et al. 2021 found that adding slow LFO modulation to delay lines between
SDN nodes prevents flutter echoes (repetitive reflections between parallel walls):

**Parameters:**
- Modulation amplitude: 0.003 × delay line length (in samples)
- Modulation frequency: 0–2 Hz, random per delay line
- Apply to inter-node connections only, NOT source/mic delay taps

**Where to add:**
- `fdn_reverb.rs` — modulate the 8 feedback delay lines
- `ambi_multi_delay.rs` — modulate the 4 feedback delay lines

**Implementation:** Per delay line, add a sine LFO with random frequency in [0, 2] Hz.
Each buffer, compute `offset = amplitude * sin(2π * freq * time)` and add to the read
position. Requires fractional delay interpolation (already exists in `delay_comp.rs`
linear interp pattern).

**Perceptual benefit:** SDN T60 measurements run 45–60% higher than Sabine prediction
(Yeoward 2021) — modulation helps diffuse the energy more naturally and reduces the
metallic quality of dense parallel reflections.

### 6.5 Perceptual Reference Thresholds

Collected from 31 research papers. These are now partially documented as comments in
the codebase (binaural.rs, multichannel.rs, speaker.rs). Full table for reference:

| Threshold | Value | Source | In Code? |
|---|---|---|---|
| ITD JND | 90 μs | BBC WHP254 | Yes — binaural.rs:30 |
| ILD JND | 2.5 dB | BBC WHP254 | Yes — binaural.rs:30 |
| Gain interpolation period | 10–50 ms | Pulkki, Gandemer | Yes — multichannel.rs:9-11 |
| VBAP panning update rate | 10.7 ms (512 samples @ 48kHz) | Gandemer 2018 | Yes — multichannel.rs:9-11 |
| VBAP LUT resolution | 1° azimuth | Shukla 2019 | Yes — speaker.rs:1049 |
| Max polar span (3D VBAP) | 40° | Baumgartner 2015 | N/A (2D only) |
| Head-tracking latency budget | < 75 ms total | Shukla 2019 | Not applicable yet |
| HRIR length (acceptable) | 256 samples | Yeoward 2021 | Dynamic (loaded from SOFA) |
| Speed of sound | 343 m/s | ISO 9613-1 | Yes — atmosphere.rs:10 |
| Front-back confusion (2nd-order HOA) | ~15% without head-tracking | Medina 2024 | N/A (FOA only) |