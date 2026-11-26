# Focus tones

Notes for a tone generator in the way of Endel, brain.fm and harmonance.com, kept while the
idea takes shape. Nothing here claims an effect: the names are the labels these sites use,
and no frequency has a measured effect on focus or health (see "What the evidence says").

## The frequencies

Logged 2026-11-22, as Pascal listed them (the "Solfeggio frequencies" set as harmonance.com
shows it). 369 Hz ("Tesla 3 6 9") is not one of the traditional solfeggio tones; it is in
the site's set. 333 Hz and 666 Hz are from its "angel number" set.

| Hz | Label |
|---|---|
| 174 | Foundation |
| 285 | Restoration |
| 369 | Tesla 3 6 9 |
| 396 | Liberation |
| 417 | Change |
| 528 | Transformation |
| 639 | Connection |
| 741 | Expression |
| 852 | Intuition |
| 963 | Unity |

The "Angel numbers" set, as the site lists it:

| Hz | Label |
|---|---|
| 111 | Intention |
| 222 | Balancing |
| 333 | Expansion |
| 444 | Stability |
| 555 | Transformation |
| 666 | Earthly Matters |
| 777 | Insight |
| 888 | Continuity |
| 999 | Completion |
| 1111 | Alignment |

The "Planetary frequencies" set, as the site lists it. These are Hans Cousto's "cosmic
octave" (1978): an orbit's or rotation's period, turned into a frequency and raised by
octaves until it is audible. Earth's year checks out: 2³² / 31 556 926 s = 136.10 Hz. The
arithmetic is real; any effect is not measured. The Schumann resonance is a real physical
resonance of the cavity between the ground and the ionosphere (7.83 Hz fundamental), far
below hearing, so in a tone it can only be a rate (of a beat, a pulse or the rotation), not
a pitch.

| Hz | Label |
|---|---|
| 126.22 | Sun |
| 136.1 | Earth year |
| 141.27 | Mercury |
| 221.23 | Venus |
| 194.18 | Earth (Om) |
| 210.42 | Moon |
| 144.72 | Mars |
| 183.58 | Jupiter |
| 147.85 | Saturn |
| 172.06 | Earth, Platonic year |
| 140.25 | Pluto |
| 207.36 | Uranus |
| 211.44 | Neptune |
| 7.83 | Schumann resonance (a rate, not a pitch) |

Each value checked against the arithmetic, 2^n / period (`sources/tones/planetary/*.yaml`
states the same in its note). Periods are the standard astronomical values; the site's labels
are kept as labels.

| Hz | Period | Octaves (n) | Note |
|---|---|---|---|
| 126.22 | none observed | — | Cousto's Sun tone: a hypothetical body circling the Sun at its gravitational radius at nearly the speed of light, once every 30.95 µs (32 312.5 Hz), lowered 8 octaves: 32 312.5 / 256 = 126.22 Hz. As stated by Meinl's planetary tuning-fork listings (Thomann), not checked against Cousto's book |
| 141.27 | Mercury's orbit, 87.97 d | 30 | |
| 221.23 | Venus's orbit, 224.70 d | 32 | |
| 136.10 | Earth's (tropical) year, 365.24 d | 32 | |
| 194.18 | Earth's day, 86 400 s | 24 | The site's card calls this Earth's year; in Cousto's system it is the day |
| 172.06 | Platonic year, 25 920 years | 47 | Precession of Earth's axis |
| 210.42 | Moon's synodic month, 29.53 d | 29 | New moon to new moon |
| 144.72 | Mars's orbit, 686.98 d | 33 | |
| 183.58 | Jupiter's orbit, 4332.6 d (11.86 y) | 36 | |
| 147.85 | Saturn's orbit, 10 759 d (29.46 y) | 37 | |
| 207.36 | Uranus's orbit, 30 685 d (84.01 y) | 39 | |
| 211.44 | Neptune's orbit, 60 186 d (164.79 y) | 40 | 2^40 / 60 189 d (the usual figure) gives 211.43 |
| 140.25 | Pluto's orbit, 90 737 d (248.43 y) | 40 | A blog table of Cousto's values (gongs-unlimited.com) gives 90 740.5 d; Cousto's own tuning-fork list gives 140.64 Hz, from a shorter period |

And 432 Hz, "A4 (concert)", which the site calls Verdi's A. Verdi did argue for A = 432 Hz in
the 1880s, but before the 440 Hz standard (an international conference in 1939, ISO 16 in
1975) concert pitch varied widely from place to place, so 432 Hz was one tuning among many,
not *the* old one. Listener reports of a "warmer" sound come from small studies that did not
control the music (Calamassi & Pomponi 2019 among them). Under the octave rule: 432, 216,
108 Hz.

## How harmonance.com builds its "Harmonics" sound

Read 2026-11-22 from two pages' data and the site's player script
(`/frequencies/angel-number-frequency-333-hz` and `-666-hz`). The `.ogg` files the site
serves (`/audio/frequencies/*.ogg`) are only its "Pure tone" mode: measured, a plain sine
at the named pitch, 20 s, no other tone within 94 dB, level steady within 0.4 dB. The
Harmonics and Strings modes are made in the browser.

The Harmonics voice (`longform-octaves-moving`):

1. **Octaves down.** The named pitch, then up to three octaves below it, none lower than
   60 Hz. That is the rule the site's Harmonics collection page states (read 2026-11-22),
   with 528 Hz over 264, 132 and 66 Hz as its own example, so the lowest tones carry fewer
   lines. Both pages measured fit it: 333 Hz plays three lines (41.6 Hz would be under
   60 Hz), 666 Hz four (three octaves, the most).
2. **A level per frequency, lowest loudest.** The same frequency has nearly the same level
   on both pages, whatever its place in the stack:

   | Line | 333 Hz page (dBFS RMS) | 666 Hz page (dBFS RMS) | re 83.25 Hz |
   |---|---|---|---|
   | 666 Hz | — | −62.1 | −30.8 |
   | 333 Hz | −52.8 | −55.3 | −23.6 / −24.0 |
   | 166.5 Hz | −42.4 | −44.8 | −13.2 / −13.4 |
   | 83.25 Hz | −29.2 | −31.4 | 0 |

   The collection page says each line is set to sound as loud as the others; this is how it
   does it. It is more bass than equal loudness asks (very roughly 12–16 dB more at 83 Hz
   than at 333 Hz, ISO 226): the deep octave is the body and the named pitch a faint top
   line.
3. **The emphasis rotates.** About every 24 s the weight passes from the named pitch down
   through each octave and back to the pitch: each line's gain swings ±4 dB (8 dB from low to
   high), log-sinusoidally, the lines spread evenly round the cycle (a third apart for three
   lines, a quarter for four).

### The stacks this rule gives for the logged frequencies

Up to three octaves below the pitch, none under 60 Hz (the collection page's rule).

| Hz | Lines (Hz) |
|---|---|
| 111 | 111 alone (55.5 would be below 60) |
| 174 | 174, 87 (43.5 would be below 60) |
| 222 | 222, 111 (55.5 would be below 60) |
| 285 | 285, 142.5, 71.25 (35.625 would be below 60) |
| 333 | 333, 166.5, 83.25 (measured) |
| 369 | 369, 184.5, 92.25 |
| 396 | 396, 198, 99 |
| 417 | 417, 208.5, 104.25 |
| 432 | 432, 216, 108 |
| 444 | 444, 222, 111 |
| 528 | 528, 264, 132, 66 (the page's example) |
| 555 | 555, 277.5, 138.75, 69.375 |
| 639 | 639, 319.5, 159.75, 79.875 |
| 666 | 666, 333, 166.5, 83.25 (measured) |
| 741 | 741, 370.5, 185.25, 92.625 |
| 777 | 777, 388.5, 194.25, 97.125 |
| 852 | 852, 426, 213, 106.5 |
| 888 | 888, 444, 222, 111 |
| 963 | 963, 481.5, 240.75, 120.375 (three octaves, the most; 60.19 would still be above 60) |
| 999 | 999, 499.5, 249.75, 124.875 (three octaves, the most) |
| 1111 | 1111, 555.5, 277.75, 138.875 (three octaves, the most) |

The planetary tones all lie between 126 and 222 Hz, so under the same rule each is two
lines, the tone and its octave below (63.11 Hz for the Sun up to 110.615 Hz for Venus); the
second octave below falls under 60 Hz for all of them.

## What the evidence says

- Yuvacı 2026 (J. Interdisciplinary Art and Education 7(2): 93–102) measured 150 popular
  "healing frequency" tracks: 8 % had the promised frequency as their loudest tone, 6 % were
  a pure tone, 86 % were slow, simple music. Listeners reported relief regardless. The author
  puts it down to slow, low-stimulation music and expectation, not the number.
- Studies of 432 or 528 Hz report small effects in small samples, with music type and tempo
  not controlled. Reviews of binaural beats find mixed effects and inconsistent EEG
  entrainment.

So a focus tone in Atrium is built as a sound that is calm, steady and slowly alive, not as a
frequency with a promise.

## In the engine

`focus_tone` (`src/synth/focus_tone.rs`), rebuilt 2026-11-22 as the Harmonics octave stack
above, replacing a first version with overtones above the pitch and a beating shimmer
(Pascal: "an idea, not what I wanted").

- **Lines:** the pitch and up to three octaves below it, none under `floor` (60 Hz). At most
  four lines; the pitch always plays.
- **Levels:** each line's level is read from its own frequency on the curve measured from the
  site's two pages (re 83.25 Hz: 166.5 → −13.3 dB, 333 → −23.8, 666 → −30.8; the 166.5 and
  333 Hz values the mean of both pages), linear in log2(frequency), the end slopes extended
  (−13.3 dB per octave below 83.25 Hz, so a 66 Hz line is +4.5 dB; −7.0 dB per octave above
  666 Hz). Their choice, from their data, not a loudness standard.
- **Rotation:** gain_dB = (depth / 2) · cos(2π t / period − 2π · index / lines), index 0 the
  pitch, so the weight passes from the pitch downwards. `rotation_period` 24 s,
  `rotation_depth` 8 dB.
- **Live settings** (`SetSynthParam`): `pitch`, `floor`, `rotation_period`, `rotation_depth`,
  `master_gain`. Phase-continuous oscillators, 50 ms glides (a line's level follows its
  gliding frequency at once, so a glide never steps harder than its two ends), a 0.5 s
  fade-in, nothing at or above `min(0.95 · Nyquist, 20 kHz)`, no randomness.
- **Heard everywhere:** it is an ambient (field) source, like the winds; a scene's
  `shape.ambient: false` makes it a point. Open: on speakers a pure tone from several
  speakers interferes with itself, so its level changes as you move. On headphones the 528 Hz
  example (offline render, 2026-11-22) came out 3 to 7 dB louder in the left ear, line by
  line, as an ambient source, and balanced as a point: the decorrelating all-passes only shift a
  sine's phase, so how the ears' mix adds up depends on the frequency. The same render, as
  a point or ambient, also had the 66 Hz line about 12 dB under the curve against the
  132 Hz line on headphones (the generator itself is on the curve to 0.001 dB), so the
  headphone path loses deep bass; not yet looked into.
- **Presets:** one per frequency above, in `sources/tones/<set>/` (`solfeggio/528.yaml`,
  `angel-numbers/333.yaml`, `planetary/earth-day.yaml`, `concert-pitch/432.yaml`; 34 in all,
  not the 7.83 Hz Schumann resonance, which is a rate). Each has `group:` set to its set's
  name and an `about:` note the app's library shows for the preset being previewed: the
  site's label as a label, where the number comes from, the lines it plays, and that no effect
  is measured. `reference_spl: 50` dB at 1 m, a quiet listening level chosen, not measured.
  The calibration hears a 20 s preview, five sixths of a rotation, so over a whole rotation a
  tone sits about 0.1 dB under that level.

## Open: the Studio's controls (research first)

harmonance.com's Studio mixes each frequency with four controls (read 2026-11-22 from its
page): **Tone level**, **Strings level** (a string pad at the same pitch, played from
recordings, one per pitch, `strings-v1-<hz>`), **Warmth** (brighter ↔ warmer: the colour)
and **Space** (closer ↔ more space). In Atrium these would be a tone layer and a strings
layer in the Lab's mixer, a colour setting on the tone, and the scene's own distance and
reverb. Pascal, 2026-11-22: "we can do them in a bit, not yet, we need some research
first". Questions for that research: where string recordings could come from (and their
licence), or whether a string pad is a generator; what "warmth" should be physically; and
whether "space" needs one control or is just placement. Research on warmth, ensemble
spread, bowl doublets, and seven ambient pieces measured: `tone-and-noise-movement.md`.
