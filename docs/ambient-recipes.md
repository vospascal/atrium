# Ambient pieces taken apart

Pascal's reference pieces, each taken apart into its layers: which notes, how each layer
moves, where it sits between the ears, and what the occasional events are. The aim is a
recipe Atrium could build from, live and never repeating. The pieces themselves are other
people's recordings, sampled with `scripts/clip.sh` (YouTube's lossy audio; `assets/clips/`,
not in git). The measures are `analyse`'s (`docs/tone-and-noise-movement.md` says what each
tells), plus one-off reads of note names, of when each note sounds, and of whether a band
repeats (autocorrelation of its level in 2 s frames every 0.5 s). One stretch of each piece
is not the whole piece.

Each piece has a listening scene, `scenes/reference-<name>.yaml`, playing the stretch's two
channels from where stereo speakers stand (presets in `sources/references/`; the YouTube
link, the minutes and a description are in each preset's `about`). Fetch the clip first
with the `scripts/clip.sh` line in the scene.

Levels below are relative: "dB re Leq" is a partial's share of the whole piece's power;
"p10–p90" how far a level moves (its 90th percentile minus its 10th); "repeats every"
the lag of the strongest autocorrelation between 8 and 60 s, with its value (1 = exactly
the same each time, under 0.3 = hardly).

## "it wasn't always empty" (spiritual brother Sci-Fi), 20:00–23:00

`gP3BBaoU0_E` (https://www.youtube.com/watch?v=gP3BBaoU0_E), measured 2026-11-26; scene
`reference-it-wasnt-always-empty`. Everything is in **E major** (E F# G# A B C# D#) over
a pedal on E2. One line near C3 (+30 cents, 37 dB under the pedal) is off the scale.

| Layer | What it is | How loud | How it moves | Between the ears |
|---|---|---|---|---|
| Sub | broadband rumble, 20–60 Hz | 32 dB under the whole | steady: 1.2 dB sd, no cycle | |
| Pedal | a pure sine on E2, 82.41 Hz | the loudest by far: −1.3 dB re Leq, three quarters of the power | pitch 0.6 cents, level 1.2 dB (p10–p90); a tiny but very regular cycle every 10.5 s (0.4 dB sd, autocorrelation 0.91), faint sidebands 0.4 Hz either side | identical in both ears: centred, mono |
| Pad | the scale's notes from E3 to B4; F#3 and E3 the strongest (about 9 dB under the pedal in power), G#3 and A3 18–20 dB under, the higher notes 30–50 dB under at their peaks | together 5–6 dB under the pedal: a fifth of the power | each note on its own: 1.9–4.4 dB sd per 0.5 s, 8–14 dB p10–p90, no shared cycle (autocorrelation 0.10–0.47, correlation with each other and the air about 0); pitch 2–5 cents; each a hump 3–6 Hz wide | wide (width 0.57); G#3 detuned between the ears by 0.58 Hz, the off-scale line by 0.86 Hz |
| High notes | single notes on E5, F#5, B5, C#6 | 55–60 dB under the pedal: quiet | 16 in three minutes, 2 to 17 s apart (median 7 s); each rises in under a second and fades over 2–6 s, audible for 2–10 s: a soft bell | |
| Air | noise, 2–12 kHz | about 50 dB under the whole | swells 4.2 dB (sd) on a regular 22.5 s cycle (autocorrelation 0.75), with nothing else | |

The whole holds almost still: its level moves 0.5 dB (sd), no swells. What moves is the
balance inside the pad, the air's cycle and the bells.

Two kinds of movement, then:

- **Regular, as from a producer's LFO:** the air's 22.5 s swell and the pedal's 10.5 s
  ripple. Atrium's rule is that movement comes from a cause, not a fixed cycle
  (`harmonance-variation.md`, "What Atrium could take"). In this piece the cycles are slow
  and small or quiet (the air is 50 dB under), so they read as breathing rather than a
  pattern. Whether we copy them or give the air a cause (a slow wind) is a choice to make.
- **Free:** each pad note wanders by itself, a few dB over tens of seconds, and the bells
  come at irregular times on notes of the scale.

### What it would take in Atrium (an idea, not a decision)

- **Pedal:** a focus tone of one line, E2, steady, with or without a 10 s ripple of a few
  tenths of a decibel. It already exists except for the ripple.
- **Pad:** 8–10 lines on the scale's notes from E3 to B4, together 5–6 dB under the pedal,
  the bottom notes strongest and B4 about 25 dB fainter. Each line's level wanders by itself (a few dB sd,
  over 10–40 s), its pitch a few cents. Two or three lines are detuned between the ears by
  0.5–1 Hz. This is the living focus tone from `tone-and-noise-movement.md`, with notes of a
  scale instead of octaves.
- **Bells:** a sparse event source choosing a note of the scale from E5 up, every 2 to
  17 s, rising in under a second and fading in 2–6 s, never the same sequence twice.
- **Air:** a noise colour cut to 2–12 kHz, 50 dB under the whole, swelling about ±4 dB over
  20–25 s.
- **Sub:** a steady low noise under 60 Hz, 30 dB under the whole.

Open: the pad's timbre close up (whether each note is a single voice or a soft synth pad
with its own movement); and whether the bells are struck (a sharp attack the 0.5 s frames
miss) or bowed.

## "now, i rest" (spiritual brother Sci-Fi), 7:00–10:00

`hVFaaUEIpzE` (https://www.youtube.com/watch?v=hVFaaUEIpzE); scene `reference-now-i-rest`.
**C minor** (C D D# F G G# A#) over a pedal on C2. The same maker's recipe as "it wasn't
always empty", with the pad moving in chords and an arpeggio in place of bells.

| Layer | What it is | How loud | How it moves | Between the ears |
|---|---|---|---|---|
| Sub | rumble under 60 Hz | 33 dB under the whole | steady, 1.3 dB sd | |
| Pedal | a pure sine on C2, 65.41 Hz | −8.6 dB re Leq | pitch 0.6 cents, level 1.6 dB (p10–p90); a slow regular cycle (repeats every 26.5 s, 0.79) | identical in both ears |
| Pad | C3, D3, D#3, F3, G3, G#3, A#3, C4, D#4; G#3 and A#3 each doubled by a second voice 21–40 cents away | each −10 to −16 dB re Leq | two chords take turns every 16 s (the band repeats every 16 s, 0.77; each note every 32 s, 0.64–0.79: F3 sounds at 5–22, 37–56, 69–88 s, D#3 at 22–40 and 54–72 s); each note 14–21 dB (p10–p90); pitch 2.6–6.9 cents | wide (0.94); D3, G#3, A#3 and D#4 detuned between the ears by 0.5–0.8 Hz |
| Arpeggio | single notes entering in the upper mids, 500 Hz–2.5 kHz, mostly climbing in steps (seen in the picture), each held a few seconds | quiet | 19 a minute, 2.0 s apart (median; 0.6 to 6.4 s) | |
| Air | noise, 2–12 kHz | 48 dB under the whole | steady, 1.0 dB sd | |

The whole moves 0.9 dB (sd), brighter as it gets louder (+0.27).

## "the starships never pass by here anymore" (spiritual brother Sci-Fi), 7:00–10:00

`Bx5byPcxk5E` (https://www.youtube.com/watch?v=Bx5byPcxk5E); scene
`reference-the-starships-never-pass-by-here-anymore`. Tuned **30 to 45 cents above concert
pitch** throughout (about A = 449 Hz), so note names are rounded; no pedal held still, no
events.

| Layer | What it is | How loud | How it moves | Between the ears |
|---|---|---|---|---|
| Sub | rumble under 60 Hz | strong: 5 dB under the whole | 3.1 dB sd, loosely every 32 s | |
| Drone | 55.9 Hz (A1, sharp), 113.6 Hz and 150.9 Hz | −5.6 to −9.3 dB re Leq | pitch wanders 7–12 cents; level 8–11 dB (p10–p90); loosely every 32 s (0.35) | placed: 55.9 and 150.9 Hz 5.6–6.8 dB to the right, 113.6 Hz 4.3 dB to the left and detuned between the ears by 1.2 Hz |
| Upper notes | near E3, E4, A4, B4 | −13.6 to −21 dB re Leq | 11–17 dB (p10–p90); pitch 1.6–4.8 cents; together every 32 s (0.32–0.64) | the E3 voices 7–8 dB to the right |
| Air | noise, 1–12 kHz | 43–47 dB under the whole | small (1.0–1.8 dB sd) but regular: every 32 s (0.79–0.80) | |

The whole moves 1.5 dB (sd), in swells that rise in 7 s and fall over 24 s (median of five),
darker as it gets louder (−0.60). Width 0.75: instruments placed left and right more than
in the others.

## "i must rest here a moment" (spiritual brother), 15:00–17:00

`IXsWr2CK4SI` (https://www.youtube.com/watch?v=IXsWr2CK4SI); scene
`reference-i-must-rest-here-a-moment`. **E minor or G major** notes in the mids over B-flat
and F in the bass. Unlike the others, mostly texture: only a third of the power is in
distinct partials.

| Layer | What it is | How loud | How it moves | Between the ears |
|---|---|---|---|---|
| Sub | rumble under 60 Hz | 31 dB under the whole | 4.4 dB sd, every 38.5 s (0.56), against the air (−0.43) | |
| Bass | A#1, F2, A#2 | about 17–19 dB under the strongest notes | | |
| Texture | a broad soft band, 300 Hz–1 kHz, holding nearly all the power, with notes in it: E4, D4, A4, G4, B4, D5, E5 | each note −14 to −18 dB re Leq | each note 12–19 dB (p10–p90), E4, A3 and G4 sounding only 58–68 % of the time; pitch 1.2–3.7 cents; everything every 38.5–40 s (0.33–0.58) | almost unrelated ears (width 0.99); no steady detunes |
| Air | noise, 1–12 kHz | 21–41 dB under the whole | 2.8–4.0 dB sd, every 39–43 s (0.52–0.63) | |

The whole moves 0.9 dB (sd) and swells together on that 39 s cycle; brighter as it gets
louder (+0.41). No events.

## "Dark Space Music for Stellar Nurseries and Molecular Clouds" (Cryo Chamber), 19:00–21:00

`Yk2bpo5paPw` (https://www.youtube.com/watch?v=Yk2bpo5paPw); scene
`reference-stellar-nurseries-19-21`. **One rich low tone** at concert pitch and what grows
out of it.

| Layer | What it is | How loud | How it moves | Between the ears |
|---|---|---|---|---|
| Sub | rumble under 60 Hz | heavy: 6 dB under the whole | 3.6 dB sd | |
| Low tone | D#1, 38.91 Hz, with its 2nd to 6th harmonics (77.8, 116.7, 155.6, 194.5, 233.9 Hz) and its 15th (583.9 Hz); a fifth (58.3 Hz) beside it | the 3rd harmonic strongest (−4.2 dB re Leq), the rest −7 to −23 | each harmonic 9–14 dB (p10–p90), pitch 4.7–7.6 cents; no regular cycle (0.13–0.46) | no detunes |
| Two notes | C#4 (277–280 Hz) and G#4 (411–414 Hz) | about −25 dB re Leq | come and go: C#4 sounds half the time, held 2–24 s; G#4 a quarter to a third, held 4–11 s | |
| Air | noise, 1–12 kHz | 23–32 dB under the whole | 4.4–4.6 dB sd, moving with the mids (+0.72) | |

The whole moves 2.1 dB (sd), in swells of 17 s up and 15 s down (median of four), darker as
it gets louder (−0.66). Width 0.89. No events.

## The same piece, 29:00–33:00

Scene `reference-stellar-nurseries-29-33`. A busier stretch, **tuned off concert pitch**
(most lines 25–49 cents off), with a throb and a returning chord.

| Layer | What it is | How loud | How it moves | Between the ears |
|---|---|---|---|---|
| Sub | rumble under 60 Hz | 8 dB under the whole | 5.5 dB sd, throbbing every 8 s (0.63) | |
| Low tone | lines near the harmonics of about 29.3 Hz (30.4, 58.9, 87.7, 146.1, 174.6, 233–235 Hz), within 1–2 % of them; 174.6 Hz the strongest (−7.7 dB re Leq) | −7.7 to −17 dB re Leq | throbs every 8 s (0.50–0.77); the bass lines' pitch wanders 14–30 cents; 15–31 dB (p10–p90) | 58.9 Hz detuned between the ears by 0.37 Hz (± 0.15), 30.4 Hz by 0.88, 235.5 Hz by 1.0; 58.9 Hz 7.6 dB to the left |
| Chord | 283.8, 356.4/358.5 and 569.6 Hz: a root, its major third and its octave (near C#4, F4, C#5, about 40 cents sharp) | −16.8 to −18 dB re Leq | comes back every 42 s (entering at about 53, 95 and 138 s), rising over 2–8 s, held about 17 s, fading over 7–13 s | |
| Air | noise, 2–12 kHz | 32 dB under the whole | 5.8 dB sd, no regular cycle | |

The whole moves 4.1 dB (sd; L10−L90 12.2 dB), in swells of 12 s up and 14 s down (median
of seven). Width 0.85.

## "Calm | Serene Meditative Music for Late Nights" (Astral Ambience), 31:00–33:00

`caAVbyTCptw` (https://www.youtube.com/watch?v=caAVbyTCptw); scene
`reference-calm-serene-meditative`. **C major or A minor**: a chorus pad whose two chords
take turns. No pedal, no events.

| Layer | What it is | How loud | How it moves | Between the ears |
|---|---|---|---|---|
| Sub | under 60 Hz | 28 dB under the whole | 3.5 dB sd | |
| Pad | D2, A2, C3, D3, E3, G3, A3, C4, D4, G4; D3 the strongest (−4.2 dB re Leq); E2, G2, A2, G3, E4 and G4 each doubled by two voices 27 cents either side of the note | −4 to −20 dB re Leq | two groups take turns: D2, D3, E3, A3 sound at 16–34 and 44–62 s, C3, G2, G3 at 30–48 and 58–76 s, so a group lasts about 18 s and the other starts 14 s after it (everything repeats every 27–27.5 s, 0.32–0.73); each note swells 20–33 dB (p10–p90), rising in under a second to 6 s, fading over 5–24 s; humps 3–11 Hz wide, pitch 2–11 cents | wide (0.96); no steady detunes (the chorus voices wander between the ears) |
| Beating | the chorus voices about 1 Hz apart | 1.7 dB deep | a gentle 1.01 Hz pulse, rising and falling alike | |
| Air | noise, 2–12 kHz | 41 dB under the whole | 5.8 dB sd, every 40 s (0.39) | |

The whole moves 1.7 dB (sd), darker as it gets louder (−0.48).

## Across the seven

- **A low anchor in five of seven**: a pure pedal held still (C2, E2), a rich low tone
  with its harmonics (D#1, about 29 Hz), or a drone that breathes (A1), always under 90 Hz.
  The other two (i must rest, Calm) rest on their pad or texture.
- **A pad of scale notes**, a few dB to 20 dB under the anchor, each note's level moving
  8–33 dB. Either each note on its own (it wasn't always empty, the Stellar Nurseries
  19–21 stretch) or in chords that take turns every 14–16 s (now, i rest; Calm).
- **Slow cycles of 16 to 42 s** in six of seven: the pad's chords, the air, the whole
  breathing (i must rest, starships), a returning chord (Stellar Nurseries 29–33). Faster
  only the 8 s throb in one stretch.
- **Events are sparse when there are any**: bells every 2–17 s, an arpeggio 19 a minute,
  notes coming and going over tens of seconds. Three of seven have none.
- **Air at 2–12 kHz, 30–50 dB under the whole**, sometimes on a regular cycle (22, 32,
  39 s).
- **Tuning is not always concert pitch**: two of seven sit 25–49 cents off. A generator
  need not tie itself to A = 440.
- **Stereo**: wide (0.57–0.99), from slow detunes between the ears (0.4–1.2 Hz), from
  voices panned to one side (up to 8 dB), or from a chorus whose voices wander.
