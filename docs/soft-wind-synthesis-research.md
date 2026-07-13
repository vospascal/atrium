# Soft-wind synthesis research

## Reference

Local source: `research papers/soft wind.mp3` (28.7 s, 48 kHz stereo,
downmixed to mono for analysis).

Two measurements are kept deliberately separate:

- Long-window Goertzel probes describe a compact, octave-like fingerprint.
- A 4096-sample Hann-window STFT with a 50 ms hop measures the actual energy
  distribution and its movement over time.

| Measure | Recording | `soft_wind` |
|---|---:|---:|
| Crest factor | 16.7 dB | 14.9 dB |
| 250 ms RMS swing | 7.1 dB | 5.9 dB |
| STFT centroid, median | 5,451 Hz | 5,478 Hz |
| STFT energy above 2 kHz, median | 75.4% | 77.1% |
| STFT body/air tilt, median | -8.1 dB | -8.5 dB |
| STFT spectral flatness, median | 0.422 | 0.455 |

The recording is not simply a quieter `field_wind`. It has little low-frequency
pressure energy, a persistent broadband upper bed, and level-dependent
brightening. Its median energy is concentrated in brilliance (4-8 kHz) and air
(8-16 kHz), while its level floor remains high.

## Driver evidence

The reference contains three useful modulation scales:

- slow weather below roughly 0.2 Hz;
- frequent gust/ruffle motion around 0.2-1 Hz;
- faster spectral texture around 1-5 Hz.

A first version that reused only `field_wind`'s combined speed/activity output
put about 93% of its level modulation in the slow band. This was visibly and
numerically too static. The revised shared wind model exposes its existing
weather, gust, and eddy trajectories separately. All wind synths consume the
same physical state, but each maps those trajectories to its own acoustic
filterbank.

This agrees with the literature rather than being a recording-only trick:

- Karl Bolin's vegetation-noise model separates average wind from time-varying
  turbulence, treats turbulence as a stochastic process, and reports broadband
  vegetation sound with a characteristic leafed-tree component around 4 kHz.
  <https://doi.org/10.3813/AAA.918189>
- Komatsu et al. measured wind-induced foliage sound across six tree species;
  they found stable 100-1,000 Hz components, a strong wind-speed/level
  relationship, and distance-dependent loss of high-frequency energy.
  <https://doi.org/10.11372/souonseigyo1977.24.268>
- Boersma measured natural ambient sound in open grassland as a function of wind
  speed for one-third-octave bands through 20 kHz. The low-frequency spectrum
  follows turbulence-like behavior, supporting a distinct pressure/body layer
  rather than a single broadband gain control.
  <https://doi.org/10.1121/1.418141>
- Van den Berg shows that microphone wind noise is produced by atmospheric
  turbulence and depends on wind speed and windscreen geometry. This is a
  reason not to copy every low-frequency recording artifact into the audible
  environmental model.
  <https://doi.org/10.1121/1.2146085>

## Resulting model

The shared sample-clocked driver now exposes:

1. weather/evolution time range;
2. gust time range and strength;
3. turbulence time range and depth.

Every wind type can additionally configure gust-dependent and
turbulence-dependent brightness. `field_wind`, `canopy_wind`, and `storm_wind`
retain their previous defaults (zero extra brightness response), while
`soft_wind` uses both responses to reproduce the reference's ruffling upper
spectrum. All clocks advance per audio sample and remain independent of graphics
FPS and audio callback size.

## Limits

The reference is short and has no calibrated SPL or documented wind speed,
distance, vegetation, microphone, or windscreen. Its spectrum is therefore a
perceptual target, not evidence that the default 1-5 m/s range or 42 dB SPL is a
measured property of the recording. Those remain authoring defaults.
