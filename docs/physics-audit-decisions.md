# Physics Audit — Decisions & Rationale

Tracking decisions made during the physics/consistency fix plan (phases 1A–7A).
Each entry explains *why* a particular approach was chosen.

## Phase 1A: Speed of Sound Unification

**Decision**: Use `AtmosphericParams::speed_of_sound()` method instead of threading a bare `f32` through context structs.

**Rationale**: The user explicitly rejected passing `speed_of_sound: f32` everywhere — "are you now really putting speed_of_sound as a number argument everywhere? that doesn't sound like a good idea." Instead, `AtmosphericParams` already flows through all contexts (SourceContext, ResolveContext, MixContext, PathEffectContext), so adding a method to it gives every call site access without new plumbing. The formula `c = 331.3 + 0.606 × T_celsius` lives in one place.

**Removed constants**:
- `SPEED_OF_SOUND = 343.4` from `room_acoustics.rs`
- `pub const SPEED_OF_SOUND = 343.0` from `atmosphere.rs`

**Test pattern**: `const TEST_ATMOSPHERE: AtmosphericParams` with `'static` lifetime solves borrow issues in test helpers that return structs containing `&AtmosphericParams`.

## Phase 1B: ref_distance Default

**Decision**: Changed `DistanceModel` and `DistanceParams` defaults from `0.3` to `1.0`.

**Rationale**: 1.0m is the WebAudio/OpenAL standard. The 0.3m value was an arbitrary choice that made sources louder than expected at normal distances.

## Phase 1C: Ambisonics ITD Comment

**Decision**: Corrected stale doc comment claiming "ITD from rotation" to accurately describe "ILD from rotation-based level difference."

**Rationale**: The code only applies level differences (ILD) via asymmetric W/Y weights. True ITD (inter-aural time delay) requires per-ear delay lines, which are not implemented.

## Phase 1D: WorldLocked Per-Source Distance Model

**Decision**: Removed `ref_distance`, `max_distance`, `rolloff`, `model` from `WorldLockedParams`. WorldLocked now reads these from `ctx.distance_model` (per-source).

**Rationale**: Each source should have its own distance model — a whisper and a loudspeaker behave differently. The old code used a single global distance model for all sources.

## Phase 2A: Directivity Factor

**Decision**: Added `directivity_factor(pattern) -> f32` using Simpson's rule (64 steps) for the integral γ = 2 / ∫₀^π g(θ)² sin(θ) dθ. Critical distance is now per-source: `d_c = 0.057 × √(γ × V / RT60)`.

**Rationale**: A cardioid source (γ≈3) has a critical distance √3× further than omni — its direct sound dominates over reverb at greater distances. This is physically significant and was previously hardcoded to γ=1.0 (omni assumption).

**Caching note**: γ is computed per-frame in the source loop. Since `directivity_factor()` is pure arithmetic (no allocations, ~64 iterations), caching was deferred — profile first.

## Phase 3A: Per-Wall Reflection Energy

**Decision**: Changed `ImageSourceResolver` from single `wall_reflectivity: f32` to per-wall `wall_gains: [f32; 6]`. Each wall's broadband gain is derived from its `WallMaterial` via `broadband_reflection_gain()`.

**Broadband formula**: `α_broadband = avg(α[1..5])` using indices 1–5 (250Hz–4kHz). Index 0 (125Hz) is excluded because:
- Low frequencies contribute little to perceived reflection loudness (Fletcher-Munson)
- Including 125Hz skews the average for frequency-dependent materials like carpet (low α at 125Hz, high α at 4kHz)

**Energy vs amplitude**: Material α is in energy domain. Amplitude gain = `√(1 - α_broadband)`. This matters because audio signals are pressure (amplitude), but absorption coefficients describe energy loss.

**Spectral coloring**: `WallAbsorptionEffect` (PathEffect) still applies per-band spectral coloring on top of the broadband gain. The broadband gain sets the overall reflection level; the spectral filter shapes its timbre. These don't fight each other because the broadband gain is derived from the same absorption data — it's a consistent decomposition of the full frequency-dependent absorption into "level" and "color" components.

**Wall index mapping**: [0: -X, 1: +X, 2: -Y, 3: +Y, 4: -Z (floor), 5: +Z (ceiling)]

## Phase 4A: FDN Pre-Delay from Room Geometry

**Decision**: Pre-delay derived from mean free path time: `t = 4V / (S × c)`. Falls back to 20ms when room geometry is degenerate (volume ≈ 0 or surface area ≈ 0).

**Rationale**: The mean free path is the average distance a sound ray travels between wall reflections in a diffuse field. This is the natural onset time for the late reverb tail, physically grounded in Kuttruff's room acoustics formulation.

## Phase 4B: FDN HF Decay from Materials

**Decision**: Replaced fixed `RT60_HIGH_RATIO = 0.5` with per-band Sabine RT60 computed from actual wall material absorption coefficients. RT60_low at band 2 (500 Hz), RT60_high at band 5 (4 kHz).

**Sabine per-band formula**: `A(f) = Σᵢ Sᵢ × αᵢ(f) + 4 × m(f) × V`, where m(f) = ISO 9613 air absorption in Nepers/m (= α_dB/m / 4.343). `RT60(f) = 0.161 × V / A(f)`.

**Band selection rationale**: 500 Hz (band 2) captures the room's broadband decay character. 4 kHz (band 5) captures HF-specific absorption from materials and air. These map directly to the Jot FDN's one-pole damping filter: g_dc from RT60_low, g_nyq from RT60_high.

**Hard walls**: α is nearly uniform (0.02–0.05), so RT60_high/RT60_low ratio is close to 1.0 (slightly below due to air absorption at 4 kHz).
**Carpet**: α rises sharply with frequency (0.08 at 500 Hz → 0.40 at 4 kHz), so RT60_high/RT60_low < 0.5 — HF decays more than twice as fast.

**Helper functions added to room_acoustics.rs**: `wall_surface_areas(room_min, room_max) -> [f32; 6]` and `sabine_rt60_at_band(volume, wall_areas, wall_materials, atmosphere, band_index) -> f32`.

## Phase 5A: Dynamic Delay Buffer Sizing

**Decision**: Replaced fixed-size delay buffers with dynamically-sized buffers computed from room geometry. Buffer capacity = `max_image_source_distance / speed_of_sound × sample_rate`, rounded to next power of 2.

**Max image-source distance formula**: For each axis, the worst-case distance is when a source is at one wall and the listener is at the opposite corner. The image is mirrored across the wall:
- X-wall: `√((2·Lx)² + Ly² + Lz²)`
- Y-wall: `√(Lx² + (2·Ly)² + Lz²)`
- Z-wall: `√(Lx² + Ly² + (2·Lz)²)`
- Take the maximum of all three.

**Minimum capacities**: `PropagationDelayEffect` minimum = 8192 samples (~170ms at 48kHz). `ReflectionCore` minimum = 4096 samples (~85ms at 48kHz). Small rooms use these minimums; large rooms (e.g. 50×50×10m) get proportionally larger buffers.

**Implementation**: Changed both `PropagationDelayEffect` and `ReflectionCore` from fixed `Box<[f32; CAPACITY]>` to dynamic `Box<[f32]>` (boxed Vec). Capacity passed at construction time. Added `room_min`/`room_max` to `PipelineParams` so factories can compute the right size.

**Example**: 50×50×10m room → max distance ≈ 112m → ~327ms → 16384 samples (next power of 2 above 15696). 3m cube → max distance ≈ 7.35m → ~21ms → 8192 samples (uses minimum).

## Phase 5B: LFE Bass Management (Linkwitz-Riley Crossover)

**Decision**: Replaced single 2nd-order Butterworth lowpass on LFE with full Linkwitz-Riley 4th-order (LR4) bass management. Renamed `LfeCrossoverStage` → `LfeBassManagementStage`.

**What changed**:
- **LFE channel**: receives LR4 lowpass (two cascaded Butterworth 2nd-order LP sections)
- **All non-LFE channels**: receive LR4 highpass (two cascaded Butterworth 2nd-order HP sections)
- **Bass redirection**: low-frequency content removed from main channels by the highpass is summed into the LFE channel

**Why LR4 over single Butterworth**:
- Single Butterworth (2nd-order): -12 dB/octave slope, -3 dB at crossover. LP + HP = +3 dB bump at crossover frequency — phase cancellation and a bump in the response.
- LR4 (4th-order): -24 dB/octave slope, -6 dB at crossover. LP + HP = 0 dB (flat). This is because two cascaded Butterworth sections produce in-phase outputs that sum perfectly.

**Why bass management (HP on mains) instead of LFE-only filtering**:
- With only LP on LFE, main speakers receive full-range signal including bass they may not reproduce well
- Bass management redirects the low-frequency content to the subwoofer, which is designed for it
- No-op when layout has no LFE channel (stereo, quad) — mains stay full-range because there's no sub to redirect to

**Bass redirection signal flow**:
1. Each main channel: `original` → HP filter → `highpassed` (written back to channel)
2. `bass = original - highpassed` (implicit LP by LR4 reconstruction property)
3. All `bass` contributions summed into LFE channel
4. Existing LFE content is separately lowpassed (renderer may have put content there directly)
5. The redirected bass bypasses the LFE lowpass because it's already lowpass-shaped by construction — applying LP again would double-filter and attenuate it

**Why redirected bass bypasses the LFE LP filter**:
LR4 guarantees `original = HP(original) + LP(original)`. Therefore `original - HP(original) = LP(original)`. The subtraction produces an exact LP response. Running it through the LP filter again would give `LP(LP(original))` — double-attenuation near the crossover frequency. The reconstruction test (`lr4_reconstruction_flat`) verifies that `HP(main) + redirected_bass ≈ original` with < 0.01 error.

**Crossover frequency**: 120 Hz (ITU-R BS.775 standard for bass management in multichannel audio).

## Phase 6A: Air Absorption Multi-Band

**Decision**: Replaced single lowpass cutoff approach with two shelving filters (low shelf at 500 Hz, high shelf at 4 kHz) derived from ISO 9613-1 absorption coefficients. Removed `air_absorption_lp_cutoff()`, replaced with `air_absorption_shelf_gains()`.

**Why the old approach was wrong**:
The old code collapsed the entire frequency-dependent ISO 9613 absorption curve into a single LP cutoff frequency, derived from the 4 kHz absorption value. A lowpass filter applies its slope uniformly above the cutoff — but real atmospheric absorption has a frequency-squared relationship. The single-cutoff approach over-attenuated mid frequencies and under-represented the steep HF rolloff.

**Why two shelving filters**:
The ISO 9613 absorption curve has two main spectral features:
1. O₂ vibrational relaxation around 500 Hz — creates a baseline absorption that affects all frequencies
2. N₂ relaxation + classical absorption above 2 kHz — creates a steep HF rolloff

Two shelving biquads capture this shape with 4 coefficients (2 center frequencies, 2 gains) instead of trying to map everything to one cutoff.

**High shelf reference frequency choice (8 kHz, not 4 kHz)**:
A 2nd-order shelving biquad provides approximately half its gain at the center frequency and full gain one octave above. By computing the high shelf gain from the 8 kHz absorption, the filter naturally provides:
- ~half the 8 kHz attenuation at 4 kHz (close to the actual ISO value)
- Full attenuation at 8 kHz (matching the steep rolloff)

Using 4 kHz as the reference gave only -0.73 dB at 4 kHz and -2.26 dB at 8 kHz (vs ISO's -5.26 dB at 8 kHz). With 8 kHz reference: -2.62 dB at 4 kHz and -5.28 dB at 8 kHz — within 1.2 dB across the full 250 Hz–8 kHz range.

**Accuracy at 50m, 20°C, 50% RH**:
| Frequency | Filter | ISO 9613 | Error |
|-----------|--------|----------|-------|
| 250 Hz    | -0.00 dB | -0.07 dB | 0.07 dB |
| 500 Hz    | -0.00 dB | -0.14 dB | 0.13 dB |
| 1000 Hz   | -0.02 dB | -0.23 dB | 0.21 dB |
| 2000 Hz   | -0.30 dB | -0.49 dB | 0.20 dB |
| 4000 Hz   | -2.62 dB | -1.48 dB | 1.13 dB |
| 8000 Hz   | -5.28 dB | -5.26 dB | 0.01 dB |

**Hysteresis**: Shelf coefficients are only recomputed when the gain changes by > 0.5 dB (same approach as the old cutoff hysteresis, but in dB instead of percentage). Prevents unnecessary trig recomputation on small distance changes.

**Files changed**:
- `src/audio/atmosphere.rs`: `air_absorption_lp_cutoff()` → `air_absorption_shelf_gains()`
- `src/pipeline/stages/air_absorption.rs`: `AirAbsorptionFilter` changed from 1 LP biquad to 2 shelving biquads; `Biquad` struct gained `set_low_shelf()` and `set_high_shelf()` methods
- Both consumers (`AirAbsorptionEffect`, WorldLockedRenderer) automatically use the new filter via the shared `AirAbsorptionFilter`.

---

## Phase 7A: Measurement Mode Bypass

**Decision**: Add a `measurement_mode: bool` flag that bypasses soft clipping and gain clamping while keeping NaN/Inf sanitization and a ±100.0 stability ceiling.

**Rationale**: When calibrating a speaker array or measuring impulse responses, the soft-clip knee at ±0.9 distorts the signal and makes it impossible to verify that gain staging is linear. Measurement mode lets the pipeline output signals > 1.0 so we can confirm energy scaling without nonlinear artifacts. However, we still need safety rails — an unstable FDN coefficient or a NaN from a degenerate room could damage speakers or explode the delay network. The ±100.0 ceiling is ~40 dB above unity, far beyond any legitimate measurement signal but well below what would cause hardware damage.

**What gets bypassed in measurement mode**:

| Location | Normal mode | Measurement mode |
|----------|-------------|------------------|
| `MasterGainStage` | `soft_clip(sample * gain)` | `sanitize_finite(sample * gain)` |
| `FdnReverbStage` output | `soft_clip(wet)` | `sanitize_finite(wet)` |
| `FdnReverbStage` delay write | `.clamp(-4.0, 4.0)` | `.clamp(-100.0, 100.0)` |
| `FdnReverbStage` damping norm | `.clamp(0.01, 1.0)` | finite check only |

**What stays active in both modes**:
- `sanitize_finite()`: converts NaN/Inf → 0.0, clamps to ±100.0 — prevents DAC damage and FDN explosion
- All filter computations, delay lines, and gain staging remain identical

**`sanitize_finite()` helper** (`stages/mod.rs`):
```rust
pub fn sanitize_finite(x: f32) -> f32 {
    if x.is_finite() { x.clamp(-100.0, 100.0) } else { 0.0 }
}
```

**Propagation path**: `AudioScene.measurement_mode` → `RenderParams.measurement_mode` → `MixContext.measurement_mode` → each stage reads from context.

**Files changed**:
- `src/pipeline/mix_stage.rs`: added `measurement_mode: bool` to `MixContext`
- `src/pipeline/mod.rs`: added `measurement_mode: bool` to `RenderParams`, propagated to `MixContext`
- `src/engine/scene.rs`: added `measurement_mode: bool` to `AudioScene`
- `src/config.rs`: default `measurement_mode: false` in AudioScene construction
- `src/pipeline/stages/mod.rs`: added `sanitize_finite()` helper
- `src/pipeline/stages/master_gain.rs`: conditional soft_clip vs sanitize_finite
- `src/pipeline/stages/fdn_reverb.rs`: conditional clamping and clipping throughout
