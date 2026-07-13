# Living Atrium — Implementation Roadmap

Step-by-step plan combining `docs/spatial-port-candidates.md` (what to port) and
`docs/behavior-ecs-headless.md` (how the behavior layer works). Ordered by
dependency: engine plumbing → one-shots → behavior → weather → UI.

Already done (2026-07-12): `crates/behavior` spike (headless bevy_ecs proven,
IntensityLfo + OrbitMotion systems, CommandSender moved out of bevy-ui), old
3D-garden assets deleted.

---

## Phase 1 — Synth sources become first-class scene citizens
*The blocker for everything else. The generators exist; they just can't be placed.*

1. **Source definition enum** — ✅ DONE 2026-07-12: `SourceDef` is now
   Sample/Synth (serde untagged + `synth:`-tagged `SynthSpec` with typed
   per-generator Option params). Synth defs: `sources/synth-*.yaml`.
2. **Scene build** — ✅ DONE 2026-07-12: `build_one_synth_source` renders a 6 s
   preview control-side for RMS + Bark bands (same loudness path as samples),
   wraps generators in `SynthNode` (SPL/directivity/spread/mute). Live-add via
   bevy preset browser also works (`main.rs::add_source`). Test scene:
   `scenes/synth-test.yaml` (wind W, waves S, rain v2 N, rain v1 E — A/B).
   Note: rain_v2's `drop_rate` knob is deliberately NOT exposed in YAML — it's
   dead code in the DSP (audit pending).
3. **Live parameters**: add `Command::SetSynthParam { index, param, value }`
   with a small `Copy` param enum; give `SoundSource` a default-no-op
   `set_param`. Wind speed / rain intensity / wave period become live-tunable.
4. **Scene maker UI**: synth kinds in the add-source flow + a per-generator
   parameter panel (driven by a param descriptor table, not hand-built per
   generator).

**Verify**: scene YAML round-trip tests; a scene with wind+rain+waves audibly
plays; every public synth param provably reaches the DSP ("knob reaches DSP"
test per generator); no RT-thread allocation.

## Phase 2 — Port thunder (strike + rumble)
1. Port `ThunderStrikeProcessor` + `ThunderRumbleProcessor` from the TS project
   into `src/synth/` as self-terminating one-shot sources, keeping the
   near/mid/far/massive presets and the distance-dependent HF damping.
2. Extend `analyze_synth` coverage + physics tests (crack length, tail decay
   time, spectral tilt vs distance parameter).

**Verify**: rendered strike/rumble match expected duration/spectrum; the infra
layer (<25 Hz) actually reaches the LFE path through bass management.

## Phase 3 — One-shot triggering + repertoire sources
1. **`Command::TriggerSource { index, clip, gain }`** (`Copy`, fits the ring).
2. **Repertoire sources**: a source hosting N preloaded clips (decoded
   control-side), idle-silent, playing a chosen clip on trigger with a short
   attack/release envelope (sample-accurate, engine-side). Thunder one-shots
   are the synth flavor of the same trigger mechanism.
3. Extend source YAML: repertoire list (multiple clips) instead of single path.

**Verify**: trigger → exactly one playback, idle → digital silence; no RT
allocation on trigger; clip index out of range is safely ignored.

## Phase 4 — Behavior layer goes live (ECS, headless + editor)
1. **Headless integration**: tick a `MinimalPlugins` + `BehaviorPlugin` App from
   the headless path in `main.rs` (30–60 Hz), owning the `CommandSender`.
   Decide: make `atrium-behavior` a default dependency (it currently rides the
   `bevy` feature only).
2. **`CallScheduler`** component + system: uniform-random gap
   (`min + rand * (max - min)`, redrawn per trigger), random repertoire pick →
   `TriggerSource`. Seeded per-entity RNG for reproducibility.
3. **Scene YAML `behavior:` block** per source (repertoire timing, intensity
   LFO, motion) → spawned as behavior entities on scene load.
4. **Editor side**: bevy-ui adds `BehaviorPlugin`; inspector panels compose by
   component presence (species/repertoire/call-frequency/intensity, mirroring
   the TS panel model).

**Verify**: seeded run produces expected call-gap distribution; headless run
(no `--bevy`) produces identical command stream to editor run with same seed.

## Phase 5 — Motion authority moves to ECS (delete engine orbit)
1. Wire scene YAML orbit fields to the ECS `OrbitMotion` component (already
   implemented in `crates/behavior`).
2. Delete `SetSourceOrbitSpeed/Radius/Angle`, engine-side orbit state, and
   `ResetScene`'s orbit handling; update bevy-ui observers/panels and tests to
   the ECS path. No compat layer.
3. Check position-update smoothing: 30–60 Hz `SetSourcePosition` streams must
   not zipper (listen + render test); size the command ring for
   N moving sources × rate.

**Verify**: existing orbit scenes sound identical; grep for stale orbit
references in comments/docs after deletion.

## Phase 6 — WeatherState (the glue)
1. `WeatherState` resource (wind speed, rain intensity, storm level,
   temperature, humidity) + slow transition system — recover the interpolation
   machinery from the deleted `weather.rs` (git `dba6163`).
2. Mapping systems: weather → synth params (`SetSynthParam`), weather → event
   rates (storm level scales thunder/gust frequency in `CallScheduler`),
   weather → `SetAtmosphere` (command already exists).
3. Weather presets + controls in bevy-ui (and a TUI readout); optional visual
   resurrection (rain particles) later.

**Verify**: physics test — humidity/temperature change measurably alters
air-absorption output; transition produces no audible parameter steps.

## Phase 7 — UI wave (after it sounds alive)
1. **Per-channel meters**: peak/RMS dB per speaker from the existing telemetry
   frames (`channel_peaks` already ships).
2. **Attenuation & range viz**: ref/max-distance rings for selected/playing
   sources; distance-model-specific field rendering; meter-labeled distance
   grid around the listener.
3. **Audio-reactive map**: pulsing playing sources, gain-scaled speaker arcs,
   annotated listener→source line.
4. Later: group mute/solo, listener hearing cone + preset bundles, smart clone
   placement, zoom-to-fit including audible range.

---

**Sizing (rough)**: P1 the biggest (schema + engine + UI touch), P2/P3 medium,
P4 medium, P5 small-but-wide (deletion sweep), P6 small once P1/P4 exist,
P7 incremental. Each phase lands green (`cargo fmt`, `clippy`, full tests) and
is independently useful.
