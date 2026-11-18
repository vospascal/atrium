# Follow-up ideas

Things that are not done and still make sense. Nothing here is scheduled; the
active plan is `docs/living-atrium-roadmap.md`. Pruned on 2026-11-18: the
legacy cleanup this file used to track is finished, and the items below were
checked against the code on that date.

Judge each by one question: does it make an environment more believable to
listen to?

## Known gaps in what exists

### Reflection taps in WorldLocked move in whole samples

`ReflectionCore` (`src/pipeline/stages/reflections.rs`) truncates each tap's
delay to an integer number of samples. A moving source makes the taps jump one
sample at a time, which can zipper for walls close to a speaker. The per-path
`PropagationDelayEffect` already reads fractionally and glides; the taps could
do the same. Only WorldLocked uses `ReflectionCore`.

### WorldLocked has no second-order floor bounce

For speakers raised above the floor, each wall reflection should also arrive
via the floor. Only first-order images are rendered.

### rain_v2

It is 1.3 dB louder at 96 kHz than at 48 kHz (its inline filters are tuned in
samples), and it still has the subnormal-decay pattern that was fixed in rain
v1.

## Extensions the pipeline is shaped for

Each plugs in through an existing contract (`PathResolver`, `PathEffect`,
`Renderer`, `MixStage`) and a new file in `src/pipeline/builders/`.

- **Ray-traced paths.** A `PathResolver` that produces paths from traced
  geometry, for environments that are not a box. `MAX_PATHS` is 7 today (one
  direct, six first-order images) and would have to grow. See
  `docs/ray-tracing-upgrade-analysis.md`.
- **Occlusion and transmission.** Needs geometry beyond the box in
  `Environment`: something that can stand between a source and a receiver. An
  earlier barrier model was removed because nothing could place a barrier in a
  scene; start from the scene description, not from the DSP.
- **Path count by distance.** Skip reflections for far sources, in the
  resolver, when CPU matters.
- **Reverb level from the resolved reflections.** The late reverb's level is
  set by the critical-distance send law. Feeding it the energy of the resolved
  reflection paths would tie early and late sound together, but mix stages do
  not see paths today.

## From the paper synthesis (`docs/paper-synthesis.md`)

- **Three-band reverb decay** (Carpentier 2017, IRCAM Spat). The FDN damping
  filter matches the decay time at 500 Hz and 4 kHz. A third band would let
  bass decay differently, which matters for glass (bass-absorbing) against
  stone.
- **Early-to-late handoff.** The pre-delay comes from the mean free path or an
  explicit value; it is not matched to where the image-source reflections end.
- **VBAP over a convex hull** (Choi 2012). Only relevant with height speakers;
  the pair search is simpler and sufficient for a horizontal ring.
- **One-pole air absorption** (Moorer, via Yeoward 2021). About half the cost
  of the current shelves, less accurate, and blind to temperature and
  humidity. Only worth it if profiling shows air absorption matters.

## Perceptual reference values

| Threshold | Value | Source |
|---|---|---|
| Interaural time difference, just noticeable | 90 μs | BBC WHP254 |
| Interaural level difference, just noticeable | 2.5 dB | BBC WHP254 |
| Gain interpolation period | 10–50 ms | Pulkki; Gandemer 2018 |
| VBAP lookup resolution | 1° azimuth | Shukla 2019 |
| Largest polar span for 3D VBAP | 40° | Baumgartner 2015 |
| Head-tracking latency budget | under 75 ms end to end | Shukla 2019 |
| Acceptable HRIR length | 256 samples | Yeoward 2021 |
