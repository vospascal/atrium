# Atrium

Atrium renders an environment. It is a spatial audio engine in Rust: a world of
positioned sounds (birds, a river, wind, rain, an instrument), and a renderer
that works out what arrives at each speaker or each ear.

Built for a NUC driving 5.1 speakers plus binaural headphones, and it runs on
any machine with a stereo output.

To run it, see **[run.md](run.md)**: speakers, headphones, head tracking,
analysis tools, profiling and ports.

## Quick start

```bash
cargo run --release                                   # scenes/default.yaml on your speakers
```

```bash
cargo run --release -- --headphones scenes/nature.yaml  # a meadow, binaural
```

```bash
cargo test --workspace
```

Needs Rust 1.85 or newer (tested on 1.93.1) and an audio output device. macOS
and Linux are the tested platforms.

## What a scene is made of

A scene file wires separate YAML files together, so each can be swapped alone:

| Directory | Describes |
|---|---|
| `scenes/` | Which sounds are where, the speaker layout, the render mode |
| `environments/` | The virtual acoustic space: size, wall materials, ground, reverb |
| `rooms/` | The physical room the speakers stand in |
| `sources/` | One sound: a recording or a synth, its level and directivity |
| `atmospheres/` | Air temperature, humidity and pressure |

Sounds are either decoded recordings (`assets/`) or procedural synths
(`src/synth/`: four winds, two rains, river, waves, and a small flying creature).

## How it renders

Every source goes through the same three steps. Which implementation runs in
each step depends on the render mode.

1. **Resolve paths.** How does the sound reach the receiver: directly, and off
   up to six faces of the environment (first-order image sources).
2. **Per-path effects, then the renderer.** Each path gets its own delay, air
   absorption (ISO 9613-1), the wavering of wind-stirred air (see
   [Moving air](#moving-air)), ground effect (ISO 9613-2) and wall colouring;
   the renderer then places it in the output.
3. **Mix stages** on the whole buffer: late reverb (FDN), bass management,
   speaker delay compensation, master gain.

| Mode | For | Paths | Renderer | Mix stages |
|---|---|---|---|---|
| `world_locked` | speakers | direct only | each speaker is a microphone in the world: its own delay, air absorption, reflection taps | bass management, static delay compensation, master gain |
| `vbap` | 3 or more speakers | direct + reflections | amplitude panning relative to the listener | reverb, bass management, listener-relative delay compensation, master gain |
| `hrtf` | headphones | direct + reflections | HRTF convolution to two ears | reverb, master gain |
| `dbap` | speakers | direct + reflections | distance-based panning from source to speaker positions | reverb, bass management, static delay compensation, master gain |
| `ambisonics` | speakers or stereo | direct + reflections | first-order B-format encode | B-format delays, decode, reverb, bass management, listener-relative delay compensation, master gain |

`world_locked` and `dbap` do not depend on where the listener stands: everyone
in the room gets the same speaker feed. `vbap` and `ambisonics` are rendered for
one listening position.

## Listening to one part

A rendered scene is a sum: direct sound, first reflections, late reverb and
ambient fields, each shaped by the air and the ground. To judge one of them by
ear, a probe (`atrium_core::probe::Probe`, `Command::SetProbe`) lets only some
of them reach the output, switches single effects off (air absorption, moving
air, ground effect, wall colour), or leaves one loudspeaker or one ear alone.

Nothing else about the scene changes, which a test holds every render mode
to: the four parts heard alone add up, sample for sample, to the whole.

Decisions that look wrong and are right:

- **The reverb is still fed while the direct sound is hidden.** A probe scales
  what a path sends to the output and leaves what it sends to the reverb. Were
  the path itself silenced, the reverb heard alone would be silence.
- **A hidden part fades over one callback.** A cut would click, and a click is
  the last thing wanted while listening closely.
- **The delay cannot be switched off.** Without it a reflection is no longer
  one; the other four path effects can.
- **It is the listener's, not the scene's.** Like a mute it is never saved and
  undo does not touch it.

For a comparison that can be repeated: a scene with a `seed:` plays the same
way every time, pause holds it, and the timeline returns to any moment.

## Moving air

Outdoors a far sound does not arrive steadily. Wind shear and the sun on the
ground fill the air with eddies, and the level of a distant sound comes and
goes with them, the treble most. `src/audio/turbulence.rs` computes how much
from published theory, and every path through open air gets it.

It is the line-of-sight theory of Ostashev and Wilson for a spherical wave in
von Kármán turbulence, with the surface layer from Monin–Obukhov similarity,
as written out by Kamrath et al. (Acta Acustica 8, 2024), who render it the
same way: a time-variant high-shelving filter. For a bird 40 m off:

| Air | 500 Hz | 1 kHz | 4 kHz |
|---|---|---|---|
| Light air, 2 m/s, night | 0.3 dB | 0.4 dB | 0.9 dB |
| Breeze, 5 m/s, night | 0.6 dB | 0.9 dB | 1.9 dB |
| Still, clear noon | 0.5 dB | 0.8 dB | 1.6 dB |
| Strong wind, 9 m/s, night | 1.1 dB | 1.6 dB | 3.0 dB |

These are standard deviations of the level. At 5 m they are a few tenths of a
dB in the treble and less below, so a near sound stays steady; indoors and in
still air at night the filter is not in the signal at all, bit for bit.

The scene's weather drives it: the wind condition is metres per second (1 is
13 m/s at 10 m), daylight is the heat of the sun (1 is 200 W/m² into the air),
and `weather.wind_from` is the compass bearing the wind comes from. A wind
across a path makes it waver faster than one along it.

Decisions that look wrong and are right:

- **Nothing is exaggerated.** The first plan was to push the effect until it
  was audible. Working the theory through showed it is audible as it is, at
  the distances a scene has, so the numbers are the theory's.
- **Arrival time is left alone.** The same eddies also move a path's travel
  time, by some 65 µs at 40 m in a breeze. On one path that is under a cent of
  pitch, slowly; nothing can hear it, so it is not rendered.
- **Thermals set the pace, not the depth.** The large eddies of a sunny day
  (hundreds of metres) hardly change a level over 40 m. They are left out of
  the strength, as the paper leaves them out, and only help decide how fast
  the air moves across a path on a still day.
- **A still sunny scene wavers.** Every scene without weather is a calm,
  bright day, so its far sounds shimmer a little. That is the heat, and real.

Known gaps:

- **Two parts stand in for a smooth curve.** The filter splits the sound at
  2.4 kHz. Measured on tones it is within 15 % of the theory for a sound 40 m
  off in a breeze, and up to a fifth short in a gale at 60 m.
- **Saturation is borrowed.** How strong fluctuations level off (never past
  5.6 dB) uses the paper's fit to aircraft flyovers; near the ground that is
  an extrapolation. It matters only in gales at the far edge of a scene.
- **The pace is a rule of thumb.** The level holds for as long as the air
  takes to carry a Fresnel zone (at 4 kHz) across the path. The depth is
  theory; this timing is the usual frozen-turbulence estimate, not measured.
- **The wind does not bend sound yet.** Upwind of a breeze a sound near the
  ground can fall into a shadow within tens of metres, and downwind it carries
  further. Now that the wind has a direction this can be built; it is not.
- **The wind does not veer.** Its bearing is one number per scene; a passing
  storm does not turn it.
- **Clouds do not dim the sun.** Daylight alone sets the heat; rain and storm
  do not lower it.
- **Each speaker's path wavers by itself in world-locked.** Paths to speakers
  a few metres apart share some of their eddies in reality.

## Project structure

```text
src/
├── main.rs                 # entry point: load a scene, open the device, run
├── audio/                  # decoding, output device, filters, atmosphere,
│                           #   ISO 9613-2 ground effect, HRTF convolver
├── synth/                  # procedural sources, one file or folder per generator
├── config/                 # scene YAML → AudioScene
│   ├── scene.rs            #   the scene file's own fields
│   ├── environment.rs      #   environment file, wall material names
│   ├── source.rs           #   source files (recording or synth)
│   ├── synth/              #   one file per synth's parameters
│   └── build.rs            #   builds the engine's AudioScene
├── engine/
│   ├── scene/              # AudioScene: all state owned by the audio thread;
│                           #   commands, render, channels, edits, reset
│   ├── edit.rs             # live add/remove of sources
│   ├── telemetry.rs        # levels and positions sent back to the control side
│   ├── profiler.rs         # profile_span! (feature `profiler`)
│   └── memprof.rs          # allocation tracking (feature `memprof`)
├── pipeline/
│   ├── environment.rs      # Environment: bounds, wall materials, ground, air
│   ├── wall_material.rs    # absorption per octave band
│   ├── path.rs             # PathResolver and PathEffect contracts
│   ├── path_resolvers/     # direct path, image sources
│   ├── path_effects/       # delay, air absorption, ground effect, wall absorption
│   ├── renderer.rs         # Renderer contract, SourceContext
│   ├── renderers/          # one file per renderer, plus diffuse field emitters
│   ├── mix_stage.rs        # MixStage contract
│   ├── stages/             # reverb, bass management, delay compensation, …
│   ├── room_acoustics.rs   # Sabine RT60, critical distance, delay sizing
│   ├── builders/           # which pieces each render mode is made of
│   ├── render.rs           # RenderPipeline and the per-buffer render loop
│   └── tests/              # end-to-end pipeline tests, grouped by subject
├── head_tracking/          # OpenTrack UDP, Sony headphone bridge
├── server/                 # WebSocket telemetry and control (port 3333)
└── bin/                    # analysis tools for recordings and synths

crates/
├── core/                   # atrium-core: math and panning with no I/O
│                           #   (Vec3, listener, speakers, VBAP, DBAP, ambisonics,
│                           #    directivity, commands, telemetry types)
├── ffi/                    # the engine as one object for the Kotlin app
├── tui/                    # terminal dashboard (`--ui`)
└── world/                  # atrium-world: a scene's sounds as entities
                            #   (id, sound, shape, place, moves)

tests/
└── audio_thread_allocations.rs   # the audio callback never touches the heap
```

Crate conventions (private modules behind a small facade, one implementation
per file) are in [AGENTS.md](AGENTS.md).

## Threads

```text
Control thread                          Audio thread (cpal callback)
     │                                        │
     │── Command (rtrb ring buffer) ────────▶ │  process_commands
     │   SetListenerPose, SetRenderMode, …    │
     │                                        │  AudioScene::render
     │                                        │    tick sources
     │                                        │    render_pipeline (active mode)
     │ ◀──────── TelemetryFrame (rtrb) ───────│    push telemetry
     │                                        │
     │                                   cpal → device
```

All five pipelines are built at startup, so switching mode allocates nothing.

**The audio callback must not allocate, lock or do I/O.** Commands arrive over
a wait-free ring buffer and all state is owned by `AudioScene` on the audio
thread. `tests/audio_thread_allocations.rs` enforces the allocation part: it
counts every heap operation inside the callback, in every render mode, across
the shipped scenes, with live commands arriving.

## Profiling

Two optional features, both compiled out when disabled. Commands are in
[run.md](run.md#profiling).

- **`profiler`** instruments the render path with `tracing` spans and can write
  terminal timings, a Perfetto trace or a flame graph.
- **`memprof`** counts allocations on the audio thread per stage while the
  engine runs.

| Span | Location | What it measures |
|---|---|---|
| `callback` | `audio/output.rs` | The whole cpal callback |
| `process_commands` | `engine/scene/commands.rs` | Draining the command queue |
| `render` | `engine/scene/render.rs` | One buffer: tick, analysis, pipeline, telemetry |
| `source_tick` | `engine/scene/render.rs` | Advancing every source |
| `perceptual` | `engine/scene/render.rs` | Masking analysis (telemetry only) |
| `pipeline` | `engine/scene/render.rs` | The active render pipeline |
| `renderer` | `pipeline/render.rs` | One source through its renderer |
| `mix_stages` | `pipeline/render.rs` | The post-mix chain |
| `telemetry` | `engine/scene/render.rs` | Building and pushing a telemetry frame |

For external profilers (Instruments, perf) there is a `profiling` Cargo profile:
a release build with debug symbols.

```bash
cargo run --profile profiling -- scenes/default.yaml
```

## Dependencies

| Crate | Purpose |
|---|---|
| [cpal](https://github.com/RustAudio/cpal) | Audio device I/O |
| [symphonia](https://github.com/pdeljanov/Symphonia) | Decoding MP3, WAV, AAC, Ogg Vorbis |
| [rtrb](https://github.com/mgeier/rtrb) | Wait-free single-producer ring buffer |
| [realfft](https://github.com/HEnquist/realfft) | FFT for the HRTF convolver |
| [sofar](https://crates.io/crates/sofar) | Reading SOFA HRTF files |
| [tungstenite](https://github.com/snapview/tungstenite-rs) | WebSocket server |
| serde, serde_yaml, serde_json | Scene files and telemetry |

## Extension points

| Trait | File | Implement it to add |
|---|---|---|
| `SoundSource` | `crates/core/src/source.rs` | A new kind of sound (a synth, a player) |
| `PathResolver` | `src/pipeline/path.rs` | Another way sound reaches the receiver (for example ray tracing) |
| `PathEffect` | `src/pipeline/path.rs` | Per-path processing |
| `Renderer` | `src/pipeline/renderer.rs` | A spatialisation method |
| `MixStage` | `src/pipeline/mix_stage.rs` | Whole-buffer processing after the mix |
| `AudioOutput` | `src/audio/output.rs` | Another audio backend |

A new render mode is a new file in `src/pipeline/builders/` that picks one of
each.

## Further reading

- [docs/physics-audit-decisions.md](docs/physics-audit-decisions.md): why the models are the way they are
- [docs/living-atrium-roadmap.md](docs/living-atrium-roadmap.md): the synthesis roadmap
- [docs/listener-tracking.md](docs/listener-tracking.md): head and position tracking
- [REFERENCES.md](REFERENCES.md): libraries and papers the engine draws on
