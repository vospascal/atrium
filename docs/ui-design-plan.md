# UI design plan

Status: 2026-11-20. The UI is the Compose app in `examples/compose-scene`, which calls the
engine in its own process through `crates/ffi` (decided 2026-11-19). The scene editor
described here is a proposed concept: nothing in it is built unless a table says so, and
the decisions at the end are open. The "Idea pool" is the wider list to pick from.

A clickable mockup of the editor, drawn and not live:
<https://claude.ai/artifact/4duK9knzgyQkuufEqZFpjn>

The React page in `web/` and the WebSocket server are still in the repository. They are not
part of this plan, and whether they stay is undecided.

## What the UI is for

Atrium renders an environment: a world of sounds, an open block placed somewhere in it, and
a renderer that works out what arrives at each speaker. The UI has one job: let someone
**see the place they are hearing, and change it while listening**.

Every element is judged by the project's own test: does it help make an environment more
believable to listen to? A control that does not change what you hear, or a display that
does not explain what you hear, does not belong on the main screen.

It is not a room-acoustics simulator. No free-camera 3D viewport, no ray paths, no
materials library. Those belong to enclosed worlds and can be revisited when a church or
hall environment exists.

## Principles

1. **The map is the screen.** One map fills the window. Panels sit on its edges and can be
   closed. Everything you can point at on the map can be selected and edited.
2. **The engine is the only source of truth.** The app draws what the engine reports and
   sends commands; it never keeps its own copy of the scene. The same goes for generator
   settings: their names, ranges and defaults come from the engine, not from Kotlin.
3. **No second physics model in the app.** Anything that needs the distance model or air
   absorption (the loudness layer) is computed by the engine and handed over, so the
   picture can never disagree with the sound. One exception exists: the circle around a
   sound on the editor's map is where its level has fallen to 60 dB by distance alone. It
   was asked for so that level can be seen (2026-11-20), and it leaves out air and ground.
4. **Listening first.** Changes are heard immediately. Saving is a separate, deliberate act.
5. **Two faces, one engine.** The player is for living with a place, the editor for
   building one. Both are in the same app and share one running engine.
6. **The same question is always in the same place.** Every sound is described by the same
   five sections in the same order. A bird, a river and a wind differ in their answers,
   not in where you look.
7. **A place never comes round again.** Nothing repeats exactly at any time scale, and
   what changes has a shared cause. This weighs more than any panel, and the next section
   is the model for it.

## Keeping a place alive

The ear catches a loop at every time scale, so a place has to vary at every time scale.
The variation also needs a shared cause, or it sounds like chance instead of weather: the
birds go quiet *because* the rain starts.

| Time scale | What varies | Meadow with a breeze | Passing storm |
|---|---|---|---|
| One sound (seconds) | Which take, its level, a touch of pitch | The blackbird sings a different phrase | Every thunderclap has its own crack and tail |
| A phrase (up to a minute) | Gaps, bouts, answers | Five phrases, a pause, the robin answers | A gust rises and dies, the rain thickens |
| A mood (minutes) | Who is active and how busy | A bird moves to another tree, the breeze drops | The birds fall silent, the gust front arrives |
| The weather and the day (up to hours) | The conditions themselves | Morning turns into midday | The storm comes, passes, and leaves the trees dripping |

Three building blocks cover all four. Most of the pieces were already named in
`spatial-port-candidates.md` and `living-atrium-roadmap.md`; this section ties them into
one model.

### Conditions

A few slow values that the whole place shares: **wind**, **rain**, **storm** and
**daylight**, next to the temperature and humidity the engine already has. They move
slowly, and every sound can listen to them. This is what gives changes a cause.

### Repertoire

A sound is a set of takes, not one file. Each time it sounds, a take is picked, given a
slightly different level and pitch, and followed by a different gap. Two ways of picking:

- **Varied**: never the same take twice in a row.
- **Bouts**: one take a few times over, then a switch, the way a great tit keeps to one
  song for a while.

`assets/birds/` already holds 20 takes of five garden birds (dunnock, great tit, blackbird,
blue tit, robin). No source uses them, and a source file names one recording.

Generators need no repertoire: wind, rain and river are made fresh as they play. Thunder
is the generator form of a repertoire, with every strike made new.

### Still, wander, follow

Every setting of a sound has a base value, and any of these can be added on top. They
stack; they are not alternatives (decided 2026-11-20). Each one is a component on the
entity, which is what the entity-component idea is for.

- **Still**: the base value alone.
- **Wanders**: a range and a pace. The value drifts inside the range and never settles into
  a regular swing.
- **Follows**: tied to a condition, with an amount. Wind strength follows *wind*. Bird
  activity follows *daylight* and falls with *rain*.
- **Varies per play**: a small random offset each time the sound starts.

A blackbird's level can be 62 dB, fall as the rain picks up, drift a little over minutes,
and still differ slightly on every phrase.

This is one idea instead of a patch panel. Presets ship with their links, so a blackbird
from the Library already sings more at dawn and stops in the rain without anyone wiring
it. The field wind already works this way inside: its speed wanders between a minimum and
a maximum. The rule gives every sound what the wind has.

### Weather is a story

Something moves the conditions. A scene picks one:

- **Held**: you set them by hand.
- **Drifting**: they wander inside bands you set. A meadow on a fair day is a narrow band:
  wind low, rain none.
- **A passing storm**: stillness, gust front, first drops, downpour with close thunder,
  easing, dripping, birds back. A different length every time, returning now and then.
- Later: following the real clock and the real weather outside.

```text
          still   gusts downpour  easing  after rain
minutes   0       8     14        24      32         45
wind      ▁▁▁▁▂▂▂▃▄▅▇██▇▇▇███▇▇▆▆▅▅▅▄▄▃▃▃▂▂▂▂▂▂▂▂▁▁▁▁▁▁
rain      ▁▁▁▁▁▁▁▁▁▁▁▁▃▄▆▇██████▇▇▆▅▅▄▄▃▃▂▂▁▁▁▁▁▁▁▁▁▁▁▁
thunder        ·   ·  • ● ●●  ●  •   ·     ·      ·
birds     ██████▇▅▄▃▂▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▂▃▃▄▅▆▆▇███████
dripping  ▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▂▃▃▄▅▅▆▇▇██▇▇▇▆▆▅▄▃▃▂▂▁▁
```

In a place nobody draws these curves. The storm moves three conditions, and wind, rain,
thunder, birds and dripping each follow them.

### The session: decisions planned ahead

The engine does not decide each sound at the moment it plays. It plans ahead, the way a
video player buffers, and writes the plan down. The written plan is the **session**.

A session holds decisions, not sound:

- the scene it started from, and the seed;
- the conditions as curves over the coming hours;
- the events for the coming minutes: which take, when, how loud, at what pitch, where. A
  generated event such as a thunderclap carries its own seed.

Sound itself is not buffered. Every device renders its own sound from the same session, so
the speaker block and each headphone listener hear the same blackbird phrase at the same
moment, each from their own position. Buffered sound would fix where the listener stands.

The plan is made in pieces. Before one runs out the next is made. Each piece starts with
the full state (the conditions, where each bird is in its bout), so a device that joins
late needs only the piece now playing.

What this gives:

- **One change, heard by everyone.** A change replaces the plan from a moment ahead. Sounds
  already playing finish. Every device on the session gets the new plan.
- **Shared and personal stay apart.** The session is what the place does. Where a listener
  stands and faces, their volume and their output are their own and never in it.
- **A session can be kept**, and the same half hour played again.
- **Pause is stopping the session's clock.** Events are timed on that clock, not on the
  time of day, so a pause needs no new plan and every device on the session pauses
  together. Resuming, jumping and joining late all need the same thing: a long sound, such
  as a narrator, has to be able to start partway through. A place that follows the real
  clock is silenced instead of paused, because its day goes on.
- **The editor draws the plan.** The planned half of the activity strip is the session.
  Time-lapse and the day view are the plan made without playing it.
- **The plan is testable without a sound device.** Planning is a pure function of scene,
  seed, state and time window. The audio thread only starts what the plan says, when it
  says.

It runs in the engine because the engine plays with no app open, and a timer in the app
would stop the place when the app closes.

Built 2026-11-21 for voices with takes (`src/plan/`): a scene seed (`seed:`, or a new one
each play), a clock the audio thread counts in samples, and a plan per voice decided from the
seed and the entity's id. A planner thread hands the audio thread only the next 0.4 s, so an
edit is heard almost at once. Not yet: generators are not planned (each still has its own
seed), a session cannot be kept, and there is no link between devices. Pause (24) and moving the
scene to any moment are built: the plan replays each voice from its seed up to that moment.

### A place or a piece

A scene is one of two kinds.

- **A place** has no end. Its plan is generated ahead, as above.
- **A piece** has a set length, 20 minutes say, and is composed, not generated. Its plan is
  written by hand and is part of the scene file. It plays once or loops.

Composing a piece does not mean placing every bird call. There are three levels:

- **Draw the conditions**: the wind, rain and storm curves over the length of the piece.
- **Lay blocks**: the blackbird sings from 2:00 to 5:30. Inside a block the takes and gaps
  still come from the repertoire, fixed by the seed, so the piece plays the same every
  time.
- **Place single events** where the moment matters: the thunderclap at 12:30, the bell at
  the end.

A piece uses the same sounds, conditions and plan format as a place. A kept session of a
place can therefore be opened as a piece and reworked.

### Editing something that takes half an hour

- **Grab the conditions.** Drag wind, rain or storm by hand and the place responds at
  once, so any moment of a storm can be heard without waiting for it.
- **Time-lapse.** The conditions run fast while sounds play at normal speed: a storm in a
  minute, a day in a few.
- **The strip zooms out**, from three minutes to a day. It is drawn from the schedule, so
  a dead hour or a too-regular frog shows without listening for an hour.

### What gives a loop away

| You notice | Remedy |
|---|---|
| The same recording again | Several takes, varied or in bouts |
| A bed that comes round (stream, crickets) | A generator where one exists. Otherwise a level that wanders, and two recordings of different length under each other |
| Even spacing | Gaps from a range, grouped in bouts |
| The same loudness every time | A small offset per take |
| Always the same spot | Perches and wandering, under Moves |
| Everything changing by itself | Shared conditions |
| The whole scene coming round | A place has no timeline, and its weather stories differ in length every time. A piece does come round when it loops |

### Beyond nature

The same three blocks reach past nature, toward sound made for a purpose in the way of
Endel and brain.fm. None of this is first.

- **A monastery.** `environments/cathedral.yaml` is already a stone interior with a
  five-second tail. Chant is a repertoire sung in bouts. The bell follows the real clock.
  Footsteps are a path. It needs recordings, not a new model.
- **Focus tones.** A tone generator with a pitch (333 Hz, say), overtones, and a slow
  shimmer that wanders. The engine has no tone generator. A pattern can choose the pitch
  from a set, such as the nine "solfeggio" values (174 to 963 Hz), and give each tone its
  own place around the listener. Only two pairs in that set form a clean interval (396 with
  528, and 639 with 852, both a fourth), so a pattern should play one tone at a time or
  those pairs. 432 Hz is a tuning, not a tone: the same music played a third of a semitone
  lower. The health claims made for these numbers are not established, so the app names
  them by pitch and makes no claim.
- **A purpose** such as focus, relax or sleep is one more condition, say *energy*, that
  layers follow the way they follow wind. Sleep is a slow story that takes it down over an
  hour.
- **Music in space**, which YouTube calls 8D audio: a track that circles the listener on
  headphones. The engine has the core of it: a sound that orbits the listener, the
  headphone render mode, and head tracking in the engine binary. Two things are missing
  for music. A stereo file is mixed down to one point, so a track loses its width. And the
  better form needs stems: each instrument is its own sound with its own place and
  movement, composed as a piece.
- **Patterns, in the way of TidalCycles.** A short text pattern that repeats in cycles and
  can drive anything: which sound, when, how loud, and here also where. In TidalCycles a
  pattern is a function that is asked "what happens between these two moments?", which is
  how the session's plan is made piece by piece. So a pattern is a third way to make a plan,
  next to a generated place and a composed piece. It fits music and rhythm; a place should
  not sound like a cycle.
- **A storybook.** A piece with a narrator. The voice is a sound like any other, and the
  wolf, the door and the storm are events timed to it. The events are tied to markers in
  the narration, not to the clock, so a re-recorded voice does not shift them. Told in
  pages, each page is a short piece that settles into a place and waits there until the
  page is turned. It needs pieces, play, pause and jump, and a way to move from one scene
  to the next.

## The scene editor

A scene is a document. The map is the canvas, sounds are layers, and the inspector shows
whatever is selected. The three-panel layout is built (2026-11-20): layers and library on
the left, a top-down map of every sound in the middle, the inspector on the right. It
replaced the composer, which showed one sound at a time on a tilted grid.

Decided on 2026-11-20:

- **It looks like the player.** Black and white in the paper style, following the Auto,
  Light and Dark setting. The only colour is each sound's own ink colour, and Monochrome
  turns that off too. The dark palette of the mockup is not used.
- **The layer list works like a picture editor's**, with an ear where that has an eye: it
  silences its own layer. There is no solo; an `S` that muted the others was tried and read
  as the opposite of what it did. The actions sit in a bar under the list: add, delete.
- **On the Mac, Save writes the scene back into the project's `scenes/` folder.** A saved
  file is rewritten from the engine's values and loses its comments. So the first save of
  a hand-written scene asks for a new name and leaves the original alone. After that, Save
  overwrites the app's own copy. A phone has no project folder and offers no Save.
- **A sound is added from the Library tab** next to Layers: the presets in `sources/`,
  dragged onto the map. Pressing a preset plays it at the listener, so it can be heard
  before it is placed.
- **The model comes first.** A sound is an entity with behaviour. The basics are modelled
  and built before anything else: the sound model below, then layers, audio and library.
- **The editor comes before the birds.** The shell first, then scenes as documents, so
  every later feature has a place to land.

### The sound model

A sound in a scene is an entity: one thing with a name that keeps its identity while
everything about it changes. It has five parts, the same five the inspector shows.
Version 1 is built (2026-11-20) as the crate `atrium-world`, and every scene file is
written in its layout. Its design record is `crates/world/README.md`.

| Part | Holds | In a scene file |
|---|---|---|
| Identity | A stable id, a name, a colour, a group | the key of the entry, `name`, `color`, `group` |
| Sound | The preset it starts from, what it overrides, its level | `sound: { preset, audio, level }` |
| Shape | Ambient or a beam of any width with some sound behind it, and a size | `shape: { width, behind, size, facing }`. Ambient is fixed per generator |
| Place | Position and height | `place: [x, y, z]` |
| When | A behaviour: always, now and then, in bouts | **nothing** |
| Moves | A behaviour: still, orbit, path, wanders, perches | `moves: { orbit: { radius, speed } }` |

This is the entity-component-system idea: the entity is the id, the parts are its
components, and the systems are the engine code that carries each behaviour out, one
system per behaviour. A part can be absent: no When means always, no Moves means still.
It is the shape that is taken, not a library. A scene has at most 16 voices and a fixed
set of parts, so plain structs and one small function per behaviour do the job.

Three rules keep it small:

- **A behaviour is data, not code.** When and Moves are each one choice from a short list,
  with a few numbers. The engine carries them out. A new behaviour is a new entry in the
  list, and the shape of the model does not change.
- **The basics are the entries that exist.** When starts with "always" only, and Moves
  with "still" and "orbit". The model already has the two slots, so repertoire, perches
  and the rest arrive later without touching the layers, the library or the saved files.
- **An entity is not a voice.** The scene describes the sound; the engine gives it one of
  its 16 voices while it plays. The id is what the editor, undo, the saved file and later
  the session's events all point at, so it must not change when a slot does.

```text
┌──────────────────────────────────────────────────────────────────────────────────┐
│ ATRIUM  Meadow at dusk · edited  [Undo] [Save as…]   7/16 voices  5.1  [Player]  │
├───┬──────────────────┬──────────────────────────────────────┬────────────────────┤
│ V │ Layers │ Library │                                      │ FROGS              │
│ A │ ▾ Weather        │     ░░░░ wind (ambient) ░░░░         │ ▸ Sound   68 dB    │
│ P │   ● Wind      S  │                                      │ ▸ Shape   Omni     │
│ L │   ○ Rain      S  │  ● birds       ┌─◉───◉───◉─┐         │ ▸ Place   12 m     │
│ M │ ▾ Water          │                │   block   │         │ ▸ When    Always   │
│   │   ● Stream    S  │   ● frogs      │     ▲     │         │ ▸ Moves   Still    │
│   │ ▾ Creatures      │                └─◉───────◉─┘         │                    │
│   │   ● Frogs     S  │                      ~ stream ~      │                    │
│   │   ● Birds     S  │ [Top-down│Tilted]     10 m ├───┤     │                    │
├───┴──────────────────┴──────────────────────────────────────┴────────────────────┤
│ activity   last 2 min, as heard ───────┤now├─── next 45 s, as planned   ▮▮▮▮▮▮   │
└──────────────────────────────────────────────────────────────────────────────────┘
```

| Area | Holds |
|---|---|
| Top bar | Scene name, an "edited" mark, Undo, Save as, the voice counter (the engine has 16 slots), the output, the way back to the player |
| Tool rail | Select, Add, Path, Listen from here, Measure |
| Left panel | Layers and Library tabs |
| Map | The world, the block, the sounds |
| Inspector | The selected sound, speaker or block. With nothing selected, the scene itself |
| Activity strip | What sounded when, and a small meter row per output channel |

### Five questions per sound

The inspector shows the same five sections for every sound. Each section shows one to three
controls and keeps the rest folded behind it.

| Section | You choose | On the map | In the engine today |
|---|---|---|---|
| **Sound**: what is it? | A recording with one or more takes, or one of nine generators: four winds, two rains, river, waves, wisp. Its level in dB at 1 m. | Colour and name | Level can change while playing. The binding has no call for it. **A recording is one file.** |
| **Shape**: how does it fill the air? | **Ambient**: everywhere, optionally from one side. **Omni**: a point, equal all round. **Focused**: a beam with a facing and a width. Plus a size, from a point to wide. | A wash, rings, or a cone you turn and widen by hand | Omni, focused and size can change while playing; the binding has no call. **Ambient is fixed per generator, and only the four winds are.** |
| **Place**: where? | Position and height | Drag it. A dashed line gives distance and bearing to the listener. | Works: drag a sound on the editor's map. |
| **When**: how often? | Always. Now and then, with a gap range. In bouts. And when it is active: at dawn, in dry weather. | The activity strip | **Missing.** Every recording loops without a pause. |
| **Moves**: does it travel? | Still. Orbit. Along a drawn path. Wanders in a territory. Hops between perches. | A circle, a line, a territory | Orbit can change while playing; the binding has no call. The wisp wanders by itself. **Paths and perches are missing.** |

### Generator settings at three depths

Wind, rain and water are layers like any other. Their settings sit under Sound. The field
wind has 19 values: too many to make it windier, and all needed when matching a recording.
So every generator panel has the same three depths.

| Depth | Field wind | Shows |
|---|---|---|
| Simple | 2 | One or two dials and a few named starting points: Strength, Gustiness, and Light air, Breeze, Fresh, Storm |
| Detailed | 7 | The same generator in groups: Speed, Rhythm, Gusts, Tone, Turbulence. Ranges and curves instead of single numbers |
| All | 19 | Every value under its real name, as in the sound file. For tuning sessions |

Rules that keep the panels small:

- **A minimum and a maximum are one slider with two handles.** The wind has four such
  pairs, so eight values become four controls. Two handles always mean the same thing: the
  value wanders between them.
- **A setting that follows a condition shows a chip**, such as "follows wind", with its
  amount. Any slider can be still, wander or follow.
- **Band levels are one curve.** The wind's low, body, mid, presence and air levels are one
  shape you pull.
- **A generator's settings are sorted under the question they answer.** The wisp's
  territory, heights and resting time sit under Moves. Its wing and hum levels sit under
  Sound.
- **A sound starts from a preset and remembers what you changed.** A changed value gets a
  dot and a reset. The scene file already works this way: an entry points at a file in
  `sources/` and overrides single values.
- **The engine publishes each generator's settings**: name, group, unit, range, default,
  and whether it can change while playing. The inspector is drawn from that list, so a new
  generator gets its panel without new Kotlin.

### Nothing selected: the scene

Click empty ground and the inspector shows the place itself.

- **World**: the environment and its size.
- **Weather**: the conditions (wind, rain, storm, daylight, temperature, humidity) and the
  story that moves them: held, drifting, or a passing storm. Every layer that follows a
  condition shows the link, so nothing happens that you cannot see.
- **Block**: its size and position, and the speakers on its edge.
- **Output**: render mode, speaker set, volume.

### Layers and Library

- **Layers** lists every sound in the scene in groups (Weather, Water, Creatures, Music).
  Each row has an ear that silences it and a small live level. A group has an ear of its
  own. Turning a whole group down is not built.
- **Library** lists the presets in `sources/`. Drag one onto the map and it lands with the
  level, shape and size from its preset.

### The map

| Element | What it shows | What you can do |
|---|---|---|
| Sounds | Drawn by shape: ambient as a wash, omni as rings, focused as a cone. Height is a small label, not a third dimension. | Drag to move, click to select. The ring is the size. The cone's edges are its width and its tip is its facing. |
| Movement | Orbit as a dashed circle, a path as a line, a territory as an area | Drag the orbit as a whole. Draw a path with the Path tool. |
| The block | A rectangle with the speakers on its edge and the listener inside | **Drag the whole block through the world** |
| Speakers | Each speaker pulses with its own output level | Click to see which sounds arrive there |
| Listener | Position and facing, with distance rings at 5, 10 and 20 m | Drag, rotate |
| Loudness layer | Optional coloured ground layer: total level at every point, or for the selected sound alone | Toggle |

Dragging the block is the central gesture. It is what "an open block placed anywhere in a
space" looks like as an interaction: pull the block toward the stream and the stream gets
loud in the front speakers.

Settings that have a shape are changed on the shape. The inspector shows the same numbers
for when you need to type one.

| Tool | Key | Does |
|---|---|---|
| Select | `V` | Click to select, drag to move. Shift-click adds to the selection. Alt-drag makes a copy, which is how one frog becomes three. |
| Add | `A` | Stamps the sound picked in the Library. Click several times for several. |
| Path | `P` | Click points on the map. The selected sound follows the line. |
| Listen | `L` | Hold on any spot to hear the place from there. Let go and you are back in the block. |
| Measure | `M` | Distance and bearing between two points. |

- **Height** is a number under Place. In the tilted view it is a stalk you pull up.
- **Compare**: hold `S` to hear the selection alone. Hold `\` to hear the scene as last
  saved.
- **Undo**: one gesture is one step, including a whole drag.

### The activity strip

Under the map: the last two minutes as they were heard and the next 45 seconds as planned.
The planned half is the session, and it changes when you change the scene.

Built 2026-11-21 as **Tracks**, the heard half only: all 16 voices as tracks, the way a
multitrack editor shows them, each with the sound in it or "Empty", and clips where it sounded,
named after the take and outlined by the voice's own level. It zooms to 30 s, 2 min or 10 min.
It is recorded in the app from telemetry, so it starts empty when a scene starts. Right of
"now" it shows the takes the plan has already decided, drawn open (feature 23), and above
the voices the four conditions as lanes, past and future, read from the weather story.

For a place the strip is not a timeline. A place has no start and no end, so there is no
playhead to scrub, and the strip only makes rhythm visible. For a piece it is one: the
length is fixed, and its condition curves, blocks and single events are edited on it.

The strip zooms out to a whole day, with the conditions drawn above the layers.

### Where each thing lives

| You want to | Go to |
|---|---|
| Add a sound | Library tab. Drag it onto the map. |
| Change one sound | Select it. The inspector shows its five sections. |
| Tune wind, rain or water | Select the layer, open Sound, pick a depth. |
| Change the place itself | Click empty ground. The inspector shows world, weather, block, output. |
| Make everything windier or stormier | Weather in the scene inspector. |
| Stop a sound repeating | Its takes under Sound, its gaps under When. |
| Hear how a storm will go | Time-lapse, or drag the conditions by hand. |
| Quieten a family, or hear one alone | The layer groups. |
| See what sounds when | The activity strip. |

### Bottom drawer: diagnostics

Closed by default. Spectrum per output channel, per-speaker breakdown, the perceptual
score. For tuning sessions only.

## The player and the phone

The player (Home, Scenes, Settings) is the face for living with a place, and it exists. It
builds for iOS, but there is no iOS host app yet.

A phone running the app runs its own engine, because the binding runs the engine in the
app's own process. To hear the same place as the NUC, or to change it, the phone needs a
link to it. What the link carries is the session and a shared clock. The link itself is
not designed yet.

## Idea pool

Unsorted on purpose. Mark each one **keep**, **later** or **no**; add your own. Ideas
already described above are not repeated.

### The world

- Density: "more birds, fewer frogs" as one slider per kind instead of placing each one.
- Cut a long recording into takes at its silences, so one field recording becomes a
  repertoire.
- Birds that answer each other: a take from one layer prompts a take from another.

### Time and weather

- Follow the real clock, so the place has the same time as the building.
- Follow the real weather outside: wind and rain in the place match the day.
- Seasons as a slower dial on top of time of day.

### The block and the people in it

- Rotate the block, not only move it.
- Bookmarks: named spots for the block ("by the stream", "under the trees").
- A journey: the block drifts slowly along a path through the world over an hour.
- Headphone listeners drawn as heads on the map, each with their own position and facing,
  with a head-tracking indicator.
- Speaker check: play a sound from one speaker at a time to confirm wiring and placement.

### Understanding what you hear

- "Why can't I hear it?": select a sound and see whether it is too far, muted, or drowned
  out by another sound. The engine computes a perceptual score per sound; the binding does
  not report it yet.
- Compare render modes: a quick A/B switch that keeps everything else the same.
- Clipping and loudness warning on the top bar.

### Places

- Crossfade between places instead of a hard switch.
- Snapshots: save the current state under a name and return to it.
- Schedule: forest in the morning, sea in the evening.
- Sleep timer and a slow fade-out.
- Quiet hours: a volume ceiling at night.

### Using it

- Keyboard nudges for precise placement.
- A wall-tablet view: the map with no controls, just showing the place that is playing.

## What exists and what each part needs

The binding sends the engine's own `Command`s, so anything the engine has a command for is
one binding method away. Bold marks what is missing.

| Part | Engine | Binding (`crates/ffi`) | App |
|---|---|---|---|
| Pick a scene and play it | builds any file in `scenes/` | `Engine::new` | exists: the player's Scenes tab |
| Output and volume | `SetRenderMode`, `SetChannelMode`, `SetMasterGain` | `set_output`, `set_volume` | exists: Settings |
| Move a sound | `SetSourcePosition` | `set_source_position` | exists: drag any sound on the editor's map. `scene()` remembers where it was put |
| Every sound on one map | scene description | `scene()` reports each sound's position, level, ambient or not, beam width and facing | exists: the editor's map |
| Levels | telemetry: per sound, and a peak per output channel | `latest_telemetry` | each layer and each sound on the map shows how strongly it reaches the listener. Channel peaks are not drawn |
| Speakers and the block | the scene has a speaker layout | **not in the scene description** | new (6) |
| Mute | `SetSourceMuted` | `set_source_muted`; telemetry reports `muted` | exists: the editor's layers |
| Level | `SetSourceSpl` | `set_source_level` | exists: the selected layer |
| Width and what is heard behind | `SetSourceDirectivity` with a `Beam` | `set_source_beam` | exists: the inspector, with Omni, Focused and Narrow as starting points and a drawing of the beam |
| Size | `SetSourceSpread` | **no call.** Size is not in the description | new (10) |
| Orbit | `SetSourceOrbitSpeed`, `SetSourceOrbitRadius`, `SetSourceOrbitAngle` | **no call.** Orbit is not in the description | new (33) |
| Temperature and humidity | `SetAtmosphere` | **no call** | new (39) |
| Listener position and facing | `SetListenerPose` | **no call** | new (39) |
| Head tracking | listens for OpenTrack packets and can start the Sony bridge. Telemetry carries the listener's heading | `head_trackers`, `set_head_tracker`; telemetry reports the facing and whether a tracker is sending | exists on desktop: Settings switches it and says whether head movement arrives; the 3D map draws where the listener faces. Seen working with the Sony headphones on 2026-11-20. The Sony bridge is a Mac script, so iPhone needs its own source of head motion |
| Move a speaker | `SetSpeakerPosition` | **no call** | new |
| Generator settings while playing | `SetSynthParam` reaches every generator: the winds and the river, rain intensity, drop and drip rate, the waves' period and crash chance, the wisp's speed and hovering, and every master gain | **no call, and no list of a generator's settings** | new (27) |
| Ambient or point | **fixed per generator**: only the four winds are ambient | reported | drawn. Any sound ambient is 11 |
| Add a sound, hear a preset first | `SceneEdit` add, and a builder kept from the scene build | `list_presets`, `audition`, `add_sound` | exists: the editor's library |
| Remove a sound | `SceneEdit` remove | `remove_sound`; each sound in `scene()` carries its voice | exists: the inspector |
| Turn a sound | `SetSourceFacing`; a scene file's `shape.facing` | `set_source_facing` | exists: a handle on the map for a focused sound |
| Save a scene | `SceneConfig::save`, which refuses a hand-written file | `save_scene`; every edit is recorded in the scene file the engine keeps | exists: Save and Save as in the editor's bar, ⌘S |
| Several takes per sound | a preset lists `takes`; four bird kinds use 19 of the 20 on disk | `scene()` names them, telemetry says which is sung | exists: the inspector lists them, the map dims a resting bird |
| When: gaps and chances | a `rests` rule with a gap range, bouts and level and pitch variation; a scene's `when:` changes them per sound | `set_source_gap`, `set_source_bout`, `set_take_chance`, `audition_take` | exists: the inspector's When, with bouts and a play button per take |
| The session: a plan made ahead, kept and shared | `src/plan/`: a seed, a sample clock, and a plan per voice with takes, handed to the audio thread 0.4 s ahead. **Not kept, not shared, no pause; generators unplanned** | rhythm calls change the plan | the tracks show what was heard (23 would add what is planned) |
| A piece: a composed scene of set length | **nothing** | | new (41) |
| A setting that wanders | **nothing** at the scene level. The winds and the river vary inside themselves | | new (28) |
| Conditions, and settings that follow them | `src/plan/`: wind, rain, storm and daylight; `follows:` on kinds and sounds ties activity, level or a generator setting to one | `weather`, `set_weather`, `conditions`; each sound's `follows` | the scene inspector's Weather, a sound's Follows, and condition lanes over the tracks |
| Weather stories | held, drifting and a passing storm, pure functions of time and seed; `weather:` in a scene | `set_weather` | the story picker in the scene inspector |
| Thunder | `src/synth/thunder.rs`, ported from the spatial project: a strike and a rolling rumble with an infra layer for the LFE, never the same clap twice. Its distances are its takes, and the plan strikes it as storm rises | through `set_weather` | `sources/thunder.yaml`, `scenes/passing-storm.yaml` |
| Paths, wandering, perches | **nothing** beyond orbit and the wisp's own flight | | new (33) |
| Tone generator | **nothing** | | new (43) |
| Group level | **nothing** | | new (36) |
| Drag the block | **no command.** Speakers, listener and block rectangle must move together in one step | | new (37) |
| Loudness layer | **nothing yet.** Needs a pure function over the scene and a coarse grid | | new (38) |
| What arrives at a speaker | **nothing yet.** Telemetry has per-sound level at the listener and per-channel peaks, not per-sound-per-speaker | | new |
| Spectrum per output channel | **nothing yet** | | new |

## Order of work

Rewritten 2026-11-21 as one numbered list, replacing the two tracks (A1–A6, E0–E6). The
features keep their numbers when the order changes, so they can be named in conversation.
Size: **S** is a day or less, **M** a few days, **L** a week or more. "Needs" names what has
to exist first.

**Built so far.** The sound model and its crate, the three-panel editor (layers with an ear
and a live meter, library with play and add, one top-down map, the inspector), remove, move,
turn, mute, level, omni or focused, head tracking on desktop, and kinds with takes that sing
with gaps from a range and a small level and pitch offset.

### Suggested order

1. **1, then 14, then 9.** Save first, so tuning chance, gaps and width is not lost.
2. **15, 16, 17.** Small, and they make the birds visibly and audibly alive.
3. **21, then 22.** The session. Most of groups 4, 5 and 7 build on it.
4. **26, then 29, then 30 and 31.** Weather that has a cause, with thunder.
5. Then pieces, the storybook and the tone generator, in whichever order matters most.

The small ones (3–8, 10–13, 36, 39) fill the gaps between the big steps.

### 1. Make editing stick

Without these, nothing done in the editor survives a restart.

1. **Save a scene** (Mac). The first save of a hand-written scene asks for a new name; added
   and removed sounds are kept in the scene's entities. **M**
2. **Undo and redo.** **M**, needs 1
3. **Drop a sound where it is let go** when dragged from the library, instead of just ahead
   of the listener. **S**
4. **Duplicate a sound.** **S**
5. **Rename, recolour and regroup** a sound in the inspector. **S**, needs 1
6. **Speakers and the block on the map.** The scene description has to report them. **S–M**
7. **Zoom and pan the map.** **S**
8. **Arrow keys nudge** the selected sound. **S**

### 2. Shape and reach of an entity

9. **Width in degrees plus how much is heard behind**, replacing Omni, Focused and Narrow
   (see `entity-idea.md`, "Shape and reach"). **M**, engine, binding and app
10. **Size settable.** The engine has `SetSourceSpread`. **S**
11. **Any sound can be ambient**: rain, a recorded forest bed. **M**
12. **Each kind comes with its own shape and reach.** **S**, needs 9
13. **Show what an individual changed**, with a way back to the kind's value. **S**, needs 1

### 3. When an entity sounds

14. **Chance and gaps editable** in the inspector. **S**, needs 1 for the values to be kept
15. **A button per take** to hear that take on its own. **S**
16. **Bouts**: the same take a few times, then a switch. **S–M**
17. **The activity strip**, first as the last two minutes, each phrase a mark. **M**
18. **Cut a long recording into takes** at its silences. **M**
19. **More bird takes.** The great tit has one, and `bird_1` and `bird_2` are unnamed.
    Recordings, not code.
20. **Registers**: song, call and alarm as separate sets of takes. **M**, later

### 4. Planning ahead: the session

The base for pause, sharing between devices, pieces and the storybook.

21. **A scene seed and a clock** in the engine. **M**
22. **Phrases and rests decided ahead of time**, outside the audio thread, as a pure
    function of scene, seed, state and time window. **L**, needs 21
23. **The strip shows the next 45 seconds as planned.** **S**, needs 17 and 22
24. **Pause stops the session's clock**, and a long sound can start partway through. **M**,
    needs 22
25. **Keep a session** and play the same half hour again. **S**, needs 22

### 5. A living place

26. **Rain, waves and the wisp accept changes while playing.** Today only the four winds and
    the river do. **M**
27. **Generator panels** at three depths. **M**, needs 26
28. **Wander**: a setting drifts inside a range. **M**, needs 26
29. **Conditions**: wind, rain, storm and daylight, and sounds that follow them. **L**,
    needs 22
30. **Weather stories**: held, drifting, a passing storm. **M**, needs 29
31. **Thunder**, ported from the spatial project. **M**
32. **Time-lapse, and dragging the conditions by hand.** **M**, needs 30
33. **Moves**: perches, wandering, paths, and orbit editable in the inspector. **L**
34. **Entities answer each other**: the robin answers the blackbird. **M**, needs 22
35. **More creatures than voices**: a resting entity gives its voice back. **L**

### 6. The whole place

36. **Group level**: all the birds louder or softer together. **S**
37. **Move the block, speakers and listener together**, in one command. **M**
38. **The loudness layer** on the map. **M**
39. **Listener position, temperature and humidity** in the scene inspector. **S**
40. **"Why can't I hear it?"**: too far, muted, or drowned out. The engine has a perceptual
    score the binding does not report. **S–M**

### 7. Beyond nature, and the player

41. **Pieces**: a set length with drawn conditions, blocks and single events. **L**, needs 1,
    22 and 29
42. **The storybook**: a narrator, events tied to markers in the narration, pages. **L**,
    needs 41
43. **A tone generator** for focus tones, named by pitch. **M**
44. **Patterns** in the way of TidalCycles, and **music in space** with stereo width and
    stems. **L** each
45. **An iOS host app**, **head tracking on iPhone**, and **the link between devices** that
    shares a session and a clock. **M**, **M**, **L**
46. **Player extras**: crossfade between places, sleep timer, quiet hours, schedule,
    wall-tablet view, speaker check. **S** each

Not numbered yet: what arrives at a speaker, the diagnostics drawer, and the rest of the
idea pool.

## Open decisions

From the editor concept, each with a suggestion:

- **Top-down or tilted for editing?** Built top-down, because distances and angles read
  true; height is written under a sound's name. A tilted view as a switch is still open.
- **Where does aliveness run?** Takes, gaps, wandering, conditions and weather stories.
  Suggested: all of it in the engine, planned ahead into a session.
- **How far ahead is the plan?** Far enough to share and to draw, short enough that a
  change is heard soon. Suggested: conditions for a day, events for a few minutes, and a
  change lands about a second ahead. Moving a sound or changing its level stays immediate.
- **How is a piece composed?** A scene of set length is composed, not generated (decided
  2026-11-20). Open is how much is done by hand. Suggested: draw the conditions, lay
  blocks, place single events, and let the repertoire fill the inside of a block.
- **Who may change a shared session?** Suggested: one device owns it, the NUC in the room,
  and the others ask it. Whether a listener may change the place is a setting of the
  session.
- **Which conditions to start with?** Suggested: wind, rain, storm and daylight, next to
  the temperature and humidity the engine has. Energy later.
- **May any sound be ambient?** Today only the four winds are. Rain and waves play from a
  point. Suggested: a choice per layer, with rain ambient by default. A recorded forest bed
  is the obvious first use.
- **Is weather a master over the layers?** Suggested: yes. The conditions drive every
  layer that follows them. Each link shows on its layer and can be cut. The rain and wave
  generators have to accept changes while playing first.
- **Three frogs: three layers, or one layer with a count?** Each layer costs one of the 16
  voices. Suggested: start with copies. Add a count with scatter later if voices run short.
- **The roadmap's home for scheduling, motion and weather is gone.**
  `living-atrium-roadmap.md` plans thunder, triggering with a repertoire, a call scheduler
  and a weather state, which is groups 3 to 5 of the order of work. But it puts the scheduler,
  motion and weather in the behaviour crate and the Bevy editor, both removed on
  2026-11-17, and it would delete the engine's orbit in their favour. Suggested: the
  engine takes all three, as this plan assumes, and the roadmap is rewritten to match.

Carried over:

- **Is the block allowed to leave the listener behind?** Moving the block could carry the
  listener with it (simple, proposed here) or leave them independent.
- **Loudness layer: at which height and for which listener?** Proposed: ear height, total
  of all unmuted sounds, direct sound only. Reverb and ground effect are left out at first
  and the legend says so.
- **How coarse is the layer?** A 1 m grid over a 60 × 60 m meadow is 3600 values; cheap to
  hand over on change, too much 15 times a second. Proposed: recompute only when a sound
  or the weather changes, and not while a drag is in progress.

## Not planned

- A free-camera 3D view. The tilted view is a fixed projection of the same map.
- Ray paths, impulse-response graphs, materials editing.
- A scene editor that works without the engine running.
