# atrium-recipe

What a piece is made of, as data, and the decisions it leaves to chance, made from a seed.
A recipe names its chords, how one follows another, how long each holds and how it swells,
and the voice that plays them. The engine's `recipe` voice (`src/synth/recipe/`) plays it;
this crate decides what plays when. The design, and the seven measured pieces it comes from,
are in `docs/recipe-design.md` and `docs/ambient-recipes.md`.

```rust
let recipe = serde_yaml::from_str::<RecipeDef>(yaml)?.resolve()?;   // checked, notes in Hz
let mut planner = Planner::new(&recipe, seed);                      // the same seed, the same piece
let turn = planner.next().unwrap();   // ChordEvent { chord, at, hold, swell_in, swell_out }
```

```yaml
chords:
  - { name: D, notes: [D2, A2, D3, A3, D4], colour: [E4, F4] }
  - { name: C, notes: [G2, C3, G3, C4], colour: [D4, F4] }
change: alternate          # or chances: { D: { C: 0.8, Am7: 0.2 }, ... }
hold: 12..16 s             # or per chord: { D: 12..14 s, C: 10..12 s }
swell: { in: 0.5..6 s, out: 5..24 s }
colour_level: -10          # dB under the chord; a chord may set its own
pings:                     # one or more kinds, each planned on its own
  - { range: [C4, A4], every: 5..12 s, ring: 1.5..3 s, level: -9 }   # lower
  - { range: [C5, G5], every: 9..20 s, ring: 2..4 s, level: -14 }    # higher, cleaner
swoosh: { every: 15..40 s, rise: 1.5..5.5 s, fall: 1.5..5 s, band: 1500..4000 Hz, level: -32 }
tuning: A = 440 Hz
voice: { voices: 2, detune: 27 cents, drift: 2..11 cents, wander: 4, wander_over: 20..40 s, brightness: 0.3 }
```

Notes are names in scientific pitch (`D3`, `Eb2`, `F#4`), with cents (`D3 +27c`), or
frequencies (`528 Hz`). A value is a number or a range drawn anew each time (`12..16 s`);
its unit may be written and must then be the field's.

Pure: no audio, no engine type, no file, serde its only dependency. The engine's `recipe`
synth (`src/config/synth/recipe.rs`) reads a preset's fields into a `RecipeDef`.

## Decisions that look wrong and are right

- **A chord's `hold` is the time to the next chord's arrival, not how long it sounds.** The
  next chord swells in while this one swells out, so they overlap for as long as the fades
  last. Calm's turns, measured as "the chord loudest", are 10–14 s apart while its fades
  reach 24 s; with `hold` as the sounding time, every fade would have pushed the changes
  apart.
- **The first chord is already sounding** (no swell in). The engine calibrates a generator
  from a 20 s preview of it; a piece that began with a 6 s swell from silence would be
  measured too quiet and played too loud.
- **A swell in is never longer than its hold.** A turn that took longer to arrive than to
  give way would still be rising when the next began.
- **Chords are a list, not a map by name.** The order is `alternate`'s order, and a YAML map
  read through serde alone keeps none.
- **Mistakes are refused, not clamped.** Every other synth clamps what it is given; a recipe
  naming a note (`H3`) or a chord that is not there has no nearest valid value. Each error
  names its field (`chords[1].notes[2]`), and the engine checks a recipe when its preset is
  resolved, because presets are read through an untagged enum that would hide why one
  failed.
- **Randomness is a hand-written xorshift64\***, as the engine's plan has: no random crate in
  the workspace, and the same seed must give the same piece on every platform.
- **`MOST_NOTES` (12, colour notes included) is part of the contract.** The voice holds room
  for that many in each sounding chord, made before the audio thread runs; a recipe with
  more is refused here instead of cut there.
- **A ping is told no note, only a range.** The voice takes a note of the chord sounding at
  the ping's time and moves it by octaves into the range, so pings always belong to the
  harmony and follow its changes without the recipe naming notes twice. Calm's tings are
  exactly that: E4 to D5, each a note of the chord under it.
- **A ping is a pure tone.** It began as a small bell (a partial at 2.76 times, dying twice
  as fast); measured against the clip, that partial stood 21 dB out of the 1.5–4 kHz band
  where Calm's tings add nothing above 700 Hz. Removed.
- **Events plan from their own seeds** (the recipe's seed mixed with a constant per event and
  per kind of ping), so adding pings to a recipe leaves its chords, and its other pings,
  exactly as they were.
- **Pings are a list of kinds, not one setting with wider ranges.** Pascal hears Calm's tings
  as "higher cleaner ones and lower ones": two registers with their own pace, ring and level.
  One kind with a range of C4 to G5 would mix them evenly; two kinds keep the lower ones
  frequent and the higher ones rare and soft.
- **A ping comes alone by default and rings for seconds** (every 5–12 s, 20 dB down in 1–3 s).
  The first version, measured as runs of one to six 0.1–0.7 s apart ringing 0.2–0.6 s, was
  heard as too close together and too short: "pingggg, 5 s later, if it fits the flow, it
  can ping again or not". The scan behind it counted onsets 6 dB over a 4 s median, which
  splits one ringing ting into several and finds the pad's shimmer too; ears decide here.
- **Layout opt-out.** One crate of six small files (`api.rs` the types and checking,
  `events.rs` the pings and swooshes, `note.rs`, `span.rs`, `planner.rs`, `random.rs`); no `gpu.rs` or adapters, which nothing
  would fill (AGENTS.md: no placeholder files).

## Known gaps

- Built: the chord layer of `docs/recipe-design.md` (chords, change, hold, swell, colour,
  tuning, the voice's settings) and two events, pings and swooshes. Not built: other layers
  (sub, air, arpeggios, a register cloud, a harmonic tone, a returning chord), per-note placement and ear detunes,
  follows of conditions, a script.
- The voice's drift and wander are decided in the engine's voice, not here: they are
  continuous modulation, read every 32 samples, not turns to plan. If they ever need to be
  planned ahead (seeking within a turn), they move here.
- Seeking within a performance replays the planner from its start (`restart`); cheap for a
  turn every 10 s, not for hours.
- Listening rounds against Calm (`analyse --compare` of the clip with a render, the render's
  first 6 s left out for its fade-in; Pascal's ears judge):
  - Round 0: "not the same as the original at all … just striking a chord and holding it".
    Each note was two clean lines (1.9 Hz wide, pitch wander 2.9 against 6.8 cents), the
    level wander shallow (17.8 against 24.5 dB), the change sudden (skew +0.96 against
    +0.17).
  - Round 1, life inside the chord: up to six voices per note spread across the detune,
    `drift_over` for a faster shimmer, `swell.stagger` so a chord blooms note by note, a
    4 s fade-in instead of a strike. Now pitch wander 6.6 cents, notes 4.4 Hz wide, level
    wander 25.1 dB, skew +0.11.
  - Round 2, the bed: the Calm scenes gained the original's two quiet layers, air (blue
    noise) and rumble (brown noise), set to its levels: rumble −28.2 dB under the whole
    (the clip −28.2), air −45.6 dB in 4–12 kHz (the clip −45.4). Steady where the clip's
    swell (3.5 and 8.4 dB spread): the noise colours have no slow variation yet. The width
    hardly moved (0.78 from 0.74): the pad itself is the narrow part.
  - Round 3, tone and space, compared fairly: the original rendered through Atrium (its
    reference scene, two speakers) against ours, both on headphones, instead of our render
    against the raw stereo clip, which goes straight into the ears.
    - Space needed nothing: through Atrium the original measures width 0.44 and our pad
      0.78; "too narrow" was the unfair comparison. What differs is the presentation (two
      speakers in front against heard all round), a scene's choice.
    - Tone: a `cutoff` (a 12 dB per octave low-pass in hertz), 400 Hz for Calm. 630 Hz–2.5 kHz
      went from 9–21 dB too loud to within −7 to +3 dB; centroid 188 against 183 Hz; the RMS
      of the third-octave differences from 9.3 to 4.8 dB. The air became pink noise (blue was
      9–24 dB too bright above 8 kHz; pink is still 7–20 dB over, flat where the original
      falls away).
  - Round 4, the high tones (Pascal: "in the original there are some higher tones"): each
    chord carries its note an octave up, C5 over C and D5 over D, 12–28 dB under the whole.
    Added as colour notes (Am7 C5 D5, D +D5 E5, C +C5 G5). The 500 Hz–2.5 kHz band, both
    through Atrium: median −22.2 against −22.0 dB (from −25.8); the original's swings more
    (10 dB from p10 to p90 against 5) and C5 leads more often (38 of 56 steps against 27).
  - Round 5, events and the top notes' swing (Pascal: "these pings once a while make it way
    more dynamic", "the swoosh is top middle band"). Both measured through Atrium, ours from
    6 s on:
    - Pings: 44 struck notes in 23 runs in two minutes, 7.7 dB over the 600 Hz–1.5 kHz band
      (median; the clip 49 in 24 runs, 6.8 dB). The bell partial removed (see above).
    - Swooshes: three in two minutes, 11–13 dB over 1.5–4 kHz, flatness 0.53–0.57 (the clip
      four, 6–12 dB, 0.37–0.43).
    - Per-chord colour levels (Am7 −10, D −20, C −6 dB): C5 the loudest high note in 41 of
      57 frames (the clip 40 of 59). The 500 Hz–2.5 kHz band still swings less, 7 dB from p10
      to p90 against 10: the pings now fill its quiet frames.
  - Round 6, pings that ring out (Pascal: pings "too close together", should "echo out",
    "higher cleaner ones and lower ones" that "follow the flow"). Pings became a list of kinds:
    lower (C4–A4, every 5–12 s, 20 dB down in 1.5–3 s, −9 dB) and higher (C5–G5, every
    9–20 s, 2–4 s, −14 dB), each alone. Measured: the clip's strongest tings are C4, D4, F4
    and A4, the clearest falling 20 dB in about 2 s; no steady beat in their times (the best
    period's phase coherence 0.34 against 0.15 by chance). Both kinds stand 6–9 dB over their
    band, as the clip's do. Pascal: "not the same, but very good".
  - Still off (round 3): the bed is steady where the original's swells; the air too bright
    above 8 kHz; no 1 Hz beating; the other chord's notes fall silent where the original keeps
    them 20–30 dB under. Below 40 Hz the original rendered as two points loses some 20 dB
    (the headphone path's known deep-bass loss for points, `docs/focus-tones.md`), not a
    target.
  - Still off before round 3: the tone (a buzzy 800 Hz–2.5 kHz, 13–19 dB too much, tilt −13 against −8.8 dB
    per octave), the space (width 0.74 against 0.96: one source, nearly mono in the bass),
    no air above 3 kHz or rumble below 60 Hz, no 1 Hz beating, and the other chord's notes
    fall silent where the original keeps them 20–30 dB under (so its notes still move
    together more, 0.44 against 0.18).
