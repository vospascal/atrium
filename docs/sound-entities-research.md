# Sound entities: research

Status: 2026-11-20. Research for a crate of our own that models sounds as entities with
behaviour, inspired by Entity Component System designs. Nothing here is decided. The last
section lists what the definition has to settle.

How it was gathered: two surveys of primary documentation (ECS libraries; game-audio tools
and TidalCycles), plus this repository's own history. A claim without a source in the list
at the end is our own reasoning and is marked as such. Items the surveys could not confirm
are listed under "Not verified".

## What we are modelling

From `ui-design-plan.md`: a sound is an entity with an identity and five parts (Sound,
Shape, Place, When, Moves). When and Moves are behaviours. A few slow shared values, the
conditions, give changes a common cause. The engine plans ahead into a session instead of
deciding at the moment of playback.

Our constraints:

1. The world is saved to a plain file and read back.
2. Behaviour is repeatable from a seed.
3. Systems run ahead of real time, so the world's state must be snapshotted and restored.
4. The audio thread must not allocate, so the world lives on a control thread.
5. The Kotlin app reads it through generated bindings, so plain structs matter.

## What this repository already learned

The removed behaviour crate (last in commit `de1754f`, with `docs/behavior-ecs-headless.md`)
ran on Bevy's ECS. Four of its conclusions still hold:

- **The behaviour layer decides, the engine renders.** What, when and where at a slow rate
  on one side; everything per sample on the other.
- **An entity is linked to an engine voice by a separate link** that is rebuilt on load and
  never saved.
- **Every entity has its own seeded random source.**
- **Calls are triggered inside one voice that holds all its takes**, so a bird that calls
  every few seconds does not use up the 16 voices by being added and removed.

Two no longer fit: it ran once per tick and only knew "now", and it depended on all of Bevy.

## ECS designs

| Concept | Bevy | The small libraries |
|---|---|---|
| Entity | An id with an index and a generation; freed slots are reused | hecs: the same, with `spawn_at` to keep an id when loading |
| Component | A plain Rust struct | hecs: any type, no registration |
| System | A plain function; its parameters decide what it may touch | hecs has no systems at all: you query from ordinary code |
| Query | Iteration order is not guaranteed | hecs: also arbitrary order |
| Resource | A component that belongs to no entity | flecs: a singleton |
| Commands | Structural changes are queued and applied at fixed points | hecs: a command buffer |
| Order of systems | Parallel by default; order only where declared | flecs: fixed phases, and declaration order inside a phase |
| Change detection | Tracked per component, with false positives | not in hecs |
| Templates | Required components filled from defaults | flecs prefabs: shared values, per-entity overrides |

### What a tiny one can leave out

- **Table storage and storage choice.** They exist for fast iteration over many entities.
  hecs itself says an ECS "may be overkill" without batch processing.
- **A parallel executor.** It is the source of unpredictable order.
- **Tick-based change detection and observers.** Both have ordering and false-positive
  problems the libraries document.
- **Runtime reflection and a type registry.** A closed set of component structs with serde
  does not need them. *(our reasoning)*
- **Generic relationships.** They cost lookups and fragment storage.
- **Generations on ids.** Only needed after enormous numbers of spawns.

### What is worth copying

- **The file as rows: entity id, then component name, then data.** hecs recommends this
  form for data that people read and write. *(constraint 1)*
- **Component names chosen by us, not derived from types.** *(constraint 1)*
- **Prefab semantics from flecs.** The preset holds the values, an entity stores only what
  it overrides, and removing an override brings the preset's value back. This is our
  "starts from a preset and remembers what you changed". *(constraints 1 and 5)*
- **Fixed phases, and a fixed order inside each.** *(constraint 2)*
- **Sort before iterating wherever order can be heard.** *(constraint 2)*
- **Snapshot only registered state, with a checksum and a test that replays and compares.**
  This is how rollback netcode finds divergence. *(constraint 3)*
- **Apply structural changes at fixed points**, so a snapshot is never taken halfway through
  a change. *(constraints 3 and 4, our reasoning)*
- **The seed and the clock are part of the world**, so they are snapshotted like everything
  else. *(constraints 2 and 3)*

### Traps the documentation warns about

- **Hidden state is not restored.** Anything a system keeps for itself outside the world
  causes silent divergence.
- **Events sent in a frame that is later redone are not taken back.** Our session has the
  same problem when an edit replaces the plan.
- **Ids can change when a world is restored.**
- **Unordered systems only appear correct.** Bevy ships a shuffle option just to expose
  missing ordering.

## Game-audio tools

Wwise and FMOD solve our actual problem under other names.

| Ours | Theirs | Settings they have |
|---|---|---|
| Repertoire | Wwise Random Container; FMOD Multi Instrument and Scatterer; Godot AudioStreamRandomizer | Shuffle (every take once before any repeats) or random with "avoid the last N"; a weight per take; a delay or spawn interval from a range; volume and pitch randomised per play; a chance that a play happens at all; a polyphony limit and what to steal; a play count |
| Conditions | Wwise Game Parameter; FMOD parameter | A range and a default; smoothing by slew rate or by time, with separate rise and fall; built-in ones per sound such as distance and direction to the listener |
| A discrete mood | Wwise State | A default transition time, and a table of times per pair of states |
| Follows | Wwise RTPC curve; FMOD automation curve | A curve with control points from the parameter to the property; several curves on one property are combined |
| Wanders | LFO or noise modulator in both | Shape (including stepped and ramped noise), rate, depth, and a scope: per voice or shared |
| Moves | Wwise 3D automation paths | Paths of control points with a duration; a random range per point; paths played in sequence or at random; a new path picked when the sound starts. Their own example is seagulls circling a ship |

### Where they differ from what we wrote down

- **Follow, wander and per-play variation stack. They are not alternatives.** In both
  tools the final value is the base plus every curve plus every modulator plus the
  per-play random offset. Our "still, wanders or follows" as one choice of three cannot
  say "follows the wind, and wanders a little on top".
- **Conditions also choose content, not only scale a setting.** A Wwise Blend Container
  crossfades between sets of sounds along a parameter, and their example is farm sounds by
  time of day.
- **They have settings we had not listed:** weights, shuffle as distinct from
  avoid-last-N, an interval measured start to start (so plays can overlap) as distinct
  from a gap after the end, chance, polyphony with stealing, and whether the play history
  is shared between sounds or kept per sound.
- **We have one thing they do not: bouts.** No equivalent was found.
- **No named feature was found for orbit, wandering in a territory or hopping between
  perches.** The nearest to perches is several one-point paths played at random.

### Words

- **"Condition" means something else in FMOD**: a trigger condition that gates a sound.
  The industry word for our concept is "parameter". We keep "condition" in the app because
  it is the plainer word, and note the clash here.
- **"State" is a discrete global mood in Wwise.** Avoid it for anything else.
- **"Automation" is ambiguous** between the two tools. Avoid it.
- **Worth adopting:** shuffle and random, weight, polyphony and stealing, slew rate.

## Planning ahead

- **Wwise and FMOD decide at the moment of playback.** Their documentation says random
  values are generated when a sound is played. Wwise notes that a value computed at play
  time is "unknown at the moment the Play Action is posted", so it cannot drive a delay.
- **TidalCycles is the only tool surveyed that plans ahead.** A pattern is a function from
  a time span to events. Continuous patterns such as a sine have no events of their own
  and are sampled by a pattern that has. Its scheduler works in windows of 0.05 s and
  starts 0.3 s ahead.
- **What follows for us** *(our reasoning)*: planning ahead means the shuffle history moves
  forward at planning time, and a change to a condition cannot reach an event that is
  already planned. So the plan for events should be short, an edit has to be able to
  replace it from a given moment, and an event must be identifiable so it can be withdrawn.

## What the definition has to settle

1. **How a setting is built.** Decided 2026-11-20: a base value with follows, a wander and
   a per-play variation stacked on top, each as its own component.
0. **What the first version holds.** Decided 2026-11-20: an entity with an id, a name, a
   colour and a group; Sound, Shape and Place; Moves with "still" and "orbit"; reading and
   writing the scene file. It changes nothing you hear. When, wander, follow,
   vary-per-play and conditions are added right after, as further components.
2. **Which repertoire settings are in the first version of When.** At least: shuffle or avoid the
   last N, a gap range, level and pitch variation. Weights, chance, overlap and bouts could
   follow.
3. **Whether a gap is measured after the end of a take or from start to start.**
4. **Whether conditions can choose content**, such as a day set and a night set of takes.
5. **Whether there are discrete moods** next to the continuous conditions, with their own
   transition times.
6. **How far ahead events are planned, and what happens to planned events when the scene
   is edited.**
7. **What the saved file looks like.** Decided and built 2026-11-20: a mapping from id
   to entity, each part under its own key, and only what the scene changes written down.
8. **What the crate owns and what it does not.** Decided 2026-11-20: the sounds only.
   Speakers, listener and environment stay with the engine. Later the conditions, the
   seed and clock, and the planning. Never audio, voices, files or drawing.
9. **Its name.** `atrium-world`, in `crates/world`.

## Not verified

- Whether a hecs or Bevy world can simply be cloned.
- Whether TidalCycles' random values are repeatable per cycle. Its pages only say
  "(pseudo-)random".
- The scheduler numbers for TidalCycles come from a page that does not identify itself as
  official.
- Whether FMOD has position paths.
- specs and legion were not read; EnTT was the contrasting design.

## Sources

- Bevy ECS: <https://bevy.org/learn/quick-start/getting-started/ecs/>,
  <https://docs.rs/bevy_ecs/latest/bevy_ecs/> and its pages for `Commands`, `Query`,
  `Observer`, `Component`, the schedule and change-detection modules.
- hecs: <https://github.com/Ralith/hecs>, <https://docs.rs/hecs/latest/hecs/>, its `World`,
  `CommandBuffer` and `serialize::row` pages.
- flecs: <https://www.flecs.dev/flecs/PrefabsManual.html>,
  <https://www.flecs.dev/flecs/Relationships.html>,
  <https://www.flecs.dev/flecs/ComponentTraits.html>,
  <https://www.flecs.dev/flecs/Quickstart.html>.
- EnTT: <https://github.com/skypjack/entt/wiki/Entity-Component-System>.
- Rollback: <https://docs.rs/bevy_ggrs/latest/bevy_ggrs/> and the pitfalls document in
  <https://github.com/gschup/bevy_ggrs>.
- Wwise Help 2024.1 at <https://www.audiokinetic.com/en/public-library/>: random, sequence,
  switch and blend containers, randomizers, game parameters and RTPCs, states, LFOs,
  animation paths.
- FMOD Studio 2.03 at <https://www.fmod.com/docs/2.03/studio/>: instrument reference,
  parameters reference, modulator reference, authoring events.
- Godot: <https://docs.godotengine.org/en/stable/classes/class_audiostreamrandomizer.html>.
- Unreal:
  <https://dev.epicgames.com/documentation/en-us/unreal-engine/sound-cue-reference-for-unreal-engine>.
- TidalCycles: <https://tidalcycles.org/docs/innards/what_is_a_pattern/>,
  <https://tidalcycles.org/docs/reference/oscillators/>,
  <https://tidalcycles.org/docs/reference/randomness/>.
