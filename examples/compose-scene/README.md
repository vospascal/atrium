# Atrium app

A Compose Multiplatform app for macOS desktop and iOS. It runs the Atrium engine
**in its own process**: Kotlin calls Rust through bindings that
[OxiKT](https://github.com/UbiqueInnovation/oxikt) generates from `crates/ffi`. No server is
involved and no WebSocket is used.

It has two faces that share one running engine.

**The player** has three tabs:

- **Home** is the scene now playing. The ink shows it as ink in water: each sound is a spring at
  the edge where it lies, releasing ink in its scene color, and you are the rings at the centre.
  Louder sounds release more ink and push harder. A moving sound drags the water and stirs
  eddies; a still one flows laminar. **Zen 2** pairs a slower veil
  with a sharper layer. Ink drifts around and past the listener and fades throughout the canvas.
  Moving sources carry their momentum between telemetry updates. Fields with a bearing flow
  in from that side, and all-direction winds drift in broad currents from the edges.
  A directional object uses a narrower plume according to its built directivity.
  **Zen 3** turns the same flow into side-lit smoke. It follows the appearance and the ink
  setting exactly as Zen 2 does: each sound's color on paper or on near-black, or one tone.
  The source's short-time level controls density in both. Low frequencies drive the slow,
  broad layer and high frequencies bring out finer detail. The layers share the midrange.
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

**The editor** is opened from Settings, with or without a scene playing. Without one it lists the
scenes to open; Open… (⌘O) in its top bar switches to another scene at any time, and asks first
when the one open has changes that are not saved. It is in the player's paper style,
and on desktop the window widens for it. It has three panels; in a narrow window they are tabs.

The editor works like other editors. The layers and library on the left, the inspector on the
right and the tracks underneath can each be resized by dragging their border and hidden with
the toggles at the top right; Tab hides or shows them all, leaving the map. The layout is kept
between launches. Lists scroll with a thin bar, have every other row shaded, light up under the
pointer, and mark the selected row with a bar on its left. These come from `Chrome.kt`, so a new
panel or list gets them by using it.

- **Layers** are the sounds of the scene in their groups. The ear in front of a layer silences
  it and brings it back, like the eye in a picture editor's layer list; a group's ear does it
  for the whole group. The line shows how strongly each layer reaches you. The bar underneath
  has the actions: plus opens the library, and the bin deletes the selected layer.
- **Library** lists the presets in `sources/`. The play button in front of one plays it just
  ahead of you until you press it again; the sound starts about half a second after the press.
  Add puts it in the scene there, as a new layer.
- **Save** is in the bar at the top, next to the scene's name and file; a dot after the name
  means there are changes that are not saved. ⌘S saves and ⇧⌘S saves under a new name. A scene
  from the repository is written by hand and keeps its comments, so its first save asks for a
  name and writes a copy beside it. That copy is the editor's own, and later saves replace it.
  Mutes, volume, output and head tracking are yours and are not saved with the scene.
- **Tracks** under the map are the engine's 16 voices, as in a multitrack editor. Each shows the
  sound in it, or Empty, and the last minutes as clips: where it sounded, which take, and how
  loud the take itself was. A sound that rests leaves gaps; one that loops is one long clip; a
  muted one is faint. Right of the "now" line are the takes the plan has already decided, drawn
  open. Above the voices, wind, rain, storm and daylight are lanes, the past and what the weather
  story will do next. Click a track to select its sound. It zooms to 30 s, 2 min or 10 min.
- **The timeline** is the top of the tracks panel: three hours of the scene with its weather drawn
  in. Press or drag anywhere on it to move the whole scene there: every sound and the weather are
  as they would be at that moment, a take under way starts partway, and it plays on from there.
  Dragging the ruler above the tracks scrubs more finely. 1×, 10× and 60× run the weather faster
  while the sounds keep their own pace. **Pause** in the top bar, or the space bar, stops the
  scene's clock: sounds, plan and weather hold where they are.
- **Air** is under Weather: the temperature and humidity, which decide how fast sound travels
  and how much treble the air takes from a distant sound. A point also has a **Size**, from a
  point to all around, drawn as a soft disc on the map.
- **Channel meters** sit in the header of the tracks: one per output channel, named as the room
  names its loudspeakers, bold when a channel clips.
- **Listen** is in the inspector when nothing is selected. *Hear only* plays one part of the
  scene alone: the direct sound, the reflections, the late reverb or the ambient sounds. *With*
  switches single effects off and on: air absorption, moving air, the ground, wall colour, and
  the near field (on headphones, the near ear's extra level for a sound inside about a metre; the
  engine's README, "A sound beside the ear"), and the headphone EQ (on headphones, the dummy
  head's own loudspeaker and ear canal taken out of what reaches the ears; off, they are heard as
  measured; the engine's README, "Two ear canals").
  *From* plays one loudspeaker or one ear alone. While anything is hidden the top bar says
  what is heard, and pressing that brings the whole scene back. It is yours, like a mute: not
  saved, not undone.
- **Wind from** is under the wind's band in Weather: the compass bearing the wind comes from. An
  arrow in the map's top-left corner points where it blows. It decides how fast far sounds waver
  in the moving air (the engine's README, "Moving air").
- **Weather** is in the inspector when nothing is selected: the story (Held, Drifting or Passing
  storm) and the band each condition keeps to, with where it is now. A selected sound's
  **Follows** says how it answers them, such as "Activity 100 % → 0 % as rain rises".
- **The map** shows every sound from above, north up, with you in the middle and rings at 5, 10
  and 20 m. Sounds are black and white, and the selected one is in its own colour. A sound's
  circle is how far its level carries, so a louder sound is larger; a focused sound has a wedge
  as wide as its beam instead. A ring travels outward from every sound that is heard. A wind is
  drawn as bands arriving from the side its anchor lies on, and as rings all round when the
  anchor is at you. Click a sound to select it, drag it to move it, click the ground to select
  nothing. Shift or ⌘ with a click adds a sound to the selection or takes it out; dragging one
  of several moves them all as one step, and the arrow keys nudge them. Hold Alt while dragging
  and the sounds stay put while copies land where you let go. Hold **S** to hear only what is
  selected, for as long as the key is down. **Loudness**, bottom left, colours the ground by how
  loud the place is there: everything that is heard, or the one selected sound alone. Each band
  is 6 dB, a doubling of distance, and it shows direct sound only. A wind is grabbed near its anchor; moving it changes where it is drawn to come from,
  not how it sounds, since an ambient sound is heard everywhere. The selected sound shows its
  distance and bearing on a dashed line, and a focused one has a handle at the end of its facing
  line: drag it to turn the sound.
- **The inspector** shows the selected sound in a fixed order: its level, which can be set; its
  shape, first whether it is a **Point** or **Ambient** (heard everywhere at its level, with no
  distance, direction or echo; any sound can be either, and switching starts it over), then for
  a point a width from a narrow beam to all round and how much is heard behind it, with Omni,
  Focused and Narrow as starting points and a drawing of the beam turned the way it faces; then its place, when it plays and whether it
  moves. **When** is All the time or Now and then; Now and then has a range with two handles for
  the shortest and longest rest. Anything with takes lists them, with the one sounding now lit
  and "Resting" between two, and each take's chance as steps from never to four times as often.
  The play button in front of a take plays that take once on its own, just ahead of you. A
  sound with several takes that comes and goes can hold on to one for a bout: a second range
  sets how many times running it is uttered before another is chosen. With nothing selected it shows the scene.

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
  vorticity confinement. The scene covers use one 112-cell water grid whose ink drains into the centre. Zen 2 and Zen 3 use separate 68- and
  104-cell grids so the slower wash and nearer ribbons can drift at different speeds. The dye
  grids start at twice the water resolution. On larger canvases Zen 2 and Zen 3 increase that to
  at most three times for the veil and six times for the sharper layer. The dye
  moves by MacCormack advection, so its edges stay sharp like ink instead of blurring like smoke.
  The cover grid costs about 1.1 ms a step on desktop. Zen 2 and Zen 3 have not been profiled on a device. A Skia shader
  (`InkShader.kt`) upscales the dye, keeps thin veils clear, deepens dense ink and draws dark
  lines where sheets fold. The fluid parameters were tuned by eye from rendered stills, not
  watched live. A slight stair-step can still show along folds that cross the water grid. The
  SkSL compiles at runtime. `InkShaderTest` renders it and checks the pixels at ink folds and in clear water.
- **Not from the Shadertoy fluid shaders.** "Multiscale MIP Fluid", its forks and "Chimera's
  Breath" are CC BY-NC-SA 3.0 (non-commercial, share-alike), so none of their code is used here. The solver is
  written from the published methods.
- **Ink strength maps 20–65 dB SPL at the listener to faint–dense.** The numbers were chosen by
  eye, not from a perceptual model.
- **A sound's circle on the map is distance alone.** It is where the sound's level has fallen
  to 60 dB, losing 6 dB for every doubling of distance. Air, ground and direction are left
  out, so it compares sounds and does not say what you hear. That is computed in Kotlin, not
  by the engine.
