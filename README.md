# Atrium

Atrium renders an environment. It is a spatial audio engine in Rust: a world of
positioned sounds (birds, a river, wind, rain, an instrument), and a renderer
that works out what arrives at each speaker or each ear.

Built for a NUC driving 5.1 speakers plus binaural headphones, and it runs on
any machine with a stereo output.

To run it, see **[run.md](run.md)**: speakers, headphones, head tracking,
analysis tools, profiling and ports.

## Quick start

```bash
cargo run --release                                   # scenes/default.yaml on your speakers
```

```bash
cargo run --release -- --headphones scenes/nature.yaml  # a meadow, binaural
```

```bash
cargo test --workspace
```

Needs Rust 1.85 or newer (tested on 1.93.1) and an audio output device. macOS
and Linux are the tested platforms.

## What a scene is made of

A scene file wires separate YAML files together, so each can be swapped alone:

| Directory | Describes |
|---|---|
| `scenes/` | Which sounds are where, the speaker layout, the render mode |
| `environments/` | The virtual acoustic space: size, wall materials, ground, reverb |
| `rooms/` | The physical room the speakers stand in |
| `sources/` | One sound: a recording or a synth, its level and directivity |
| `atmospheres/` | Air temperature, humidity and pressure |

Sounds are either decoded recordings (`assets/`) or procedural synths
(`src/synth/`: four winds, two rains, river, waves, and a small flying creature).

## How it renders

Every source goes through the same three steps. Which implementation runs in
each step depends on the render mode.

1. **Resolve paths.** How does the sound reach the receiver: directly, and off
   up to six faces of the environment (first-order image sources).
2. **Per-path effects, then the renderer.** Each path gets its own delay, air
   absorption (ISO 9613-1), the wavering of wind-stirred air (see
   [Moving air](#moving-air)), ground effect (ISO 9613-2) and wall colouring;
   the renderer then places it in the output.
3. **Mix stages** on the whole buffer: late reverb (FDN), bass management,
   speaker delay compensation, master gain.

| Mode | For | Paths | Renderer | Mix stages |
|---|---|---|---|---|
| `world_locked` | speakers | direct only | each speaker is a microphone in the world: its own delay, air absorption, reflection taps | bass management, static delay compensation, master gain |
| `vbap` | 3 or more speakers | direct + reflections | amplitude panning relative to the listener | reverb, bass management, listener-relative delay compensation, master gain |
| `hrtf` | headphones | direct + reflections | HRTF convolution to two ears, equalised for headphones; ambient sound through the head's diffuse field, or the part of it its arc covers | reverb, master gain |
| `dbap` | speakers | direct + reflections | distance-based panning from source to speaker positions | reverb, bass management, static delay compensation, master gain |
| `ambisonics` | speakers or stereo | direct + reflections | first-order B-format encode | B-format delays, decode, reverb, bass management, listener-relative delay compensation, master gain |

`world_locked` and `dbap` do not depend on where the listener stands: everyone
in the room gets the same speaker feed. `vbap` and `ambisonics` are rendered for
one listening position.

## Listening to one part

A rendered scene is a sum: direct sound, first reflections, late reverb and
ambient fields, each shaped by the air and the ground. To judge one of them by
ear, a probe (`atrium_core::probe::Probe`, `Command::SetProbe`) lets only some
of them reach the output, switches single effects off (air absorption, moving
air, ground effect, wall colour, and on headphones the near field and the
headphone equalisation), or leaves one loudspeaker or one ear alone.

Nothing else about the scene changes, which a test holds every render mode
to: the four parts heard alone add up, sample for sample, to the whole.

Decisions that look wrong and are right:

- **The reverb is still fed while the direct sound is hidden.** A probe scales
  what a path sends to the output and leaves what it sends to the reverb. Were
  the path itself silenced, the reverb heard alone would be silence.
- **A hidden part fades over one callback.** A cut would click, and a click is
  the last thing wanted while listening closely. A probe set before the scene
  first renders (a render to a file, `--probe`) does not fade: nothing was heard
  yet to fade from, so it holds from the first sample.
- **The delay cannot be switched off.** Without it a reflection is no longer
  one; the other four path effects can.
- **It is the listener's, not the scene's.** Like a mute it is never saved and
  undo does not touch it.

For a comparison that can be repeated: a scene with a `seed:` plays the same
way every time, pause holds it, and the timeline returns to any moment.

## Moving air

Outdoors a far sound does not arrive steadily. Wind shear and the sun on the
ground fill the air with eddies, and the level of a distant sound comes and
goes with them, the treble most. `src/audio/turbulence.rs` computes how much
from published theory, and every path through open air gets it.

It is the line-of-sight theory of Ostashev and Wilson for a spherical wave in
von Kármán turbulence, with the surface layer from Monin–Obukhov similarity,
as written out by Kamrath et al. (Acta Acustica 8, 2024), who render it the
same way: a time-variant high-shelving filter. For a bird 40 m off:

| Air | 500 Hz | 1 kHz | 4 kHz |
|---|---|---|---|
| Light air, 2 m/s, night | 0.3 dB | 0.4 dB | 0.9 dB |
| Breeze, 5 m/s, night | 0.6 dB | 0.9 dB | 1.9 dB |
| Still, clear noon | 0.5 dB | 0.8 dB | 1.6 dB |
| Strong wind, 9 m/s, night | 1.1 dB | 1.6 dB | 3.0 dB |

These are standard deviations of the level. At 5 m they are a few tenths of a
dB in the treble and less below, so a near sound stays steady; indoors and in
still air at night the filter is not in the signal at all, bit for bit.

The scene's weather drives it: the wind condition is metres per second (1 is
13 m/s at 10 m), daylight is the heat of the sun (1 is 200 W/m² into the air),
and `weather.wind_from` is the compass bearing the wind comes from. A wind
across a path makes it waver faster than one along it.

Decisions that look wrong and are right:

- **Nothing is exaggerated.** The first plan was to push the effect until it
  was audible. Working the theory through showed it is audible as it is, at
  the distances a scene has, so the numbers are the theory's.
- **Arrival time is left alone.** The same eddies also move a path's travel
  time, by some 65 µs at 40 m in a breeze. On one path that is under a cent of
  pitch, slowly; nothing can hear it, so it is not rendered.
- **Thermals set the pace, not the depth.** The large eddies of a sunny day
  (hundreds of metres) hardly change a level over 40 m. They are left out of
  the strength, as the paper leaves them out, and only help decide how fast
  the air moves across a path on a still day.
- **A still sunny scene wavers.** Every scene without weather is a calm,
  bright day, so its far sounds shimmer a little. That is the heat, and real.

Known gaps:

- **Two parts stand in for a smooth curve.** The filter splits the sound at
  2.4 kHz. Measured on tones it is within 15 % of the theory for a sound 40 m
  off in a breeze, and up to a fifth short in a gale at 60 m.
- **Saturation is borrowed.** How strong fluctuations level off (never past
  5.6 dB) uses the paper's fit to aircraft flyovers; near the ground that is
  an extrapolation. It matters only in gales at the far edge of a scene.
- **The pace is a rule of thumb.** The level holds for as long as the air
  takes to carry a Fresnel zone (at 4 kHz) across the path. The depth is
  theory; this timing is the usual frozen-turbulence estimate, not measured.
- **The wind does not bend sound yet.** Upwind of a breeze a sound near the
  ground can fall into a shadow within tens of metres, and downwind it carries
  further. Now that the wind has a direction this can be built; it is not.
- **The wind does not veer.** Its bearing is one number per scene; a passing
  storm does not turn it.
- **Clouds do not dim the sun.** Daylight alone sets the heat; rain and storm
  do not lower it.
- **Each speaker's path wavers by itself in world-locked.** Paths to speakers
  a few metres apart share some of their eddies in reality.

## Ambient sound

An ambient sound (`shape.ambient`) has no distance: it is the sound around
the listener, at its level wherever the listener stands. Any sound can be
ambient: a frog chorus, a robin, rain with single drops. So whatever spreads
it around must leave an attack heard once. Left as it is, it is heard from
all round, as described here; `shape.arc` narrows it to the side its place
lies on (see [An arc of ambient sound](#an-arc-of-ambient-sound)).

### On speakers

Every speaker gets its own copy through its own filter, so the copies sound
the same and carry the same power but are different waveforms, and the room
fills instead of the sound sitting in one speaker. The filter is four
Schroeder all-passes in series per channel (Schroeder and Logan, "Colorless
artificial reverberation", 1961), each with gain 0.5 and a delay between 0.5
and 2 ms drawn from a fixed seed per channel (`src/pipeline/renderers/field/`).
It replaced one all-pass per channel with a delay of 7 to 31 ms, which
answered every click with a train of echoes. Worst channel or pair, at 44.1,
48 and 96 kHz:

| | Before | Now |
|---|---|---|
| Energy of a click within 10 ms | 32 % | 92 % |
| Strongest tap after 15 ms, against the strongest | 0 dB (it was the strongest) | −22.5 dB |
| Correlation of two channels, white noise, at zero lag | 0.37 | 0.11 |
| Largest correlation within ±30 ms | 0.46 | 0.32 |
| Colouring, third octaves 100 Hz to 16 kHz | 0 dB | 0 dB |
| Coherence below 200 Hz, mean over 28 pairs | 0.32 | 0.64 |

Each channel keeps all of the source's energy, so N channels at 1/√N add up
to it. The ambisonic bus gets four of these copies as W, Y, Z and X.

### On headphones

The ears hear an ambient sound as the measured head hears a diffuse field:
sound of equal strength from every direction at once, unrelated between
directions. What that does at two ears is known from the head itself. From
all 710 measured directions of `assets/hrtf/default.sofa`, each weighted by
the solid angle it covers, follow each ear's diffuse-field response, its
colour (the ear canal lifts 2.5 kHz some 20 dB above 250 Hz), and the two
ears' coherence at each frequency: nearly the same in the bass, unrelated
from about 500 Hz up (`src/pipeline/renderers/binaural/diffuse_field/`).

That colour is the measurement's and the dummy head's ear canal, which the
headphone equalisation divides out of every point source (see
[Two ear canals](#two-ear-canals)). Equalised, a diffuse field reaches each
ear flat, so the ambient sound carries only the coherence; with the probe's
headphone equalisation off it carries the colour too, as described here.

Every ambient sound joins one mono bus, and two fixed filters give the left
and the right ear that coherence, and, unequalised, that colour. Each is
the ear's diffuse-field response (unequalised only), then the sound and one
echo of it, the echo of opposite sign in the two ears: Lauridsen's complementary comb (1954), with
the echo as strong as the head's coherence asks at each frequency. The echo
comes later as the frequency falls, 2.3 ms at 8 kHz, 9.8 ms at 1 kHz and
30 ms below 300 Hz, so that within every third octave it is unrelated to the
sound and later than the ±1 ms over which the ear compares its two inputs.

The coherence, as the ear compares (the largest normalised cross-correlation
within ±1 ms, per third octave, ISO 3382-1's IACC by band), of ambient white
noise:

| | 125 Hz | 500 Hz | 1 kHz | 2 kHz | 4 kHz |
|---|---|---|---|---|---|
| The measured head, all 710 directions | 0.91 | 0.05 | 0.07 | 0.03 | 0.00 |
| Rendered now | 0.91 | 0.10 | 0.16 | 0.05 | 0.08 |
| Before: two decorrelated copies, no head | 1.00 | 0.87 | 0.96 | 0.53 | 0.41 |

From 100 Hz to 8 kHz no third octave is off by more than 0.085, equalised or
not. Unequalised, each ear is within 0.8 dB of the head's diffuse-field
response in every third octave from 100 Hz to 12.5 kHz (before there was no
head at all); equalised, within 0.61 dB of flat. The two ears are within
0.02 dB of each other over the whole band, and a click keeps 99.9 % of its energy within 13 ms,
with nothing after 18 ms above −48 dB. Facing any way, and turning, the ears
hear the same sample for sample: a diffuse field is the same from every side.

Unequalised, an ambient sound at level L is as loud at each ear as a point
source of level L at its reference distance straight ahead: for white
noise, +0.05 dB.
By third octave it is −0.1 dB at 125 Hz, +0.2 dB at 500 Hz, +1.5 dB at 1 kHz,
−0.3 dB at 2.5 kHz, −1.5 dB at 4 kHz and +12.9 dB at 8 kHz, where the
response from straight ahead has a deep notch that sound from all round
fills in. Before, each ear got half the power and no ear's colour, so
ambient sounds on headphones are now 3 dB louder than they were, and as
bright as an ear makes them: at the same scale, the frog listening sample
peaks 4.4 times higher. The headphone equalisation then took 10.5 dB of
that back from the frogs, whose chorus lies where the ear canal lifted it:
an ambient sound now reaches each ear at its own level, 3 dB above the half
it had before the head.

It costs one mono bus and two convolutions of 60 ms filters however many
ambient sounds play: 18.5 µs per 512-frame callback at 48 kHz with one and
21 µs with four (3.4 and 12.5 µs before), a quarter of what one point source
costs (77 to 113 µs), and nothing when none plays. Both ears' filters,
equalised and not, always run, so the probe's switch finds either whole:
two convolutions more, some 12 µs
(`cargo test --release --lib ambient_sound_cost -- --ignored --nocapture`).

`AMBIENT_OUT=target/listening/ambient-headphones/after cargo test --release
--lib write_ambient_listening_samples` writes the field wind, frogs, a robin,
clicks, and white noise while the head turns a full circle, through the whole
headphone render.

Decisions that look wrong and are right:

- **Not the velvet-noise decorrelator.** The published state of the art
  (Alary, Politis and Välimäki, DAFx 2017; Schlecht et al., DAFx 2018) was
  built first, as the 2018 paper gives it: 30 ms of signed impulses, one per
  millisecond, decaying 60 dB, then optimised for a flat third-octave
  response. It puts 99 % of a click in 10 ms, but so few impulses carry the
  energy that two channels stay alike: the best eight of 2,400 optimised
  sequences reach 0.38 within ±30 ms (the paper's own best pair 0.40), their
  bass coherence is 0.94, and only one sequence in twenty stays within 3 dB.
  The all-pass is flat by construction and decorrelates better on every
  measure.
- **The speakers' four numbers are measured, not published.** No paper gives
  a short cascade for this. Four stages, gain 0.5 and 0.5 to 2 ms is the one
  point of a coarse grid (two to six stages, gain 0.5, 0.618 or 0.7, shortest
  delay 0.5 ms, longest 1 to 3 ms) where a typical draw of delays meets every
  criterion: shorter delays leave channels alike, longer ones smear the
  attack. The seeds are the first set that passes at all three rates,
  because with filters this short a single rounded delay can make one pair
  too alike.
- **The old filter decorrelated the bass better.** Its delays were ten times
  longer. That is the price of hearing an attack once.
- **Not a set of directions around the head.** The plan was a fixed set of
  directions, each fed its own decorrelated copy and heard through its own
  measured HRIR, so that colour and coherence would follow from the ears. It
  was measured first, on every arrangement that fits the set (third-octave
  errors against the 710 directions, 100 Hz to 8 kHz for coherence, to
  12.5 kHz for colour; coherence here without the ±1 ms search, which only
  adds):

  | Directions | Coherence off by | Colour off by | With truly unrelated signals |
  |---|---|---|---|
  | Cube, 8 | 0.85–0.88 | 7.4 dB | 0.63–0.68, 3.8–6.7 dB |
  | Ring of 8 at ear height | 0.74 | 7.8 dB | 0.50, 3.8 dB |
  | Two rings, 4 at ±30° | 0.67 | 8.1 dB | 0.60, 3.4 dB |
  | Ring of 5 at −20°, 3 at +50° | 0.70 | 6.7 dB | 0.49, 1.8 dB |
  | Ring of 6, 3 at +50°, top, 10 | 0.82 | 7.2 dB | 0.47, 1.6 dB |
  | Ring of 8, 4 at +45°, top, 13 | 0.81 | 8.7 dB | 0.41, 3.3 dB |

  One signal decorrelated a few ways stays one signal below 400 Hz: the
  copies add up there, 6 to 9 dB too much bass, and what likeness is left
  between them makes the ears alike everywhere. With truly unrelated
  signals, a dozen directions still do not average the interaural phase as
  710 do. An icosahedron or a dodecahedron cannot even be placed within the
  set: turned any way, one vertex is below −52°, and nothing below −40° was
  measured. A diffuse field at two ears is three spectra; rendering those
  directly is exact by construction, and costs less.
- **Nothing follows the head.** Planned too: directions fixed in the world,
  their HRIRs following the head. A diffuse field is the same from every
  side, so there is nothing to keep in place; the ears hear the same however
  the head turns, as they do in a real one.
- **An echo, not an all-pass.** Schroeder's all-pass around the same delay
  keeps each ear's colour exact in every bin, and was built first. It puts
  most of the energy where the ears are alike at the delay: every sound's
  bass 30 to 50 ms behind its treble. The comb keeps the sound first and adds
  one echo. Its notches lie closer together than the ear resolves; smoothed
  over an equivalent rectangular bandwidth it is gone, under 0.2 dB from
  300 Hz to 10 kHz.
- **The delay's three numbers are measured.** Two turns of its phase per
  third octave, 1.2 ms past the ear's ±1 ms, and 30 ms at most. On a grid of
  turns (1, 1.5, 2, 3, 4), margin (1.0, 1.2, 1.5 ms) and longest delay (30,
  40, 50 ms, all within the 50 ms over which the ear fuses a later sound with
  the first), two turns is the fewest at which all nine combinations pass
  every test, and 1.2 ms with 30 ms gives it the most margin. Fewer turns
  leave the echo too like the sound; more push it later.
- **The measured directions are counted nearest, not interpolated.** The SOFA
  reader interpolates by blending neighbouring responses sample by sample,
  and responses that arrive at slightly different times blended dull the
  treble: over the sphere, 1.8 dB at 8 kHz and 2.6 dB at 12.5 kHz. Each
  measured direction counts for the share of 2048 evenly spread directions
  nearest to it, its Voronoi cell, which is within 0.01 dB of the exact one.
- **The coherence below 100 Hz is held at 100 Hz.** The set's measurement
  loudspeaker gave little bass (the diffuse response is 13 dB down at
  100 Hz), and its coherence falls towards 0.91 at 0 Hz, where two ears hear
  the same pressure.
- **No decorrelator on headphones.** The two filters do all of it, from one
  bus; the decorrelator stays for speakers and the ambisonic bus.

Known gaps:

- **On speakers the deep bass stays together.** Below 200 Hz two channels
  have a coherence of 0.64 on average, from −0.53 to 0.99 by pair: a filter a
  few milliseconds long cannot move a 10 ms wave far. Between speakers 3 m
  apart a real field is coherent only below about 30 Hz (sin kd / kd, Cook et
  al. 1955). On headphones this is solved: the ears' bass coherence is the
  head's.
- **On speakers a pair can be alike at one pitch.** An all-pass gives each
  pair a phase difference that changes with frequency, so a tonal sound can
  land where a pair is nearly in phase.
- **On headphones an attack where the ears are unalike has a faint echo.**
  Above 500 Hz about half of a click's energy comes again 2 to 19 ms later
  (the later, the lower), of opposite sign in the two ears. Two ears cannot be unrelated across a third
  octave unless something in each spans about the inverse of its width; a
  real diffuse field has countless such arrivals, this has one. Whether it
  is heard on a dry click is for the ear to judge.
- **The rendered ears are a little too alike where the head's are not.**
  Where its ears are unrelated (0.00 to 0.07: at 500 Hz and from 1 kHz up)
  the rendered ones reach 0.04 to 0.16: within a band the echo does not
  become quite unrelated to the sound.
- **Below the measured sphere the lowest ring stands in.** Nothing below −40°
  was measured. The renderer gives the cap under it to the −40° ring, as it
  plays a point source down there; the test counts only the measured part.
  The two differ by at most 0.4 dB and 0.03 in coherence.
- **One head.** The diffuse field is KEMAR's, not the listener's.
- **An ambient recording is not a diffuse field.** Spreading one mono
  recording makes copies of the same sound; a real chorus is many frogs in
  many places, each with its own direction and arrival. One loud frog in the
  recording is everywhere at once, and on headphones its echo is the same
  frog again. Many sources, or a multichannel recording, would be the real
  thing.
- **Attacks still flutter in the ambisonics modes.**
  `src/pipeline/stages/ambi_multi_delay.rs` mixes four delayed copies, at 7,
  11, 17 and 23 ms and each 16.5 dB down, into the whole B-format bus,
  ambient or not, so every attack there is followed by four quiet echoes.

## A sound beside the ear

On headphones the direct sound is convolved with head-related impulse
responses measured on a dummy head (`assets/hrtf/default.sofa`, MIT KEMAR) at
one distance: 1.4 m. Without more, a sound 15 cm from the ear would get the
same two ear responses as one 15 m off in that direction, only louder. In
reality, inside about a metre the near ear hears a sound markedly louder than
the far one, most of all in the bass, where far off the ears differ hardly at
all. That bass difference is what makes a sound feel beside your ear and not
merely loud. `src/audio/near_field.rs` works it out, and the headphone
renderer applies it to each ear of the direct sound.

The physics is the rigid sphere of Duda and Martens (JASA 104, 1998), a point
source at any distance from a sphere of the standard 8.75 cm head radius. The
correction is Kan, Jin and van Schaik's distance variation function (JASA 125,
2009): the sphere's response at the source's distance over its response at the
measurement distance, per ear, with the ears at ±100°. Each ear gets it as a
gain and a first-order shelf, the filter of Spagnol, Tavazzi and Avanzini
(Applied Acoustics 115, 2017), fitted to the sphere when the renderer first
loads. For a source straight to the side, the ears' level difference:

| Distance | Sphere, 200 Hz | Sphere, 4 kHz | Added to the 1.4 m set, 200 Hz | Added, 4 kHz |
|---|---|---|---|---|
| 15 cm | 16.3 dB | 23.1 dB | 14.5 dB | 13.6 dB |
| 25 cm | 9.3 dB | 16.5 dB | 7.6 dB | 7.0 dB |
| 50 cm | 4.6 dB | 12.1 dB | 2.9 dB | 2.6 dB |
| 1 m | 2.4 dB | 10.1 dB | 0.6 dB | 0.6 dB |

At 1.4 m the sphere's are 1.7 and 9.5 dB; the dummy head's own difference is
in the measured responses and is left as it is. Through the whole headphone
render, a 200 Hz tone 20 cm to the right gains 9.9 dB of level difference
over the same tone at 5 m, where the sphere says 10.1. The probe's near field
switch turns it off; off, and for any source 1.2 m or further, the output is
the same sample for sample as without it.

Decisions that look wrong and are right:

- **Spagnol's filter, not Spagnol's coefficients.** Their published table
  was the plan. Checked against the sphere, its 90° row has a sign that puts
  the bass gain 0.75 dB off where every neighbour is within 0.005 dB, its
  120° row has a pole inside the range, the unit of the corner frequency is
  not stated (it is kHz), and it is fitted against a source infinitely far
  where this needs 1.4 m. Fitting the sphere directly costs some tens of
  milliseconds once, and is within 0.81 dB of the sphere from 100 Hz to 8 kHz
  at every distance and angle tested.
- **No 1/r in the correction.** The distance variation function as published
  includes the change in level with distance. The engine already applies its
  distance law from the head's centre, so only the change in each ear's
  response relative to that is added; with both, a near sound would get
  louder twice.
- **It leaves the signal at 1.2 m, not 1.4 m.** There the sphere changes no
  ear by more than 0.22 dB at any frequency and angle, and the ears'
  difference by 0.32 dB, under the half decibel that is about the smallest
  interaural change anyone hears. At 1 m it is still 0.53 and 0.76 dB, so it
  stays in up to 1.2 m.
- **Some angles do not fade steadily with distance.** Between about 20° and
  90° from an ear, its correction grows and shrinks again as the source comes
  in, and changes sign: where the response crosses 0 dB moves towards the
  source as it nears. That is the sphere (Spagnol's "low-frequency parallax"),
  and the correction follows it.
- **Only the direct sound.** Reflections arrive from image sources metres
  away, and loudspeakers have no ears to correct.
- **Arrival time is left alone.** The sphere's interaural time difference
  grows too up close, by at most 49 µs at 20 cm and 14 µs at 50 cm at the
  onset (Duda and Martens' equation 15), and by under 20 µs in the bass. That
  is under the 90 µs anyone notices, so the measured delays stand.

Known gaps:

- **Acoustic parallax.** Each ear sees a near source from a different
  direction than the head's centre does, and the HRIR is still chosen from
  the centre: up to 26° off at 20 cm (23° more than the 1.4 m measurements
  already contain) and 10° at 50 cm (6.5° more).
- **One head for everyone.** The sphere is 8.75 cm, the measured head is
  KEMAR's; neither is the listener's.
- **A sphere has no pinna and no shoulders.** Above a few kHz near the ear
  the outer ear shapes the sound in ways that also change with distance, and
  the correction knows nothing of them.
- **A point source.** A real bird 15 cm from the ear is not a point; Duda and
  Martens' own bowling-ball measurements part from the theory inside two head
  radii for that reason. Nearer than 10 mm from the skin a source is taken as
  there.
- **The wisp seldom comes close.** As `scenes/wisp.yaml` flies it, the wisp
  is within 1 m of the listener for about 2 % of the time and within 50 cm
  for about half a percent, over ten minutes. Within 25 cm it is only for
  its first second: it starts 1.2 m above its anchor, which in that scene is
  exactly where the listener's head is.

## Two ear canals

The headphone renderer's measured responses (`assets/hrtf/default.sofa`, MIT
KEMAR, Gardner and Martin 1995) hold more than what tells one direction from
another. Every direction also carries the loudspeaker they were measured
with, whose bass falls away below 125 Hz, and the dummy head's ear canal:
its microphones sit where an eardrum would be, so its canal's resonance is
in every response, some 20 dB at 2.5 kHz over 250 Hz. On headphones the
listener's own ear canal adds its resonance to that, and a sound arrives
through two. Ambient sound, which carried that colour by design, had a frog
chorus 13.5 dB louder than before the head was added.

What every direction shares is the set's diffuse-field response, the power
average over all directions; dividing it out leaves each response only
what is its own. That is Møller's transfer function "referenced to diffuse
field" ("Fundamentals of binaural technology", Applied Acoustics 36, 1992,
§2.2, eq. 9, realised minimum-phase as he describes on p. 198), the
equalisation Larcher, Jot and Vandernoot recommend for binaural synthesis
(AES 105th Convention, 1998), and how the set's own authors equalised its
compact version (KEMAR FAQ, Q14). The renderer inverts each ear's
third-octave smoothed diffuse-field response, as the ambient sound already
measures it from all 710 directions, by Kirkeby and Nelson's regularised
inversion C* / (C* C + β) (JAES 47, 1999), and applies it as one 20 ms
minimum-phase filter per ear to the sum of every source's direct sound
(`src/pipeline/renderers/binaural/diffuse_field/inverse.rs`,
`equaliser.rs`). Ambient sound leaves the colour out instead of having it
divided out, so it reaches the ears flat, exactly.

| | 50 Hz | 100 Hz | 250 Hz | 1 kHz | 2.5 kHz | 4 kHz | 8 kHz | 12.5 kHz | 20 kHz |
|---|---|---|---|---|---|---|---|---|---|
| Equaliser, each ear (third octaves) | +13.2 dB | +12.8 dB | +9.6 dB | +4.9 dB | −10.5 dB | −3.5 dB | −3.4 dB | +2.1 dB | +10.8 dB |

Below 100 Hz and above 16 kHz it is held at its value there. The ears'
equalisers differ by 0.014 dB at most. Applied to the set's own diffuse
field, from 100 Hz to 12.5 kHz no third octave is more than 0.71 dB from
flat (0.73 dB with the set's directions weighted by ring instead), and
from 14.3 to 16 kHz it is 1.07 dB short (1.9 dB weighted by ring).

The level is pinned so that a diffuse field reaches each ear as loud as the
mono signal that made it. Against before, a point source straight ahead at
its reference distance:

| | Broadband (white noise) | 125 Hz | 250 Hz | 500 Hz | 1 kHz | 2 kHz | 4 kHz | 8 kHz |
|---|---|---|---|---|---|---|---|---|
| Change, per octave | −1.5 dB | +10.4 dB | +9.7 dB | +8.7 dB | +4.3 dB | −9.4 dB | −4.7 dB | −0.1 dB |

The middle and the bass come up, the canal's 2 to 4 kHz go down. A robin,
whose song lies there, is 7.7 dB quieter at 3 m in the listening sample. An
ambient sound now reaches each ear at its own level: the frog chorus 10.5 dB
below what the head's colour made it, and 3 dB above the half it had before
that. Within a third octave the ratio of an ambient sound to a point source
is what it was, since both are divided by the same.

What stays: the interaural level difference of a source to one side, the
ears' level difference at each frequency averaged over each octave from
500 Hz to 8 kHz, moves by under 0.01 dB; its interaural time difference not
at all; the ears' coherence in an ambient sound stays the measured head's,
within 0.08 in every third octave from 100 Hz to 8 kHz. The near field is a ratio of two sphere
responses and does not see the equaliser: through the whole render a 200 Hz
tone 20 cm to the right still gains 9.9 dB of level difference.

The probe's headphone equalisation switch turns it off; off, the render is
the one before, sample for sample (point sources raw, ambient sound
coloured), and switching glides over one callback. It costs 12 µs per
512-frame callback at 48 kHz for the direct sound and 12 µs for the second
pair of ambient filters: `scenes/nature.yaml` on headphones took 445 µs and
takes about 465
(`cargo test --release --lib headphone_scene_cost -- --ignored --nocapture --test-threads=1`).
`HEADPHONE_EQ_OUT=target/listening/headphone-eq cargo test --release --lib
write_headphone_equalisation_listening_samples` writes a robin ahead and
circling, the frogs and the field wind as ambient sound, and 15 s of the
meadow, each equalised (`on/`) and not (`off/`) at one scale.

Decisions that look wrong and are right:

- **One filter on the sum, not in every response.** The equaliser is the
  same for every direction, so filtering the sum of the sources is the
  same as filtering each, and it costs one filter however many sources
  play. Reflections are panned, not convolved with the measured responses,
  and the late reverb never was; neither carries the colour, so neither is
  equalised.
- **Held below 100 Hz.** Each response is 512 samples at 44.1 kHz, 11.6 ms:
  nothing finer than 86 Hz is resolved in it. Below that the set's diffuse
  response falls 13 dB an octave with the loudspeaker to 40 Hz and then lies
  flat at −33 dB, which is where the response was cut off, not a sound.
  Following it down would lift the bass by 30 dB on the strength of no
  measurement; 100 Hz is the lowest third octave above what the response
  resolves.
- **Held above 16 kHz.** From 12.7 to 16 kHz the set falls 9 dB in one
  third octave, more steeply than anywhere else, into the end of what was
  measured. The set's authors held their own inverse of this loudspeaker
  above 15 kHz (the documentation says 18 kHz); 16 kHz lies between.
- **The regularisation is the cap and nothing else.** Kirkeby and Nelson's
  inverse can never lift more than 1/(2√β). β is set so that is 20 dB above
  the equaliser's geometric-mean gain from 100 Hz to 16 kHz, the clip the
  set's authors used inverting this loudspeaker (FAQ, Q15). It is not
  reached: the largest lift, 13.2 dB below 100 Hz, is 10 dB under it, and
  within the trusted range the regularisation changes the inverse by at most
  0.21 dB, at 100 Hz. Where a set had a hole deeper than the cap, the
  inverse would fall again rather than fill it.
- **Ambient sound leaves the colour out; it is not divided out.** Dividing
  the head's colour out of itself gives flat, so the ambient filters simply
  have none. Below 100 Hz, where the equaliser stops following the set, an
  ambient sound is still flat; a point source there keeps the measurement
  loudspeaker's fall.
- **Both renders always run.** The equaliser filters the sum, and the ears'
  filters run with and without the colour, whether the switch is on or off,
  so when it changes both are whole and the glide is a crossfade between
  two complete renders. Started only when switched to, each would miss the
  tail of what came before: up to 60 ms of the ambient echo.
- **Unit gain for a diffuse field, not for a point ahead.** Free-field
  equalisation would make the frontal response flat instead. A scene is
  heard from all round, an ambient sound is all round, and the reference
  that is the same for every direction is the diffuse field (Møller, §5).
- **Within a third octave the ears were never balanced.** The ambient
  sound's echo has the opposite sign in the two ears, so by third octave
  they are up to 1.1 dB apart (1.5 dB unequalised) and alike over the whole
  band. The equalisation does not change that.

Known gaps:

- **One dummy head for everyone.** It removes KEMAR's ear canal, not a
  listener's; what remains is KEMAR's pinna and head, not theirs.
- **No headphone is modelled.** Headphones differ from each other by as much
  as the colour taken out here; the render is right for headphones that are
  themselves diffuse-field flat, and no others.
- **Below 100 Hz a point source keeps the loudspeaker's fall.** What the set
  holds there is the measurement loudspeaker, not the head, and the
  equaliser stops following it, so point sources lose their deep bass as
  before (ambient sound does not).
- **The SOFA reader dulls the treble of point sources.** It interpolates
  between measured directions by blending neighbouring responses sample by
  sample; responses arriving at slightly different times blended lose
  treble, 1.8 dB at 8 kHz over the sphere on average (the ambient
  measurement counts the measured directions themselves and does not). The
  equaliser divides out the measured responses' colour, not this, so a
  point source between measured directions stays a little dull. Nobody has
  fixed it.
- **The steepest third octave is short.** From 14.3 to 16 kHz the smoothed
  inverse of so steep a fall leaves the diffuse field 1.07 dB down; above
  16 kHz it is not followed at all.

## An arc of ambient sound

Many ambient sounds are not around the listener but on one side: a
shoreline, a tree line, a river bank, traffic beyond a hill. Such a sound is
many unrelated contributions from the directions of an arc, each as strong
as the next. `shape.arc` gives the arc's width in degrees, from 10 to 360;
left out it is 360, all round, and everything is rendered exactly as before
there were arcs, sample for sample, in every mode. The arc is centred on the
bearing over the ground from the receiver (what the mode measures paths to:
the listener; for world-locked the middle of the speaker ring it faces out
from) to the sound's place, and lies along the horizon. Moving the place
moves the arc; the level stays the sound's level wherever the place is. A
point ignores its arc, as an ambient sound ignores its width.

**On headphones** it is the diffuse-field computation of the measured head
restricted to the arc (`src/pipeline/renderers/binaural/diffuse_field/arc/`):
each ear's power and the two ears' complex cross-spectrum, the mean over the
arc's directions, each counted for the solid angle it stands for. The band is
the three measured rings nearest the horizon, −10°, 0° and +10°, each for its
±5°: the sounds an arc is for lie along the horizon at a distance (water's
edge 7 m or more from ears 1.7 m up is within 15° below it, a tree line 10 m
high 30 m off within 15° above, and the ground's reflection of either in the
band too). Unlike all round, the cross-spectrum is complex: its phase is the
time by which the near ear hears the arc first. Two filters per sound give
the ears exactly these three spectra: the all-round pair's sound and echo
(Lauridsen's complementary comb), with the right ear's mix of the two turned
by the complex coherence, `left = a + b·D`, `right = γ*·(a + b·D) + s·(b − a·D)`;
the coherence averaged over each third octave after the arc's time
difference is turned back, and given back half to each ear; each ear's
colour the arc's power over the head's diffuse field, divided by what the
headphone equaliser does to it, so that the arc joins the direct sound on the
equaliser's bus and comes out of it with exactly its share of the diffuse
field. All round that share is one and the filters are the all-round pair.

The arc is fixed in the world and the head turns, so each width is designed
once for every bearing relative to the head, 5° apart, 72 pairs of 60 ms
(40 ms to design all of them on a laptop's cores), and the audio thread picks
the nearest and crossfades to it over a callback, with two pairs of
convolvers as a point source does. Widths are designed in whole degrees: the
widths a scene starts with before audio runs, a width set while it plays on
a thread of its own, the sound meanwhile heard as it was. A sound going from
all round to an arc, or back, glides between the all-round bus and its
filters over a callback.

Against the head (`src/pipeline/tests/ambient_arc_headphones.rs`; ambient
white noise; arcs 30°, 90°, 180° wide centred ahead, at 60°, to the left and
behind; worst third octave from 100 Hz to 8 kHz, after the equaliser):

| Arc | Each ear's level | Level difference, from 500 Hz / below | Coherence | Time difference |
|---|---|---|---|---|
| 30° | 0.95 dB | 0.95 / 0.28 dB | 0.050 | 3 µs |
| 90° | 0.77 dB | 0.97 / 0.97 dB | 0.046 | 19 µs |
| 180° | 0.82 dB | 0.59 / 1.43 dB | 0.056 | 42 µs |

The head itself, for a 30° arc to the left: the ears differ by 4.5 dB at
500 Hz and 11.2 dB at 4 kHz, their coherence is 0.995 and 0.57, and the left
ear hears it 700 µs first. Straight ahead the ears are within 0.04 dB of
each other and the time difference is under 19 µs. A click keeps 94 % or
more of its energy within 13 ms at each ear. Turning the head at 180°/s
through a 60° arc, no step between two samples is larger than with the head
held at any bearing it passes, and the level difference follows the turn
within 0.1 dB of the held one.

**On speakers** each speaker keeps its own decorrelated copy, at the share of
the arc's power that panning over the arc would give it: the integral over
the arc of its squared panning gain, the layout's own two-dimensional VBAP
(Pulkki 1997) seen from the receiver, normalised so the shares add up to the
sound's power (`src/pipeline/renderers/field/speaker_shares.rs`). On the
ambisonic bus the arc is a covariance, not a direction: the mean over the arc
of each direction's encoding times itself (Epain and Jin, IEEE/ACM TASLP 24,
2016), in closed form, with a plane wave's trace, so the energy-preserving
decoder plays it as loud as a point; three decorrelated copies mixed by its
Cholesky factor give the bus exactly that covariance
(`src/pipeline/renderers/field/bformat_arc.rs`). Its intensity points at the
arc's middle with length sin α / α for an arc 2α wide, the arc's energy
vector in Gerzon's sense ("General metatheory of auditory localisation", AES
92nd Convention, 1992).

The energy vector at the speakers of `scenes/nature.yaml`'s 5.1 layout,
through the whole pipeline (direction off the arc's middle, worst of centres
ahead, left, behind and right; length ahead):

| Mode | 60° | 120° | All round, length |
|---|---|---|---|
| VBAP | 0.0°, 0.90 | 0.5°, 0.74 | 0.16 |
| DBAP, world-locked | 5.1°, 0.92 | 6.0°, 0.77 | 0.26 |
| Ambisonics | 4.0°, 0.67 | 6.2°, 0.58 | 0.08 |

The speakers carry the source's power within 0.1 dB at every width; on the
ambisonic bus an arc decodes as loud as a point within 0.1 dB.

It costs one forward transform and two convolutions of 60 ms per arc and
callback, four in a callback where the head crosses to the next bearing, and
nothing for a sound all round: `scenes/nature.yaml` on headphones with four
winds added at the compass points took 645 µs per 512-frame callback at
48 kHz before arcs, takes 647 to 656 µs with them all round, and 686 to
698 µs with each from a 90° arc (the all-round filters then fall silent).
Each width's filters take 1.7 MB at 48 kHz; the designer keeps the last four
(`cargo test --release --lib ambient_arc_cost -- --ignored --nocapture`).

`AMBIENT_ARC_OUT=target/listening/ambient-arc cargo test --release --lib
write_ambient_arc_listening_samples` writes the field wind as a 90° arc on
the left and all round, the frogs ahead and to the right, white noise ahead
while the head turns a full circle, running water behind, and the wind on
the left in VBAP to 5.1.

Decisions that look wrong and are right:

- **The arc's filters go through the equaliser.** Ambient sound all round
  leaves the head's colour out of its filters instead; an arc is the head
  from some directions, which is what the equaliser divides by the diffuse
  field, so it is on the same footing as a point source. Its filters are
  designed with the equaliser's own response divided out of the colour, so
  through it each ear gets the arc's share of the diffuse field exactly,
  below 100 Hz too, where the equaliser stops following the set; all round
  that is flat, as the all-round filters give it, so an arc widened to 359°
  sounds as the 360° it is next to. One pair of filters per arc, not two:
  the equaliser on the sum serves both renders the probe switches between.
- **Five turns of the echo per third octave, not two.** All round, the ears
  only need to be alike or not; an arc asks for a level and a time
  difference between them, and within a third octave the echo adds a little
  to one ear and takes from the other. Measured on a grid of two to six
  turns and a longest echo of 25 to 40 ms, five turns and 30 ms keep the
  level difference within 1 dB from 500 Hz up and the time difference
  straight ahead under 20 µs, while 94 % of a click still arrives within
  13 ms (six turns: 92.6 %). The longest echo stays the all-round 30 ms;
  40 ms made the bass worse, not better.
- **The time difference is read from the phase to the nearest half turn.**
  A half circle ahead is alike between the ears in the bass and opposed
  around 500 Hz: its cross-spectrum changes sign, which a phase followed to
  the nearest whole turn, or the strongest shift of the cross-correlation,
  takes for a time difference of 800 µs.
- **One band of elevation for every arc.** The ears' level difference and
  coherence hardly depend on it: for a 30° arc to the left, the ring at the
  horizon alone gives 4.3 dB and 9.8 dB (500 Hz, 4 kHz) and a coherence of
  0.58 at 4 kHz; ±15° gives 4.5 dB, 11.2 dB and 0.57; ±25° gives 4.7 dB,
  11.6 dB and 0.44. The time difference is 706, 700 and 693 µs.
- **On speakers the arc lies on the horizon,** and on the ambisonic bus it
  has no height: a 5.1 layout has nothing above or below it.
- **The world-locked arc is seen from the middle of the speaker ring,** not
  from the point its paths are measured to, which is the listener: world-locked
  is meant to be the same for everyone in the room, and its microphones
  already face out from that middle (`Renderer::ambient_receiver`).

Known gaps:

- **Below 500 Hz the level difference reaches 1.4 dB.** There 30 ms of echo
  turns fewer than three and a half times across a third octave, too few to
  average out; a half circle ahead or behind, whose ears are only partly
  alike there, is 1.43 dB off at 200 Hz and 1.28 dB at 400 Hz. Longer echoes
  would be heard as echoes.
- **Behind a 5.1 layout a narrow arc and a wider one sound the same.** The
  rear speakers of `scenes/nature.yaml` are 112° apart, and any arc between
  them is shared by those two equally: 60° and 120° behind both have an
  energy vector 0.554 long in VBAP. That is panning; only more speakers
  behind can tell them apart.
- **A real tree line also has a distance and a near part.** The arc is
  directions only: no part of it is nearer, louder or earlier than another,
  and no part of it is a point the listener can walk up to.
- **The arc's level does not change with how far its place is.** Only its
  direction follows the place. How loud a shore is at the listener is the
  scene's level, as for every ambient sound.
- **One elevation band for all arcs.** A tree line close by rises higher than
  ±15°, and a sea seen from a cliff lies below it.
- **On ambisonics the all-round field is quieter than an arc.** All round it
  puts a quarter of its power in Z, which a horizontal layout never plays:
  1.25 dB below a point or an arc of the same level, so an arc widened to
  all round there gets 1.25 dB quieter. It was so before arcs, and all round
  is left exactly as it was.
- **The arc's filters are KEMAR's,** as all of the headphone render is.
- **A new width on headphones arrives a moment late.** Designed beside the
  audio thread, it takes some tens of milliseconds; until then the sound is
  heard with the width it had.

## Project structure

```text
src/
├── main.rs                 # entry point: load a scene, open the device, run
├── audio/                  # decoding, output device, filters, atmosphere,
│                           #   ISO 9613-2 ground effect, HRTF convolver
├── synth/                  # procedural sources, one file or folder per generator
├── config/                 # scene YAML → AudioScene
│   ├── scene.rs            #   the scene file's own fields
│   ├── environment.rs      #   environment file, wall material names
│   ├── source.rs           #   source files (recording or synth)
│   ├── synth/              #   one file per synth's parameters
│   └── build.rs            #   builds the engine's AudioScene
├── engine/
│   ├── scene/              # AudioScene: all state owned by the audio thread;
│                           #   commands, render, channels, edits, reset
│   ├── edit.rs             # live add/remove of sources
│   ├── telemetry.rs        # levels and positions sent back to the control side
│   ├── tap.rs              # a copy of every finished block, for the live analyser
│   ├── profiler.rs         # profile_span! (feature `profiler`)
│   └── memprof.rs          # allocation tracking (feature `memprof`)
├── pipeline/
│   ├── environment.rs      # Environment: bounds, wall materials, ground, air
│   ├── wall_material.rs    # absorption per octave band
│   ├── path.rs             # PathResolver and PathEffect contracts
│   ├── path_resolvers/     # direct path, image sources
│   ├── path_effects/       # delay, air absorption, ground effect, wall absorption
│   ├── renderer.rs         # Renderer contract, SourceContext
│   ├── renderers/          # one file per renderer, plus diffuse field emitters
│   ├── mix_stage.rs        # MixStage contract
│   ├── stages/             # reverb, bass management, delay compensation, …
│   ├── room_acoustics.rs   # Sabine RT60, critical distance, delay sizing
│   ├── builders/           # which pieces each render mode is made of
│   ├── render.rs           # RenderPipeline and the per-buffer render loop
│   └── tests/              # end-to-end pipeline tests, grouped by subject
├── lab/                    # the generator lab: candidates in step in one voice,
│                           #   a seeded run's state kept at its cue, level
│                           #   matching, the session file (docs/generator-lab-plan.md)
├── plan/                   # the session: which take each voice utters when,
│                           #   and the weather story, decided ahead from the seed
├── offline/                # a scene rendered to a file: settings, the render,
│                           #   levels, provenance, the WAV and its sidecar
├── head_tracking/          # OpenTrack UDP, Sony headphone bridge
├── server/                 # WebSocket telemetry and control (port 3333)
└── bin/                    # `render` (a scene to a file), and analysis tools
                            #   for recordings and synths

crates/
├── core/                   # atrium-core: math and panning with no I/O
│                           #   (Vec3, listener, speakers, VBAP, DBAP, ambisonics,
│                           #    directivity, commands, telemetry types)
├── ffi/                    # the engine as one object for the Kotlin app
├── tui/                    # terminal dashboard (`--ui`)
└── world/                  # atrium-world: a scene's sounds as entities
                            #   (id, sound, shape, place, moves)

tests/
└── audio_thread_allocations.rs   # the audio callback never touches the heap
```

Crate conventions (private modules behind a small facade, one implementation
per file) are in [AGENTS.md](AGENTS.md).

## Rendering to a file

`cargo run --release --bin render -- <scene.yaml>` plays a scene into a WAV file
(see [run.md](run.md#render-a-scene-to-a-file)), for analysing and comparing what
the engine plays. `src/offline/` drives the scene exactly as the audio callback
does, block by block through `AudioScene::process_commands` and `render`, prepared
as the output device prepares it, so a file cannot drift from what is heard; a test
holds it to a scene driven by hand.

Decisions that look wrong and are right:

- **The plan is driven in step, not on its thread.** Playing, the plan runs on its
  own thread, waking every 50 ms of the wall clock and handing over what is due
  0.4 s ahead of the audio clock. Rendered twenty times faster than real time, that
  thread would fall behind: takes would arrive late or not at all, differently on
  every run. Offline the same plan is driven between blocks instead
  (`plan::SteppedPlanner`): before each one, everything due to its end plus the same
  lead is taken and converted exactly as the live planner converts it. What a voice
  utters and when depends only on the seed, so the takes are the live planner's,
  which a test shows against the live planner stepped by a hand-moved clock. Only
  the air and the settings of generators that follow the weather are taken at every
  block instead of every wake, as they stand at that moment.
- **A render without a seed is not a new session.** A scene without `seed:` plays
  differently every time; rendered, it takes seed 1, so the same arguments give the
  same file.
- **`--start` moves the plan, not the generators.** As the timeline does: the takes
  are where they would be and a take under way starts partway, but a wind or a river
  plays on from its own beginning and the reverb starts empty. The birds of a render
  from 20 s are the last part of a render from 0; its wind is not.
- **The 16-bit format truncates as the 16-bit output does**, `x * 32767` towards
  zero, so a 16-bit file is what a 16-bit device would have been sent.

## Threads

```text
Control thread                          Audio thread (cpal callback)
     │                                        │
     │── Command (rtrb ring buffer) ────────▶ │  process_commands
     │   SetListenerPose, SetRenderMode, …    │
     │                                        │  AudioScene::render
     │                                        │    tick sources
     │                                        │    render_pipeline (active mode)
     │ ◀──── output blocks (rtrb, the tap) ───│    copy the output to the tap
     │ ◀──────── TelemetryFrame (rtrb) ───────│    push telemetry
     │                                        │
     │                                   cpal → device
```

All five pipelines are built at startup, so switching mode allocates nothing.

One more thread runs beside the audio thread on headphones: it designs the
filters of an ambient sound's arc when its width changes while the scene
plays, and frees the ones no sound uses any more. It talks to the audio
thread through three rings of its own (see "An arc of ambient sound").

**The audio callback must not allocate, lock or do I/O.** Commands arrive over
a wait-free ring buffer and all state is owned by `AudioScene` on the audio
thread. `tests/audio_thread_allocations.rs` enforces the allocation part: it
counts every heap operation inside the callback, in every render mode, across
the shipped scenes, with live commands arriving.

## Profiling

Two optional features, both compiled out when disabled. Commands are in
[run.md](run.md#profiling).

- **`profiler`** instruments the render path with `tracing` spans and can write
  terminal timings, a Perfetto trace or a flame graph.
- **`memprof`** counts allocations on the audio thread per stage while the
  engine runs.

| Span | Location | What it measures |
|---|---|---|
| `callback` | `audio/output.rs` | The whole cpal callback |
| `process_commands` | `engine/scene/commands.rs` | Draining the command queue |
| `render` | `engine/scene/render.rs` | One buffer: tick, analysis, pipeline, telemetry |
| `source_tick` | `engine/scene/render.rs` | Advancing every source |
| `perceptual` | `engine/scene/render.rs` | Masking analysis (telemetry only) |
| `pipeline` | `engine/scene/render.rs` | The active render pipeline |
| `renderer` | `pipeline/render.rs` | One source through its renderer |
| `mix_stages` | `pipeline/render.rs` | The post-mix chain |
| `telemetry` | `engine/scene/render.rs` | Building and pushing a telemetry frame |

For external profilers (Instruments, perf) there is a `profiling` Cargo profile:
a release build with debug symbols.

```bash
cargo run --profile profiling -- scenes/default.yaml
```

## Dependencies

| Crate | Purpose |
|---|---|
| [cpal](https://github.com/RustAudio/cpal) | Audio device I/O |
| [symphonia](https://github.com/pdeljanov/Symphonia) | Decoding MP3, WAV, AAC, Ogg Vorbis |
| [rtrb](https://github.com/mgeier/rtrb) | Wait-free single-producer ring buffer |
| [realfft](https://github.com/HEnquist/realfft) | FFT for the HRTF convolver |
| [sofar](https://crates.io/crates/sofar) | Reading SOFA HRTF files |
| [tungstenite](https://github.com/snapview/tungstenite-rs) | WebSocket server |
| serde, serde_yaml, serde_json | Scene files and telemetry |

## Extension points

| Trait | File | Implement it to add |
|---|---|---|
| `SoundSource` | `crates/core/src/source.rs` | A new kind of sound (a synth, a player) |
| `PathResolver` | `src/pipeline/path.rs` | Another way sound reaches the receiver (for example ray tracing) |
| `PathEffect` | `src/pipeline/path.rs` | Per-path processing |
| `Renderer` | `src/pipeline/renderer.rs` | A spatialisation method |
| `MixStage` | `src/pipeline/mix_stage.rs` | Whole-buffer processing after the mix |
| `AudioOutput` | `src/audio/output.rs` | Another audio backend |

A new render mode is a new file in `src/pipeline/builders/` that picks one of
each.

## Further reading

- [docs/physics-audit-decisions.md](docs/physics-audit-decisions.md): why the models are the way they are
- [docs/living-atrium-roadmap.md](docs/living-atrium-roadmap.md): the synthesis roadmap
- [docs/listener-tracking.md](docs/listener-tracking.md): head and position tracking
- [REFERENCES.md](REFERENCES.md): libraries and papers the engine draws on
