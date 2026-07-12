# Ray-Traced Room Acoustics — Upgrade Analysis

Detailed analysis of what it would take to support per-triangle materials, ray-traced absorption, and non-rectangular room geometry. Documents current architecture boundaries, required changes, and difficulty estimates.

---

## 1. Per-Triangle Material Assignment

### Current state
- Materials are `[WallMaterial; 6]` — one per axis-aligned wall face
- Wall index mapping: `[0: -X, 1: +X, 2: -Y, 3: +Y, 4: -Z (floor), 5: +Z (ceiling)]`
- `WallMaterial` has `alpha: [f32; 6]` — absorption coefficients at 6 octave bands (125, 250, 500, 1k, 2k, 4k Hz)
- `ImageSourceResolver` uses `wall_gains: [f32; 6]` derived from `broadband_reflection_gain()`
- `WallAbsorptionEffect` applies spectral coloring per-path using `wall_index` to look up material

### Required changes
- **New mesh data structure**: vertices + triangles + material index per triangle
- **Material table**: `Vec<WallMaterial>` instead of `[WallMaterial; 6]`
- **Update `PathContribution`**: `wall_index: Option<u8>` → `material_index: Option<u16>` (or keep u8 if <256 materials)
- **Update `ImageSourceResolver`**: ray-triangle intersection instead of axis-aligned plane intersection
- **`WallAbsorptionEffect`**: already indexes by `wall_index` to get material — just needs a larger table

### Difficulty: **Medium**
This is primarily data plumbing. The physics (`WallMaterial` struct, absorption coefficients, broadband gain formula) stays identical. The algorithm changes from "mirror source across 6 planes" to "mirror source across N triangle planes" (or ray-cast).

### Files affected
- `src/pipeline/path.rs` — `PathContribution.wall_index` type change, `WallMaterial` table
- `src/pipeline/path_resolvers.rs` — `ImageSourceResolver` geometry queries
- `src/pipeline/path_effects.rs` — `WallAbsorptionEffect` material lookup
- `src/pipeline/mix_stage.rs` — `MixContext.wall_materials` type change
- `src/pipeline/mod.rs` — pipeline construction
- `src/engine/scene.rs` — material storage
- `crates/core/src/room.rs` — `Room` trait extension

---

## 2. Ray-Traced Absorption (replacing Sabine + Image-Source)

### Current architecture boundaries

#### PathResolver trait (`src/pipeline/path.rs:122-131`)
```rust
pub trait PathResolver: Send {
    fn resolve(&self, ctx: &ResolveContext<'_>, out: &mut PathSet);
}
```
- `ResolveContext` carries: source_pos, target_pos, room_min, room_max, barriers, atmosphere
- `PathSet` has fixed capacity of `MAX_PATHS = 12`
- `ImageSourceResolver` produces 1 direct + up to 6 first-order reflections

#### Room trait (`crates/core/src/room.rs`)
```rust
pub trait Room: Send {
    fn bounds(&self) -> (Vec3, Vec3);
    fn contains(&self, point: Vec3) -> bool;
}
```
- Only `BoxRoom` implementation — axis-aligned box with bounds + containment test
- **No ray intersection, no geometry database, no BVH**

#### RenderPipeline (`src/pipeline/mod.rs:202-224`)
- Each mode has one `resolver: Box<dyn PathResolver>`
- Per-source path effects: `path_effects: Vec<[PathEffectChain; MAX_PATHS]>`
- Reverb send buffer: mono bus written by renderer, read by FDN

### What a ray-traced resolver needs

#### Core algorithm
1. Cast N rays from source position in random/stratified directions
2. Trace each ray through geometry (bounce off surfaces, accumulate absorption)
3. Classify arrivals by time:
   - **Early reflections** (0–50ms): individual paths with direction, distance, gain
   - **Late tail** (50ms+): energy histogram → feed into FDN or convolve with noise
4. For each ray segment hitting a surface: apply that surface's `WallMaterial` absorption

#### New components needed
- **BVH or spatial acceleration** — `src/spatial/bvh.rs` (new module)
  - For real-time performance on arbitrary meshes
  - Build once at scene load, rebuild on geometry changes
  - Query: nearest intersection along ray direction
- **Ray struct** — origin + direction + max_distance
- **RayHit result** — intersection point, surface normal, material index, distance
- **Extended Room trait**:
  ```rust
  fn intersect_ray(&self, origin: Vec3, direction: Vec3) -> Option<RayHit>;
  fn material_at(&self, hit: &RayHit) -> &WallMaterial;
  ```
- **RayTraceResolver** implementing `PathResolver` trait
  - Configurable: max bounces (3–8), ray count (64–512)
  - Stochastic or deterministic ray distribution

#### Performance considerations
- **Budget**: Audio callback is ~5ms at 48kHz/256 frames. Ray tracing cannot exceed this.
- **Typical cost**: 512 rays × 4 bounces × BVH intersection ≈ 10k–50k intersections per source per buffer
- **Solutions**:
  1. **Async computation**: trace rays on a background thread, audio thread uses most recent result
  2. **Amortization**: trace a few rays per buffer, accumulate over multiple buffers
  3. **Precomputation**: for static geometry, trace a grid of positions offline
  4. **LOD**: fewer rays for distant/quiet sources

#### Integration with existing pipeline
- `PathSet` capacity would need to increase (or become dynamic) for many reflections
- Each ray bounce becomes a `PathContribution` with direction, distance, gain, material_index
- `PathEffectChain` per-path already handles air absorption + wall absorption per reflection
- FDN late reverb could receive energy histogram instead of distance-weighted send:
  - Currently: `reverb_send = source_signal × d²/(d²+d_c²)`
  - Ray-traced: `reverb_send = source_signal × (total_late_energy / total_direct_energy)`

### Difficulty: **Hard** (significant new code, performance-critical)
Estimated scope: 2000–5000 lines for core ray tracer + BVH + resolver. The pipeline architecture already supports it through `PathResolver` — this is its intended extension point. No changes needed to FDN, renderers, or mix stages.

### Files affected (new + modified)
- `src/spatial/bvh.rs` — **NEW**: BVH construction + traversal
- `src/spatial/ray.rs` — **NEW**: Ray/RayHit types
- `src/spatial/mod.rs` — **NEW**: module declaration
- `src/pipeline/path_resolvers.rs` — add `RayTraceResolver`
- `src/pipeline/path.rs` — increase `MAX_PATHS` or make dynamic, extend `ResolveContext`
- `crates/core/src/room.rs` — extend `Room` trait with intersection methods
- `src/pipeline/mod.rs` — new pipeline variant using `RayTraceResolver`

---

## 3. Non-Rectangular Room Modes

### What are room modes?
Standing waves at low frequencies (<200 Hz) where room dimensions create resonant peaks/nulls. For a rectangular room, modal frequencies are:

```
f(n_x, n_y, n_z) = (c/2) × √((n_x/L_x)² + (n_y/L_y)² + (n_z/L_z)²)
```

Three types:
- **Axial** (1 dimension): strongest, simplest
- **Tangential** (2 dimensions): moderate
- **Oblique** (3 dimensions): weakest

### Why this is hard for non-rectangular geometry
- Rectangular modes have **analytic solutions** (formula above)
- Arbitrary geometry requires **numerical methods**:
  - **FEM (Finite Element Method)**: solve Helmholtz equation on a 3D mesh
  - **BEM (Boundary Element Method)**: solve on surface mesh only (fewer elements)
  - Both produce eigenvalues (modal frequencies) and eigenvectors (spatial mode shapes)
- Computational cost: a 10m room at 200 Hz needs ~100k elements → eigenvalue problem of same size
- Typically runs **offline** (minutes to hours)

### What you'd get
- A set of modal frequencies + Q factors + spatial amplitude patterns
- Feed into a **modal resonator bank**: parallel biquad filters tuned to each mode
- Spatially dependent: mode amplitude varies with listener/source position in the room
- Only relevant below ~200 Hz (above this, ray-based methods dominate)

### Current codebase support
- **None**. No modal computation, no resonator bank.
- The FDN approximates diffuse late reverb but doesn't model individual modes.

### Integration approach (if pursued)
1. **Offline computation**: solve eigenvalue problem for room mesh → store modal data
2. **Modal resonator stage** (new `MixStage`):
   - Bank of biquad filters, one per mode (typically 20–100 modes below 200 Hz)
   - Each filter's gain depends on source/listener position within the mode shape
   - Insert before FDN in the pipeline (modes are faster than FDN onset)
3. **Hybrid approach**: modes for <200 Hz, ray tracing for 200 Hz–20 kHz

### Difficulty: **Very Hard** (and questionable value)
- FEM/BEM solver is a major undertaking (or requires external dependency)
- Perceptual impact is subtle — mainly affects bass response at specific positions
- Almost no real-time audio engines implement this (even high-end tools like CATT-Acoustic only use it for analysis, not rendering)
- **Recommendation: skip entirely** unless the project specifically targets room acoustics research or studio design

---

## 4. Current Architecture — Integration Point Summary

### Pipeline composition (`src/pipeline/mod.rs`)

| Mode | Resolver | Path Effects | Renderer | Mix Stages |
|------|----------|--------------|----------|-----------|
| WorldLocked | DirectPathResolver | (none) | WorldLockedRenderer | LfeCrossover, DelayComp, MasterGain |
| Vbap | ImageSourceResolver | PropDelay, AirAbs, GroundEffect, WallAbs | MultichannelRenderer | LfeCrossover, DelayComp, FdnReverb, MasterGain |
| Hrtf | ImageSourceResolver | PropDelay, AirAbs, GroundEffect, WallAbs | HrtfRenderer | FdnReverb, MasterGain |
| Dbap | ImageSourceResolver | PropDelay, AirAbs, GroundEffect, WallAbs | DbapRenderer | LfeCrossover, DelayComp, MasterGain |
| Ambisonics | ImageSourceResolver | PropDelay, AirAbs, GroundEffect, WallAbs | AmbisonicsRenderer | AmbiDecorrelation, AmbiDecode, FdnReverb, LfeCrossover, DelayComp, MasterGain |

### Key abstraction boundaries

| Boundary | File | Current | Ray-Trace Extension |
|----------|------|---------|-------------------|
| `PathResolver` | path.rs:122 | Image-source (6 reflections) | RayTraceResolver with N bounces |
| `PathEffect` | path.rs:229 | Air absorption, ground, wall material | Add scattering, impedance |
| `Room` | core/room.rs | Axis-aligned bounds + containment | Ray intersection, material query, BVH |
| `WallMaterial` | path.rs:142 | 6-band α coefficients | Add scattering coefficient, impedance |
| `room_acoustics` | room_acoustics.rs | Sabine RT60, mean free path | Recompute from ray energy distribution |
| `FdnReverbStage` | fdn_reverb.rs | Sabine-derived damping | Accept ray-measured late energy |
| `rtrb` | commands.rs | Main→audio commands | No changes needed |
| `RenderPipeline` | mod.rs:202 | 5 modes with fixed resolver | Add RayTrace variants |

### Thread model (`src/engine/`)
- **Main thread**: WebSocket server, TUI, pushes `Command` variants via `rtrb::Producer`
- **Audio thread**: pops commands, runs `AudioScene::render()`, pushes `TelemetryFrame` back
- **No background computation thread currently** — ray tracing would likely need one
- Pattern for async ray tracing:
  ```
  [Background Thread]  ─── ray trace results ───► rtrb::Producer
  [Audio Thread]        ◄── rtrb::Consumer ────── uses latest PathSet per source
  ```

---

## 5. Recommended Upgrade Path

### Phase A: Per-triangle materials (prerequisite for B)
1. Extend `Room` trait with mesh + intersection
2. Change material storage from `[WallMaterial; 6]` to `Vec<WallMaterial>` + per-face index
3. Update `ImageSourceResolver` for planar reflections off arbitrary triangles
4. All existing physics (PathEffects, FDN) continues working unchanged

### Phase B: Ray-traced early reflections
1. Add BVH spatial acceleration
2. Implement `RayTraceResolver` (fixed ray count, configurable bounces)
3. Run synchronously on audio thread initially (profile to find budget)
4. If too slow: add background thread with rtrb for async results

### Phase C: Ray-traced late reverb (optional)
1. Extend ray tracer to collect energy histogram (arrival time → energy)
2. Feed late-arriving energy into FDN instead of Sabine-derived send
3. RT60 can be estimated from ray energy decay rate

### Skip: Non-rectangular room modes
- Minimal perceptual benefit for the effort
- Only relevant for <200 Hz bass frequencies
- Requires FEM/BEM solver (massive scope or external dependency)
- No real-time audio engine does this in practice

---

## 6. External References

- **raytraced-audio** crate: persistent ray architecture, Rust. See `REFERENCES.md` in project root.
- **Embree**: Intel's high-performance ray tracing library (C, with Rust bindings). Industry standard for BVH traversal.
- **Kuttruff, "Room Acoustics" (5th ed., 2009)**: Ray tracing chapters 5–6, image source chapter 4.
- **Vorländer, "Auralization" (2nd ed., 2020)**: Comprehensive treatment of ray-based room simulation.
- **ISO 3382-1**: Measurement of room acoustic parameters (RT60, EDT, C80) — useful for validating ray tracer output.
