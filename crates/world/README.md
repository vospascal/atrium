# atrium-world

The sounds of a scene as entities. An entity is one sound: an id, and the parts that say
what it is (`Sound`), how it fills the air (`Shape`), where it is (`Place`), when it sounds
(`When`), what it follows (`Follow`) and whether it travels (`Moves`). `Entities` is the set a scene holds, and is what the `sounds:` section
of a scene file reads into and writes from.

```yaml
sounds:
  frogs:
    name: Frogs
    color: "#6db33f"
    group: Creatures
    sound: { preset: sources/frog.yaml, level: 68 }
    shape: { width: 180, behind: -12, size: 0.2, facing: 225 }
    place: [10.1, 10.1, 0.0]
    when: { gap: [20, 45], bout: [2, 4], chance: { "Call 2": 3 } }
    moves:
      orbit: { radius: 1.5, speed: 1.0 }
```

Only `sound` and `place` are required. A part left out is the plain case: the preset's own
level and shape, standing still, and the id shown as the name.

The crate describes and does not play. It opens no file, decodes no audio and knows nothing
about voices or speakers. The engine (`src/config/`) resolves the preset and builds the
playing sound.

This is version 1, the foundation. It changes nothing you hear. The design it follows is in
`docs/ui-design-plan.md` ("The sound model") and the research behind it in
`docs/sound-entities-research.md`.

## Decisions that look wrong and are right

- **It is shaped like an Entity Component System and uses none.** The id is the entity, the
  parts are components, and the engine code that carries a behaviour out is a system. A
  scene has at most 16 voices and a closed set of parts, so plain structs do the job. The
  authors of a small Rust ECS say themselves that one "may be overkill" without batch
  processing of many entities. An earlier attempt in this repository used all of Bevy for
  two behaviours and was removed with it.
- **A sound is known by its id, not by its place in the list.** The engine still gives out
  its voices in file order, and a voice number changes when sounds are added or removed.
  An editor's selection, an undo step and later a timed event all need something that does
  not. Ids are 1 to 40 characters of `a-z`, `0-9` and `-`, so they are written bare as keys
  and never need quoting.
- **The file is a mapping keyed by id, and its order still matters.** A mapping makes two
  sounds with one id impossible to write by accident. The order is kept, because it decides
  which voice and which default colour and seed a sound gets. `Entities` reads the mapping
  in file order with its own visitor, so no ordered-map crate is needed.
- **A shape is a width and a level behind, not a named pattern.** Omni, cardioid and
  supercardioid were three points on one range, and most real sounds fall between them. A width
  of 180 with nothing behind is exactly the old cardioid, and 360 is omni, so nothing was lost.
  The names live on in the editor as starting points. `behind` is in dB because that is how
  an ear and a mixer think of "duller and softer from behind".
- **`when:` is not about birds.** It holds the rest between takes and each take's chance,
  which a bell, lines of a story or a tone have as much as a bird. A gap of `[0, 0]` is "all
  the time", so there is no separate switch for it, and a recording that loops can be given a
  gap to make it come and go. Chances are keyed by take name, not position, so a kind that
  gains a take does not shift what a scene said about the others. `bout` holds on to one take
  for a few utterances before choosing another, the way a great tit keeps one song a while.
- **`follows:` is names, not types.** A link says `condition: rain, setting: activity`. The
  crate does not know which conditions or settings exist; the engine checks the names when it
  builds the sound, against what the sound is (a recording has activity and level, a generator
  its own settings). New conditions or settings then need no change here. `follows:` on an
  entity replaces its preset's links; an empty list cuts them all. `over: [low, high]` limits a link to
  part of its condition, because nature is rarely a straight line: birds stop as soon as it
  really rains, not halfway through a downpour. Several links on one setting multiply, so a frog
  can like light rain and still stop in a downpour or a gale.
- **`moves:` is a struct with one optional key per way of moving, not a tagged enum.**
  serde_yaml writes an enum variant as a YAML tag (`!orbit`). A hand-written scene file
  should not need tags, and the crate should not depend on one YAML library's behaviour.
  `Moves` is an enum in Rust and converts through a private struct. A new way of moving is
  one more optional key there.
- **An unknown key is an error.** A misspelt `colour:` or an outdated `position:` would
  otherwise be read as "not set" and the scene would silently play differently. Every part
  refuses keys it does not know.
- **An override is `None` when the scene does not set it.** "68 dB because the preset says
  so" and "68 dB because this scene says so" are different facts: the first follows the
  preset when it changes. Writing a scene back therefore adds nothing that was not in it,
  and a test checks that.
- **The crate does not read the preset.** Whether `audio:` may replace a preset's sound
  depends on what is inside the preset file, so that check stays with the engine, which
  opens it. The crate only refuses a sound that names neither a preset nor a recording.
- **There is no reader for the layout before entities.** The 13 scenes in the repository
  were converted in the same change, and an engine test
  (`every_repository_scene_reads_and_names_its_sounds`) fails by name if one is left behind.

## Layout

A small crate that opts out of the default layout: it has no runtime state, no backend and
one domain, so there is no `state.rs`, `gpu.rs` or domain directory yet.

| File | Holds |
|---|---|
| `lib.rs` | The public names and `WorldError` |
| `api.rs` | `Entity` and its parts, as plain data |
| `id.rs` | `EntityId`, what makes an id usable, and a free id for a new sound |
| `entities.rs` | `Entities`: unique ids in file order, and the mapping it reads and writes |

When the behaviours arrive (when a sound plays, how it wanders), each gets its own file in
a private directory and the planning gets a `state.rs`.

## Known gaps

- **Changes are by id, one at a time.** `Entities` can add one at the end, change one in place
  and take one out. Reordering is not offered, because nothing needs it yet: the order decides
  voices only when a scene is loaded.
- **Telemetry still speaks in voice numbers.** The scene description carries each sound's id
  and group, and the app's layer list is drawn from them.
- **Wander is not here.** They are the
  components this version makes room for.
- **A generator cannot orbit.** The engine refuses `moves: orbit` on a generator preset, as
  it did before entities.
- **`size`, `level` and `facing` are not range-checked.** The engine uses them as given.
- **`facing` is a compass bearing, the engine's heading is not.** A scene file says 0 for north
  and 90 for east, as its comments always have. The engine counts radians the other way round
  from east, and converts when it builds the sound.
