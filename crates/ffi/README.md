# atrium-ffi

The Atrium engine for the Kotlin Multiplatform app in `examples/compose-scene`. Everything
Kotlin can reach is declared here:

- `list_scenes(directory)`: the scene files a player can pick from. Each comes with its title and,
  for each sound, its name, color and direction from the listener. They are read from the YAML
  alone, with no audio decoded.
- `list_presets(directory)`: the presets a sound can be made from, with a name, a group and
  whether each is a recording or a generator.
- `head_trackers()`: the head trackers that can work on this platform.
- `Engine`: load a scene and play it. Its methods are `scene()`, `outputs()`, `set_output`,
  `set_volume`, `set_source_position`, `set_source_facing`, `set_source_pattern`,
  `set_source_muted`, `set_source_level`, `audition`, `add_sound`, `remove_sound`,
  `save_scene`, `set_head_tracker` and `latest_telemetry()`.
- The records it returns: `Position`, `SceneDescription`, `SceneFile` (which says whether the
  editor wrote it), `Output`, `Telemetry`. Telemetry
  includes where the listener faces and whether a head tracker is sending.
- `EngineError`.

[UniFFI](https://github.com/mozilla/uniffi-rs) proc-macros describe the surface.
[OxiKT](https://github.com/UbiqueInnovation/oxikt) turns that surface into Kotlin for desktop
JVM (a `cdylib` loaded by JNA) and iOS (a `staticlib` linked through cinterop). The Gradle build
runs Cargo itself. There is nothing to build here by hand.

## Decisions that look wrong and are right

- **Only the commands a screen uses are exported:** moving a source (the editor's map), the output,
  volume and head tracker (the player's Settings), and mute, level, pattern, facing, audition,
  add and remove (the editor). Every other `Command` gets a method when a
  Kotlin screen calls it. An exported method with no caller is surface the generated Kotlin
  carries for nothing.
- **`scene()` is the scene as it is now, kept by this object.** The audio thread reports where a
  sound is and whether it is muted, but not its level, and it knows nothing of names, ids or
  groups. So every call that changes a sound (level, position, facing, pattern, add, remove)
  records it in
  the description this object holds, and the app reads `scene()` again after each. The position
  kept there is where a sound was placed; telemetry has where it is, which differs for one
  that moves by itself.
- **The same calls also change the scene file this object keeps, and `save_scene` writes it.**
  The description and the file are held under one lock, so they cannot disagree. Only what
  belongs to the place is recorded: a mute, the volume, the output and head tracking belong to
  the listener and are never saved. Places are kept to the centimetre, facings to the degree
  and levels to a tenth of a dB, so a saved file stays readable.
- **`save_scene` refuses to replace a hand-written scene.** A saved file is rewritten from
  values and loses its comments. A file the editor wrote starts with its mark, and only such a
  file is replaced. The rule is in the engine, not only in the app, so no screen can break it.
- **Each sound in `scene()` carries the voice it plays from.** The list is the scene's sounds,
  not the engine's 16 voices. Once a sound is removed and another added, the third entry is no
  longer in the third voice, so the app matches telemetry by `slot` and never by place in the
  list. `add_sound` takes the lowest free voice, which is why a removed sound's voice and id
  are reused.
- **A sound keeps the facing it was turned to.** Before 2026-11-20 a still sound always faced
  east and an orbiting one faced its centre; nothing could turn either. `SetSourceFacing` sets
  a heading that then wins over both. A sound that was never turned behaves as before.
- **`set_source_facing` takes a direction, not an angle.** The app has the vector from the sound
  to the pointer and should not need to know which way the engine counts angles.
- **A kind can sing.** A preset with `takes` and a `sings` rule utters one take, rests for a gap
  from a range, and utters another, never the same twice running. `scene()` gives each sound's
  take names and telemetry says which one it is uttering, or nothing while it rests. The choice
  is made on the audio thread from a seed per voice, not planned ahead: that was chosen on
  2026-11-21 to hear birds sooner, and it means nothing knows the next phrase in advance.
- **There is no solo.** The engine only knows muted or not, and the app's first layer list had
  an `S` that muted every other layer. Tried on 2026-11-20, that read as the opposite of what
  it did, so each layer now has one ear that silences that layer and nothing else.
- **A sound added while playing goes through the same builder as the scene's own.**
  `SoundBuilder` is kept from the scene build with its loudness calibration and distance model,
  so an added frog is exactly the frog a reloaded scene would have.
- **An audition plays from the last voice and is not in `scene()`.** A scene's sounds fill the
  voices from the first, so the last is the one they reach last. A scene that uses all 16 cannot
  audition, and both calls say so with `NoFreeVoice`.
- **`audition` and `add_sound` block while the sound is built.** A recording is decoded and a
  generator is measured for a few seconds of output: 0.3 to 0.6 s per sound, measured
  2026-11-20 in the unoptimised build the app runs. The app calls them one at a time from a
  background thread, so a stop never overtakes its start. For the same reason the library has
  a play/stop button and not press-and-hold: with a hold, an ordinary short press started the
  sound and stopped it in the same instant, and nothing was ever heard.
- **A sound the audio thread takes out is dropped on the calling thread.** Freeing memory is
  not safe on the audio thread, so it sends the box back, and the next edit collects it.
- **Head tracking belongs to the engine object, so it stops and starts with every scene.** The
  tracker steers one engine's listener through that engine's command ring. The app keeps the
  choice and gives it to each engine as it starts. The cost is that the Sony bridge restarts on a
  scene change. Keeping one tracker alive across engines would need a listener that can be pointed
  at a new ring, which `atrium::head_tracking` does not have.
- **`set_head_tracker(None)` has let go of the port when it returns, and so has a dropped
  engine.** Every tracker listens on OpenTrack's port 4242, and the next scene binds it at once. A
  listener left running would refuse it. The engine's listener thread polls a stop flag every
  100 ms, so a stop blocks for at most that long.
- **Stopping the Sony bridge also terminates the script's children.** The bridge script ends in
  `tail -f … | grep …`, and a shell runs a TERM trap only after its foreground command ends.
  `tail -f` never does. Before 2026-11-20 the engine sent TERM to the script alone. That was
  never noticed because the engine binary only stops at exit, usually by Ctrl+C, which signals
  the whole group. From an app that keeps running, the stop hung forever and left the bridge app
  running. `sony.rs` records the measurement and a test with a script of the same shape guards it.
- **Switching head tracking off turns the listener back to the scene's heading.** Otherwise the
  place would stay turned the way the head last pointed until the scene was restarted. The engine
  does this when its tracker is dropped, so the binding has nothing to remember.
- **Telemetry says whether a tracker is sending, and where the listener faces.** "Switched on"
  and "working" are different things: the port binds and the bridge starts whether or not the
  headphones offer their sensor. On 2026-11-20 the first try in the app did nothing, with no
  hint why. macOS listed the headphones with audio services only, so no head turn could arrive.
  `Telemetry.head_tracking` is now `Off`, `Waiting` or `Receiving`, and `listener_facing` is the
  heading the audio thread renders with. The app draws both on its 3D map.
- **`head_tracking` is filled in by `latest_telemetry()`, not by the frame conversion.** A frame
  comes from the audio thread, which knows the listener's heading but not whether a tracker
  exists. The engine object knows that. `Telemetry::from(&frame)` therefore always says `Off`.
- **`head_trackers()` is decided in Rust, like `outputs()`.** The Sony bridge is a macOS program.
  On iOS nothing is offered, because the listener accepts packets from the same device only and no
  tracker on an iPhone sends them. `set_head_tracker` refuses a tracker that is not in the list.
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
- **Frequency balance travels with each source.** Fixed-size meters read the emitted mono audio
  before spatialization so each source can drive its own ink. Two broad crossovers and smoothed
  RMS windows keep the audio callback allocation-free and let the Kotlin player weight a broad
  and a fine layer independently. The level is relative to the source's build-time calibration.
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
- **Four tests are ignored by default.** Run them with
  `cargo test -p atrium-ffi -- --ignored --test-threads=1`. One at a time, because two of them
  bind the tracker's port.
  `output_and_volume_change_while_playing` needs an output device and plays under a second of
  sound. On 2026-11-19 it passed on a 2-channel headphone device.
  `layers_and_library_change_the_playing_scene` auditions, adds, mutes and sets a level at zero
  volume. It passed on 2026-11-20.
  `head_tracking_holds_its_port_only_while_it_is_on` and
  `telemetry_shows_whether_a_tracker_is_sending_and_where_the_head_points` open the device at
  zero volume and bind port 4242, so no other tracker may be running. Both passed on 2026-11-20.
- **The Sony path works in the app, checked by hand only.** On 2026-11-20, after the headphones
  were paired again and listed their sensor service, the wedge on the 3D map turned with the
  head in the running app. Before that, a one-off test drove the Settings screen with a running
  engine and a simulated click on the Sony chip: the status went from waiting to receiving and
  closing the engine stopped the bridge after 298 samples. Whether the sound itself stays put
  has not been reported. The automated tests use a stand-in tracker over loopback.
- **The headphones' sensor service comes and goes.** It was there on 2026-11-19, gone the next
  evening, and back after forgetting and pairing the headphones again. The cause is not known.
  `system_profiler SPBluetoothDataType` must list `HID` among the headphones' services.
- **`Waiting` does not say why.** Headphones that are off, a missing Input Monitoring grant and a
  sensor the headphones do not offer all look the same: nothing arrives. The app shows the Sony
  setup step that has been the cause so far. The bridge's own log is in
  `$TMPDIR/atrium-sony-bridge.log`.
- **The first Sony start on a machine builds the bridge.** The script clones a pinned commit from
  GitHub and compiles it into `target/sony-head-tracker/`. The script's path is fixed when the
  engine is compiled, so a build moved away from this repository cannot find it.
- **A sound's size cannot be set.** `SetSourceSpread` exists in the engine; no screen uses it.
- **A new sound always lands 2 m ahead of where the scene puts the listener.** Dropping it at a
  chosen spot needs the map of all sounds.
- **A killed app leaves its Sony bridge running.** The bridge is stopped when the engine is
  closed. A process that is killed outright never closes it, and whether closing the app's
  window always does has not been checked. A bridge left behind keeps sending to a port nobody
  listens on, and the next start adds a second one. `pkill -f SonyBridge` stops them.
- **The head turns, the listener's position does not.** A tracker sends the scene's own listener
  position with every heading. Once the app can move the listener, the two have to be combined.
- **The WebSocket server is still in the engine binary** for `web/`. This crate does not use it.
