# Research Paper Findings

Detailed technical extraction from 31 research papers (16 VBAP, 6 DBAP, 9 Ambisonics). Organized by topic with exact formulas, parameter values, perceptual test data, and implementation notes.

---

## Table of Contents

1. [VBAP Core Algorithm](#1-vbap-core-algorithm)
2. [VBAP Spread Control (MDAP)](#2-vbap-spread-control-mdap)
3. [VBAP Triangulation Methods](#3-vbap-triangulation-methods)
4. [Pre-Computed VBAP Gain Lookup](#4-pre-computed-vbap-gain-lookup)
5. [Directional Reflections (SDN + VBAP)](#5-directional-reflections-sdn--vbap)
6. [VBAP vs Ambisonics: Off-Centre Performance](#6-vbap-vs-ambisonics-off-centre-performance)
7. [VBAP Trajectory Perception](#7-vbap-trajectory-perception)
8. [VBAP Sagittal Plane Localization Limits](#8-vbap-sagittal-plane-localization-limits)
9. [Extended VBAP (Polarity Inversion)](#9-extended-vbap-polarity-inversion)
10. [Distance Perception: WFS vs VBAP](#10-distance-perception-wfs-vs-vbap)
11. [DBAP Core Algorithm](#11-dbap-core-algorithm)
12. [Improved DBAP (Sundstrom 2021)](#12-improved-dbap-sundstrom-2021)
13. [DBAP Perceptual Evaluation](#13-dbap-perceptual-evaluation)
14. [IRCAM Spat / Panoramix Architecture](#14-ircam-spat--panoramix-architecture)
15. [Ambisonics Encoding and Decoding](#15-ambisonics-encoding-and-decoding)
16. [AllRAD Decoder](#16-allrad-decoder)
17. [Ambisonics Rotation](#17-ambisonics-rotation)
18. [Bilateral Ambisonics](#18-bilateral-ambisonics)
19. [Multi-Delay Spatial Ambisonics Effects](#19-multi-delay-spatial-ambisonics-effects)
20. [Ambisonics vs Dolby Atmos Perception](#20-ambisonics-vs-dolby-atmos-perception)
21. [Latency of Spatial Audio Plugins](#21-latency-of-spatial-audio-plugins)
22. [Per-Speaker Delay Compensation](#22-per-speaker-delay-compensation)
23. [Perceptual Reference Thresholds](#23-perceptual-reference-thresholds)
24. [VBAP vs Ambisonics Decision Matrix](#24-vbap-vs-ambisonics-decision-matrix)
25. [Known Gotchas and Failure Modes](#25-known-gotchas-and-failure-modes)
26. [Prioritized Implementation Ideas](#26-prioritized-implementation-ideas)

---

## 1. VBAP Core Algorithm

**Sources:** Pulkki 1997 (sap_Pulkki1997.pdf), Pulkki ~2001 (vbap.pdf)

### 2D Formulation

Source positioned between two speakers by solving:

```
p = g_1 * l_1 + g_2 * l_2

g = p^T * L_12^{-1}

where L_12 = [l_1, l_2]^T
```

Normalize for energy preservation:

```
g_normalized = g / sqrt(g_1^2 + g_2^2)
```

Speaker pair selection: find the pair where source direction falls between them (both gains positive). Pairs must be adjacent by azimuth.

### 3D Formulation

For loudspeaker triplet {i, j, k}:

```
L = [l_i, l_j, l_k]^T    (3x3 matrix of speaker direction unit vectors)
g_tilde = p^T * L^{-1}
g = g_tilde / ||g_tilde||
```

Triplet selection: source direction vector intersects exactly one triangle (all three gains non-negative).

### Key Properties

- Only 2 speakers active (2D) or 3 speakers (3D) at any time
- Energy preserved: `sum(g_i^2) = 1`
- When source coincides with speaker: that speaker gets gain 1.0, others 0.0
- On triangle edge: two speakers share gain, third is zero

### Gain Interpolation for Moving Sources

- Interpolation period: 10-50ms to avoid zipper noise
- Interpolate gains, not source positions
- When moving between triangles, gains crossfade naturally at shared edges

### Distance Encoding (BCI variant)

```
g_1^2 + g_2^2 = c,    0 <= c <= 1
```

c = 1.0 is maximum loudness, lower values encode distance via energy reduction.

**Source:** Nishikawa et al. (356.pdf)

---

## 2. VBAP Spread Control (MDAP)

**Source:** Pulkki (ExtendingPerceivedVBAP.pdf)

Multiple Direction Amplitude Panning increases perceived source width by adding auxiliary VBAP sources around the main direction.

```
g_total = (1/sqrt(N_spread)) * sum(g_i)    (energy-normalized sum)
```

### Parameter Guidelines

| Spread Angle | Perceived Effect |
|---|---|
| 0 degrees | Point source (standard VBAP) |
| 10-20 degrees | Noticeable widening, localization preserved |
| > 30 degrees | Source becomes diffuse, localization degrades |

- Distribute spread sources symmetrically around nominal direction
- For 3D: distribute on a cone around the source direction vector
- 4-8 spread sources typically sufficient for smooth widening
- Perceived width depends on frequency content: broadband sources widen more easily than narrowband

---

## 3. VBAP Triangulation Methods

### Method A: Convex Hull + Ray-Triangle Intersection

**Source:** Choi 2012 (an-alternative-implementation-of-vbap.pdf)

1. Quickhull3D algorithm for speaker triangulation (convex hull of speaker positions on unit sphere)
2. Ray-triangle intersection for triplet selection: cast ray from origin through source position
3. Intersection point gives barycentric coordinates = gain ratios

Gain computation for speakers S0, S1, S2 with intersection point I:
```
g_i = (dist(S_j, I) + dist(S_k, I)) / (dist(S0,I) + dist(S1,I) + dist(S2,I))
```

Advantages:
- Standard computational geometry — any convex hull library works
- GPU-acceleratable (ray-triangle is a standard graphics primitive)
- Handles up to 200+ speakers in real-time
- Computed once at startup unless layout changes

Gotcha: when source is inside a speaker triangle, per-speaker delay becomes negative (physically impossible).

### Method B: Matrix Inverse (Original Pulkki)

Pre-compute `L^{-1}` for each speaker triplet. At runtime, multiply `g = p^T * L^{-1}`, check if all gains are non-negative. This is the method currently used in the Atrium engine.

---

## 4. Pre-Computed VBAP Gain Lookup

**Source:** Shukla 2019 (Skukla Real-time binaural 2019 Accepted.pdf)

Pre-compute gains at 1-degree azimuth/elevation resolution into a lookup table at startup.

### Specifications from Shukla's Implementation

- Platform: Bela Mini (1 GHz ARM Cortex-A8)
- 8 virtual speakers, 16-bit 44.1 kHz
- Buffer: 2048 samples
- Total latency: 2581 samples = 59ms (within 75ms threshold)
- Head-tracking update: BNO055 IMU at 86 Hz (every ~512 audio samples)

### VBAP vs FOA Binaural Error Comparison

| Location | VBAP ITD (us) | FOA ITD (us) | VBAP ILD (dB) | FOA ILD (dB) |
|---|---|---|---|---|
| All positions | 150 +/- 114 | 202 +/- 123 | 3.18 +/- 2.58 | 4.69 +/- 3.05 |
| Front only | 98 | 157 +/- 117 | 1.75 +/- 1.45 | 3.90 +/- 2.50 |
| Horizontal | 140 +/- 120 | 216 +/- 138 | 1.62 +/- 1.43 | 3.85 +/- 2.08 |

VBAP outperforms FOA consistently for both ITD and ILD across all positions.

### Implementation Notes

- HRIR length: 256 samples is sufficient (vs standard 512 — halves convolution cost)
- 86 Hz head-tracking update rate is sufficient
- ITD measurement: 10th-order Butterworth LPF at 3 kHz, cross-correlation of L/R
- ILD measurement: 10th-order Butterworth HPF at 1.5 kHz, mean-squared power difference

---

## 5. Directional Reflections (SDN + VBAP)

**Source:** Yeoward et al. 2021 (21532.pdf), JAES Vol.69 No.11

### Scattering Delay Network (SDN)

Isotropic scattering matrix for K=5 nodes (cuboidal room, 6 surfaces):
```
A = (2/K) * 1 * 1^T - I
```

Per-node operation: 5 additions, 1 multiplication, 5 subtractions (2K+1 operations).

Delay from source to microphone:
```
D_{S,M} = floor(F_s * ||x_S - x_M|| / c)
```

### Surface Absorption Coefficients

| Surface | alpha |
|---|---|
| Carpet on concrete | 0.18 |
| Hard walls | 0.0343 |
| Perforated ceiling tiles | 0.7 |

### Air Absorption Filter

First-order LPF (Moorer's approximation):
```
T(z) = (1 - g) / (1 - g * z^{-1})
g = (1/5) * log(d/3 + 1)
```
(humidity 50%, Fs = 44.1 kHz, d = distance in meters)

### Flutter Mitigation

Modulated delay lines between nodes (NOT source/mic connections):
- Linear interpolation for fractional delays
- Modulation amplitude: 0.003 (of delay line length)
- Modulation frequency: 0-2 Hz, random per node

### 5 Spatialization Variants Tested

| Algorithm | VBAP Inputs | Description |
|---|---|---|
| FullSpat | 7 | Each node output independently VBAP-panned |
| LatSpat | 5 | Wall nodes panned, floor/ceiling distributed equally |
| MonoIS | 7 | All nodes summed then sent to each VBAP input |
| MonoD | 1 | All summed, distributed equally to all speakers |
| MonoP | 1 | All summed, panned with dry source |

### Perceptual Results (20 participants, MUSHRA-style, Sennheiser HD650)

**Envelopment — Large Hall (12m x 10m x 6m):**
- Friedman: chi^2 = 138.68, p < 0.001
- FullSpat vs MonoIS: p <= 0.001
- LatSpat vs MonoIS: p < 0.001
- **No significant difference between FullSpat and LatSpat**

**Naturalness — Large Hall:**
- chi^2 = 104.49, p < 0.001
- Either spatialized method significantly more natural (p <= 0.005) than each Mono method

**Small Room (5m x 5m x 3m):**
- Spatialized methods NOT more natural than MonoIS or MonoD
- Envelopment still higher for spatialized methods
- For small rooms, simplified reverb may sound equally natural

### Room Parameters Used

| Room | Dimensions | T60 Predicted | T60 Measured | Source Distance |
|---|---|---|---|---|
| Small office | 5m x 5m x 3m | 0.50s | 0.80s | 1.5m |
| Large library | 12m x 10m x 6m | 1.01s | 1.47s | 3.0m |

SDN T60 measurements run 45-60% higher than Sabine prediction.

### Key Takeaway

**LatSpat is perceptually equivalent to FullSpat** — spatializing only lateral (wall) reflections saves 2 VBAP evaluations per source with no perceptual loss. For small rooms, even mono reverb may be sufficient for naturalness.

---

## 6. VBAP vs Ambisonics: Off-Centre Performance

**Source:** Satongar, Dunn, Lam, Li (BBC R&D) 2013 (WHP254.pdf)

### Test Setup

- Sphere model: radius 9cm
- Source distance: 1.4m
- Layouts: Hexagon, Octagon, ITU 5.0
- HRTF: Analytical sphere + MIT KEMAR validation

### Central Position Errors

| Method | ITD Error (us) | ILD Error (dB) |
|---|---|---|
| Ambisonics 1st order | 32 | 3.2 |
| Ambisonics 2nd order | 6.6 | 2.2 |
| Ambisonics 3rd order | 1.4 | 1.4 |
| VBAP ITU 5.0 | 92 | 1.8 |

### Off-Centre Errors (critical for multi-listener)

| Method | ITD Error (us) | ILD Error (dB) |
|---|---|---|
| Ambisonics 1st order | 537 | 5.6 |
| Ambisonics 2nd order | 460 | 5.0 |
| Ambisonics 3rd order | 392 | 4.2 |
| Ambisonics 4th order | 262 | 3.3 |
| VBAP ITU 5.0 | 326 | 4.5 |

### 5 dB ILD Error Threshold Frequencies

| Order | Frequency |
|---|---|
| 1st order | 800 Hz |
| 2nd order | 1600 Hz |
| 3rd order | 2500 Hz |

### Key Conclusions

- **4th-order Ambisonics needed to beat VBAP on ITU 5.0 for off-centre ITD** (requires 25 speakers)
- **3rd-order needed for off-centre ILD** (requires 16 speakers)
- VBAP degrades gracefully off-centre; Ambisonics degrades rapidly
- For multi-listener scenarios: **VBAP is more robust on 5.1**

---

## 7. VBAP Trajectory Perception

**Source:** Gandemer et al. 2018 (Gandemer (2018) Acta Acustica.pdf)

### Setup

- 42-loudspeaker geodesic sphere, 3m diameter
- Genelec 8020C (65Hz-21kHz +/-2.5dB)
- Room RT60 = 538ms at 125Hz, 155ms at 8kHz
- Fs = 48 kHz, panning updated every 10.7ms (512 samples)
- HOA: 5th order, basic decoding, energy preserving, SN3D

### Results: VBAP vs 5th-Order HOA

| Metric | VBAP | HOA 5th | Statistic |
|---|---|---|---|
| Surrounding trajectory recognition | 90-95% | ~60% | chi^2(1) = 273.67, p < 0.0001 |
| Trajectory fluidity | Lower | Higher | chi^2(1) = 46.23, p < 0.0001 |
| Height perception accuracy | At ear level | ~0.5m too high | chi^2(1) = 185.41, p < 0.0001 |

### Magnetization Effect

VBAP sources "snap" toward loudspeaker positions during movement, making trajectories less fluid. This is VBAP's primary trajectory weakness.

Mitigation: denser speaker arrays or gain interpolation at <= 10ms intervals.

---

## 8. VBAP Sagittal Plane Localization Limits

**Source:** Baumgartner & Majdak 2015 (emss-65316.pdf), JAES Vol.63 No.7-8

### The 40-Degree Rule

Loudspeaker spans below **40 degrees polar angle** needed for good sagittal-plane localization.

- At 40-degree span: VBAP explains >= 50% of localization variance (r^2)
- Polar error increases up to **50 degrees** at large spans
- Variability across listeners: up to **40 degrees** error increase

### Speaker Layout Recommendations

| Layout | Max Polar Span | Expected Performance |
|---|---|---|
| 22.2 (NHK) | ~34 deg | Good |
| 30 deg elevation layer | ~34 deg polar at 30 deg azimuth | Good |
| 45 deg elevation layer | ~55 deg polar | Poor |
| Auro-3D 9.1 | Variable | Adequate in front, poor at sides |

### Design Rules

1. Frontal region most sensitive to spectral cues — pack speakers densely there
2. 30 degrees elevation is optimal for elevated layer
3. 45 degrees elevation is too high — causes ~55-degree polar angle errors
4. Median plane is worst case — relies entirely on spectral cues that VBAP disrupts
5. Individual HRTF variation means some listeners always have worse performance

### Implication for Atrium

The 5.1 layout has no height speakers, so sagittal-plane localization is impossible via VBAP. This is fine since the engine treats VBAP as horizontal-only.

---

## 9. Extended VBAP (Polarity Inversion)

**Source:** Pulkki (ExtendingPerceivedVBAP.pdf)

Negative gains can extend perceived source positions ~40% beyond the physical speaker boundary. Implementation requires careful management to avoid comb filtering artifacts.

---

## 10. Distance Perception: WFS vs VBAP

**Source:** Gutierrez-Parera et al. 2014 (35.pdf)

### Setup

- WFS: 64 loudspeakers, octagonal, 18cm spacing
- Aliasing frequency: ~1 kHz
- 25 subjects (15M, 10F, ages 24-35)
- Synthesized distances: 1.74, 2.45, 3.46, 4.9, 6.91, 9.78, 13.81m
- Room: 96 m^3, T60 < 0.25s

### Statistical Results

| Factor | Statistic | p-value | Conclusion |
|---|---|---|---|
| WFS vs VBAP distance | t = 16, gl = 2799 | p < 0.001 | WFS significantly better |
| Sound type | F = 131.62, gl = 3 | p < 0.001 | Significant (guitar best) |
| Early reflections | — | p = 0.388 | **NOT significant** |
| Listening angle | F = 56.08, gl = 1 | p < 0.001 | Frontal better than lateral |

### Sound Type Ranking for Distance Perception

1. Guitar (best)
2. Door closing
3. Voice
4. Pink noise (worst)

### Key Findings

- Inverse-square law (-6 dB per distance doubling) is the primary distance cue for both systems
- **Early reflections (4 virtual walls, ~20ms delays) did NOT significantly improve VBAP distance perception**
- Frontal presentation significantly better than lateral for distance judgment
- Both systems compress distance: perceived < synthesized

---

## 11. DBAP Core Algorithm

**Sources:** Lossius et al. 2009/2011 (icmc2009-dbap-rev1.pdf, dbap-distance-based-amplitude-panning.pdf, DBAP_-_Distance-Based_Amplitude_Panning.pdf)

### CRITICAL: Version Discrepancies

The original ICMC 2009 proceedings has WRONG formulas. The 2011-corrected version fixes equations 3-6 and 9-10. **USE THE CORRECTED VERSION BELOW.**

### Corrected Formulas

Distance with spatial blur:
```
d_i = sqrt((x_i - x_s)^2 + (y_i - y_s)^2 + r_s^2)
```

Rolloff exponent:
```
a = R / (20 * log10(2))
```

Gain for speaker i:
```
v_i = k / d_i^a
```

Normalization (constant intensity):
```
k = 1 / sqrt(sum_{i=1}^{N} (1 / d_i^(2a)))
```

Combined form (avoids computing k separately):
```
v_j = 1 / sum_{i=1}^{N} (d_j^(2a) / d_i^(2a))
```

With per-speaker weights:
```
v_i = k * w_i / d_i^a
k = 1 / sqrt(sum_{i=1}^{N} (w_i^2 / d_i^(2a)))
```

### Rolloff Parameter R

| R Value | Environment | a Value |
|---|---|---|
| 6 dB | Free field / anechoic | ~1.0 |
| 3-5 dB | Reverberant room | 0.5-0.83 |
| < 3 dB | Very reverberant / large space | < 0.5 |

R = 6 dB gives a = 6 / (20 * log10(2)) = 6 / 6.0206 ~ 0.9966 ~ 1.0

### Spatial Blur

`r_s` prevents division by zero and coloration when source approaches a speaker.

Behavior at speaker position with r_s = 0:
```
lim_{d_j->0} v_i = { 1 if i == j, 0 if i != j }
```
Only that speaker emits — causes unwanted coloration changes.

Too much blur triggers the precedence effect — perceived direction gravitates to nearest speaker to listener.

### Sources Outside the Speaker Field (Original Method)

1. Compute convex hull of loudspeaker positions
2. Test if source is inside/on hull boundary
3. If outside: project source onto hull (nearest point)
4. Use projected point for gain calculations
5. Distance from source to hull can drive: gain attenuation, air filtering, Doppler, reverb

---

## 12. Improved DBAP (Sundstrom 2021)

**Source:** Sundstrom 2021 (2109.08704v1.pdf), arXiv

### Problems with Original DBAP

1. **Convex hull projection at vertices produces nonunique solutions** — a source circling a vertex-speaker produces a "flat spot" where perceived movement stops
2. **Spatial discontinuities at hull boundary** — crossing the threshold causes wild power undulations
3. **Convex hull is expensive in 3D** — requires point-in-hull test + quadratic programming for projection

### The Fix: Power Scaling Variable p

Replaces the convex hull entirely:

```
q = max(d_s) / d_rs
p = q    if q < 1
p = 1    otherwise
```

Where:
- `max(d_s)` = distance from reference point to the most distant speaker
- `d_rs` = distance from reference point to the virtual source
- Reference point: best placed at the field centroid

New normalization:
```
k = p^(2a) / sqrt(sum_{i=1}^{N} (w_i^2 / d_i^(2a)))
```

Effect: creates a circle around the reference with radius = distance to farthest speaker. Power = 1 inside. Power falls off at rate R outside. No convex hull needed.

### Biasing for Far-Outside Sources

When source is very far, all distance ratios approach 1 and source collapses to center. Fix:

```
v_i = k * w_i * b_i / d_i^a
k = p^(2a) / sqrt(sum_{i=1}^{N} (b_i^2 * w_i^2 / d_i^(2a)))
b_i = (u_i / u_m * (1/p - 1))^2 + 1
u_i = (d_i - max(d))^2_normalized + epsilon
```

Where:
- m = index of median-distanced speaker from source
- max(d) = most distant speaker from source
- epsilon = r_s / N

### Recommended Spatial Blur Scaling

```
r_s = (sum_{i=1}^{N} d_ic / N) * r_scalar
```

Where d_ic = distance from centroid to speaker i. **Recommended range: 0.2 <= r_scalar <= 0.5** (default: 0.2).

Makes blur adapt automatically to different layout sizes.

### Complete Implementation Pseudocode

```
// Input: source (x_s, y_s), N speakers at (x_i, y_i),
//        rolloff R (dB), blur r_scalar, weights w_i

// 1. Compute blur
r_s = mean(dist(centroid, speaker_i)) * r_scalar    // r_scalar in [0.2, 0.5]

// 2. Rolloff exponent
a = R / (20 * log10(2))

// 3. Distances with blur
for each speaker i:
    d_i = sqrt((x_i - x_s)^2 + (y_i - y_s)^2 + r_s^2)

// 4. Power scaling (replaces convex hull)
d_rs = dist(centroid, source)
max_ds = max(dist(centroid, speaker_i) for all i)
q = max_ds / d_rs
p = if q < 1.0 { q } else { 1.0 }

// 5. (Optional) Biasing for far-outside sources

// 6. Gains
for each speaker i:
    raw_i = w_i / d_i^a

sum_sq = sum(raw_i^2 for all i)
k = p^(2*a) / sqrt(sum_sq)

for each speaker i:
    v_i = k * raw_i
```

### DBAP Layout Guidance

- Grid layouts are ideal for DBAP — evenly sample the space
- **DBAP should NOT be first choice for regular polygonal layouts** — use VBAP or Ambisonics instead
- DBAP is specifically for arbitrary speaker positions

---

## 13. DBAP Perceptual Evaluation

**Source:** Kostadinov, Reiss, Mladenov 2010 (0000285.pdf), ICASSP

### Setup

- 16 speakers, 3 tiers, azimuth -152.4 to 180 degrees, elevation -30 to 28.3 degrees
- 12 participants (7 experienced, 2 some experience, 3 none)
- 5 sound clips (3 bands, 1 female voice, 1 instrumental)
- All tests used delay compensation (see Section 22)

### DBAP vs VBAP (3 speakers, 3 positions)

| Position | Intended Az/El | DBAP Avg Az | DBAP Avg El | VBAP Avg Az | VBAP Avg El |
|---|---|---|---|---|---|
| 1 | 25/10 | 26.29 | 8.58 | 26.33 | 5.9 |
| 1 (1m back) | 25/10 | 33.75 | 7.75 | 27.68 | 4.36 |
| 2 | 130/0 | 140.64 | 4.54 | 131.79 | 5.17 |
| 3 | -20/-20 | -10.75 | 7.5 | -10.96 | -2.9 |

DBAP comparable to VBAP at sweet spot. When listener moves 1m back, DBAP azimuth error increases ~7.5 degrees vs ~1.4 for VBAP. DBAP has slightly better standard deviation (more consistent across listeners).

### DBAP vs 3rd-Order Ambisonics (16 speakers, 2 positions)

| Position | Intended Az/El | DBAP Avg Az | DBAP Avg El | Ambi Avg Az | Ambi Avg El |
|---|---|---|---|---|---|
| 4 | 45/30 | 35.13 | 10.67 | 5.16 | 49.0 |
| 4 (1m left) | 45/30 | 32.13 | 9.67 | 23.95 | 25.0 |
| 5 | 160/45 | 163.25 | 36.08 | 8.33 | 62.0 |

**DBAP massively outperforms 3rd-order Ambisonics on irregular layouts.** Ambisonics gave catastrophically wrong results (position 5: intended azimuth 160, got 8.33). The asymmetric array (no speakers below horizontal) broke Ambisonics entirely.

### Key Findings

- DBAP localization accuracy close to VBAP
- DBAP far more robust than Ambisonics on irregular layouts
- DBAP is insensitive to listener position
- Moving 1m back or left barely changed DBAP results (~3 degree shift)

### Caveats

- Only 1 speaker arrangement tested
- Only 12 subjects
- No statistical significance tests reported (no p-values)

---

## 14. IRCAM Spat / Panoramix Architecture

**Source:** Carpentier 2017 (panoramix_lac2017.pdf), LAC

### 4-Segment Room Model

| Segment | Time Range | Character | Spatialization |
|---|---|---|---|
| Direct sound | 0 ms | Point source | VBAP to source direction |
| Early reflections | ~1-80 ms | 8 or 16 discrete echoes | VBAP per reflection direction |
| Late reflections | ~80-200 ms | Diffuse, decorrelated | Distributed across speakers |
| Reverb tail | 200+ ms | Exponential decay | Source-count-independent |

### Reverb Engine (FDN)

Feedback Delay Network by Jot and Chaigne (1991):
- 8 feedback channels typical
- 3-band decay control (lo/mid/hi)
- Mixing matrix: orthogonal

### Architecture Patterns

**Parallel bussing:** Each track feeds up to 3 busses simultaneously:
1. Multiple format mixes (VBAP + Ambisonics for A/B comparison)
2. Hybridize techniques (binaural + stereo blend to combat HRTF coloration, front-back confusions, in-head localization)

**Signal flow:** Input tracks -> preprocessing (compression, EQ, delay) -> parallel busses (spatialization + reverb) -> Master output

### Supported Techniques

- VBAP (2D and 3D)
- HOA (any order, all normalizations: N3D, N2D, SN3D, SN2D, FuMa, MaxN; all orderings: ACN, SID, Furse-Malham)
- Binaural (SOFA/AES-69 HRTFs, both HRIR convolution and SOS+ITD)
- HOA decoders: sampling, mode-matching, energy-preserving, all-round; dual-band with adjustable crossover

---

## 15. Ambisonics Encoding and Decoding

**Sources:** Arteaga 2023 (Introduction_to_Ambisonics.pdf), Zotter & Frank 2019 (1007063.pdf)

### FOA B-Format Encoding (FuMa)

```
W = signal * (1/sqrt(2))           // omnidirectional (pressure)
X = signal * cos(az) * cos(el)     // front-back
Y = signal * sin(az) * cos(el)     // left-right
Z = signal * sin(el)               // up-down
```

### FOA Decoding to Speaker l at (az_l, el_l)

```
speaker_l = (1/sqrt(2)) * W + cos(az_l)*cos(el_l) * X
          + sin(az_l)*cos(el_l) * Y + sin(el_l) * Z
```

### HOA Encoding (ACN/SN3D — use this, it's the modern standard)

```
Channel index: ACN = n^2 + n + m    (order n, degree m, -n <= m <= n)
Total channels for order N: (N+1)^2
  Order 0: 1 ch,  Order 1: 4 ch,  Order 2: 9 ch,  Order 3: 16 ch

SN3D normalization:
  N_n^|m| = sqrt((n-|m|)! / (n+|m|)!) * sqrt((2 - delta_m) / 2)

Encoding coefficient:
  Y_n^m(az, el) = N_n^|m| * P_n^|m|(sin(el)) * { cos(m*az) if m >= 0
                                                    sin(|m|*az) if m < 0 }
```

FuMa to SN3D conversion: `W_FuMa = W_SN3D * sqrt(2)`, `XYZ_FuMa = XYZ_SN3D * sqrt(3)`

### Decoder Types and rE Values

| Decoder | 2D rE formula | 3D rE |
|---|---|---|
| Basic/Physical | 1/(N+1) | 1/(N+1) |
| max-rE | cos(pi/(2N+2)) | see below |
| in-phase | N/(N+1) | N/(N+1) |

3D max-rE weights:
```
a_n = P_n(cos(137.9 / (N+1.51)))    // P_n = Legendre polynomial
```

| Order N | 3D max-rE | 3D in-phase |
|---|---|---|
| 1 | 0.577 | 0.500 |
| 2 | 0.775 | 0.667 |
| 3 | 0.861 | 0.750 |
| 4 | 0.906 | 0.800 |
| 5 | 0.933 | 0.833 |

### Irregular Layout Decoding (Mode-Matching)

```
D = Y_N^T * (Y_N * Y_N^T)^{-1}
```

Where Y_N is the re-encoding matrix (spherical harmonics sampled at speaker positions).

### Headphone Decoding

```
left(t)  = sum_{n,m} chi_nm(t) * h_left_nm(t)
right(t) = sum_{n,m} chi_nm(t) * h_right_nm(t)
```

Where h_nm are the spherical harmonic decomposition of the HRTF set. Requires HRTF datasets sampled at (N+1)^2 directions minimum (t-design preferred).

For headphones, order 3-5 is sufficient for good externalization; beyond order 5 the perceptual improvement diminishes.

### Associated Legendre Polynomial Recurrence

```
P_{n+1}^m(x) = (2n+1)/(n-m+1) * x * P_n^m(x) - (n+m)/(n-m+1) * P_{n-1}^m(x)

Starting values:
  P_m^m(x) = (-1)^m * (2m)! / (2^m * m!) * sqrt(1-x^2)^m
  P_{m+1}^m(x) = x * (2m+1) * P_m^m(x)
```

### Bass Management

- High-cut all channels at 70-100 Hz with 4th-order Linkwitz-Riley
- Send only omnidirectional channel (ACN 0) to subwoofer
- Time-align subwoofer with main speakers

### Dynamic Compression

- **NEVER compress individual Ambisonics channels independently** (destroys spatial image)
- Use omnidirectional channel (W/ACN_0) as side-chain for all channels
- Apply same gain to all channels simultaneously

---

## 16. AllRAD Decoder

**Source:** Zotter & Frank 2019 (1007063.pdf)

The recommended decoder for irregular speaker layouts (like 5.1).

### Algorithm

```
1. Create dense virtual layout on t-design (e.g., 240 points)
2. Decode Ambisonics to virtual layout using basic/max-rE decoder
3. Map each virtual speaker to real speakers using VBAP
4. Combine: D_AllRAD = D_VBAP * D_ambi_virtual
```

Result: gain matrix mapping Ambisonics channels to real speaker channels.

### EPAD (Energy-Preserving AllRAD)

SVD correction on top of AllRAD:
```
D_AllRAD = U * S * V^T
D_EPAD = U * (mean(diag(S)) * I) * V^T
```

### Key Design Choice

AllRAD reuses existing VBAP infrastructure — the engine already has `compute_gains_vbap()`. AllRAD just adds a virtual-to-real mapping layer.

---

## 17. Ambisonics Rotation

**Source:** Zotter & Frank 2019 (1007063.pdf), Kronlachner & Zotter 2014

### Rotation as Matrix Multiplication

```
chi_rotated = Q * chi
```

Q is block-diagonal (one block per order n, size (2n+1) x (2n+1)):
```
Q = diag(Q_0, Q_1, Q_2, ..., Q_N)
```

### First-Order z-axis Rotation by angle alpha

```
Q_1 = [cos(alpha)  0  sin(alpha)]
      [0           1  0          ]
      [-sin(alpha)  0  cos(alpha)]
```

For general 3D rotation: decompose into z-y-z Euler angles and chain. Higher-order blocks computed recursively from Q_1 using Ivanic & Ruedenberg method.

Computational cost: O(N^3) per sample for full 3D rotation.

### DSHT (Discrete Spherical Harmonics Transform)

**Source:** Kronlachner & Zotter 2014

Enables spatial modifications in sample domain:

```
Forward:  f_t = Y_t^T * chi_N         (Ambisonics -> spatial samples)
Inverse:  chi_N = (4*pi/T) * Y_t * f_t  (spatial samples -> Ambisonics)
```

Requires 2N-design minimum for exact reconstruction.

### Warping (Elevation Manipulation)

```
zeta_new = (zeta - alpha) / (1 - alpha * zeta)
where zeta = cos(elevation), -1 <= alpha <= 1

De-emphasis gain: g(zeta) = (1 - alpha * zeta) / sqrt(1 - alpha^2)
```

Gotcha: sharp spatial cuts require high orders or produce Gibbs ringing. Smooth windows (cosine-shaped) work at order 3-5.

---

## 18. Bilateral Ambisonics

**Source:** Ben-Hur et al. 2020 (246866784_349080850309095_8627456256655430198_n.pdf)

### Core Innovation

Standard Ambisonics-to-binaural applies the same SH truncation to both ears. Bilateral uses ear-aligned coordinate systems — each ear gets its own rotated Ambisonics representation.

### Algorithm

```
1. Rotate Ambisonics to left-ear-aligned frame:
   chi_left = Q_left * chi    (rotate so ipsilateral direction = front)
2. Decode with left HRTF in rotated frame:
   left(omega) = sum_{nm} chi_left_nm * H_left_nm_rotated(omega)
3. Same for right ear with Q_right

Rotation angles:
  Left ear:  rotate by +90 degrees azimuth
  Right ear: rotate by -90 degrees azimuth

For head-tracked system:
  Q_left  = R_head * R_left_ear
  Q_right = R_head * R_right_ear
```

Key insight: HRTFs have most energy and variation on the ipsilateral side. Aligning the coordinate system to each ear concentrates the limited SH resolution where it matters most.

### MUSHRA Test Results

| Condition | MUSHRA Score (median) | 95% CI |
|---|---|---|
| Reference (35th order) | 100 | — |
| Standard 1st-order | 32 | [28, 38] |
| **Bilateral 1st-order** | **45** | [40, 52] |
| Standard 3rd-order | 58 | [52, 64] |
| **Bilateral 3rd-order** | **71** | [65, 76] |
| Standard 5th-order | 78 | [73, 82] |
| **Bilateral 5th-order** | **84** | [80, 88] |

### Key Findings

- **Bilateral improves quality by ~1 order equivalent** — bilateral 3rd-order sounds like standard 5th-order
- Improvement most dramatic at low orders (1st: +13 MUSHRA points; 5th: +6)
- Computational cost: ~2x standard (two rotations + two decodings)
- No additional HRTF data required — same set, just pre-rotated
- Compatible with head tracking

### Memory

Same as standard decoding — no extra HRTF storage. Rotation is applied to the Ambisonics signal, not the HRTFs.

Gotcha: if pre-computing binaural filters (rather than runtime convolution), you need TWO sets of filters, each in their rotated frame.

---

## 19. Multi-Delay Spatial Ambisonics Effects

**Source:** Rudrich et al. 2016 (Rudrich-TMT16-Efficient_Spatial_Ambisonic_Effects.pdf)

### Spatial Feedback Delay

- 36 feedback delay lines
- Delay times: 100-300 ms
- Feedback attenuation: 2.4 dB per tap
- Each delay output encoded at different spatial position (uniformly distributed)
- Source-count-independent: process once, not per-source

### Rotational Delay

- 9 degrees rotation per 300 ms delay (preset 1)
- 10 degrees rotation per 350 ms delay (preset 2)
- Sounds spiral around listener with each echo

### Slapback Delay

- Single delay: 80 ms
- Encoded to opposing direction from source

### Widening Effect (from Zotter & Frank)

Frequency-dependent dispersive rotation:
```
R(m * phi_hat * cos(omega * tau))
```
- tau = 1.5 ms: widening (perceived width saturates above N > 2)
- tau = 15 ms: diffuseness/distance impression
- Causal-sided FIR (keep only q >= 0 terms) sounds more natural

### Implementation

All effects operate in Ambisonics domain — no decoding needed. Can be chained. Rotation is cheapest: just matrix multiply on (N+1)^2 channels. All implemented for live concert use in 5th-order (36 channels).

---

## 20. Ambisonics vs Dolby Atmos Perception

**Source:** Malecki et al. 2024 (Assessing_Spatial_Audio_A_Listener_.pdf)

### Setup

- 31 trained listeners
- Systems: Dolby Atmos (object-based) vs 3rd-order Ambisonics
- Playback: 5.1, 7.1.4, binaural headphones
- 4 musical excerpts

### Results

| Attribute | Layout | Atmos Preferred | Ambi Preferred | Significant? |
|---|---|---|---|---|
| Spatial quality | 7.1.4 | 62% | 38% | p < 0.05 |
| Spatial quality | 5.1 | 57% | 43% | Not significant |
| Spatial quality | Binaural | 55% | 45% | Not significant |
| Envelopment | 7.1.4 | 58% | 42% | p < 0.05 |
| Envelopment | 5.1 | 52% | 48% | Not significant |
| Source width | All | ~50% | ~50% | Not significant |
| Timbral quality | All | ~50% | ~50% | Not significant |

### Key Conclusions

- **Dolby Atmos advantage ONLY on 7.1.4** — the height speakers give Atmos an edge
- **On 5.1 and binaural: no significant difference**
- Source localization accuracy NOT significantly different on any configuration
- **For 5.1 without height speakers, 3rd-order Ambisonics performs comparably to Atmos**

---

## 21. Latency of Spatial Audio Plugins

**Source:** Tomasetti et al. 2023 (Latency_of_spatial_audio_plugins.pdf)

### Results at 48 kHz

| Plugin Suite | Buffer 64 | Buffer 128 | Buffer 256 | Buffer 512 | Notes |
|---|---|---|---|---|---|
| IEM | 64 | 128 | 256 | 512 | Zero added latency |
| ambix | 64 | 128 | 256 | 512 | Zero added latency |
| SPARTA | 640 | 640 | 640 | 640 | Fixed 640 samples |
| Noise Makers | 64 | 128 | 256 | 512 | Zero added latency |

### Recommendations

- Target: zero additional latency beyond system buffer for encoding/decoding
- For HRTF convolution: use non-uniform partitioned convolution (first partition = buffer size)
- Buffer 128 at 48 kHz = 2.67 ms (acceptable for live)
- Buffer 256 = 5.33 ms (acceptable)
- Buffer 512 = 10.67 ms (borderline for live monitoring)
- Binaural decoders add more latency than loudspeaker decoders due to HRTF convolution

---

## 22. Per-Speaker Delay Compensation

**Source:** Kostadinov et al. 2010 (0000285.pdf)

When listener position is known, align wavefronts from all speakers:

```
delay_n = (max(D_L1, D_L2, ..., D_LN) - D_Ln) * f_s / v_s
```

Where:
- `D_Ln` = distance from speaker n to listener
- `f_s` = sample rate
- `v_s` = speed of sound (343 m/s)

Tightens phantom image localization for both VBAP and DBAP. Used in all perceptual tests in the DBAP evaluation paper.

Implementation: ~20 lines — a per-channel fractional delay in the final output stage.

---

## 23. Perceptual Reference Thresholds

Collected from all papers:

| Threshold | Value | Source |
|---|---|---|
| ITD JND | 90 us | BBC WHP254 |
| ILD JND | 2.5 dB | BBC WHP254 |
| Azimuth accuracy (frontal) | ~1 degree | Smith 2019 |
| Azimuth accuracy (lateral) | ~7 degrees | Smith 2019 |
| Head-tracking latency budget | < 75 ms total | Shukla 2019, Yeoward 2021 |
| Head-tracking update rate | 86 Hz sufficient | Shukla 2019 |
| Gain interpolation period | 10-50 ms | Pulkki, Gandemer |
| VBAP panning update rate | every 10.7 ms (512 samples at 48kHz) | Gandemer 2018 |
| HRIR length (acceptable) | 256 samples | Yeoward 2021 |
| Max polar span (VBAP 3D) | 40 degrees | Baumgartner 2015 |
| Front-back confusion rate (2nd-order HOA) | ~15% without head-tracking, ~5% with | Medina 2024 |
| 2nd-order HOA azimuth error | 12-18 degrees | Medina 2024 |
| Lower limit of azimuth via ITD | ~80 Hz | Smith 2019 |
| Head shadowing effective above | ~500 Hz | Smith 2019 |

---

## 24. VBAP vs Ambisonics Decision Matrix

| Criterion | VBAP Wins | Ambisonics Wins |
|---|---|---|
| Off-centre listening | Yes (graceful degradation) | No (needs 4th+ order to match) |
| Trajectory smoothness | No (magnetization effect) | Yes (more fluid) |
| Localization accuracy | Yes at center with good layout | Comparable at 3rd+ order |
| Computational cost | Lower (2-3 speakers active) | Higher (all speakers active) |
| Arbitrary layouts | Yes | Needs regular layouts or AllRAD |
| Source width control | Manual (MDAP) | Natural (order-dependent) |
| Distance encoding | Gain only | Gain only (same limitation) |
| 5.1 vs Atmos | Matched or better on 5.1 | Atmos only wins on 7.1.4 |

---

## 25. Known Gotchas and Failure Modes

### VBAP

1. **Magnetization effect** — sources snap toward speaker positions during movement. Mitigate with denser arrays or gain interpolation at <= 10ms. (Gandemer 2018)
2. **Negative delay problem** — when source is between speakers, per-speaker time delay can go negative. (Choi 2012)
3. **Sagittal plane errors** — VBAP disrupts spectral cues for elevation at large spans. Keep polar span < 40 degrees. (Baumgartner 2015)
4. **Triangulation holes** — incomplete speaker arrays leave un-renderable directions. (Pulkki)
5. **Small room comb filtering** — first reflections < 20ms cause timbral coloration. (Yeoward 2021)
6. **Energy drop at panning domain borders** — 6 dB loss in hybrid Ambi-VBAP. (Zotter 2010)
7. **SDN T60 overshoot** — measured T60 is 45-60% higher than Sabine prediction. (Yeoward 2021)
8. **Source width variation** — perceived width depends on speaker span. (Pulkki, Zotter)

### DBAP

1. **Source at speaker with r_s=0** — division by zero. Always use spatial blur. (Lossius 2009)
2. **Source far outside field without p-scaling** — collapses to center. Use Sundstrom's method. (Sundstrom 2021)
3. **Convex hull vertex projection** — nonunique solutions, perceived motion stops. Use p-variable instead. (Sundstrom 2021)
4. **Too much spatial blur** — precedence effect, direction collapses to nearest speaker. (Lossius 2009)
5. **Regular polygonal layouts** — DBAP is worse than VBAP/Ambisonics. Use DBAP for arbitrary positions only. (Sundstrom 2021)
6. **Speaker directivity ignored** — all papers assume omnidirectional. Real speakers have radiation patterns. (All)

### Ambisonics

1. **Never compress channels independently** — destroys spatial image. Use omnidirectional as sidechain. (Zotter & Frank)
2. **Irregular layouts without AllRAD** — simple decoders produce terrible results. (Kostadinov 2010)
3. **Off-centre degradation** — ITD errors grow rapidly. Need 4th+ order to match VBAP on 5.1. (BBC 2013)
4. **Gibbs ringing** — sharp spatial cuts at low orders produce ringing. Use smooth windows. (Kronlachner 2014)
5. **HRTF order mismatch** — binaural decoding at low order misses high-frequency spatial cues. Use bilateral. (Ben-Hur 2020)

---

## 26. Prioritized Implementation Ideas

### Tier 1 — High Impact, Low-to-Medium Effort

| # | Idea | Effort | Key Source |
|---|---|---|---|
| 1 | VbapReflectionsRenderer (7 VBAP inputs: direct + 6 walls) | Medium | Yeoward 2021: FullSpat p < 0.001 |
| 2 | Per-speaker delay compensation (~20 lines) | Low | Kostadinov 2010 |
| 3 | Pre-computed VBAP gain LUT at 1-degree resolution | Low-Medium | Shukla 2019 |
| 4 | Configurable DBAP rolloff R parameter (single f32) | Low | Lossius 2009, Sundstrom 2021 |

### Tier 2 — High Impact, Medium Effort

| # | Idea | Effort | Key Source |
|---|---|---|---|
| 5 | IRCAM 4-segment room model architecture | Medium | Carpentier 2017 |
| 6 | Bilateral Ambisonics for HRTF mode (~1 order quality gain for 2x compute) | Medium | Ben-Hur 2020 |
| 7 | AllRAD decoder for Ambisonics mode | Medium | Zotter & Frank 2019 |

### Tier 3 — Medium Impact, Medium-to-High Effort

| # | Idea | Effort | Key Source |
|---|---|---|---|
| 8 | Multi-delay SH reverb (source-count-independent) | Medium-High | Rudrich 2016 |
| 9 | Polarity inversion for extended VBAP (~40% beyond boundary) | Medium | Pulkki |
| 10 | QuickHull + ray-triangle VBAP | Medium | Choi 2012 |

### Tier 4 — Research / Future

| # | Idea | Note |
|---|---|---|
| 11 | WFS for distance perception | Requires dense arrays (64+ speakers) |
| 12 | BCI (Binaural Cue Imaging) | For multi-listener with shared speakers |
| 13 | DSHT transforms | For Ambisonics scene rotation |
| 14 | PBAP (Physics-Based Panning) | Overlaps with existing AirAbsorption/GroundEffect stages |

### Recommended Implementation Order

1. **Per-speaker delay compensation** — smallest effort, immediate quality win
2. **Configurable DBAP rolloff** — single parameter addition
3. **VbapReflectionsRenderer** — the big one, empirically validated
4. **Pre-computed VBAP gain LUT** — optimization that makes #3 practical at 7x evaluations per source
