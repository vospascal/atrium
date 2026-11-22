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
| `hrtf` | headphones | direct + reflections | HRTF convolution to two ears, equalised for headphones; ambient sound through the head's diffuse field | reverb, master gain |
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
  the last thing wanted while listening closely.
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

An ambient sound (`shape.ambient`) has no position: it is the sound around
the listener. Any sound can be ambient: a frog chorus, a robin, rain with
single drops. So whatever spreads it around must leave an attack heard once.

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
├── head_tracking/          # OpenTrack UDP, Sony headphone bridge
├── server/                 # WebSocket telemetry and control (port 3333)
└── bin/                    # analysis tools for recordings and synths

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

## Threads

```text
Control thread                          Audio thread (cpal callback)
     │                                        │
     │── Command (rtrb ring buffer) ────────▶ │  process_commands
     │   SetListenerPose, SetRenderMode, …    │
     │                                        │  AudioScene::render
     │                                        │    tick sources
     │                                        │    render_pipeline (active mode)
     │ ◀──────── TelemetryFrame (rtrb) ───────│    push telemetry
     │                                        │
     │                                   cpal → device
```

All five pipelines are built at startup, so switching mode allocates nothing.

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
