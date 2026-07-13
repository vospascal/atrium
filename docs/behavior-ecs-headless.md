# Behavior Layer on Bevy ECS — Headless-Capable Design

Question explored: can the dynamic-behavior layer (birds, ambient events, motion,
intensity variation, weather) be built on Bevy's ECS, shared between the bevy-ui
editor and a headless run, without pulling the rendering stack into the headless
binary?

**Verdict: yes, cleanly.** Bevy is modular: `MinimalPlugins` (task pool, time,
schedule runner) runs an `App` with no window, no renderer, no winit. A behavior
crate can depend on `bevy` with `default-features = false` (or directly on the
`bevy_app`/`bevy_ecs`/`bevy_time`/`bevy_transform` sub-crates) and be added as a
plugin to either the full editor App or a minimal headless App. Verify the exact
feature set compiles at implementation time, but this is a standard Bevy pattern.

---

## 1. Why ECS fits (better than the TS original)

The TS project *faked* composition: `BirdNode`/`AmbientNode` were classes that
hand-instantiated component objects and manually delegated every getter/setter,
with callbacks wired in constructors. Bevy ECS gives that model natively:

- **A "BirdNode" is not a class** — it's an entity with a component set:
  `(SoundSource, Bird, Repertoire, CallScheduler, IntensityLfo, Transform, …)`.
  "Grouping animals into nodes" costs nothing: marker components + queries
  replace the class hierarchy.
- The TS panel lists map to component presence almost 1:1, which is exactly how
  the inspector should compose:

  | TS panel                | atrium equivalent                                   |
  |-------------------------|-----------------------------------------------------|
  | species                 | `Bird { species }` component + species asset table  |
  | sound-repertoire        | `Repertoire { clips }` component                    |
  | call-frequency          | `CallScheduler { min_gap, max_gap }` component      |
  | intensity-variation     | `IntensityLfo { min, max, period }` component       |
  | audio-fade              | control-rate fade on trigger (part of scheduler)    |
  | {wind\|rain\|wave} params | `SynthParams` component (per-generator)           |
  | spatial-distance        | **engine-owned** — UI shows read-only telemetry     |
  | volume-pipeline         | **engine-owned** — read-only telemetry              |
  | attenuation / basic     | already in atrium (SPL, spread, directivity, model) |

- Existing precedent in-repo: `crates/bevy-ui/src/ecs/observers.rs` already
  syncs component changes → `Command`s over rtrb (`sync_source_properties`).
  The behavior crate reuses this exact bridge.

## 2. Division of authority — "ECS decides, engine renders"

The engine already solves everything per-sample; do not duplicate it in ECS:

- **RT audio thread (authoritative, per-sample)**: distance attenuation, air
  absorption, directivity, spread, reflections, reverb, LR4/LFE, sample-accurate
  envelopes of the synth generators.
- **ECS (authoritative, control-rate ~30–60 Hz)**: *what, when, where* —
  event scheduling and repertoire choice, slow intensity LFOs, source motion,
  weather state + transitions, time-of-day. Output = small `Copy` commands and
  occasional `SceneEdit`s.

## 3. Crate layout

New crate `crates/behavior` (`atrium-behavior`), no UI dependencies:

- **Components**: `Bird { species }`, `AmbientEmitter { environment }`,
  `Repertoire`, `CallScheduler`, `IntensityLfo`, `Motion` (orbit / waypoint path
  / one-time flight), `SynthParams`.
- **Resources**: `WeatherState { wind_speed, rain_intensity, storm_level,
  temperature_c, humidity_pct }`, `WeatherTransition` (slow interpolation —
  machinery recoverable from the deleted `weather.rs`, git `dba6163`).
- **Systems** (plain schedule, no render deps): `evolve_weather`,
  `schedule_calls` (uniform-random gap + random repertoire pick),
  `breathe_intensity`, `move_sources`, `apply_weather_to_synths`,
  `apply_weather_to_atmosphere` (→ existing `Command::SetAtmosphere`).
- **Bridge resources**: `CommandSender` (move out of bevy-ui into behavior or a
  shared spot) + a `SceneEdit` producer.

Consumers:
- `atrium-bevy` adds `BehaviorPlugin` to its full App — entities get icons/
  gizmos in the 2D view. (The old 3D-garden GLB models were removed from
  `assets/models/`; recoverable from git `dba6163` if a 3D view returns.)
- `main.rs` headless path builds `App::new().add_plugins(MinimalPlugins)
  .add_plugins(BehaviorPlugin)` and either lets `ScheduleRunnerPlugin` loop it
  or calls `app.update()` from the existing control loop. The TUI/telemetry
  thread is unaffected.

## 4. Command vocabulary gaps (small additions to `atrium_core::commands`)

1. **One-shot triggering** — bird calls / thunder claps. Recommended: sources
   host their preloaded repertoire (multi-clip `SoundSource` with voice
   playback), triggered by a new `Copy` command
   `TriggerSource { index: u16, clip: u8, gain: f32 }`. No RT allocation, no
   16-slot churn from transient AddSource/RemoveSource pairs.
2. **Synth parameter updates** — `SetSynthParam { index: u16, param: SynthParam,
   value: f32 }` with a small shared param enum (wind speed, gustiness, rain
   intensity, …), dispatched by the source itself.
3. **Gain trim for LFO breathing** — either reuse `SetSourceSpl` (verify the
   engine slews amplitude; add smoothing if a 30 Hz command stream zippers) or a
   dedicated smoothed `SetSourceGainTrim`.

## 5. Motion: move authority to ECS, delete engine-side orbit

Orbit currently lives **on the audio thread** (`SetSourceOrbitSpeed/Radius/Angle`
commands + per-source orbit state, reset by `ResetScene`). Two motion systems
would be muddy. Per the no-backward-compat principle: move motion to the ECS
`Motion` component (systems update `Transform`, observer sends
`SetSourcePosition`), then delete the three orbit commands and the engine-side
orbit state, updating all callers (bevy-ui panels, scene YAML, tests).

Gained: arbitrary paths (a bird actually flying between perches, a bee circling
the listener), one-time flights, interval motion — the TS `OrbitalMotionComponent`
patterns that were designed but never used. Check at implementation: engine's
position-change smoothing (delay interpolation) is happy with 30–60 Hz updates,
and size the command ring buffer (currently 256) for N moving sources × rate.

## 6. Headless nuances

- Tick rate: 30 Hz is plenty for scheduling/weather; motion may want 60 Hz.
  Event *timing* jitter of one tick (~16–33 ms) is inaudible for nature sounds.
- Determinism: seed per-entity RNG (component-held, e.g. splitmix from entity id
  + scene seed) so headless runs are reproducible — matches the synth crates'
  seeded `Rng` style.
- The behavior crate must not use `bevy::prelude` items gated behind render
  features; CI should build `--no-default-features --features tui` to keep the
  headless path honest.

## 7. Open decisions

- Where scene YAML gains behavior blocks: per-source `behavior:` section
  (repertoire, call gaps, lfo, motion) — extends the schema work in
  `docs/spatial-port-candidates.md` §3.1.
- Whether `SoundSource` (the bevy-ui component) splits into smaller components
  (SPL/spread/directivity vs identity/color) now that more systems write to it —
  change detection granularity says yes, but can wait.
- Fade handling for one-shots: engine-side per-voice attack/release (sample
  accurate, preferred) vs control-rate gain ramps.
