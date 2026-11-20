# atrium-ffi

The Atrium engine for the Kotlin Multiplatform app in `examples/compose-scene`. Everything
Kotlin can reach is declared here:

- `list_scenes(directory)`: the scene files a player can pick from. Each comes with its title and,
  for each sound, its name, color and direction from the listener. They are read from the YAML
  alone, with no audio decoded.
- `Engine`: load a scene and play it. Its methods are `scene()`, `outputs()`, `set_output`,
  `set_volume`, `set_source_position` and `latest_telemetry()`.
- The records it returns: `Position`, `SceneDescription`, `Output`, `Telemetry`.
- `EngineError`.

[UniFFI](https://github.com/mozilla/uniffi-rs) proc-macros describe the surface.
[OxiKT](https://github.com/UbiqueInnovation/oxikt) turns that surface into Kotlin for desktop
JVM (a `cdylib` loaded by JNA) and iOS (a `staticlib` linked through cinterop). The Gradle build
runs Cargo itself. There is nothing to build here by hand.

## Decisions that look wrong and are right

- **Only the commands a screen uses are exported:** moving a source (the composer), and the output
  and volume (the player). Every other `Command` gets a method when a Kotlin screen calls it. An
  exported method with no caller is surface the generated Kotlin carries for nothing.
- **`outputs()` is decided in Rust and fixed when the engine starts.** A channel mode is offered
  only when every channel it uses exists in the scene's layout and on the opened device. Quad uses
  channels 0, 1, 4 and 5, so a 4-channel device cannot play it. Without this rule the UI would offer
  a speaker set that renders into speakers nobody hears. `set_output` refuses anything not in the
  list. The rule is pure (`output.rs`) and unit-tested per device size.
- **`set_output` sends the channel mode and the render mode under one lock, and checks for two free
  slots first.** The audio thread then applies both in the same callback. It never renders a buffer
  with a render mode that does not support the active speaker set.
- **The UI pulls telemetry; the engine never calls Kotlin.** `latest_telemetry()` drains the ring
  and returns the newest frame, once per drawn frame. A push callback would cross into Kotlin from
  an engine thread. On Kotlin/Native that thread would have to attach, and UI state would be
  written off the main thread. Pulling avoids both, and the frame rate already bounds how often
  the UI can use a frame.
- **A thread exists only to hold the audio stream.** `cpal::Stream` is not `Send` on Apple
  platforms. A UniFFI object must be `Send + Sync`, so the engine cannot hold the stream. The
  stream is created, kept and dropped on `atrium-output`, which waits until the engine is
  dropped. Start failures are sent back over a channel, so the constructor still fails
  synchronously with `EngineError::Audio`.
- **`uniffi = "=0.32.0"`, pinned exactly.** The generated Kotlin depends on the FFI layout of one
  UniFFI release, and OxiKT 1.4 generates for 0.32.0 only. Upgrade both together.
- **Records mirror engine types instead of deriving UniFFI on them.** `atrium` and `atrium-core`
  do not depend on UniFFI, and no engine type appears in a public signature here. Conversions live
  in `api.rs`, so a change to an engine type stops at that file.
- **The built source reports whether it is an object or a field.** The editor uses that
  classification to show ambient sound without a point-source falloff, even when a field has
  an anchor for its visual direction. Beam width comes from the built node's radiation
  pattern, and telemetry carries its current facing so the editor can show moving lobes.
- **This must stay the only Rust library linked into the iOS app.** Every Rust `staticlib` bundles
  its own copy of std. Two in one app give duplicate symbols. New bindings go into this crate.

- **A scene's sounds can be listed before it plays.** The listing is read from YAML only. Its
  colors and directions come from the same rules as the built scene (`source_color`, and
  positions relative to the listener, since every position shares the spawn offset). An engine
  test (`summary_agrees_with_the_built_scene`) checks that both agree, so a cover never points a
  sound somewhere other than where it plays.

## Known gaps

- **Scene paths resolve against the working directory**, as they do for the engine binary. The
  desktop Gradle `run` task sets the working directory to the repository root. An iOS host app
  would have to bundle `scenes/`, `environments/`, `sources/`, `assets/` and the HRTF set, and
  start in that directory. No host app exists yet.
- **Gradle builds Rust with Cargo's `dev` profile.** Dependencies get `opt-level = 3` from the
  workspace profile, but `atrium` and `atrium-core` are not optimised. The frog scene starts and
  plays without stream errors. Heavier scenes have not been tried in a debug build. Run Gradle with
  `-PreleaseBuild=true` for an optimised build. That flag also builds the other desktop
  platforms, which needs their cross toolchains.
- **One test is ignored by default:** `output_and_volume_change_while_playing` needs an output
  device and plays under a second of sound. Run it with `cargo test -p atrium-ffi -- --ignored`.
  On 2026-11-19 it passed on a 2-channel headphone device.
- **The WebSocket server is still in the engine binary** for `web/`. This crate does not use it.
