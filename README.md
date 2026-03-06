# Atrium

Spatial audio engine in Rust — virtual acoustic environments with world-locked 3D sound.

Built for a NUC driving 5.1 speakers + per-listener binaural headphones, but works on any system with stereo output.

## What it does

Two sound sources (djembe + campfire) orbit independently around a listener in a 6x4m virtual room. You hear them pan smoothly in stereo using equal-power panning with distance-based attenuation (inverse distance model). The architecture is designed from day one for:

- Ray-traced reflections from room geometry
- Acoustic zone transitions (atrium → rainy outdoors)
- FDN reverb tied to room size/materials
- HRTF binaural rendering for headphones
- 5.1 surround speaker output
- Multiple simultaneous listeners
- WebSocket control for phone-based position tracking
- Localized sound props (drips, leaves, puddles) with audible radius

## Prerequisites

- **Rust** 1.85+ (tested on 1.93.1)
- Audio output device (headphones recommended for spatial effect)
- macOS, Linux, or Windows

## Build & Run

```bash
# Build
cargo build

# Run — you should hear djembe + campfire orbiting in stereo
cargo run

# Run optimized (lower CPU, tighter audio timing)
cargo run --release

# Run tests
cargo test
```

## Project Structure

```
src/
├── main.rs                  # Entry point: decode audio, setup cpal, run
│
├── audio/
│   ├── decode.rs            # MP3/WAV/FLAC → mono f32 buffer (symphonia)
│   ├── mixer.rs             # Mix N sources → stereo output with spatial panning + distance attenuation
│   └── output.rs            # Audio backend trait + cpal implementation
│
├── engine/
│   ├── commands.rs          # Lock-free command queue (rtrb ring buffer)
│   └── scene.rs             # AudioScene: owns all state on the audio thread
│
├── spatial/
│   ├── listener.rs          # Listener position + orientation
│   ├── source.rs            # SoundSource trait + TestNode (orbiting buffer player)
│   └── panner.rs            # Equal-power stereo panning + distance attenuation
│
├── world/
│   ├── types.rs             # Vec3 math
│   └── room.rs              # Room trait + BoxRoom (6x4m)
│
└── processors/
    └── mod.rs               # AudioProcessor trait (extension point for reverb, reflections, etc.)

assets/
├── djembe.mp3               # Percussive test source (close orbit)
└── campfire.mp3             # Ambient test source (wide orbit)
```

## Architecture

```
Control Thread                    Audio Thread (cpal callback)
     │                                  │
     │── Command (rtrb) ──────────────>│  SetListenerPose, SetMasterGain, ...
     │                                  │
     │                            AudioScene
     │                           ┌──────┴──────┐
     │                      Listener      Sources[]
     │                           │              │
     │                           └──── Mixer ───┘
     │                                  │
     │                           [Processors]  ← future: reverb, reflections
     │                                  │
     │                            Stereo Output
     │                                  │
     │                              cpal → OS
```

**Key design constraint:** The audio callback runs on a real-time OS thread. No allocations, no locks, no I/O. Commands arrive via a wait-free ring buffer (`rtrb`). All state is owned by `AudioScene` on the audio thread.

## Profiling

Two optional features for measuring pipeline performance — both compile to nothing when disabled.

### Time profiling (`profiler`)

Instruments the render pipeline with `tracing` spans. Choose an output backend at runtime:

```bash
# Terminal span timing
cargo run --features profiler -- scenes/default.yaml --profile fmt

# Perfetto trace (drag trace.pftrace into https://ui.perfetto.dev)
cargo run --features profiler -- scenes/default.yaml --profile perfetto

# Flame graph (then: inferno-flamegraph < tracing.folded > flamegraph.svg)
cargo run --features profiler -- scenes/default.yaml --profile flame
```

Without `--profile`, no subscriber is installed and spans are ~1ns no-ops.

### Allocation tracking (`memprof`)

Detects heap allocations on the audio thread — any allocation in the callback is a real-time safety violation.

```bash
cargo run --features memprof -- scenes/default.yaml
```

### Both together

```bash
cargo run --features profiler,memprof -- scenes/default.yaml --profile perfetto
```

### Pipeline benchmark

Built-in wall-clock measurement of every pipeline stage (mixer, processors, full chain) across all render modes. Reports min/avg/max and percentage of the audio callback deadline.

```bash
# Debug mode (catches regressions, ~10× slower than real-time)
cargo test -p atrium pipeline_benchmark -- --ignored --nocapture

# Release mode (production-representative numbers)
cargo test -p atrium --release pipeline_benchmark -- --ignored --nocapture
```

Example output (release, M-series Mac, 2 sources, 512 frames @ 48kHz):

```
Pipeline Benchmark (512 frames @ 48000Hz, deadline = 10.67ms)
  20 warmup + 200 measured iterations
──────────────────────────────────────────────────────────────────────────
  Stage                              Min        Avg        Max   % Dead
──────────────────────────────────────────────────────────────────────────
  mix_sources (stereo)             15.1us      15.5us      19.0us     0.1%
  mix_sources (mono)               14.8us      15.2us      18.5us     0.1%
  mix_sources (quad)               20.3us      21.6us      28.1us     0.2%
  mix_sources (5.1 VBAP)           25.4us      28.1us      57.7us     0.3%
  binaural_mix (HRTF)              18.2us      19.7us      23.8us     0.2%
──────────────────────────────────────────────────────────────────────────
  EarlyReflections                  2.7us       2.7us       2.9us     0.0%
  FdnReverb                         5.2us       5.5us       9.2us     0.1%
  FdnReverb (6ch)                   6.5us       6.6us      12.0us     0.1%
──────────────────────────────────────────────────────────────────────────
  full_pipeline (stereo)           16.5us      16.6us      23.7us     0.2%
  full_pipeline (mono)             16.2us      16.4us      22.1us     0.2%
  full_pipeline (quad)             22.1us      22.8us      30.5us     0.2%
  full_pipeline (5.1 VBAP)         27.2us      27.7us      33.7us     0.3%
  full_pipeline (binaural)         26.5us      28.2us      34.5us     0.3%
──────────────────────────────────────────────────────────────────────────
  deadline = 10.67ms (512 frames / 48000Hz)
```

The test is `#[ignore]` so it never runs during normal `cargo test`.

### Instrumented spans

The `profiler` feature instruments the following spans (visible in Perfetto/flame graphs):

| Level | Span | Location | What it measures |
|-------|------|----------|-----------------|
| Callback | `callback` | `audio/output.rs` | Full cpal callback |
| Scene | `render` | `engine/scene.rs` | Source tick + mix + processors + telemetry |
| Scene | `process_commands` | `engine/scene.rs` | Command queue drain |
| Scene | `source_tick` | `engine/scene.rs` | All sources `.tick(dt)` |
| Mix | `mix` | `engine/scene.rs` | Mixer dispatch (multichannel or binaural) |
| Mix | `mix_source` | `audio/mixer.rs` | Per-source multichannel processing |
| Mix | `air_absorption` | `audio/mixer.rs` | ISO 9613-1 filter update |
| Mix | `ground_effect` | `audio/mixer.rs` | ISO 9613-2 ground gain |
| Mix | `reflection_update` | `audio/mixer.rs` | Image-source tap recomputation |
| Mix | `speaker_gains` | `audio/mixer.rs` | VBAP/MDAP gain computation |
| Mix | `source_render` | `audio/mixer.rs` | Per-frame gain ramp + sample generation |
| Mix | `lfe_crossover` | `audio/mixer.rs` | 120Hz Butterworth low-pass |
| Mix | `delay_compensation` | `audio/mixer.rs` | Speaker time-of-arrival alignment |
| Mix | `master_gain` | `audio/mixer.rs` | Final gain + clamp |
| Binaural | `binaural_source` | `audio/binaural.rs` | Per-source HRTF processing |
| Binaural | `distance_gain` | `audio/binaural.rs` | Inverse distance model |
| Binaural | `hrtf_update` | `audio/binaural.rs` | SOFA filter lookup + convolver set |
| Binaural | `hrtf_convolution` | `audio/binaural.rs` | Block FFT convolution (mono→L/R) |
| Processor | `processor` | `engine/scene.rs` | Per-processor (name + index) |
| Processor | `fdn_frames` | `processors/fdn_reverb.rs` | FDN frame loop (frames + lines) |
| Processor | `er_frames` | `processors/early_reflections.rs` | Early reflections frame loop (frames + taps) |
| Telemetry | `telemetry` | `engine/scene.rs` | Telemetry push (~15 Hz) |

### External profilers (Instruments, perf, etc.)

Use the `profiling` Cargo profile for release builds with debug symbols:

```bash
cargo run --profile profiling -- scenes/default.yaml
```

## Dependencies

| Crate | Purpose |
|-------|---------|
| [cpal](https://github.com/RustAudio/cpal) | Cross-platform audio device I/O |
| [symphonia](https://github.com/pdeljanov/Symphonia) | MP3/WAV/FLAC decoding |
| [rtrb](https://github.com/mgeier/rtrb) | Lock-free SPSC ring buffer |

The audio backend is abstracted behind the `AudioOutput` trait — swappable for [cubeb-rs](https://github.com/mozilla/cubeb-rs) or others without changing the engine.

## Extension Points

| Trait | File | What to implement |
|-------|------|-------------------|
| `SoundSource` | `spatial/source.rs` | New sound types (WAV loops, granular, procedural noise) |
| `AudioProcessor` | `processors/mod.rs` | Effects chain (reverb, reflections, zone blending, occlusion) |
| `Room` | `world/room.rs` | Geometry (add `cast_ray()` for ray-traced audio) |
| `AudioOutput` | `audio/output.rs` | Alternative backends (cubeb-rs, custom HDMI routing) |

See [REFERENCES.md](REFERENCES.md) for a curated list of libraries and architecture patterns to draw from.
