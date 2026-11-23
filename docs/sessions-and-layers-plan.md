# Listening sessions, and a sound's layers

Planned 2026-11-23, after Pascal tried the tones and binaural pieces in the app: "in
everything we read there are multiple layers; I can't see that", and a tone piece opened in
a scene that still showed weather, daylight and air, "so I see weather now in those".

Two parts, built in this order.

## 1. A listening session: a scene without a world

A scene says what kind it is. Today every scene is a nature scene: it has an environment,
weather, daylight and air, and the World panel shows them all. A tone has no weather rules,
so those controls do nothing to it; they only make a listening piece look like part of a
meadow.

```yaml
kind: listening          # nature is the default and needs no line
```

| | Nature scene | Listening session |
|---|---|---|
| World panel | size, voices, weather, daylight, air | voices and space only |
| Weather and daylight | run and change the sounds | none: the engine runs no weather story, and conditions stay out of the session |
| Space | the scene's environment | dry (no reverb, no reflections) by default; a room can be chosen |
| Timeline lanes Wind, Rain, Storm, Daylight | shown | not shown |
| New | — | "New listening session" opens one empty |

- Air (temperature, humidity) keeps its physical defaults but is not shown: at listening
  distances it changes nothing audible.
- A listening session's sounds that do have weather rules (a rain bed, say) hold their
  base settings, as in held weather with every condition at its default. Those rules are
  not shown.
- The existing tone and binaural scene files used for tests stay as they are; the kind is
  new and optional.

## 2. A sound's layers, visible and mixed

The generator lab plan's stage 4 contract, built for the generators that have layers
today (see [generator-lab-plan.md](generator-lab-plan.md#4-extend-the-mixer-to-other-generators-feature-27)):

- **A generator names its layers.** The focus tone: one per line ("528 Hz", "264 Hz", …,
  changing when the pitch or floor changes); the binaural beat: "left ear 108 Hz" and
  "right ear 109.618 Hz"; the soft wind: low, body, mid, presence, air. Every other sound
  is one layer and shows none.
- **The mix is shared code**: a level per layer (0 dB is the layer as the generator makes it
  today, so the untouched mix sounds exactly as before), solo and mute per layer. A
  generator never mixes its own layers; the mixer sums them.
- **Tracks** shows a sound's layers as sub-lanes under its lane, folded open with a
  triangle, each with its name and its own activity:

  ```text
  1 ● Golden Ratio                 ▬▬▬▬▬▬▬
      ├ left ear  108 Hz           ▬▬▬▬▬▬▬
      └ right ear 109.618 Hz       ▬▬▬▬▬▬▬
  ```

- **The Inspector** shows a selected sound's layers as faders with solo and mute, under
  its level.
- Layer levels are kept in the scene file with the sound (only those changed from 0 dB),
  so a saved session sounds the same reopened, and they go through undo like any edit.
- The Lab keeps its own soft-wind mixer for now; moving it onto this shared mixer is a
  later step.

## How we know it is right

| Check | Expected | Measured (test) |
|---|---|---|
| A listening session file | Loads; no weather story runs; conditions are at their defaults throughout a 10 minute offline render | `scenes/listening-session-test.yaml` (a focus tone and a rain bed that follows the rain, its file keeping a passing storm) loads as `kind: listening`; the plan has no story; 600 s rendered block by block as `offline::Rendering` does, the weather at 60×: the conditions equal the defaults in all 56 250 blocks, the air was handed over once, still (wind 0, daylight 0), and the only setting sent was the rain's intensity 0 (`a_listening_session_runs_no_weather_for_ten_minutes`) |
| Dry by default | An impulse in a listening session renders with no reflections and no reverb tail | In every render mode the whole render of an impulse 2 m ahead is the same samples as its direct sound alone (the probe with reflections and reverb off). The same impulse in the glass atrium gains −4.6 dB (WorldLocked), −3.1 (VBAP), −6.5 (HRTF), −13.4 (DBAP) and −5.1 dB (Ambisonics) of reflections and tail against its direct sound (`an_impulse_in_the_dry_space_is_its_direct_sound_alone`) |
| The World panel and timeline | Weather, daylight and air, and their lanes, are absent for a listening session and present for a nature one | A listening session shows none of the seven nature parts (weather with daylight, air, condition lanes, weather on the timeline, weather speed, wind on the map, Follows); a nature scene shows all seven, and both show the place (`ScenePartsTest`) |
| Layers untouched | Each layered generator's output with all layers at 0 dB is sample-identical to before this change | All ten layered generators: the FNV-1a fingerprint of 3 s at 48 kHz, taken before the change, is the same by itself, through an untouched mix advanced every sample, and through the scene's node (`untouched_layers_are_the_generator_as_it_was`) |
| A layer's level | −6 dB on one layer lowers exactly that layer by 6.00 dB, measured by FFT, and leaves the others within 0.01 dB | Focus tone at 528 Hz, −6 dB on its 264 Hz line, Hann-windowed DFT at each line over 1 s: 264 Hz −6.0000001 dB; 528, 132 and 66 Hz +4·10⁻⁸, −6·10⁻⁹, +4·10⁻⁹ dB (`a_layers_level_lowers_that_layer_alone`). Every layer of every layered generator at −6 dB adds 10^(−6/20) of what it adds at 0 dB, within −86.3 dB of the layer (`each_layer_is_weighed_on_its_own`) |
| Solo and mute | A soloed layer is that layer alone; a muted one is absent; no click on either (largest sample step no larger than the signal's own) | The soft wind's presence soloed is the same samples as the other four muted; all five muted is silence. Switched while playing, the largest step in the 50 ms after: soft wind presence muted 0.194, soloed 0.221, against the signal's own 0.269; focus tone 264 Hz muted and soloed 0.00253, against 0.00253 (`solo_and_mute_are_that_layer_and_do_not_click`) |
| Binaural layers | Muting the right ear leaves only 108 Hz at both ears | `layers: { right: { mute: true } }` on the alpha beat, on headphones: 118 Hz at −117.3 dB (left ear) and −117.6 dB (right ear) against 108 Hz, which is heard at both (`muting_the_right_ear_leaves_only_108_hz_at_both_ears`; the voices alone: `a_binaural_beats_layers_are_its_ears`) |
| Saved and reopened | Layer levels, solo and mute come back from the scene file and render the same | The soft wind with presence −6 dB, mid muted, air +3.5 dB soloed: saved, opened again, the same entities and the same layer settings in the description; 2 s rendered from each are the same samples, and differ from the scene as shipped (`layers_come_back_from_the_scene_file_and_render_the_same`) |
| Undo | A layer change undoes and redoes | On the playing engine: right ear muted, then left at −6.04 dB (kept as −6.0); two undos bring back 0 dB and then unmuted, two redos both changes; back at 0 dB a layer leaves the file (`a_layer_change_is_kept_undone_and_redone`) |
| Audio thread | Layer changes allocate nothing on the audio thread | No heap operation in 160 callbacks with live commands in every render mode, among them a layer's level moved, a solo and a mute switched, in `scenes/synth-test.yaml` (a field wind's layers) and `scenes/binaural-beat-test.yaml` (the two ears) (`audio_callback_never_touches_the_heap`) |

Also checked: a layer the scene file names that the sound does not have is refused with the layers it has (`an_unknown_layer_is_refused`); every generator names as many layers as it hands over (`every_synth_names_the_layers_it_hands_over`); a new empty scene of each kind saves and reopens as that kind (`a_new_scene_of_each_kind_saves_and_reopens_as_that_kind`); a passing storm switched to a listening session runs no weather and keeps standard air while its file keeps both, and switched back has its storm and air again, each switch undone (`switching_kind_and_back_restores_the_weather`).

Choosing the place in the scene (added the same day, see "As built"):

| Check | Expected | Measured (test) |
|---|---|---|
| The places | Dry and every file in `environments/`, each with its size and whether the sky is open | Dry 20 × 20 × 10 m open; atrium 14 × 10 × 9 enclosed; cathedral 40 × 25 × 20 enclosed; meadow 60 × 60 × 30 open; riverside 100 × 100 × 50 open; small atrium 6 × 4 × 3 enclosed (`lists_the_places_a_scene_can_play_in`, `each_place_says_its_size_and_whether_the_sky_is_open`); the line under one: "No room: direct sound only", "60 × 60 × 30 m, open sky" (`ScenePartsTest`) |
| A new place keeps the scene | The description reports the place; the same sounds, settings and layer mixes; the weather and the clock where they were | The passing storm, one sound at 61.5 dB and muted, the breeze's low layer at −6 dB and muted, weather at 60×, moved to 1800 s and paused, then to the cathedral and to Dry: each time `scene().space` is the new place, every sound is the same by id with the same settings and layers (placed the same from the listener), still muted, the clock 1800.058 s before and after, wind 0.15535 before and after, the story still a passing storm at 60× (`a_new_place_rebuilds_the_scene_with_the_same_sounds`). A seek alone would put a time-lapsed story elsewhere; the planner carries its weather clock over (`a_planner_carries_on_where_the_earlier_one_stood`) |
| Saved, reopened, undone | The file keeps the place, and no `environment:` for Dry; undo puts the old place back | Saved after Dry: no `environment:` line and no `seed:` (the file keeps its own), reopens in Dry with the same sounds; a listening session moved into the small atrium saves `environment: environments/small_atrium.yaml` and reopens there; undo brings back Dry with its frog, redo the room; a place that does not build is refused and the scene stays where it was (`a_place_is_saved_undone_and_redone`) |
| Dry renders the direct sound only | An impulse in the scene moved to Dry is its direct sound alone | The dry check's impulse, 2 m ahead, rendered from the engine's scene file: in the glass atrium reflections and tail add −4.6 dB (WorldLocked), −3.1 (VBAP), −6.5 (HRTF), −13.4 (DBAP), −5.1 (Ambisonics) against the direct sound; after the move to Dry the whole render is the same samples as the direct sound alone in every mode (`switching_to_dry_renders_the_direct_sound_only`) |
| New scene | Asked only its kind | `Engine::new_scene(kind)`: a nature scene in the meadow, a listening session dry, each saved and reopened as itself (`a_new_scene_of_each_kind_saves_and_reopens_as_that_kind`); the app's request carries the kind alone (`OpenSceneTest`) |

## As built

Built 2026-11-23. Where the build departs from the plan above, or goes past it at Pascal's
request, and why.

### Part 1

- **The kind decides; the file keeps.** `kind: listening` (`SceneConfig::kind`) runs no weather
  story and standard air whatever the file keeps under `weather:` and `atmosphere:`: both stay in
  the file, so a scene switched to a listening session and back has its storm and its air again.
  The plan said conditions "stay out of the session"; they are out of what runs, not out of the
  file. A listening session's `set_weather` and `set_atmosphere` are refused.
- **No weather is `None`, not a held calm.** The plan's weather is an `Option`: without one the
  conditions are the defaults (wind, rain and storm 0, daylight 1, as a held calm day has them, so
  a rain bed that follows the rain is silent), and the air sent to the audio thread is still
  (wind 0, daylight 0), so a far sound does not waver in sunlit air either.
- **Dry is the scene with no `environment:`, of either kind.** The dry space is built in code
  (`EnvironmentConfig::dry`): 20 × 20 × 10 m, every face open, hard ground. ISO 9613-2 makes hard
  ground a flat +3 dB within 30 times the source and listener heights (72 m at ear height), a
  pure gain: no colour and no delay, which is why the impulse check holds. A new nature scene
  starts in the meadow; a nature scene without an environment plays dry too.
- **The place is chosen inside the scene, by rebuilding it.** Pascal decided the place belongs to
  the scene, not to the New-scene screen ("if sound stops and rebuilds, that's fine"). The World
  panel of either kind lists Dry and every file in `environments/` (`list_places`, read from disk
  and sorted), the current one chosen, with a line under it: Dry "No room: direct sound only", an
  environment its size and "open sky" (its ceiling is open) or "enclosed". The places themselves
  are untouched: no new fields, nothing editable. An environment still cannot change while a scene
  plays (its pipelines, loudspeakers and where its sounds stand in the world are built with it),
  so `Engine::set_place` builds the scene again from its scene file with the new `environment:`
  (none for Dry) and swaps what plays: the audio thread's scene and its rings, the plan, the
  sound builder. The `Engine` object, its scene file and its history stay, so the place is an
  ordinary undoable step and undo across it rebuilds the same way. Carried over: every sound with
  its settings and layer mixes (they are in the file), the session's seed, the moment and the
  weather clock (`Planner::carry_on_from`: the same point of the story at the same speed), and
  what is the listener's: mutes (by id, since a rebuilt scene hands out voices in file order),
  probe, volume, output, pause (a paused scene starts already faded out, its clock still) and head
  tracking (its ring is swapped under it). Not carried: where a loop's playhead stood and a take
  under way, which start again as a seek starts them, and the tracks under the map, cleared because
  voices may change. The new stream starts before the old one stops, so a place that does not
  build or a device that does not open leaves the scene playing where it was.
- **What a kind shows is a whitelist, in one place.** Pascal asked that a part appear only where a
  kind lists it (`ScenePart` and `SceneKind.shows()` in `editor/SceneParts.kt`), so a part added
  later stays out of listening sessions until a kind lists it. Hidden from a listening session:
  the weather (story, bands, daylight among them), the air, the Wind, Rain, Storm and Daylight
  lanes above Tracks, the weather drawn along the timeline, the weather speed (1×, 10×, 60×), the
  wind's bearing on the map, and a sound's Follows section in the Inspector. Shown in both: the
  kind, the place (whose line gives the size, so the size shown alone for a nature scene went),
  the voices, Listen, Output and head tracking.
- **New scene, of either kind.** The app had no way to start an empty scene. "New scene" on the
  Open-a-scene screen (and in the Open dialog) asks only the kind: Nature (it starts in the
  meadow) or Listening session (it starts dry), and starts it with `Engine::new_scene(kind)`
  without a file; it opens unsaved and Save asks for its name. Its place is changed in the World
  panel like any scene's.
- **The kind is switched in the World panel**, at its top (`Engine::set_scene_kind`), and the
  switch is an undoable step.

### Part 2

- **The contract.** `SoundSource::layer_count` and `SoundSource::next_mixed(sample_rate, mix)`:
  a generator computes each layer and calls `LayerMix::sum` (`atrium_core::layers`) where it used
  to add them; `next_sample` is `next_mixed` with an untouched mix. A layer at 0 dB is weighed by
  exactly 1 and added in the order the generator added them, which is why the output is the same
  bits. Changes glide with a 10 ms time constant; the last 10⁻⁴ of a glide (−80 dB of the layer)
  is taken in one step, since in `f32` a one-pole glide stalls some 10⁻⁵ short of 1.
- **Where the gains after the sum stay the generator's own.** Every wind multiplies its sum by its
  speed, eddy and master gains, rain v2 low-passes it, the river filters it: those act on the
  mix's sum, as they acted on the generator's. The river's body, current and spray share a gain
  after their sum and its bubbles and splashes join after it, so the river calls the mixer twice
  (layers 0–2, then 3).
- **Which generators got which layers**, from what each already summed, nothing re-tuned:

  | Generator | Layers |
  |---|---|
  | Focus tone | one per line: the pitch, then each octave below it ("528 Hz", "264 Hz", …) |
  | Binaural beat | its two voices: "left ear 108 Hz", "right ear 109.618 Hz" |
  | Soft wind, field wind | low, body, mid, presence, air |
  | Canopy wind | body, leaves, dry leaves, contacts, branches |
  | Storm wind | pressure, roar, shear, tearing, debris, structure |
  | Rain (v1) | hiss, rumble, drops |
  | Rain v2 | bed, texture, drops |
  | River | body, current, spray, bubbles and splashes |
  | Waves | roar, hiss, crash |
  | Wisp | wings, hum |
  | Thunder | one: its roar and its claps meet inside a colouring and a soft limit that act on their sum, and two claps are summed after each is limited, so they cannot be weighed apart without changing its sound |

  Within a layer, parts that share one buffer stay together: rain's impacts, bubbles and drips,
  and the river's bubbles and splashes, are written into one ring before they are read.
  Recordings are one stream and show no layers.
- **A binaural beat's layers are its voices.** The node holds the mix of both; the right voice is
  set to what the mix leaves of layer 1, so a solo on either ear silences the other.
- **Names follow the settings at build.** A focus tone's lines are named from its pitch and floor
  when it is built; nothing changes a tone's pitch while it plays yet, so there is no live rename.
  Keys are stable (`pitch`, `octave-1`, `left`, `right`, `body`), names are shown.
- **The scene file keeps a layer by its key**, under the sound: `layers: { right: { mute: true },
  presence: { level: -6 } }`, only what differs from as made; a key the sound has no layer for is
  refused. Levels go from −60 to +12 dB, to a tenth of a dB in the file.
- **Each layer's activity in Tracks** comes from telemetry (`SourceSignal::layer_rms`): for a
  generator, its RMS times the square root of each layer's share of the power the mix summed
  (smoothed over 100 ms); for a binaural beat, each voice's own meter. Every layered sound's
  sub-lanes show until folded with the triangle.
- **The Inspector's faders** run evenly in dB from −60 to +12, half a dB a step, with S and M.
- The Lab keeps its own soft-wind mixer, as planned.

## Later: "at the ears" (Pascal, 2026-11-23)

A sound's placement: **in the space** (3D, the default) or **at the ears** (straight to
each ear, past the room and the head model; headphones; on speakers it falls back to the
left and right speakers). Pascal's point: the UI has no idea of left and right yet. What it
needs:

- **A two-channel sound** (a binaural piece, a stereo recording) already has its left and
  right; at the ears it plays them as they are. The control is a **balance**.
- **A one-channel sound** at the ears needs an **ear**: left, both or right, best as one
  slider from left through centre to right (headphone pan).
- **On the map** a sound at the ears is not in the space, so it cannot be dragged around
  the world. It shows on the listener's head instead: a dot at the left ear, the right ear
  or both, dragged along that line.
- **In Tracks and the Inspector** it says "At the ears · left" (or right, both, balance).

Pascal, the same day, generalising it: "you might also want 4 and 5 speakers, same thing,
just hard-code a sound to an output". So "at the ears" is one case of **direct routing**: a
sound sent straight to chosen output channels, past the 3D renderer. Headphones have two
outputs (left and right ear); quad four; 5.1 its five speakers and the LFE. It needs a
layout of its own, an **outputs view** drawn from the output in use (the speaker ring of
`rooms/atrium_6x4.yaml` for 5.1, two ears for headphones), where a sound is dropped onto
one channel, several, or a pair with a balance. Things to settle when it is built: the LFE
takes only low frequencies (bass management already exists for 5.1); a direct sound's level
per channel must stay calibrated like the rest; what a routing made for 5.1 does on
headphones and the reverse (fall back to the nearest channels, or say it is not heard).

Pascal, refining the layout: in this mode the map is **replaced** by a grid of the outputs,
and sounds are tiles or lines dropped onto it. His caveat: such a scene is made for one
output layout ("you can't do something else than that preset"): a scene routed for 5.1
means nothing on headphones and the reverse. So a routed scene names its output layout and
says so when played on another. Open question: is the whole scene routed (no 3D at all), or
can routed sounds sit beside 3D ones (a binaural piece at the ears over a forest bed in
the space)?

Decided (Pascal, 2026-11-23): **both in one scene.** The 3D render already works on every
output (headphones, stereo, quad, 5.1), so the space stays the base, and routed sounds are
added on top only where something must go to a specific output (a binaural piece to the
ears). Consequences: the map stays for the 3D sounds and the outputs grid is a view of its
own beside it: an **Outputs** tab next to Tracks, Analysis and Lab (Pascal: "good idea"); only the routed sounds
are tied to an output layout, so on another layout the 3D part plays as always and the Lab
says which routed sounds have no output there.

Labelling (Pascal, 2026-11-23: "we do need to label the scene if it has a specific output"):
a scene with routed sounds carries the output layout they were made for, and that shows
wherever the scene is named: in the "Open a scene" list (e.g. "Golden ratio · headphones"),
at the top of the editor, and in the World panel. When the current output differs, the
label says so ("made for headphones, playing on 5.1: 2 routed sounds not heard"). A scene
with only 3D sounds has no such label, since it plays on any output.

Fallback (Pascal, 2026-11-23): when the output does not match, a routed sound falls back to
the world space instead of going silent. Every output has a place to fall back to: an ear
→ held beside that ear (the 3D binaural pieces already do this, 12 cm out); a speaker → the
direction that speaker stands in (ITU-R BS.775 for 5.1: centre 0°, front ±30°, surround
±110°; quad at ±45° and ±135°), at the distance of the room's speakers; the LFE → all round,
low frequencies only. The label then reads "made for headphones, playing on 5.1: 2 sounds
placed in the space instead".
