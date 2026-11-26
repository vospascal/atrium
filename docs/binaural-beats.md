# Binaural beats

Logged 2026-11-22, from Pascal's pick of harmonance.com's "Binaural Beats" collection
(`/collections/binaural-beats`, read from the page's text and its track data). A companion
to [focus-tones.md](focus-tones.md). Built the same day as a 3D version, played through the
normal scene ("In this 3D version" below); the straight-to-the-ears path is not built.

## How every piece is made

Two pure sine tones and nothing else: **108 Hz in the left ear, 108 Hz plus the beat in the
right**. No music, noise or pulsing. The beat is not in the sound: hearing makes it from the
difference between the ears, so headphones are needed. Each piece runs nine hours (32 400 s),
fades in when started and out at the end, and stops; it never loops.

Two kinds:

- **Fixed beats** hold one beat rate for the whole nine hours.
- **Brainwave sessions** move the beat once, linearly, across the nine hours: delta, theta
  and alpha slow down; beta and gamma speed up.

| Piece | Left | Right | Beat | Band |
|---|---|---|---|---|
| Golden Ratio Delta Beat | 108 | 109.618 | 1.618 Hz, fixed | delta |
| Delta Binaural Beat | 108 | 110 | 2 Hz, fixed | delta |
| Falling Delta Session | 108 | 111.5 → 109.5 | 3.5 → 1.5 Hz over 9 h | delta |
| Low Theta Binaural Beat | 108 | 112 | 4 Hz, fixed | theta |
| Theta Binaural Beat | 108 | 114 | 6 Hz, fixed | theta |
| Falling Theta Session | 108 | 115 → 113 | 7 → 5 Hz over 9 h | theta |
| Schumann Resonance | 108 | 115.83 | 7.83 Hz, fixed | theta/alpha edge |
| Alpha Binaural Beat | 108 | 118 | 10 Hz, fixed | alpha |
| Falling Alpha Session | 108 | 120 → 117 | 12 → 9 Hz over 9 h | alpha |
| Rising Beta Session | 108 | 124 → 132 | 16 → 24 Hz over 9 h | beta |
| Beta Binaural Beat | 108 | 128 | 20 Hz, fixed | beta |
| Rising Gamma Session | 108 | 140 → 148 | 32 → 40 Hz over 9 h | gamma |
| Gamma Binaural Beat | 108 | 148 | 40 Hz, fixed | gamma |

The site's own suggestions, one line each, are about when to listen: lying back with the
1.618 Hz beat in the background, a calm pause between tasks with 10 Hz, quietly behind a
working session with 20 Hz, reading or thinking with 40 Hz. The Schumann piece "borrows the
number, not the Earth's electromagnetic field".

## How to listen, as the site says

Stereo headphones, left and right the right way round, **spatial audio and head tracking
off** so each ear gets only its own tone. Start quietly; there is nothing to gain from
loudness. Not while driving. Stop on dizziness, ear discomfort or unease; with a seizure
disorder or anything affected by rhythmic sound, ask a doctor first.

## What is and is not known

Binaural beats are a real effect of hearing. Whether they change brain activity or how people
feel is far less settled: a 2019 meta-analysis reported small to moderate average effects on
some attention and memory measures with large differences between studies, and a 2023
systematic review found the evidence that the beats change brain activity inconsistent. The
band names say where a beat rate sits, not what it does. The site itself calls each piece "a
listening experiment".

## In this 3D version

Built 2026-11-22. Pascal: "can we try to do it in 3D, then we at least have the sounds; we
can always add straight to the ears after".

- **One sound, two voices.** `binaural_beat` (`src/synth/binaural_beat.rs`) is one ear's
  pure sine. A preset builds as one sound in one voice of the scene: the left ear's tone is
  the sound, the right ear's its *second voice* (`SoundSource::second_voice`), which the
  render plays as a point of its own (its renderer state sits at index `sources + slot`).
  Everything the scene sets on the sound (mute, level, live settings) reaches both, so the
  library, the scene list, the map and the undo history see one sound.
- **Held beside the head.** Each tone is held 12 cm from the centre of the head, straight
  out to its own side (`SoundSource::beside_head`, `Listener::point_beside_head`), wherever
  the sound's place is. It turns and moves with the head, so head tracking leaves the tones
  at the ears, as the site asks ("head tracking off"). The map draws the sound where it is
  heard, at the left ear. It cannot be made ambient. On speakers the tones are panned from
  beside the listener; both ears hear both, which is the monaural (speaker) version.
- **Why 12 cm.** Close to a head the far ear is in the head's shadow even for low tones
  (Duda and Martens' sphere, which the renderer's near-field correction uses); a metre away
  a 110 Hz tone bends round the head almost whole. 12 cm is the nearest Brungart and
  Rabinowitz measured a head at (1999), 3 cm from the sphere's surface.
- **The pieces.** `sources/tones/binaural/`, 13 presets, group "Binaural beats", one per
  row of the table above, each with an `about:` note. `pitch` 108 Hz (the left ear),
  `beat`, `end_beat` for a session, `duration` 32 400 s.
- **The course of a piece.** A raised-cosine fade-in over 2 s, the beat fixed or moved in a
  straight line from `beat` to `end_beat` over `duration`, a raised-cosine fade-out over the
  last 20 s (each fade at most a quarter of the duration), then silence for good: no loop,
  and a longer duration set after the end does not start it again.
- **Live settings** (`SetSynthParam`): `pitch` (the carrier), `beat`, `end_beat` (makes a
  fixed beat a session), `duration` (shorten it to try a session in minutes),
  `master_gain`. A change glides the difference away over 50 ms with the phase kept; the
  sweep and the fades themselves are followed exactly, so the beat is on its line at every
  moment.
- **Level.** `reference_spl: 18` dB at 1 m, chosen: held 12 cm from the head that is about
  50 dB SPL at its own ear (measured below: 50.3 and 50.5 dB; the 20 s calibration preview
  hears the 2 s fade-in, so the tone sits 0.3 dB over the level asked for).

### Measured, 2026-11-22

The generator (`src/synth/binaural_beat/tests.rs`, 48 kHz, each frequency from a 20 s
FFT refined by the phase turned between its two halves):

| Piece | Left measured | Right measured (set) | Beat measured |
|---|---|---|---|
| 1.618 Hz | 108.000000 Hz | 109.618000 (109.618000) | 1.618000 Hz |
| 10 Hz | 108.000000 Hz | 118.000000 (118.000000) | 10.000000 Hz |
| 40 Hz | 108.000000 Hz | 148.000000 (148.000000) | 40.000000 Hz |

A session shortened to 120 s, the beat measured over 4 s at its start, middle and end
(the line at the stretch's centre in brackets): 7 → 5 Hz: 6.93333 (6.93333), 6.00000
(6.00000), 5.36667 (5.36667) Hz; 16 → 24 Hz: 16.26667 (16.26667), 20.00000 (20.00000),
22.53334 (22.53333) Hz. A 12 s piece is exactly silent from 12.000 s on in both ears and
for the 40 s rendered after; given 600 s afterwards it stays silent. The largest step from
one sample to the next, starting, ending and across every live change (carrier 108 → 216
and → 54 Hz, beat 10 → 40 and → 0 Hz, a 30 Hz end beat, a duration shorter than already
played, gain × 2 and → 0), is no larger than the tone's own at full level (0.001414 left,
0.001545 right at 118 Hz). The same settings give the same samples, bit for bit.

At the ears (`src/offline/tests/binaural_beat.rs`, `scenes/binaural-beat-test.yaml`, the
10 Hz piece, offline render on headphones, 10 s from 4 s in):

| | Own tone | The other ear's tone | Level swing at 10 Hz |
|---|---|---|---|
| Meadow, left ear | 50.3 dB SPL | −21.4 dB | 1.49 dB |
| Meadow, right ear | 50.5 dB SPL | −22.0 dB | 1.40 dB |
| Glass atrium, left ear | 50.3 dB SPL | −21.0 dB | 1.55 dB |
| Glass atrium, right ear | 50.6 dB SPL | −21.5 dB | 1.46 dB |
| Meadow, head turning 45°/s | same to 0.01 dB | same to 0.01 dB | same |

So each ear hears the other's tone about 21 dB down: about 8 % of its own tone's
amplitude. That is the monaural part: each ear's level swells and fades by about 1.4 dB at
the beat rate, exactly what two tones of those levels make together (measured swing within
0.02 dB of 20·log10((a + b)/(a − b))). The binaural difference is the rest. In the meadow
all of the leak is the direct sound round the head; in the glass atrium the reflections
and the reverb add about 0.4 dB of it.

How the leak depends on how close the tones are held (meadow, the 10 Hz piece, measured
once while choosing the distance, left / right ear):

| Distance from the head's centre | The other ear's tone | Level swing at 10 Hz |
|---|---|---|
| 1 m | −2.5 / −3.0 dB | 17.4 / 15.5 dB |
| 0.5 m | −4.7 / −5.3 dB | 11.7 / 10.7 dB |
| 0.25 m | −9.4 / −10.0 dB | 6.2 / 5.8 dB |
| 0.2 m | −11.9 / −12.4 dB | 4.6 / 4.3 dB |
| 0.15 m | −16.3 / −16.9 dB | 2.7 / 2.5 dB |
| **0.12 m (built)** | −21.4 / −22.0 dB | 1.5 / 1.4 dB |
| 0.10 m (the sphere model's nearest) | −26.7 / −27.2 dB | 0.8 / 0.8 dB |

A metre away almost the whole beat would be monaural: at 110 Hz the head shadows about
3 dB.

Head tracking: the tones are held beside the head, so a turning head hears exactly what a
still one does (measured above). Placed in the world instead, a quarter turn would have put
one tone ahead and the other behind, both ears hearing both alike: the beat would have
become wholly monaural (reasoned, not measured).

### What the straight-to-the-ears path will change

- The two voices would skip the scene render and be mixed after it, left to the left ear
  only and right to the right only (the "as recorded" stereo bus of the generator lab plan,
  stage 3): no leak, no near-field level boost, no reverb.
- `beside_head` and the second voice's render as a point would go; the generator and its
  settings, fades, sessions and tests stay as they are. The level would have to be restated
  at the ear (`reference_spl: 18` at 1 m is what makes 50 dB at 12 cm).
- On speakers there is no such path; there both ears hear both tones.

## What it would take in Atrium

- **A direct path to the ears.** Atrium's renderer lets both ears hear every sound (that is
  what 3D sound is), so a tone in the scene would leak into the other ear and the binaural
  beat would be lost. The pieces need the "as recorded" stereo bus from the generator lab
  plan (stage 3): mixed after the scene and before the output, left to the left ear only.
  Head tracking must not move it.
- **Headphones only.** On the 5.1 speakers both ears hear both tones; a monaural beat (the two
  tones mixed, so the level itself swells) is the speaker version. Monaural, AM and isochronic
  beats compared, and what a room does to them: `tone-and-noise-movement.md`.
- **A long, slow change.** A session moves its beat linearly over nine hours and stops; that
  fits the future timeline mode (cues at set times) better than the dynamic mode.
- **Rates, not pitches.** 1.618 and 7.83 Hz are beat rates here; neither is heard as a pitch.
