# IAMF — Immersive Audio Model and Format

Open standard by the Alliance for Open Media (AOM) for delivering spatial/immersive audio.
Branded by Google/Samsung as **Eclipsa Audio**.

## Links

- [Introducing Eclipsa Audio](https://opensource.googleblog.com/2025/01/introducing-eclipsa-audio-immersive-audio-for-everyone.html) — Google open-source blog announcement
- [IAMF v1.0.0 Specification (errata)](https://aomediacodec.github.io/iamf/v1.0.0-errata.html) — full technical spec
- [libiamf v1.0.0-errata](https://github.com/AOMediaCodec/libiamf/releases/tag/v1.0.0-errata) — reference C codec library
- [iamf-tools](https://github.com/AOMediaCodec/iamf-tools) — C++ encoder, decoder, and binaural web renderer
- [YouTube upload encoding settings](https://support.google.com/youtube/answer/1722171?hl=en) — YouTube now accepts IAMF/Eclipsa Audio

---

## What It Is

IAMF is a **delivery/distribution format** for immersive audio — not a real-time rendering engine. It defines how to package spatial audio content so that any decoder can render it to the listener's specific playback setup (stereo, 5.1, 7.1.4, headphones, soundbar, etc.).

Think of it as: the spatial audio equivalent of what H.264/AV1 is for video. A standardized container that separates content authoring from playback rendering.

## Core Architecture

An IAMF stream consists of two categories of data:

### Descriptors (metadata, written once)
- **Sequence header** — profile/version info
- **Codec config** — decoder setup (supports OPUS, AAC-LC, FLAC, LPCM)
- **Audio Element definitions** — how substreams are grouped
- **Mix Presentation definitions** — rendering recipes for different playback targets

### IA Data (time-variant, per frame)
- **Encoded audio frames** — the actual audio
- **Parameter blocks** — time-varying mix/gain/panning adjustments
- **Temporal delimiters** — sync points

## Audio Elements

The fundamental building blocks. Each element is built from multiple coded audio substreams. Two types:

### Channel-Based
Traditional surround layouts where discrete channels map to speaker positions:
- Mono, Stereo, 5.1, 5.1.2, 5.1.4, 7.1, 7.1.2, 7.1.4
- Scalable: a 7.1.4 element can contain a 5.1 base layer + enhancement layers, allowing decoders to pick what they can handle

### Scene-Based (Ambisonics)
Speaker-independent soundfield representation using spherical harmonics:
- 0th order (1 channel, mono), 1st order (4ch FOA), 2nd order (9ch), 3rd order (16ch)
- Codec-agnostic — ambisonics channels are just audio streams with spatial metadata

## Mix Presentations

The key innovation for multi-device playback. A Mix Presentation describes:
- Which audio elements to include
- Per-element gain and panning adjustments
- Target loudness (LKFS)
- Rendering layout (stereo, 5.1, binaural, etc.)

**Multiple mix presentations can coexist in one stream.** This enables:
- Stereo fallback for basic devices
- 5.1 for surround systems
- 7.1.4 for Atmos-class setups
- Binaural for headphones
- Multi-language (different dialog elements per presentation)

The decoder selects the best-matching presentation for the playback hardware.

## Parameter Animation

IAMF supports time-varying parameters via parameter blocks:
- **Mix gain**: per-element volume automation over time
- **Demixing**: reconstruction parameters for scalable channel layouts
- **Recon gain**: level corrections when downmixing scalable elements

Parameters use linear or Bezier interpolation between keyframes — similar to DAW automation.

---

## Relevance to Atrium

### Direct Intersections

**1. Multi-presentation = multi-listener rendering**
IAMF's core design problem — one audio scene, many playback targets — mirrors Atrium's architecture exactly. Our `AudioScene` renders to a listener; IAMF formalizes the concept that the same scene should produce 5.1 speaker output AND binaural headphone output simultaneously. The mix presentation model validates our approach of separating scene description from rendering.

**2. ADM compatibility**
iamf-tools accepts ADM-BWF (Audio Definition Model — Broadcast Wave Format) as input. Our planned WebSocket protocol already describes sources as ADM-style Objects (position, gain, diffuse, size). This means Atrium scenes could be directly encoded to IAMF for distribution — a live spatial scene recorded and played back on YouTube, Samsung TVs, or any IAMF decoder.

**3. Scene-based audio (Ambisonics) as interchange**
IAMF treats ambisonics as a first-class element type alongside channel-based audio. This aligns with our planned HOA intermediate format (encode sources to ambisonics, then decode to 5.1/binaural per listener). An IAMF export path would make this concrete.

**4. Scalable channel layouts**
IAMF's layered approach (5.1 base + height enhancement) is directly useful for our 5.1 speaker array. We could author content targeting 7.1.4 and let IAMF's downmix handle graceful degradation to our 5.1 setup, or use the full layout if we add height speakers later.

### Practical Use Cases for Atrium

| Use case | How |
|----------|-----|
| **Export/record scenes** | Render Atrium object positions + audio → IAMF file. Playable on YouTube, Samsung TVs, Chrome, any IAMF decoder |
| **Reference binaural renderer** | iamf-tools includes a binaural renderer — study its algorithms for our headphone rendering path |
| **Distribution format** | Live Atrium sessions could be streamed/recorded as IAMF |
| **DAW interop** | Avid Pro Tools IAMF plugin (2025) — creators author in Pro Tools → IAMF → ingest into Atrium |
| **Ingesting immersive content** | Decode IAMF files as source material — play back existing immersive content through our spatial engine |

### What IAMF Is NOT (for us)

IAMF is a delivery format, not a real-time rendering engine. It doesn't replace our cpal-based rendering pipeline. It sits alongside it as an **export/interchange layer**:
- Atrium renders the spatial scene in real-time (cpal + our processor chain)
- IAMF is how we'd package that scene for playback elsewhere
- Or how we'd ingest immersive content authored in other tools

### Integration Approaches

**Option A: FFI bindings to libiamf (C)**
- Decode-side: ingest IAMF files as audio sources in Atrium
- Mature reference implementation, v1.0.0

**Option B: FFI bindings to iamf-tools (C++)**
- Full encode + decode + binaural rendering
- More complex FFI surface (C++)

**Option C: Pure Rust implementation**
- No Rust IAMF crate exists yet — potential open-source contribution
- Could start with decode-only, expand to encode
- Aligns with our pure-Rust preference (cpal, symphonia, rtrb)

---

## Industry Adoption (as of 2025)

- **YouTube**: accepts IAMF uploads for immersive playback
- **Samsung**: 2025 TV lineup ships with Eclipsa Audio decoding
- **Chrome**: planned browser support
- **Android AOSP**: planned platform support
- **Avid Pro Tools**: IAMF plugin (spring 2025)
- **Codec support**: OPUS, AAC-LC, FLAC, LPCM — all royalty-free or widely licensed

## Comparison with Other Immersive Formats

| Format | Open? | Object audio? | Ambisonics? | Multi-presentation? | Atrium relevance |
|--------|-------|---------------|-------------|---------------------|-----------------|
| **IAMF/Eclipsa** | Yes (BSD) | Via ADM input | Yes (FOA–3OA) | Yes | Export/interchange format |
| **Dolby Atmos** | No (licensed) | Yes | No | Limited | Industry standard but proprietary |
| **MPEG-H** | No (licensed) | Yes | Yes | Yes | Used in ATSC 3.0 broadcast |
| **ADM-BWF** | Yes (EBU) | Yes | Yes | No | Our scene description model (already planned) |
| **Ambisonics (raw)** | Yes | No | Yes | No | Potential intermediate format |

IAMF is the only open, royalty-free format that supports both channel-based and scene-based audio with multiple mix presentations. For an open-source project like Atrium, this is the natural choice for a distribution format.