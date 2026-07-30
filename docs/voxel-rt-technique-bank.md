# voxel-rt — Technique Bank (Shadertoy / IQ inspiration)

Menu of borrowable techniques, triaged for a **voxel DDA** engine and tagged with
the experiment slot they feed (`voxel-rt-plan.md`). Sources are shader art, not
papers — the value is *beauty-per-millisecond tricks*, which is exactly our
weak spot after E1 measured ray-traced AO at +5.8–8.1 ms.

Primary source: **Inigo Quilez, "Painting a Landscape with Maths"** —
[shader 4ttSWf](https://www.shadertoy.com/view/4ttSWf) ·
[tutorial](https://www.youtube.com/watch?v=BFld4EBO2RE) (transcript reviewed).
Others: [7XsGDM](https://www.shadertoy.com/view/7XsGDM) (voxel DDA + bit-packed
data + triplanar + SDF instancing), [Ms33WB](https://www.shadertoy.com/view/Ms33WB)
(post-process SSAO from depth+normal), [Xl3XzS](https://www.shadertoy.com/view/Xl3XzS)
(rounded voxel edges, AA, volumetric clouds, semitransparent water),
[WtSfWK](https://www.shadertoy.com/view/WtSfWK) (analytic AO).

---

## Top candidates (highest value first)

### T1 — Soft shadows from the distance field → E1b/E2-era shadow lever
IQ gets penumbrae from a *single* shadow ray: while marching, track
`min(signed_distance_to_geometry / t)` and smoothstep it. **We already have a
chebyshev distance field (Pascal's S2 win, bindings 9/10)** — the same quantity
is already being fetched during traversal, so soft shadows may cost near zero
extra work. Hard shadows stay the voxel-purist default; soft becomes a lever.
A/B against hard shadows for look and ms.

### T2 — Three-channel exponential atmosphere → E7
One `exp(-density * distance)` per RGB channel with *different* constants:
mid-distances go blue, far distances desaturate to gray. Physically motivated
(real transmittance), costs a handful of ALU, and is arguably the single biggest
"expensive render" impression per millisecond in the whole video. Also gives
free depth separation between foreground/background layers.

### T3 — Foliage normal blending (fixes "canopy confetti") → E7 / B10
IQ blends each tree's normal with the *terrain* normal ~2:1, so vegetation
lighting reveals the hill shape underneath. Because `dot()` is linear, blending
normals == blending lighting. Our canopy/grass voxels currently read as noise;
biasing leaf-voxel normals toward the local terrain normal should make canopies
read as landform. Cost: one extra normal fetch/estimate. (voxel-sandbox has the
same confetti problem — this may port back.)

### T4 — Fake bounce light: opposite-sun direction × albedo tint → quality presets
Before real GI, IQ adds a second "sun" from roughly the opposite direction,
tinted by the ground color, at ~1/10 the key intensity. This is the **cheap GI
tier below CAGI** — exactly what a Potato/Quest preset needs, and a fallback if
CAGI's cost lands badly. Nearly free.

### T5 — Value noise with analytic derivatives + FBM → E3 (GPU worldgen)
Cubic polynomial per unit tile with corner-shared coefficients and zero edge
derivatives → C1-continuous noise whose gradient is analytic (no finite
differencing, no extra samples). Octaves summed with small-Pythagorean-triple
rotation matrices (3-4-5, 8-15-17) to decorrelate cheaply — no sin/cos. Ideal
formulation for a compute-shader generator; A/B candidate against voxel-core's
current noise and the VoxelChain subdivision+CA generator.

### T6 — Band-pass the terrain at synthesis time → E3
Omit the FBM octaves whose wavelengths fall in the vegetation scale range
(~64 m down to ~0.5 m) so rocky detail doesn't fight plants. Cheaper and
cleaner than filtering after the fact.

### T7 — Analytic AO from local occupancy → E1b
[WtSfWK](https://www.shadertoy.com/view/WtSfWK)-style closed-form occlusion. For
voxels: derive AO from the 3×3×3 neighbourhood bits already resident in the
brick — no rays. Expected orders of magnitude cheaper than E1's measured
3.4–4.3 ms per ray. Contact-only (misses distant overhangs) → pairs with T1/T8.

### T8 — SSAO from a depth+normal G-buffer → E1b (middle tier)
[Ms33WB](https://www.shadertoy.com/view/Ms33WB). Catches medium-scale occlusion
analytic AO can't, for ~0.5–1.5 ms, and is the G-buffer + bilateral machinery
that E1 said half-res AO would need anyway. Screen-space artifacts are the cost.

---

## Look-pass grab bag (all E7, all cheap)

- **Sun glare**: `pow(max(dot(view_direction, sun_direction), 0), 4)` × warm
  tint — the photographic bloom-ish glow, no bloom pass needed.
- **Vignette**: product of two parabolas (zero at screen edges, one at centre),
  flattened by a high root, scaled/biased.
- **Contrast/vibrance via smoothstep** on the final color — darkens darks,
  brightens brights, one instruction.
- **Per-channel gamma grading** (e.g. greens lifted) for lush/translucent
  vegetation feel.
- **Rim / silhouette highlights**: `pow(1 - dot(normal, view), k)` × tint,
  faded by a vertical ramp — makes voxel forms read against the sky.
- **Distance-faded local tweaks**: undo an effect smoothly with distance
  (IQ mutes distracting dark tree patches far away).
- **Specular with grazing boost** (Fresnel-ish `sqrt` term) — water at E6.
- **Rounded voxel edges + silhouette AA** ([Xl3XzS](https://www.shadertoy.com/view/Xl3XzS)):
  soften the box intersection; big perceived-quality win for little cost.

## Clouds (E7 / B-slot)

- **Volumetric clouds by density accumulation** along the ray through a noise
  field (FBM added to a thin infinite box) rather than iso-surface hits.
- **Low-pass the gradient, not the shape**: light the clouds using only ~4 noise
  octaves for the gradient while using all octaves for density — fakes multiple
  scattering, makes clouds look soft instead of crunchy. Genuinely clever.
- **Sky/ground bounce tinting**: up-facing cloud gradient gets sky blue,
  down-facing gets green from the forest below.
- **Cloud shadows on terrain, almost free**: intersect the sun ray with the
  cloud plane, evaluate the same density function at that point, smoothstep it
  into the sun term. Shadows then track the clouds automatically.
- Note: voxel-sandbox already has a clouds ring — port the *look*, not the code.

## Instancing / entities (B5, B10)

- **Domain-tiled SDF instancing**: `floor(p / tile)` for the index, hash the
  index for per-instance randomness (position jitter, size, colour, species),
  evaluate ONE ellipsoid SDF → millions of trees for the cost of one primitive.
  Relevant to the dossier's OBB/SDF + local-DDA entity path, and to placing
  vegetation during E3 generation.
- **Cheap approximate ellipsoid SDF** (exact needs a degree-6 solve; IQ uses a
  robust approximation) + squared 3D noise added to the SDF for organic
  silhouettes.
- **Triplanar mapping** ([7XsGDM](https://www.shadertoy.com/view/7XsGDM)) — UV-free
  texturing if we ever want surface detail on voxel faces.

## Not for us

Isometric projection (we target VR perspective) · full SDF-raymarched terrain as
the *renderer* (we are voxel-authoritative — but SDF/noise math is welcome as a
*generator*, see T5) · IQ's single-giant-formula architecture (our modularity
rule wins) · temporal accumulation / denoisers (noiselessness is the engine's
identity).

## Process note

Every technique above enters as a **lever with a bench scenario and a preset
tier** (per the plan's A/B and isolation rules), never as an unconditional
change. Cheap tricks (T2, T4, glare, vignette, grading) are what make a
Quest/Potato preset still look good when the expensive rays are switched off.
