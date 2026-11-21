# Thunder: what it is, and how Atrium makes it

Written 2026-11-21, when the first port (filtered noise bursts from the TypeScript garden)
sounded artificial. This note says what real thunder is made of, what that port got wrong,
and what the generator in `src/synth/thunder.rs` does instead.

## What thunder is

A lightning channel is kilometres long and jagged. When the current passes, every bit of it
heats to some 30 000 K in microseconds and bursts outward. Each bit sends out a short pressure
pulse shaped like the letter N: a sudden rise, a fall through zero, a sudden return. Thunder is
all of those pulses, from every bit of the channel, arriving at the listener one after another.

- **Why it rolls.** Sound takes about 3 seconds per kilometre. The channel's parts lie at
  different distances, so their pulses arrive over seconds: a channel spread over 3 km of
  distance is heard for about 9 s.
- **Why there are claps.** Where a stretch of channel lies side-on to the listener, all its
  parts are at nearly the same distance and their pulses arrive together: a loud clap. Ribner
  and Roy (1982) showed that such side-on stretches radiate most strongly, which is where the
  peaks in a rumble come from. Where the channel points at the listener, its pulses smear out
  into a rumble.
- **Why far thunder is dull.** The air absorbs high frequencies far more than low ones: roughly
  5 dB per kilometre at 1 kHz but well under 1 dB at 100 Hz. Near thunder still has its tearing
  crack; far thunder is a low grumble. The parts of one flash that are farther away arrive later
  and duller, so a rumble darkens as it goes on.
- **Its pitch.** The pulses lengthen as they travel (Few, 1969), so thunder's energy sits low:
  mostly 50–250 Hz, with an infrasonic part below 20 Hz that is felt more than heard.
- **Kinds of flash.** A strike to the ground is a mostly vertical channel with branches: from
  close by, a sharp crack and a short roll. A flash inside the cloud runs horizontally for
  many kilometres: long rolling thunder that swells and fades. From 10 km or more only the low
  part arrives.

## The reference

`assets/weather/thunder/thunder 1.mp3` (added by Pascal, 5.8 s): a sudden onset with a brief
bright burst, about 3 s of body, then a tail that darkens as it fades. Centroid 199 Hz, most
energy at 100–250 Hz, almost nothing above 1 kHz: a ground strike a few kilometres off, as the
physics predicts.

## What the first port got wrong

It was filtered noise: a noise burst with three fixed resonators for the crack, then brown noise
for the rumble with random swells added. Noise has no claps that come from geometry, its swells
were drawn at random rather than arising from a channel, and its brightness did not fall as the
rumble went on. That is why it sounded like a synthesizer rather than weather.

## What the generator does now

Each clap is a lightning channel built fresh from the seed, and its sound is worked out from the
geometry, in the way of Ribner and Roy (1982), Dunkin and Fleisch, and the real-time model of
Fineberg, Walters and Reiss:

1. **A channel.** A random walk of short segments with a mean turn of about 16° from one to
   the next, which is the tortuosity measured on real channels. A ground strike climbs from its
   ground point to the cloud with a few branches; a cloud flash runs horizontally at 3–5 km.
2. **A pulse per segment.** An N-wave a few milliseconds long, longer from farther away, louder
   where the segment lies side-on to the listener, softer with distance.
3. **Air on the way.** Each pulse passes a low-pass filter set by its own distance, so the
   farther parts of a flash arrive darker.
4. **Arrivals in time.** Every pulse is placed at its travel time. The rumble, its claps, its
   swells and its darkening all come from that, not from envelopes drawn by hand.
5. **The infrasonic swell.** One slow pressure pulse under the whole clap, for the LFE channel.

The four kinds are the generator's takes: **crack** (a ground strike a few hundred metres
away), **clap** (one 1.5–3 km away, like the reference), **rolling** (a long cloud flash) and
**distant** (8–14 km). The plan chooses among them; no two claps are the same channel.

## Known gaps

- Pulses are summed linearly. Very close to a strike the shock is non-linear, which this does
  not model; it matters only within a few hundred metres.
- There are no echoes from hills or buildings, which lengthen real rumbles in a valley.
- Wind and temperature layers that bend sound upward, and so silence thunder beyond some
  15–25 km, are not modelled; the distant kind simply stays within that range.

## Sources

- [Thunder acoustics, Engineering Acoustics (Wikibooks)](https://en.wikibooks.org/wiki/Engineering_Acoustics/Thunder_acoustics)
- [The Earth's Electrical Environment, chapter on thunder (National Academies Press)](https://books.nap.edu/read/898/chapter/31)
- [Ribner and Roy, Acoustics of thunder: a quasilinear model for tortuous lightning](https://scilit.com/publications/013b9b0ef4e322466fa8a5cc508cbd10)
- [Dunkin, digital simulation of thunder from three-dimensional lightning](https://www.wittenberg.edu/sites/default/files/media/physics/dunkin_sr_pres.pdf)
- [Fineberg, Walters and Reiss, real-time thunder synthesis](https://arxiv.org/pdf/2204.08026.pdf)
- [Stephens, natural sources of low-frequency sound (Institute of Acoustics)](https://www.ioa.org.uk/system/files/publications/RWB%20STEPHENS%20NATURAL%20SOURCES%20OF%20LOW%20FREQUENCY%20SOUND.pdf)
