# Recipes: a design sketch

A design draft, 2026-11-26. The chord layer is built (`crates/recipe`, the engine's `recipe`
synth, `sources/recipes/calm.yaml` and `calm-in-f.yaml`, scenes `recipe-calm` and
`recipe-calm-in-f`); the rest is not. It describes a way to make tones and
ambient music in Atrium that you can repeat and vary: a **recipe**, a preset that says what a
piece is made of, where every part can be fixed or alive. It grew out of taking seven
reference pieces apart (`ambient-recipes.md`): the same few building blocks, set differently,
made all seven. The syntax below is a sketch to read, not a decision; the fields are what
matter.

Why not TidalCycles or Strudel: their notation is expressive but cryptic, their code is
GPL/AGPL (it would bind Atrium to their licence), and their community asks that its projects
be kept away from AI work (`codeberg.org/uzu/tidal`, POLICY.md). Strudel stays a good
sketchpad for trying chord ideas by ear. A recipe borrows only general ideas (cycles,
layers, seeded randomness), which belong to no one.

## What a recipe is

- **A preset**, like the others in `sources/`, with an `about` that says what it is. The
  same recipe and seed sound the same every time; a new seed is a new performance of the
  same piece.
- **Layers**: any number, of any kind. Each layer is a **voice** (how a note sounds: a
  generator) plus **what** it plays, **when**, and **where** in 3D.
- **Any value can move.** A value is fixed (`14 s`), a range drawn anew each time
  (`12..16 s`), a wander (`±10 dB over 30 s`), a follow of something else (another layer's
  level, the daylight), a regular cycle (`every 22 s`), or a choice with chances. This is
  the base value with follow, wander and variation the project already planned for every
  sound (`sound-entities-research.md`).
- **Notes are not tied to the piano**: names (`D3`), hertz (`528 Hz`), ratios from a root
  (`3/2`), cents off (`D3 +27c`), or a whole tuning (`A = 449 Hz`). Two of the seven
  references are 25–49 cents off concert pitch.
- **Simple at the top, open below.** A recipe of two chords and nothing else plays, the
  rest falling back to defaults taken from the references. A short script could cover what
  no field says (small embeddable languages with permissive licences exist, such as Rhai or
  Lua; not chosen).

## The building blocks the seven needed

| Block | What it says | Used by |
|---|---|---|
| A held note | one note that does not move: a pedal | it wasn't always empty, now i rest |
| Chords that take turns | `chords` + `change` + `hold` + `swell` | Calm, now i rest |
| One chord that breathes | the same notes all through, the level rising and falling | starships |
| A pool and a register | the same notes, the cloud moving between a high and a low octave | i must rest |
| Core notes and colour | some notes always, others coming and going on their own | it wasn't always empty, Stellar 19–21 |
| A harmonic tone | one root and the harmonics it carries, each drifting | both Stellar stretches |
| A returning chord | a chord that comes back every so often, rises, holds, fades | Stellar 29–33 |
| Events | single notes from the notes in play, at times drawn from a range: bells, an arpeggio | it wasn't always empty, now i rest |
| Wander and cycles | a level or pitch drifting freely, or swelling on a regular cycle | all |
| Space | width, a note placed to one side, a voice detuned between the ears | all |
| Noise layers | sub and air, from the noise colours | all |

Voices they need: a pure sine (the focus tone has one), a chorus pad (each note two voices
a few cents apart), a wash (each note a soft band, not a line), a soft pad, a bell, a
harmonic tone, and something soft for an arpeggio. The noise colours already exist.

## The seven as recipes

Each line comes from the measurements in `ambient-recipes.md`; a comment says which. Lines
marked *to check* are read from the picture or inferred, not measured.

### Calm (Astral Ambience)

Two chords a whole step apart, rocking. What moves: the harmony.

```yaml
name: Calm (after Astral Ambience)
voice: { kind: chorus-pad, voices: 2, detune: 27 cents,   # each note two voices, ±27 c
         drift: 2..11 cents }                              # each note's pitch wanders
chords:
  Am7: { notes: [A2, E3, G3, C4] }                         # once, at the start
  D:   { notes: [D2, A2, D3, A3, D4], colour: [E4, F4] }  # open fifth, no third
  C:   { notes: [G2, C3, G3, C4],     colour: [D4, F4] }  # F shared by both
start: Am7
change: { Am7: { D: 1.0 }, D: { C: 1.0 }, C: { D: 1.0 } } # always rocking
hold: { Am7: 12 s, D: 12..14 s, C: 10..12 s }
swell: { in: 0.5..6 s, out: 5..24 s, depth: 20..33 dB }    # chords overlap; the whole
                                                           # moves only 1.7 dB
space: { width: wide, voices: one each side, wandering }  # width 0.96
layers:
  sub: { noise: under 60 Hz, under: 28 dB, wander: ±3.5 dB }
  air: { noise: 2-12 kHz, under: 41 dB, wander: ±6 dB over about 40 s }
```

### i must rest here a moment (spiritual brother)

The same notes moving between two octaves, as a soft wash. What moves: the register.

```yaml
name: i must rest (after spiritual brother)
voice: { kind: wash, smear: 3..5 Hz }       # only a third of the power is clear notes
notes: [D, E, F#, G, A, B, C#]              # one pool, no chord changes
  # and a C natural slipping in as the cloud moves down
cloud:
  at once: 6..8 notes
  register: { high: D5..B5, low: A3..E4, hold: 18..22 s }   # the 39 s cycle
  each note: { sounding: 0.6..0.95 }        # 58–95 % of the time
whole: { brighter: when high }              # brightness with level +0.41
space: { width: widest }                    # 0.99: the ears almost unrelated
layers:
  bass: { notes: [Bb1, F2, Bb2], under: 17..19 dB }
  sub:  { noise: under 60 Hz, under: 31 dB, wander: ±4.4 dB, against: air }
  air:  { noise: 1-12 kHz, under: 21..41 dB, wander: ±3 dB over about 40 s }
```

### it wasn't always empty (spiritual brother Sci-Fi)

A pedal held still, a pad whose colour notes come and go, bells, air on a cycle. E major.

```yaml
name: it wasn't always empty (after spiritual brother Sci-Fi)
layers:
  pedal:
    voice: sine
    note: E2                                  # three quarters of the power
    wander: ±0.4 dB every 10.5 s              # a tiny regular ripple
  pad:
    voice: soft-pad
    notes: [E3, F#3]                          # always sounding, about 9 dB under the pedal
    colour: { notes: [G#3, A3, C#4], sounding: about half the time }
    each note: { wander: 8..14 dB over 10..40 s, drift: 2..5 cents }
    space: { width: wide, detuned: { G#3: 0.6 Hz } }
  bells:
    voice: bell                               # rising under 1 s, fading 2..6 s
    notes: [E5, F#5, B5, C#6]
    when: 2..17 s apart                       # median 7 s
    under: 55..60 dB
  air: { noise: 2-12 kHz, under: 50 dB, swell: ±4 dB every 22.5 s }   # regular
  sub: { noise: under 60 Hz, under: 32 dB }
```

### now, i rest (spiritual brother Sci-Fi)

The same maker's recipe, with chords taking turns and an arpeggio. C minor over a C pedal.

```yaml
name: now, i rest (after spiritual brother Sci-Fi)
layers:
  pedal:
    voice: sine
    note: C2
    wander: ±0.8 dB every 26.5 s              # 1.6 dB p10–p90, regular
  pad:
    voice: soft-pad                           # G#3 and A#3 doubled 21–40 cents apart
    chords:
      Ab: { notes: [G#3, C4, D#4], colour: [F3, D3] }
      Eb: { notes: [D#3, G3, A#3], colour: [F4, D3] }
    change: alternate
    hold: 16 s                                # the pair every 32 s
    each note: { swell: 14..21 dB, drift: 2.6..6.9 cents }
    space: { width: wide, detuned: { D3: 0.8 Hz, G#3: 0.7 Hz, A#3: 0.5 Hz, D#4: 0.5 Hz } }
  arpeggio:
    voice: soft
    notes: from the chord in play, 500 Hz..2.5 kHz     # to check
    when: 0.6..6.4 s apart                    # median 2 s: 19 a minute
    shape: mostly climbing step by step, each note held a few seconds   # to check
  air: { noise: 2-12 kHz, under: 48 dB }
  sub: { noise: under 60 Hz, under: 33 dB }
```

### the starships never pass by here anymore (spiritual brother Sci-Fi)

One chord all the way through, breathing, placed left and right. What moves: the breath
and the space.

```yaml
name: the starships never pass by here anymore (after spiritual brother Sci-Fi)
tuning: A = 449 Hz                            # everything 30–45 cents sharp
layers:
  drone:
    voice: rich-drone
    notes: [A1, A2, D3, E3, E4, A4, B4]       # A with D, E and B; no third; never changes
    breathe: together, loosely every 32 s
    swells: { up: about 7 s, down: about 24 s }
    each note: { wander: 8..17 dB, drift: 2..12 cents }   # the low notes wander most
    space:
      A1: right 5.6 dB
      D3: right 6.8 dB
      E3: right 7..8 dB
      A2 (second voice, 113.6 Hz): { left: 4.3 dB, detuned: 1.2 Hz }
  sub: { noise: under 60 Hz, under: 5 dB, wander: ±3 dB, loosely every 32 s }
  air: { noise: 1-12 kHz, under: 43..47 dB, swell: ±1.5 dB every 32 s }  # small, regular
```

### Dark Space Music for Stellar Nurseries, 19:00–21:00 (Cryo Chamber)

One rich low tone, two notes coming and going, heavy rumble. Concert pitch.

```yaml
name: Stellar Nurseries 19–21 (after Cryo Chamber)
layers:
  tone:
    voice: harmonic-tone
    root: D#1                                 # 38.9 Hz
    harmonics: [1, 2, 3, 4, 5, 6, 15]         # the 3rd strongest
    beside: A#1                               # a fifth, 58.3 Hz
    each harmonic: { wander: 9..14 dB, drift: 5..8 cents }
  notes:
    voice: soft-pad
    C#4: { sounding: about half the time, held: 2..24 s }
    G#4: { sounding: a quarter to a third, held: 4..11 s }
  sub: { noise: under 60 Hz, under: 6 dB, wander: ±3.6 dB }   # heavy
  air: { noise: 1-12 kHz, under: 23..32 dB, moves with: 300 Hz-1 kHz }   # correlation 0.72
whole: { swells: { up: about 17 s, down: about 15 s }, darker: when louder }
```

### The same piece, 29:00–33:00

A throbbing low tone and a chord that returns. Off concert pitch.

```yaml
name: Stellar Nurseries 29–33 (after Cryo Chamber)
tuning: free                                  # lines 25–49 cents off
layers:
  tone:
    voice: harmonic-tone
    root: 29.3 Hz
    harmonics: [1, 2, 3, 5, 6, 8]             # each within 1–2 % of the exact harmonic
    throb: every 8 s
    bass drift: 14..30 cents
    space: { 58.9 Hz: { left: 7.6 dB, detuned: 0.37 Hz }, 30.4 Hz: { detuned: 0.88 Hz },
             235.5 Hz: { detuned: 1.0 Hz } }
  chord:
    voice: soft-pad
    notes: [284 Hz, 5/4, 2/1]                 # a root, its major third, its octave
    returns: every 42 s
    swell: { in: 2..8 s, hold: about 17 s, out: 7..13 s }
  sub: { noise: under 60 Hz, under: 8 dB, throb: with the tone }
  air: { noise: 2-12 kHz, under: 32 dB, wander: ±6 dB }
whole: { level: moves 12 dB, swells: { up: about 12 s, down: about 14 s } }
```

## What the seven say about the design

- **Two settings carry a piece's character**: what moves (the harmony, the register, the
  breath, a returning chord) and which voice plays. Calm and "i must rest" feel alike and
  differ in exactly those two.
- **"Different tones, same idea" is changing only the note lines.** Calm in F and E-flat
  keeps the rocking, the swells, the shimmer and the width.
- **Living, not looping**, comes from ranges and wanders, not from new structure: Calm
  repeats exactly; `hold: 12..16 s` and a 10 % chance of a third chord would not.
- **Regular cycles are real in this music** (the air every 22 s, a throb every 8 s, a chord
  every 42 s) next to free wandering. The format needs both.
- **Space is per note, not per piece**: a voice to one side, a note detuned between the ears.

## Open

- Built so far: chords, change, hold, swell (with a stagger), per-chord colour levels, the
  chorus voice with drift, wander and a cutoff, and two kinds of events, pings (struck notes
  of the chord sounding, ringing out, of one or more kinds such as lower and higher) and swooshes (a swell of noise in a band). What the
  comparisons with Calm found: `crates/recipe/README.md`.

- The syntax: the YAML above is for reading; the real form is decided when building, with
  the app's controls in mind.
- The voices to build, starting with the chorus pad that Calm needs.
- The recipe player: chords, change, hold, swell, events, wander, follow, cycles.
- Whether a script is needed at all, and which (Rhai and Lua are candidates; licences to check).
- How a recipe and the planned timeline mode meet: a recipe is a place that lives; a piece
  with cues would arrange recipes in time.
