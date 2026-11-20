# Atrium app

A Compose Multiplatform app for macOS desktop and iOS. It runs the Atrium engine
**in its own process**: Kotlin calls Rust through bindings that
[OxiKT](https://github.com/UbiqueInnovation/oxikt) generates from `crates/ffi`. No server is
involved and no WebSocket is used.

It has two faces that share one running engine.

**The player** has three tabs:

- **Home** is the scene now playing. The ink shows it as ink in water: each sound is a spring at
  the edge where it lies, releasing ink in its scene color, and you are a drain at the centre
  rings. The ink flows from every sound in to you. Louder sounds release more ink and push
  harder. A moving sound drags the water and stirs eddies; a still one flows laminar. When a
  sound stops, its last ink drains into the centre and fades. **Zen 2** pairs a slower veil
  with a sharper layer. Ink drifts around and past the listener and fades throughout the canvas.
  Moving sources carry their momentum between telemetry updates. Fields with a bearing flow
  in from that side, and all-direction winds drift in broad currents from the edges.
  A directional object uses a narrower plume according to its built directivity.
  **Zen 3** turns the flow into side-lit smoke. **Zen 4** gives it broader ink blooms,
  feathered edges and a grainy texture. Both use a near-black backdrop in their player view.
  The source's short-time level controls density in all four modes. In **Zen**, lower
  frequencies broaden the single plume and higher frequencies tighten and quicken it.
  In **Zen 2**, **Zen 3**, and **Zen 4**, low frequencies drive the slow, broad layer and
  high frequencies bring out finer detail. The layers share the midrange.
  When a source falls silent or is muted, it stops releasing ink and the remaining ink fades.
  **3D Map** shows the sounds around the listener. Prev, Play and Next move through the scenes.
- **Scenes** lists every scene in `scenes/`. Each cover is a still of that scene's own ink, drawn
  from its file, so nothing has to be painted by hand. A mini player sits above the tabs.
- **Settings** has the appearance (Auto, Light or Dark), the ink (Color or Monochrome), the
  output, head tracking, the volume, and the way into the editor. Head tracking is for
  headphones: the listener turns with your head, so the place stays where it is. On macOS it
  offers Sony headphones (WH/WF-1000XM5 or XM6, started by the app) and any tracker that sends
  OpenTrack UDP to port 4242. The choice holds across scenes until the app closes. Settings says
  whether head movement is arriving, and the 3D map draws where you face as a wedge at the
  centre that turns with your head, with a line along its bottom edge giving the status and
  the compass bearing you face. On a headphone output that line also says when head tracking
  is off. `run.md` has the headphone setup the Sony bridge needs.

The player is light or dark as chosen; Auto follows the system. Appearance and ink are remembered
between launches: in Java preferences (`atrium/app`) on desktop, in `NSUserDefaults` on iOS. The ink is one transparent layer with
the same paint in both. Only the backdrop, text, controls and listener rings change. Monochrome
draws every plume in the opposite of the backdrop, black on white or white on black. It gets its
depth from tone instead of hue.

**The editor** is opened from Settings while a scene plays. It is in the player's paper style,
and on desktop the window widens for it. It has three panels; in a narrow window they are tabs.

- **Layers** are the sounds of the scene in their groups. The dot silences a layer or a whole
  group, `S` lets one or more be heard alone, and the line shows how strongly each reaches you.
- **Library** lists the presets in `sources/`. Hold one to hear it just ahead of you, and Add
  puts it in the scene there, as a new layer. An added sound lasts until the scene is played
  again: scenes cannot be saved yet, and a layer cannot be removed.
- **The map** shows every sound from above, north up, with you in the middle and rings at 5, 10
  and 20 m. A sound is a dot with a halo as strong as it is heard, a focused one has a wedge as
  wide as its beam, and an ambient one is a wash with a dashed edge. Click a sound to select it,
  drag it to move it, click the ground to select nothing. The selected sound shows its distance
  and bearing on a dashed line.
- **The inspector** shows the selected sound in a fixed order: its level, which can be set, then
  its shape, place, when it plays and whether it moves. With nothing selected it shows the scene.

A scene can set `title:`. Without it, the player makes one from the file name.

## Layout

| Source set   | Holds                                                                           |
| ------------ | ------------------------------------------------------------------------------- |
| `commonMain` | Everything: player tabs, ink, map, editor, engine session, telemetry            |
| `jvmMain`    | `main()`: a desktop window                                                      |
| `iosMain`    | `MainViewController(sceneDirectory)` for an iOS host app                        |
| `commonTest` | Runs on JVM and the iOS simulator, including one real call into the Rust engine |

The generated bindings are in the package `atrium.engine` and appear under `build/uniffi/`.

## Run on macOS

You need rustup, Xcode for iOS, and a JDK that Gradle can provision. The first build installs the
OxiKT binding generator with `cargo install`, so it is slow once.

```sh
cd examples/compose-scene
./gradlew run
```

The player lists `scenes/` from the repository root. To list another directory, pass it as the
program argument.

```sh
./gradlew jvmTest iosSimulatorArm64Test
```

## Known gaps

- **No iOS host app yet.** The framework (`AtriumApp`, static) builds and its tests run on the
  simulator. An Xcode app still has to bundle the scene files and start in their directory.
  See `crates/ffi/README.md`.
- **The ink is a Navier–Stokes simulation (`InkFluid.kt`).** It is Stam's "Stable Fluids" with
  vorticity confinement. Zen's water grid is 112 cells across. Zen 2 uses separate 68- and
  104-cell grids so the slower wash and nearer ribbons can drift at different speeds. The dye
  grids start at twice the water resolution. On larger canvases Zen 2, Zen 3 and Zen 4 increase that to
  at most three times for the veil and six times for the sharper layer. The dye
  moves by MacCormack advection, so its edges stay sharp like ink instead of blurring like smoke.
  The original Zen costs about 1.1 ms a step on desktop. Zen 2 has not been profiled on a device. A Skia shader
  (`InkShader.kt`) upscales the dye, keeps thin veils clear, deepens dense ink and draws dark
  lines where sheets fold. The fluid parameters were tuned by eye from rendered stills, not
  watched live. A slight stair-step can still show along folds that cross the water grid. The
  SkSL compiles at runtime. `InkShaderTest` renders it and checks the pixels at ink folds and in clear water.
- **Not from the Shadertoy fluid shaders.** "Multiscale MIP Fluid", its forks and "Chimera's
  Breath" are CC BY-NC-SA 3.0 (non-commercial, share-alike), so none of their code is used here. The solver is
  written from the published methods.
- **Ink strength maps 20–65 dB SPL at the listener to faint–dense.** The numbers were chosen by
  eye, not from a perceptual model.
- **The editor's map has no loudness picture.** It shows how strongly each sound reaches you,
  from the engine, and draws no falloff of its own. A loudness layer has to be computed by the
  engine first.
- **The editor's map cannot be zoomed or panned.** It fits the scene: the farthest placed sound
  decides how much ground it shows.
- **The speakers are not on the editor's map.** The engine's scene description does not report
  them yet.
