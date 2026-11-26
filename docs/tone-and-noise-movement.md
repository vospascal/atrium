# How tones and noise move

Research, 2026-11-24, on making the focus tones, the beats and the noise colours less static.
It started from AI-chat summaries Pascal pasted (Patterson's damped and ramped envelopes, decay
times for pulses from 2 to 40 Hz, harmonics, binaural, monaural and isochronic beats). Every
claim here was checked against the papers. The last part measures seven ambient pieces Pascal
chose from YouTube. Nothing is built from this yet: tools before generators.

## The pasted claims, checked

| Claim | Verdict | Source |
|---|---|---|
| Damped sounds (fast rise, slow decay) are clear and pitched; ramped ones (slow rise, abrupt stop) are buzzy with weak pitch | **Reversed.** Ramped sounds are heard as *more* tonal, with the carrier's pitch; damped ones as a tone plus a drum, weaker pitch. Damped noise sounds like a brushed snare | Patterson 1994a,b, JASA 96:1409 and 1419; Akeroyd & Patterson 1995, JASA 98:2466; Shen & Lentz 2011 |
| Ramped sounds are judged 3–6 dB louder | **Overstated, and from a different paper.** About 1–3 dB, under 1 dB after a ramped sound; up to about 4 dB for 200–500 ms noise against a damped standard. Holds from 330 Hz to 6 kHz and for noise | Stecker & Hafter 2000, JASA 107:3358 (read in full); Ries, Schlauch & DiGiovanni 2008, JASA 124:3772 |
| The cause is hair-cell fatigue and an offset burst | **Not from the cited work.** Patterson explains it with time intervals; Stecker & Hafter with "decay suppression" (a slow decay is taken for the room's reverberation and discounted) | as above |
| Nature is mostly damped, so damped sounds safe | **Not shown.** Turbulence is symmetric in time; asymmetry comes from a mechanism (an impact, a wave breaking) | no survey found |
| Temporal modulation sensitivity peaks at 4–7 Hz | **Wrong for hearing a modulation.** Detection is flat from about 2 to 10 Hz (about −25 dB, m ≈ 0.06) and falls slowly above. About 4–5 Hz is the peak of speech's envelope spectrum, about 2 Hz music's | Viemeister 1979; Ding et al. 2017 |
| The roughness threshold is about 20 Hz | **Holds.** Fluctuation strength peaks near 4 Hz; roughness starts around 15–20 Hz and peaks near 70 Hz. A 40 Hz pulse is heard as rough, not as a rhythm | Zwicker & Fastl, *Psychoacoustics* |
| 40 Hz pulses lock the brain's steady-state response; 10 Hz "maximises locking" | **40 Hz is real** (the largest response); the 10 Hz claim is unsupported. A larger response is not a benefit | Galambos et al. 1981, PNAS; Ross et al. 2000 |
| τ ≈ 0.28/f_b, rise = clamp(0.18/f_b, 4.5 ms, 15 ms), floor 20–50 % | **Arithmetic right, no source.** At the next onset the pulse is at e^−3.57 ≈ 2.8 %. But that is a half-life of about 0.19 of a period (4.9 ms at 40 Hz): exactly the damped range where a tone loses pitch and sounds about 1–3 dB quieter. A floor of 20–50 % is a 6–14 dB swing, strongly audible, not subtle | derived; Patterson; Stecker & Hafter |
| Binaural beats need a carrier under 1 kHz and a difference under about 30 Hz | **Roughly right.** Best near 400–500 Hz, falling off towards 1–1.5 kHz; difference up to about 30–35 Hz | Licklider 1950; Perrott & Nelson 1969; Oster 1973 |
| "333 Hz music" has many tones because of harmonics, binaural beats and layering | **Holds**, and fits what was found for 150 such tracks: mostly slow, simple music | Yuvacı 2026 (see `focus-tones.md`) |
| 47 focus presets | **No: 34** focus tones and 13 binaural pieces, both in `sources/tones/` | the repo |

## Pulses: shape, depth, rate

What a pulse train sounds like depends on three things, in this order of strength:

- **Rate.** Under about 15 Hz it is heard as fluctuation, strongest near 4 Hz. From 15 to
  300 Hz it is roughness. A pulse rate above about half the carrier's auditory bandwidth
  (ERB = 24.7·(4.37·f/1000 + 1), Glasberg & Moore 1990) splits into separate tones: 40 Hz on
  108 Hz is heard as 68 and 148 Hz, not as pulsing.
- **Depth.** Annoyance grows with depth more than with anything else (wind-turbine studies:
  Ioannidou et al. 2016, JASA 139:1241; UBA TEXTE 70/2022). Use the least depth that is
  heard. Periodic modulation is worse than random: at 0.75 Hz with a 3 dB swing it was as
  annoying as +3 dB of level (Schäffer et al. 2018, IJERPH 15:1029).
- **Shape.** Damped is more percussive, less tonal and about 1–3 dB quieter. Ramped is more
  tonal and louder. Symmetric (raised cosine or sin^N) avoids both the loss of pitch and the
  loudness bias. Patterson's tests used periods of 12.5–25 ms (40–80 Hz) and half-lives of
  2–16 ms; slower rates were not tested.
- **Edges.** No published click threshold was found. A physical rule: a rise of at least
  1/ERB(carrier) keeps the onset's spread inside one auditory filter, which is about 7.5 ms at
  1 kHz and about 28 ms at 100 Hz. The chat's fixed 4.5 ms floor fails below about 500 Hz.

## Speaker beats: monaural, AM, isochronic

On the 5.1 speakers both ears hear both binaural tones, so a speaker version needs a beat in
the sound itself. The three kinds differ physically:

| | Spectrum | Envelope |
|---|---|---|
| Monaural beat (two tones summed) | two lines, nothing at the centre | rectified cosine: deep sharp nulls, with the fine structure flipping 180° at each null |
| Sinusoidal AM (100 %) | carrier, and sidebands at −6 dB | a sine; no phase flip |
| Isochronic (square gate, 50 %) | carrier −6 dB, sidebands at odd multiples (−3.9, −13.4, −17.9 dB …) | on/off with clicks unless ramped |

- **Sound modulation drives the brain's response more than binaural beats do.** At 40 Hz,
  monaural beats gave a 6.3 dB larger steady-state response than binaural beats (Orozco Perez
  et al. 2020, eNeuro 7(2); also higher at 7 Hz). Engelbregt et al. 2021 (Exp Brain Res
  239:2781) found monaural above binaural (r = 0.60), and binaural no higher than pink noise.
  Neither study found a change in mood. Reviews: Garcia-Argibay et al. 2019 (g = 0.45 overall,
  with large differences between studies); Ingendoh et al. 2023 (5 studies support
  entrainment, 8 contradict it).
- **The binaural beat is weak.** At 4 Hz it was judged as strong as AM of depth 0.087 (Grose,
  Buss & Hall 2012). The 3D version's accidental monaural leak (depth 0.085, the 1.4 dB swell
  in `binaural-beats.md`) is about as strong as the binaural beat itself.
- **A room spreads the depth across seats.** At a seat the depth of a two-tone beat is the
  ratio of the room's response at the two frequencies. Above about 5 Hz of difference these
  vary independently from seat to seat (single-tone level σ ≈ 5.6 dB). Reverberation smooths
  the envelope too: with RT 0.5 s depth keeps 0.91 at 2 Hz, 0.40 at 10 Hz and 0.11 at 40 Hz
  (diffuse-field model, Houtgast & Steeneken 1985). Computed, not measured here.
- **Modulating a whole stack in phase keeps it one sound** (common modulation fuses
  partials; Bregman 1990). The depth heard is then an average over many room responses.
- The 40 Hz piece's carrier (108 Hz) sits under the binaural range's best region, and its two
  tones are 40 Hz apart against a 38 Hz auditory filter. Every 40 Hz study used carriers of
  380–540 Hz.

## Living noise

- **Real textures are sparser than noise, and the bands move together.** In variance, skew and
  kurtosis noise sits at the lower edge of natural textures. For wind and waves, slow
  co-modulation across bands was essential to resynthesis (McDermott & Simoncelli 2011, Neuron
  71:926). One driver should move every band together.
- **Wind: one speed, many consequences.** Foliage noise power grows as U^3–U^3.6 (Fégeant's
  model in Bolin 2006, KTH thesis), which is 9–11 dB per doubling. Measured 1-s LA varied
  with σ 2.6–5.2 dB, roughly Gaussian in dB, at turbulence intensity about 0.3. A conifer's
  spectral peak sits at f = 0.2·U/needle diameter, so it moves up with the wind. Gusts
  follow the Kaimal (1972) spectrum, flat up to about 0.02 Hz at 10 m and 5 m/s, then −5/3:
  swells of tens of seconds. (The corner is computed here from Kaimal's formula, not read
  from a measurement.)
- **Surf darkens when it breaks.** Every third octave from 40 Hz to 2.5 kHz rose 5–12 dB at a
  break, most (12 dB) at 50 Hz, with ±10 dB from wave to wave (Dallas & Tollefsen 2015,
  Canadian Acoustics). No measurement of the break's envelope shape was found.
- **Variety over minutes follows 1/f** in music and speech loudness, down to 5×10⁻⁴ Hz (Voss &
  Clarke 1975, Nature 258:317). That supports a slow "weather" layer under the gusts.
- **A steady noise is not proven better for sleep** (Riedy et al. 2021, "very low" quality
  evidence). Awakenings follow individual events above the background (Wunderli et al. 2016),
  so sudden peaks are the risk.
- **Harmonance's Drift** (±15 % gain, about ±1.3 dB, 3–5 s linear ramps) changes level only:
  no colour change, no co-modulation, no cause.

A physically driven colour, as a proposal: a wind speed U(t) = Ū·(1 + I·x(t)), with x a
Kaimal process (integral time 10–40 s) plus a 1/f layer over minutes, and I = 0.15 in the
open (unverified: a textbook value, 0.1–0.2, not read from a source here) to 0.3 near trees
(Bolin's measurement). The level follows 10·n·log10(U/Ū) with n = 3–3.6 for foliage
(σ_L ≈ 4.34·n·I, 2.9 dB for n = 3.3 and I = 0.2, inside Bolin's range). For a Strouhal-type
sound the spectrum shifts by log2(U/Ū) octaves. To keep the 50 dB(A) presets level, lower
the base by 0.115·σ² dB (+1.0 dB at σ = 3). Keep periodic movement out of 0.5–4 Hz.

## Living tones

- **Bowls are inharmonic and beat.** Measured partial ratios: 1 : 2.96 : 5.41 : 8.75 : 12.6
  (Wilkerson, Serafin & Ng 2002) and 1 : 3.05 : 6.04 : 9.51 : 13.67 (Wang, Tsai & Wu 2018). The
  thin-ring formula f ∝ n(n²−1)/√(n²+1) gives 1 : 2.83 : 5.42 : 8.77 : 12.87, within about 4 %
  of the first. Each mode is a doublet: 85.5/87.1 Hz (1.6 Hz beat), 173.1/174.5 Hz (1.4 Hz)
  (Imbery, Jensen & Weber 2014), and 1.7–1.8 Hz up to 12 Hz on higher modes (Wang 2018). The
  partners differ by up to 20 dB. Decay times per partial: none found; to be measured.
- **What makes a tone sound alive is mostly per-partial level movement.** Giving every
  harmonic the same envelope was heard almost always. Removing the small pitch variations
  was heard much less (McAdams, Beauchamp & Meneguzzi 1999, JASA 105:882).
- **Shared pitch movement holds partials together; separate movement pulls one out** (McAdams
  1984).
- **Ensembles are spread.** Choir unison: listeners prefer 0–5 cents standard deviation and
  accept about 14 (Ternström 1993). Real choirs measure 20–30 cents typically. Unison violins
  measured 0–15.5 cents (Ioannou & Kob 2019). Solo string vibrato: 5.5–8 Hz, 20–35 cents
  (Meyer 1992).
- **Voice flutter** (Klatt & Klatt 1990): three sines at 12.7, 7.1 and 4.7 Hz, about 10 cents
  RMS at their realistic setting. Their rates repeat every 10 s; for a tone that never repeats
  the rates need ratios that are not simple.
- **Beating splits are set in hertz, not cents.** 1.5 Hz is 4.9 cents at 528 Hz but 39 cents
  at 66 Hz. Either way it is far under a critical band (Plomp & Levelt 1965), so it is heard
  as a slow swell, not as roughness.
- **Warmth** has no standard definition. One is energy in harmonics 1–3 against the rest, in
  practice a downward tilt. Brightness follows the spectral centroid (Schubert & Wolfe 2006).
  In the octave stack each line's 2nd harmonic is already the next line up; only odd
  harmonics (the 3rd, a fifth; the 5th, a major third) would add new pitches.
- **Unverified here:** detection thresholds for slow AM (about 0.5–0.9 dB) and FM (about
  0.2–0.5 %); T60 per bowl partial.

## Pascal's examples, measured

Seven stretches of dark ambient and sleep music, sampled with `scripts/clip.sh`, 48 kHz
stereo, YouTube's lossy audio (see `run.md`, "Sample a clip"). Measured with `analyse`, whose
movement section was built for this (`target/analysis/clips-summary.txt`; definitions in
`crates/analysis/README.md`). A throwaway script measured them first; where the tool corrected
it, that is said below.

| Clip | | Lowest tone | In partials |
|---|---|---|---|
| `IXsWr2CK4SI` 15:00–17:00 | spiritual brother, "i must rest here a moment" | bands at 327–331 Hz and 658–664 Hz | 34 % (mostly broadband) |
| `Yk2bpo5paPw` 19:00–21:00 | Cryo Chamber, dark space | 38.9 Hz with harmonics 2–5 | 91 % |
| `Yk2bpo5paPw` 29:00–33:00 | same piece | 58.9 Hz with harmonics, and 87.7 Hz | 51 % |
| `caAVbyTCptw` 31:00–33:00 | Astral Ambience, "Calm" | 73.6, 129.8, 146.5, 261.6 Hz | 88 % |
| `Bx5byPcxk5E` 7:00–10:00 | spiritual brother Sci-Fi | 55.9 Hz with harmonics, 150.9 Hz | 87 % |
| `hVFaaUEIpzE` 7:00–10:00 | spiritual brother Sci-Fi | 65.41 Hz pure (C2), C3, C4, G3 above | 72 % |
| `gP3BBaoU0_E` 20:00–23:00 | spiritual brother Sci-Fi | 82.41 Hz pure (E2) | 97 % |

What they have in common:

- **A low drone, 39–88 Hz, tuned to ordinary concert-pitch notes** (C2 65.41, E2 82.41,
  C4 261.63, D#1 38.9), not to "healing" numbers. Centroids are 109–504 Hz; little above
  2 kHz (−50 to −80 dB re Leq).
- **The overall level hardly moves; the balance does.** The broadband level's standard
  deviation is 0.5–4.1 dB (L10−L90 2.5–5.7 dB; 12.2 dB in the one stretch where the music
  builds). A single partial's level swings 8–33 dB (p10–p90) over tens of seconds, never
  periodically. Octave bands move almost independently: the mean correlation of all band pairs
  is 0.02–0.26. This is the opposite of wind, where one driver moves everything. It is McAdams'
  per-partial movement, far larger than Harmonance's ±4 dB, 24 s rotation.
- **Most partials are one voice whose pitch wanders, not a cluster.** Half of all partials are
  a single line; the others two to four (one clip, Astral Ambience, up to 16). Each fills a
  band 2–11 Hz wide at −20 dB, and its pitch wanders with a standard deviation of 1–12 cents
  (once 30). *Correction:* the throwaway script read every partial as 3–6 voices 0.45–0.8 Hz
  apart. Those were the ripple on one wandering partial's smeared peak; `analyse` counts a line
  only where the spectrum dips 6 dB between it and a stronger one.
- **Two pieces hold one pure sine under everything** (65.41 Hz and 82.41 Hz, both from
  spiritual brother). Its pitch varies by 0.6 cents and its level by 1.2–1.6 dB (p10–p90),
  with faint sidebands 0.4 Hz either side: an anchor that does not move while the rest does.
  It is identical in both ears (no turning, ILD 0.0 dB).
- **Slow movement is redder higher up in five of seven.** The slope of each band's level
  spectrum over 0.01–0.5 Hz runs from −2.4 to +0.8 in the 63–250 Hz bands and from −2.9 to
  +0.4 at 1–4 kHz. Where the top is steeper, the high bands drift in long smooth swells and
  the bass varies more evenly across time scales.
- **No consistent swell shape.** The level's change skews slightly positive in all seven
  (+0.09 to +0.40, a little quicker up than down), but the swells' own rise and decay go both
  ways: 7 s up and 24 s down in one clip, 23 s up and 15 s down in another. Slow cross-fades,
  neither damped nor ramped.
- **Louder is darker in four of seven** (centroid against level −0.20 to −0.66): the swells
  are bass swells. Wind gets brighter as it gets stronger.
- **Wide and decorrelated.** Width (1 − IACC, 500 Hz–2 kHz) is 0.57–0.99 and per-band IACC
  mostly under 0.3. The pure pedal tones are the exception.
- **No binaural beats, but steady stereo detunes in four clips.** No pair of one-ear partials
  in any. But on one to three partials in four of the seven, one ear's voice is steadily
  0.37–1.22 Hz higher than the other's (the phase between the ears turns at that rate, varying
  by less than half of it from one 10 s stretch to the next): 58.9 Hz at 0.37 ± 0.15 Hz in the
  Cryo Chamber piece, 113.6 Hz at 1.22 ± 0.46, and others at 0.5–1.0 Hz. A slowly turning image,
  not a "brainwave" rate. *Correction:* the script found only the 58.9 Hz one, at 0.46 Hz from a
  narrower band.
- **One weak pulse.** Only Astral Ambience pulses: 1.01 Hz, 1.7 dB deep, rising and falling
  alike (266 and 261 ms), which is its voices about 1 Hz apart beating.
- **Few events.** Onsets are 0–0.5 per minute in five clips; 5.3 per minute in `gP3BBaoU0_E`
  and 19.3 per minute in `hVFaaUEIpzE` (median 2 s apart), where something plays over the
  drone.

Each piece taken apart into its layers, as a recipe: `ambient-recipes.md`.

What this suggests for a focus tone (an idea, not a decision): keep one steady pure line as
the anchor. Let every other line's pitch wander a little (5–15 cents) and its level wander
independently by 10 dB or more over tens of seconds, never periodically, while the total
stays within a few dB. Detune a few lines between the ears by a fraction of a hertz to a
hertz, so the image turns slowly.

## What `analyse` measures

Since 2026-11-24 its report has a movement section (`report_version` 2) with what these
examples needed:

- **partials**: each one's frequency, lines (voices), −20 dB width, share of the power, and how
  far its level and pitch move;
- **at the ears**, per partial: level difference, the rate the phase between the ears turns at
  and its spread per 10 s (a binaural detune), and binaural beats as pairs of one-ear partials;
- **slow movement**: each octave's level movement on 1 s frames, its slope over 0.01–0.5 Hz,
  whether the octaves move together, and whether louder is brighter;
- **swells** (rise, decay, skew) and a **pulse** from 1 to 50 Hz (rate, depth, 10–90 % rise,
  90–10 % decay).

`analyse --compare` compares it too: the partials as a whole and one to one at the same pitch
(within 3 %), the slow movement, swells and pulse. So a generator can be set against one of
these pieces directly.

## Open

- The speaker version of the binaural pieces: AM of the whole octave stack in phase, or a
  literal two-tone monaural beat. The 40 Hz piece needs a higher carrier either way.
- How a slow wind-like driver for the noise colours would sit with their fixed 50 dB(A)
  level.
- Decay times of real bowl partials, from a recording.
