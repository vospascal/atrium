# Atrium — Audio Library & Architecture References

Libraries, projects, and resources we can draw from as we build each phase.
When implementing a feature, check this list first — someone may have solved it well already.

---

## Audio I/O

| Crate | What it does | Useful for us |
|-------|-------------|---------------|
| [cpal](https://github.com/RustAudio/cpal) | Cross-platform audio I/O (callback-based). Pure Rust. | **Currently used.** Our audio output backend. |
| [cubeb-rs](https://github.com/mozilla/cubeb-rs) | Mozilla's audio I/O (Firefox's backend). Pure-Rust CoreAudio + PulseAudio. | Alternative backend if we need lower latency or Android support. Drop-in via our `AudioOutput` trait. |
| [cubeb](https://github.com/mozilla/cubeb) | The original C library that cubeb-rs wraps. | Reference for how Firefox handles audio routing, device selection, and latency tuning. |

## Audio Decoding

| Crate | What it does | Useful for us |
|-------|-------------|---------------|
| [symphonia](https://github.com/pdeljanov/Symphonia) | Pure Rust decoder: MP3, WAV, FLAC, AAC, Vorbis, ALAC, MKV. | **Currently used.** Decodes audio assets at startup. |
| [hound](https://github.com/ruuda/hound) | WAV-only read/write. Simpler than symphonia. | If we ever need to write WAV files (e.g. recording output). |

## DSP Building Blocks

| Crate | What it does | Useful for us |
|-------|-------------|---------------|
| [dasp](https://github.com/RustAudio/dasp) | Sample/Frame/Signal traits, sample rate conversion, no-alloc. | Type-safe sample format conversion if we need to support multiple output formats. |
| [fundsp](https://github.com/SamiPerttu/fundsp) | Composable DSP graphs with algebraic notation. Compile-time type checking. | Building effects chains (reverb, EQ, filters). The `>>` pipe and `^` branch operators are elegant for processor chains. |
| [rustfft](https://github.com/ejmahler/RustFFT) | Pure Rust FFT. | HRTF convolution, spectral analysis, frequency-domain processing. |

## Spatial Audio & HRTF

| Crate / Project | What it does | Useful for us |
|-----------------|-------------|---------------|
| [web-audio-api-rs](https://github.com/orottier/web-audio-api-rs) | Full Web Audio API in Rust (v1.2.0). PannerNode HRTF, BiquadFilter, ConvolverNode, etc. Uses IRCAM HRTF data. | **Key reference.** Study their HRTF implementation, panner math, and biquad filter code. Same API we know from the browser. |
| [fyrox-sound](https://github.com/FyroxEngine/Fyrox) | Standalone game sound engine with HRTF, reverb, streaming. | Reference for HRTF rendering pipeline and overlap-save convolution. Note: HRTF is 5-6x slower than simple panning. |
| [hrtf](https://github.com/mrDIMAS/hrtf) | Dedicated HRTF processor using HRIR spheres + frequency-domain convolution. | Direct dependency candidate when we add binaural headphone mode. |
| [audionimbus](https://github.com/MaxenceMaire/audionimbus) | Safe Rust wrapper for Valve's Steam Audio. Physics-based propagation, occlusion, HRTF, ambisonics. | Reference for professional-grade spatial audio. Could use as dependency for room simulation if we don't want to build our own ray tracer. |
| [Mesh2HRTF](https://github.com/Any2HRTF/Mesh2HRTF) | Generates personalized HRTFs from 3D ear/head scans via BEM (boundary element method). Outputs SOFA files. | Personalized per-listener HRTFs from ear scans. Produces anechoic HRTFs — pair with our ray-traced room sim for best results. |
| [SOFA (AES69)](https://www.sofaconventions.org/) | Spatially Oriented Format for Acoustics. HDF5-based standard for storing HRTFs/BRIRs with spatial metadata. | **Standard HRTF file format.** We should load SOFA files for per-listener HRTFs. Used by Mesh2HRTF, SPARTA, and most HRTF databases (CIPIC, IRCAM LISTEN, TH Köln, York SADIE). |
| [SPARTA](https://github.com/leomccormack/SPARTA) | VST suite: ambisonic encoding/decoding, binaural rendering with head tracking, room simulation. Consumes SOFA files. | Reference for ambisonic-to-binaural pipeline and CroPaC decoder. Their AmbiRoomSim models distance with disabled reflections — relevant for our direct-sound path. |
| [sofamyroom](https://github.com/andresperezlopez/sofamyroom) | Room acoustic simulator that applies shoebox-model reflections to SOFA HRTFs. Outputs BRIRs. | Directly relevant architecture: separates anechoic HRTF from room simulation, then convolves. Similar to our intended approach of ray-traced room + per-listener HRTF. |

## Ray-Traced / Geometry-Based Audio

| Project | What it does | Useful for us |
|---------|-------------|---------------|
| [raytraced-audio](https://github.com/whoStoleMyCoffee/raytraced-audio) (Godot) | Persistent incremental rays that sense room geometry implicitly. No authored zones needed. | **Key architecture reference.** Their approach: rays persist across frames, one bounce per tick, smooth interpolation. Emergent room sensing instead of explicit zone authoring. Study `audio_ray.gd` and `raytraced_audio_listener.gd`. |
| [Steam Audio](https://valvesoftware.github.io/steam-audio/) | Ray-traced reflections, diffraction, transmission through materials. | The gold standard for game audio propagation. Available via audionimbus crate. |

## Game Audio Architecture

| Crate | What it does | Useful for us |
|-------|-------------|---------------|
| [kira](https://github.com/tesselode/kira) | Game audio manager with mixer, tweens, clocks. Lock-free command architecture. | **Study their lock-free patterns:** per-command-type ring buffers, arena pattern (arena on audio thread, controllers cloned to other threads), tween system for smooth parameter transitions. |
| [rodio](https://github.com/RustAudio/rodio) | High-level playback library on cpal. Source trait, mixer, spatial panning. | Too high-level for us, but the `Source` trait design (iterator of samples with metadata) is worth studying. |

## Measurement & Calibration

| Project | What it does | Useful for us |
|---------|-------------|---------------|
| [Impulcifer](https://github.com/jaakkopasanen/Impulcifer) | Creates personalized BRIRs (Binaural Room Impulse Responses) by recording sine sweeps through binaural in-ear mics. Python tool. | Calibrating the physical NUC room — correcting 5.1 speakers for real room acoustics. Also a reference for BRIR processing, channel balancing (`--channel_balance=trend`), headphone compensation, and impulse response validation. |
| [AutoEQ](https://github.com/jaakkopasanen/AutoEq) | Database of 3000+ headphone frequency response measurements with auto-generated EQ profiles. MIT licensed. By the same author as Impulcifer. | **Headphone compensation for binaural output.** Each listener's headphones need EQ correction to flatten their coloring before HRTF convolution. AutoEQ provides ready-made FIR/parametric EQ profiles. |
| [HeSuVi](https://sourceforge.net/projects/hesuvi/) | Windows headphone surround virtualizer. 14-channel convolution (7×L ear + 7×R ear) via Equalizer APO. | Reference implementation for multi-channel BRIR convolution routing. Their channel layout (FL-L, FL-R, FR-L, FR-R, ...) is the standard for true-stereo surround virtualization. |
| [Earful](https://sourceforge.net/projects/earful/) | Matched-frequency loudness comparison tool for A/B testing headphone virtualization vs real speakers. | Validation tool for our binaural output — compare headphone rendering against physical 5.1 speakers at matched loudness per frequency band. |

## Real-Time Safety

| Crate | What it does | Useful for us |
|-------|-------------|---------------|
| [rtrb](https://github.com/mgeier/rtrb) | Wait-free SPSC ring buffer. Bounded-time operations, no allocations. | **Currently used.** Main→audio thread command passing. |
| [basedrop](https://github.com/glowcoil/basedrop) | `Owned<T>` and `Shared<T>` smart pointers that defer deallocation off the audio thread. | When we need dynamic source add/remove at runtime. Prevents `drop` from running on the audio thread. |
| [assert_no_alloc](https://github.com/Windfisch/rust-assert-no-alloc) | Custom allocator that panics on allocation in marked sections. | Debug tool: wrap our audio callback to catch accidental allocations. |

---

## Architecture Patterns to Steal

### From raytraced-audio (Godot)
- **Persistent incremental rays**: rays maintain state across frames, advance one bounce per tick. Amortizes cost over many frames. The environment model converges through `lerpf()` smoothing.
- **Emergent room sensing**: no explicit zones — room size, indoor/outdoor, and opening direction all emerge from ray behavior.
- **Escape-weighted ambient**: rays that escape on first bounce contribute more to ambient strength than rays bouncing several times (`1.0 / bounces`).
- **Logarithmic frequency interpolation**: lowpass cutoff transitions in log2 space for perceptually linear occlusion.

### From kira
- **Per-command-type ring buffers**: separate channels for different command types.
- **Arena pattern**: audio thread owns an arena of sources; control thread holds lightweight handles.
- **Tweens**: smooth parameter transitions with configurable curves.

### From web-audio-api-rs
- **HRTF via IRCAM LISTEN database**: proven HRIR data set for binaural rendering.
- **AudioWorklet pattern**: custom DSP processors that run in the audio callback.
- **Node graph architecture**: how to compose processing chains flexibly.

### From cubeb
- **Backend selection**: prioritized list of backends per platform.
- **Stream abstraction**: `cubeb_stream` + `cubeb_ops` pattern for portable audio I/O.

### From Head-Fi BRIR community (Impulcifer / HeSuVi / Smyth Realiser)
- **Three-stage rendering**: separate Direct sound (0–3ms, HRTF-convolved) → Early reflections (3–50ms, ray-traced + HRTF) → Late reverb (50ms+, FDN, can be shared/mono across listeners). Different update rates and computational costs per stage.
- **Bass crossover at ~80Hz**: frequencies below ~80Hz carry no HRTF localization cues. Skip HRTF convolution below crossover — mix mono to both ears (headphones) or route to subwoofer (5.1). Saves significant convolution CPU per listener. Smyth Realiser A16 does this.
- **Channel balance normalization**: L/R broadband energy matching is the single biggest quality factor for binaural rendering. Use trend-based correction (broadband, preserving narrow peaks/notches that carry HRTF info).
- **Headphone compensation is mandatory**: raw HRTF convolution without headphone EQ correction sounds wrong. Stack AutoEQ profiles or measure per-listener. Apply as FIR filter after HRTF convolution.
- **Speaker delay compensation**: for physical 5.1, all speakers must be delay-aligned to within ±20μs at listener position. Haas/precedence effect causes soundstage collapse if delays differ across channels.
- **True-stereo convolution routing**: each source channel needs 2 convolutions (L-ear, R-ear), so 7.1 surround = 14 FIR filters. HeSuVi's channel layout: odd tracks = left ear, even tracks = right ear.
- **Frequency-dependent reverb decay**: highs decay faster than lows (air absorption + surface absorption). Our ray tracer should carry per-band absorption coefficients; FDN reverb should have per-band decay times.

---

## IRCAM Panoramix / Spat Architecture

Sources:
- [Carpentier 2017, LAC paper](research%20papers/panoramix_lac2017.pdf)
- [Panoramix project page](https://forum.ircam.fr/projects/detail/panoramix/)
- [IRCAM EAC team (Acoustic & Cognitive Spaces)](https://www.ircam.fr/recherche/equipes-recherche/eac/)
- [BiLi — Binaural Listening](https://www.ircam.fr/projects/pages/ecoute-binaurale)
- [WFS + Ambisonics at Espace de Projection](https://www.ircam.fr/projects/pages/systeme-wfs-et-ambisonique-a-lespace-de-projection)

### Spat 4-Segment Room Model (Jot & Warusfel, 1995)
The foundational architecture that Panoramix, SPAT Revolution, and Spat~ all build on:
1. **Direct sound** (0–3ms) — point-source panned, per-source filtering (air absorption, distance)
2. **Early reflections** (3–50ms) — 8 or 16 discrete echoes per source, each individually positioned & panned as point sources
3. **Late reflections** (50–200ms) — spatially diffuse, transition region
4. **Reverb tail** (200ms+) — shared FDN reverb across sources for efficiency, diffuse panning

Key: each segment is **individually filtered AND individually spatialized**. Early reflections are not just delays — they have their own 3D positions.

### Panoramix Signal Flow
- **Tracks** (per-source): Input → Trim → EQ → Compressor → Phase inversion → Air absorption → Doppler → Delay → split to: Direct Gain + Early Reflections (8-16 echoes) + Reverb Send
- **Busses** (shared rendering): Panning engine (VBAP/HOA/binaural) + Late Reverb FDN → Master
- **Parallel bussing**: each track feeds up to 3 busses (A/B/C) simultaneously — e.g., VBAP bus for speakers + binaural bus for headphones, rendered in parallel with shared source positions
- **Hybridization**: blend "true" binaural with conventional stereo to mitigate HRTF artifacts (timbral coloration, front-back confusion, in-head localization) when using non-individual HRTFs

### Binaural Pipeline (IRCAM BiLi Project)
- SOFA/AES-69 format for HRTFs — supports SimpleFreeFieldHRIR (convolution) and SimpleFreeFieldSOS (IIR sections + ITD)
- 1680 spatial directions for high-resolution HRTF measurement
- OpenDAP server for remote HRTF database browsing/downloading
- HRTF personalization: selection-based (RASPUTIN, pick best match) or deep-learning (HAIKUS, estimate from recordings)

### IRCAM WFS + HOA System (Espace de Projection)
- 264 horizontal speakers (WFS ring) + 75 dome speakers (HOA) — format-agnostic scene description decoded to multiple output arrays
- Separation between encoding format and reproduction system

### All parameters controllable via OSC — sessions stored as stringified OSC bundles (human-readable)

## EBU ADM Audio Types

Source: [ADM Audio Definition Model](https://adm.ebu.io/index.html) — [Audio Types](https://adm.ebu.io/background/audio_types.html)

ITU-R BS.2076 standard data model for describing spatial audio content. Directly applicable to our web GUI and scene description:

| ADM Type | Description | Atrium mapping |
|----------|-------------|----------------|
| **Objects** | Individual sounds with position/gain/size metadata, can move dynamically | Primary model — each source (campfire, water, birds) is an Object |
| **DirectSpeakers** | Channels mapped to physical speakers (mono, stereo, 5.1, 7.1, 22.2) | Our 5.1 output bus — rendered signals to specific speakers |
| **Scene-based (HOA)** | Speaker-independent soundfield (Ambisonics). 1st order=4ch, 2nd=9ch, 3rd=16ch | Potential intermediate format for encoding before decode to 5.1/binaural |
| **Binaural** | Headphone-optimized 2-channel, hard to convert to speakers | Per-listener headphone output |
| **Matrix** | Transform matrix between formats (e.g., Mid-Side, Lt/Rt) | Format conversion utilities |

Our WebSocket control protocol should describe sources as ADM-style Objects (position, gain, diffuse, size, importance) rather than low-level DSP parameters. The GUI renders these as draggable objects; the engine renders them to multiple output formats in parallel.

---

## Relevance by Future Phase

| Phase | Key References |
|-------|---------------|
| Early reflections (image-source) | web-audio-api-rs (ConvolverNode), audionimbus, sofamyroom (shoebox model), **Panoramix** (8-16 discrete echoes per source, individually spatialized) |
| Ray-traced reflections | raytraced-audio (architecture), audionimbus (Steam Audio) |
| FDN reverb | fundsp (composable DSP), web-audio-api-rs, Head-Fi patterns (freq-dependent decay), **Panoramix** (Jot-Chaigne FDN, 8 feedback channels, 3-band decay) |
| HRTF binaural | hrtf crate, fyrox-sound, SOFA format, Mesh2HRTF (personalized), SPARTA (ambisonic decoder), **IRCAM BiLi** (1680-dir HRTFs, SOFA+OpenDAP, RASPUTIN selection, HAIKUS deep-learning personalization) |
| Headphone compensation | AutoEQ (3000+ profiles), Impulcifer (measurement-based), HeSuVi (routing reference) |
| Bass crossover | Head-Fi patterns (skip HRTF <80Hz), Smyth Realiser A16 approach |
| 5.1 output | cubeb-rs (device routing), cpal multichannel, Impulcifer (room calibration), speaker delay compensation |
| Multiple listeners | basedrop (shared buffers), kira (arena pattern), SOFA per-listener HRTFs, **Panoramix parallel bussing** (simultaneous multi-format render from shared sources) |
| Validation & tuning | Earful (A/B loudness matching), Impulcifer (BRIR validation) |
| WebSocket control | Already using rtrb; add tokio + axum/warp, **ADM Object model** for scene description, OSC-style parameter addressing (Panoramix pattern) |
| Web GUI | **EBU ADM types** (Objects/DirectSpeakers/HOA/Binaural), draggable object positioning, SOFA browser (Panoramix pattern) |
| Acoustic zones | raytraced-audio (emergent sensing), audionimbus |
| Props (drips, leaves) | kira (tween system for smooth gain ramps) |
| Parallel rendering | **Panoramix parallel bussing** — same source tracks rendered to 5.1 + binaural simultaneously, format-agnostic scene description |
