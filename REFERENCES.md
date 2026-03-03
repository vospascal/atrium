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

---

## Relevance by Future Phase

| Phase | Key References |
|-------|---------------|
| Early reflections (image-source) | web-audio-api-rs (ConvolverNode), audionimbus |
| Ray-traced reflections | raytraced-audio (architecture), audionimbus (Steam Audio) |
| FDN reverb | fundsp (composable DSP), web-audio-api-rs |
| HRTF binaural | hrtf crate, fyrox-sound, web-audio-api-rs (IRCAM data) |
| 5.1 output | cubeb-rs (device routing), cpal multichannel |
| Multiple listeners | basedrop (shared buffers), kira (arena pattern) |
| WebSocket control | Already using rtrb; add tokio + axum/warp |
| Acoustic zones | raytraced-audio (emergent sensing), audionimbus |
| Props (drips, leaves) | kira (tween system for smooth gain ramps) |
