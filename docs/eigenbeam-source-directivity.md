# Eigenbeam / Spherical Harmonic Source Directivity

Research into using spherical harmonic (eigenbeam) decomposition for **source radiation patterns** in our ray-traced audio engine.

---

## Core Insight: Flipping Eigenbeam Inside Out

Eigenbeam research is almost always framed from the **capture** perspective — a spherical microphone array (like the Eigenmike em64) sits in a sound field, picks up pressure from all directions, and decomposes it into spherical harmonic components. The microphone is the focus, the sources are "out there."

But flip the perspective: **the sound source itself is a radiator sitting at the center of its own spherical sound field.** Every source — a voice, a violin, a campfire — emits sound outward in all directions with some directional pattern. That radiation pattern is a function on the sphere, which means it can be decomposed into the exact same spherical harmonic basis functions.

The math is identical, the direction of energy flow is reversed:

```
Eigenmike (capture):   sound field → sphere → SH decomposition → eigenbeam coefficients
Source directivity:    eigenbeam coefficients → SH reconstruction → sphere → radiated sound field
```

This means:
- **0th-order SH** (single coefficient) = omnidirectional source. A subwoofer, a campfire — equal energy in all directions. This is what most audio engines assume for all sources.
- **1st-order SH** (4 coefficients) = dipole, cardioid, figure-8 patterns. A human voice projecting forward, a kick drum pushing air from both sides of the head.
- **2nd-order SH** (9 coefficients) = clover-leaf patterns, narrower lobes. A loudspeaker that's starting to beam at mid frequencies.
- **3rd-order+** (16+ coefficients) = complex multi-lobed patterns. A violin's top plate creating intricate radiation at high frequencies, different for every note.

The crucial physical property: **all sources converge to omnidirectional at low frequencies.** Below ~200Hz, wavelengths are so long (>1.7m) that they diffract around any source body. Directivity only appears when the source dimensions approach the wavelength — so a 10cm violin body starts beaming above ~3.4kHz, while a 30cm guitar body starts around 1.1kHz.

This frequency-dependence means we don't store a single pattern per source — we store **SH coefficients per frequency band**. A voice might be omni at 100Hz, gently cardioid at 1kHz, and tightly beamed at 8kHz.

### Why This Works: Acoustic Reciprocity

This isn't just a convenient analogy — there's a formal physics principle behind it. The **Helmholtz reciprocity principle** states that the acoustic transfer function between two points is unchanged when source and receiver are swapped. In the spherical harmonic domain, this has been [formally extended to directional sources and directional receivers](https://pubs.aip.org/asa/jel/article/2/12/124801/2845745/Acoustic-reciprocity-in-the-spherical-harmonic):

- An HRTF describes how a listener's head shapes **incoming** sound from direction (θ, φ) — a directional receiver.
- A source directivity pattern describes how a source shapes **outgoing** sound toward direction (θ, φ) — a directional emitter.
- Reciprocity says these are mathematically dual: if you placed a tiny speaker at the ear canal and measured the radiated field, you'd recover the HRTF.

Both are functions on the sphere, both decompose into SH coefficients, and the transfer function between a directional source and a directional receiver is a product of their SH representations with the propagation channel between them. This means our whole pipeline — source directivity, room propagation, listener HRTF — operates in the same mathematical framework.

Practical consequence: measuring a source's radiation pattern with a spherical microphone array (like the Eigenmike) gives you SH coefficients that slot directly into our renderer alongside the listener's HRTF SH coefficients. No format conversion, no conceptual mismatch.

### SH Order vs Angular Resolution

How much detail does each order actually give you?

| SH Order | Coefficients | Approximate angular resolution | Equivalent |
|----------|-------------|-------------------------------|------------|
| 0 | 1 | 360° (omnidirectional) | No spatial detail |
| 1 | 4 | ~90° | Basic front/back/left/right (1st-order Ambisonics) |
| 2 | 9 | ~45° | Moderate spatial detail |
| 3 | 16 | ~30° | Good directional definition |
| 4 | 25 | ~22° | Detailed lobes visible |
| 5 | 36 | ~18° | High-resolution patterns |
| 6 | 49 | ~15° | What the Eigenmike em64 captures |

The number of coefficients grows as (N+1)², so higher orders get expensive fast. For source directivity in a real-time ray tracer, **orders 1–3 cover most practical sources** — we don't need 6th-order detail for a campfire or a voice. The higher orders matter for precise measurement and analysis, less so for perceptual rendering.

---

## Key Concept: Integration with Ray Tracer

In our ray tracer, when a ray leaves a source at direction (θ, φ), its energy is weighted by the source's SH radiation pattern evaluated at that direction. This replaces the current assumption of omnidirectional emission.

```
Source emits ray at direction (θ, φ)
    → evaluate SH radiation pattern at (θ, φ) → energy weight per frequency band
    → ray bounces off walls (existing ray tracer)
    → arrives at listener
    → decode with listener's HRTF
```

This has cascading effects on realism:
- **Reflections change character**: a voice facing a wall sends full-spectrum energy at the wall but only low frequencies behind. The reflections a listener hears from behind the speaker are duller — which is exactly what happens in reality.
- **Room excitation becomes asymmetric**: a directional source doesn't excite a room uniformly. Listeners on-axis hear a brighter direct sound AND brighter early reflections from the wall behind them. Listeners off-axis hear a duller direct sound but possibly brighter reflections from walls the source IS facing.
- **Late reverb is less affected**: by the time energy has bounced 5+ times, the directional information is largely scattered. This is why our late reverb (FDN) can remain shared/omnidirectional even with directional sources — the directivity matters most for direct sound and early reflections.

## Practical Directivity Examples

| Source type | Pattern | SH order needed |
|-------------|---------|-----------------|
| Campfire / ambient | Omnidirectional | 0th (trivial) |
| Human voice | Roughly cardioid, more directional at HF | 1st–2nd |
| Loudspeaker | Omni at LF, increasingly beamed at HF | 2nd–4th |
| Violin / guitar | Complex, frequency-dependent lobes from resonating body | 3rd+ |

Key property: all real sources are approximately omnidirectional below ~200Hz and increasingly directional at higher frequencies.

## How It Fits Atrium Architecture

- Slots into `Room::cast_ray()` — ray carries directivity-weighted amplitude from emission
- Source directivity coefficients stored per-source as SH coefficient sets (one set per frequency band)
- Simple presets (omni, cardioid, figure-8, hypercardioid) cover most use cases
- Measured directivity data available from research databases for realism
- Source has an orientation (facing direction) — rotating the source rotates the SH pattern, which is just a matrix multiply in SH domain

## Available Measured Directivity Data

Real-world instrument directivity data already exists in SH-compatible formats:

### TU Berlin Instrument Directivity Database

The most comprehensive publicly available dataset. [Ackermann et al. (2023)](https://arxiv.org/html/2307.02110) measured **41 modern and historical instruments** plus a soprano vocalist:

- **Instrument families**: strings (violin, viola, cello, double bass, guitar, harp), woodwinds (oboe, clarinet, bassoon, flute, saxophone), brass (trumpet, horn, trombone, tuba), percussion (timpani), plus historical variants
- **Measurement setup**: 32-channel spherical microphone array (pentakis dodecahedron, 2.1m radius) in anechoic chamber at TU Berlin
- **Resolution**: individual notes recorded at 44.1kHz / 24-bit, directivities averaged in 1/3-octave bands
- **Output formats**: SOFA (FreeFieldDirectivityTF convention), OpenDAFF (5° angular resolution, 2522 spatial points), GLL
- **License**: CC BY-SA 4.0
- **Download**: [tubcloud.tu-berlin.de](https://tubcloud.tu-berlin.de/s/8joeeK3fFingLgp)

Key finding: "the directivity of a musical instrument depends not only on the frequency, but also on the note being played" — so a violin playing G3 radiates differently than playing E5, even at the same frequency band.

### IRCAM Source Directivity Research

IRCAM has measured instrument radiation patterns using a rotating 24-microphone semi-circular arc in their anechoic chamber. Their work focuses on [spherical correlation as a similarity measure](https://acta-acustica.edpsciences.org/articles/aacus/full_html/2023/01/aacus220100/aacus220100.html) between 3D radiation patterns — comparing how similar different instruments' directivity patterns are, with applications for corpus-based spatial synthesis.

Also relevant: [IRCAM's violin radiation analysis and reproduction](http://articles.ircam.fr/textes/Vos03b/index.pdf) — studying how to capture and faithfully reproduce a violin's radiation pattern.

### Voice Directivity

Human voice directivity has been measured at high resolution using [2522 sampling positions with 5° spacing](https://pmc.ncbi.nlm.nih.gov/articles/PMC8329840/). Voice is a key source type for Atrium (narration, guided tours). The data can be [combined with reference data regularization for sparse measurements](https://acta-acustica.edpsciences.org/articles/aacus/full_html/2024/01/aacus230110/aacus230110.html) to create smooth spherical directivities.

### What This Means for Atrium

We don't need to guess or model directivity patterns — **measured SH data for real instruments already exists in SOFA format**, which is the same format we're planning to use for HRTFs. Our SOFA loader serves double duty: load listener HRTFs for decoding, load source directivities for encoding.

---

## References

### Acoustic Reciprocity & SH Theory

- [Acoustic reciprocity in the SH domain for directional sources and receivers (JASA Express Letters 2022)](https://pubs.aip.org/asa/jel/article/2/12/124801/2845745/Acoustic-reciprocity-in-the-spherical-harmonic) — formal proof that source directivity and receiver directivity (HRTF) are dual representations in SH domain
- [Acoustic reciprocity: extension to spherical harmonics domain (JASA 2017)](https://pubs.aip.org/asa/jasa/article/142/4/EL337/852846/Acoustic-reciprocity-An-extension-to-spherical) — earlier formulation of SH-domain reciprocity
- [SH computation of source directivity from finite-distance measurements (IEEE 2020)](https://ieeexplore.ieee.org/document/9257177/) — practical method for computing SH directivity coefficients from measured data

### Source Directivity Databases

- [TU Berlin: A Database with Directivities of Musical Instruments (Ackermann et al. 2023)](https://arxiv.org/html/2307.02110) — 41 instruments + soprano, 32-channel spherical array, SOFA/OpenDAFF/GLL, CC BY-SA 4.0. [Download](https://tubcloud.tu-berlin.de/s/8joeeK3fFingLgp)
- [Generation and analysis of radiation pattern database for 41 instruments](https://www.researchgate.net/publication/314125261_Generation_and_analysis_of_an_acoustic_radiation_pattern_database_for_forty-one_musical_instruments) — 4th-order SH, 25 coefficients per 1/3-octave band
- [IRCAM: spherical correlation as similarity measure for 3D radiation patterns](https://acta-acustica.edpsciences.org/articles/aacus/full_html/2023/01/aacus220100/aacus220100.html) — comparing instrument radiation patterns in SH domain
- [IRCAM: analysis and reproduction of violin radiation](http://articles.ircam.fr/textes/Vos03b/index.pdf) — early work on capturing and reproducing instrument directivity
- [High-resolution spherical directivity of live speech](https://pmc.ncbi.nlm.nih.gov/articles/PMC8329840/) — 2522 spatial positions, 5° resolution
- [Combining sparse measurements with reference data for voice directivity](https://acta-acustica.edpsciences.org/articles/aacus/full_html/2024/01/aacus230110/aacus230110.html) — practical method for measuring voice directivity with fewer microphones
- [Directivities of symphony orchestra instruments](https://www.researchgate.net/publication/233709078_Directivities_of_Symphony_Orchestra_Instruments) — reference measurements for orchestral instruments
- [TU Berlin: Musical instrument recording methodology for directivity databases](https://www2.users.ak.tu-berlin.de/akgroup/ak_pub/2010/Pollow_2010_MusicalInstrumentRecordingforBuildingaDirectivityDatabase_DAGA36.pdf) — measurement methodology

### Eigenbeam / Spherical Array Theory

- [Eigenbeam-space transformation for steerable frequency-invariant differential beamforming](https://advanceseng.com/eigenbeam-space-transformation-steerable-frequency-invariant-differential-beamforming-linear-arrays/) — eigenbeam-space decomposition of sound fields, weighted least-squares optimization for stable beampatterns across frequency
- [Eigenbeam-ESPRIT for 3D sound source localization with multiple spherical microphone arrays](https://www.researchgate.net/publication/361385340_Multiarray_Eigenbeam-ESPRIT_for_3D_Sound_Source_Localization_with_Multiple_Spherical_Microphone_Arrays) — direction-of-arrival estimation from SH-decomposed sound fields
- [Room geometry inference based on spherical microphone array eigenbeam processing](https://www.researchgate.net/publication/257749527_Room_geometry_inference_based_on_spherical_microphone_array_eigenbeam_processing) — using eigenbeam data to detect reflections and infer room shape (relevant to validating our ray tracer)
- [Localization of distinct reflections using eigenbeam processing](https://www.researchgate.net/publication/224034876_Localization_of_distinct_reflections_in_rooms_using_spherical_microphone_array_eigenbeam_processing) — identifying individual reflection paths from SH analysis

### Eigenbeam Signal Processing

- [Eigenbeam signal processing (ACM/Signal Processing)](https://dl.acm.org/doi/10.1016/j.sigpro.2023.109171) — TODO: access and review this paper
- [Polyhedral audio system based on at least second-order eigenbeams (US Patent)](https://patents.google.com/patent/US20140270245A1/en) — patent for spatial audio systems using eigenbeam decomposition
- [High-frequency extension for spherical eigenbeamforming microphone arrays](https://www.researchgate.net/publication/42439452_Analysis_of_the_highfrequency_extension_for_spherical_eigenbeamforming_microphone_arrays) — extending SH analysis to higher frequencies (relevant: source directivity becomes more complex at HF)

### Hardware Reference (Eigenmike em64)

- [Eigenmike em64 product page](https://eigenmike.com/eigenmike-64) — 64-capsule spherical array, 6th-order HOA, 24-bit/48kHz, Dante/PoE+, ~$15k
- [Eigenmike em64 datasheet](https://eigenmike.com/sites/default/files/documentation-2024-09/em64_datasheet_update.pdf)
- [audioXpress em64 review](https://audioxpress.com/article/fresh-from-the-bench-mh-acoustics-eigenmike-em64-sixth-order-ambisonics-microphone-and-software-suite) — detailed specs and software (EigenStudio 3, EigenUnit VST)
- [6th-order Eigenmike for spatial sound field recording (ASA)](https://pubs.aip.org/asa/jasa/article/153/3_supplement/A143/2885758/A-new-sixth-order-EigenmikeR-spherical-microphone) — academic paper on the em64's capabilities

Not needed for rendering, but relevant if we ever want to capture room impulse responses or measure source directivity patterns for validating the ray tracer against real measurements.

### Existing Atrium References (already in REFERENCES.md)

- SPARTA — ambisonic encoding/decoding, same SH math
- IRCAM Panoramix — HOA rendering with source directivity support
- SOFA format — stores both HRTFs and source directivity in the same container (FreeFieldDirectivityTF convention)

---

## TODO

- [ ] Download TU Berlin instrument directivity dataset and inspect SOFA files
- [ ] Verify our planned SOFA loader can handle FreeFieldDirectivityTF convention (source directivity) alongside SimpleFreeFieldHRIR (listener HRTFs)
- [ ] Define a `SourceDirectivity` trait/struct for Atrium (SH coefficients per frequency band, orientation)
- [ ] Implement SH rotation (matrix multiply) for orienting source directivity patterns
- [ ] Prototype with basic presets (omni, cardioid) in the ray tracer before adding measured data
- [ ] A/B test: omnidirectional vs directional sources in a simple room — does it make an audible difference?