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
