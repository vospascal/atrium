# Located river synthesis

Status: implemented as `river` with the located `EmitterKind::Object`.

## Why the first model failed

The first river was a rebranded soft-rain experiment. Its averaged spectrum
could be made river-like, but its body was still filtered broadband noise.
Resampling that carrier with `body_motion` changed roughness and bandwidth; it
did not create perceptually slower flow. The independent wash also remained
full-speed, so the result retained a static/noise character.

That model has been replaced rather than tuned further.

## Research basis

Liquid-sound synthesis research consistently identifies acoustically active
bubbles as a central primitive:

- K. van den Doel, *Physically-based models for liquid sounds* (ICAD 2004),
  develops a stochastic real-time model for streams and rivers from single
  bubble sounds: <https://hdl.handle.net/1853/50904>.
- Langlois, Zheng, and James, *Toward Animating Water with Complex Acoustic
  Bubbles* (SIGGRAPH 2016), uses bubble populations, entrainment, splitting,
  merging, and popping: <https://www.cs.cornell.edu/projects/Sound/bubbles/>.
- Xue et al., *Improved Water Sound Synthesis using Coupled Bubbles* (SIGGRAPH
  2023), shows that bubble-cloud coupling contributes perceptually important,
  fuller low-frequency emissions:
  <https://graphics.stanford.edu/papers/coupledbubbles/>.

The tuning reference is a 30.55-second CC0 field recording of a fast Swedish
creek, recorded in stereo with a Roland R-26:
<https://freesound.org/people/kentspublicdomain/sounds/325182/>.
The local research preview is `research papers/river_reference_fast.mp3`.

## Measured target

Mono analysis of the reference versus the current dry synth, both over 30.6 s:

| Metric | Creek recording | River synth |
|---|---:|---:|
| RMS | -25.2 dBFS | -25.6 dBFS |
| 250 ms p05-p95 swing | 3.22 dB | 3.23 dB |
| STFT centroid p50 | 1502 Hz | 1514 Hz |
| energy above 2 kHz p50 | 20.2% | 19.6% |
| 5 ms crest p50/p90/p99 | 8.3/9.8/11.0 dB | 8.4/9.8/11.0 dB |
| millisecond events/s | 151.2 | 150.3 |
| level/brightness correlation | -0.26 | -0.25 |

The match is intentionally structural, not a sample copy. Remaining differences
include a little excess energy below 400 Hz, a slightly smoother air-band tail,
and more very-slow modulation from the configurable 15-110 s flow evolution.

## Model

The new core contains four independently controllable components:

1. A band-limited turbulent body.
2. A faster continuous surface-current layer.
3. A stochastic population of short, chirped 450-2700 Hz bubble resonances,
   including occasional lower coupled modes.
4. Sparse obstacle splashes and very short bubble-pop/spray transients.

Two sample-clocked eddy ramps modulate these layers differently. They create
shared level motion while also changing spectral balance, instead of applying
one LFO to a fixed EQ curve.

## Controls

- `min_flow_speed` / `max_flow_speed` (0-5 m/s)
- `change_time_min` / `change_time_max`
- `eddy_time_min` / `eddy_time_max`
- `eddy_depth`
- `body_gain`
- `current_gain`
- `bubble_activity`
- `splash_rate` / `splash_gain`
- `spray_gain`
- `master_gain`

Every driver and stochastic event advances from rendered audio samples.
`tick()` is a no-op, preserving graphics-FPS independence.

## Spatial model

The river remains a located `EmitterKind::Object`, like a bird rather than a
wind field. Distance attenuation, direction, source spread, propagation,
reflections, and room reverb therefore apply normally.

Focused scene: `scenes/river-only.yaml`.
