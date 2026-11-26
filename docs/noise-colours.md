# Noise colours

Logged 2026-11-23. Pascal asked for noise presets "just like frequencys": white, pink, brown,
green, grey, violet, red, blue and orange, with a sleep app's one-line descriptions, and
supplied an example recording of each (`assets/noise/`, 60 s, 48 kHz stereo MP3, not committed).
A companion to [focus-tones.md](focus-tones.md). Built the same day:
`src/synth/noise_colour.rs` (`synth: noise_colour`, `colour:`), presets in `sources/noise/`,
group "Noise colours", test scene `scenes/noise-colours-test.yaml`.

## Definitions

Power per hertz. White, pink, red and violet are textbook slopes (the examples agree). Brown,
green and orange have no agreed definition; blue and grey do, but Pascal chose to follow the
examples for them too (2026-11-23). Those five are fitted to the recordings.

| Colour | Built as | Source |
|---|---|---|
| White | flat | textbook |
| Pink | 1/f, −3 dB/oct (Kellet) | textbook |
| Red | 1/f², −6 dB/oct (Brownian, random walk), high-pass 20 Hz | textbook |
| Brown | red × 2nd-order low-pass at 285 Hz | fitted to the example, 2.1 dB RMS |
| Blue | flat floor + 2nd-order high shelf +12.6 dB at 3228 Hz, high-pass 105 Hz | fitted to the example, 0.32 dB RMS |
| Violet | f², +6 dB/oct | textbook |
| Grey | flat floor + a low part 28.9 dB up below 54 Hz falling 6 dB/oct, × a 5 dB dip at 1964 Hz (Gaussian in log frequency, σ 0.9 oct); high-pass 20 Hz | fitted to the example, 0.18 dB RMS |
| Green | 1/f^1.45 × 3rd-order low-pass at 1640 Hz | fitted to the example, 1.6 dB RMS |
| Orange | two layers: *deep* 1/f² × low-pass 130 Hz; *bright* red 24 dB down, low-pass 3 kHz; both high-pass 40 Hz | fitted to the example, within 1.2 dB |

In physics red and brown are one spectrum (1/f², named after Robert Brown). The examples keep
them apart: their "brown" is far darker, so Atrium's red is the physical one and its brown
follows the app. Green had been guessed as a 500 Hz band; the example has no band at all, only
a soft dark noise.

## The examples, measured

Mid channel, Welch 8192, per-hertz density re 1 kHz; slope fitted over octaves 63 Hz–8 kHz.

| Example | Slope | 63 Hz | 250 Hz | 4 kHz | 8 kHz | LA (dBFS) |
|---|---|---|---|---|---|---|
| White | +0.0 | 0.0 | +0.1 | +0.8 | −0.7 | −21.9 |
| Pink | −2.6 | +11.6 | +5.8 | −3.8 | −7.1 | −21.7 |
| Red | −5.9 | +21.2 | +12.1 | −12.4 | −19.2 | −24.9 |
| Brown | −15.5 | +45.2 | +31.3 | −41.1 | floor | −35.0 |
| Blue | +2.6 | −9.6 | −4.3 | +7.0 | +8.0 | −23.5 |
| Violet | +6.0 | −22.6 | −10.9 | +13.3 | +18.4 | −25.7 |
| Gray | −4.2 | +22.7 | +13.1 | −4.4 | −2.6 | −25.1 |
| Green | −9.3 | +16.3 | +9.9 | −30.7 | −52.6 | −24.2 |
| Orange | −10.6 | +47.5 | +24.7 | −16.0 | −26.2 | −32.2 |

All nine: left and right independent (correlation 0.00–0.02), level steady (no drift), a fade-in
of about 1 s. They are not level-matched: brown sits 13 dB (A) under pink.

## Where Atrium differs from the examples, and why

- **Level.** Every preset plays at 50 dB(A) at the listener (measured 50.0–50.1 per ear on
  headphones, 49.6–50.0 on 5.1), so colours can be compared fairly. `reference_spl` is
  unweighted, so the dark colours show high numbers (brown 69.4, orange 71.0).
- **Blue** follows the example: flat above about 5 kHz. The textbook blue keeps rising
  3 dB/oct to the top.
- **Grey** follows the example: more bass and a dip near 2–3 kHz, but flat at the top. The
  textbook grey is the inverse ISO 226 equal-loudness contour, which rises again 12–14 dB at
  8–12 kHz. Built first that way (within 0.77 dB of the contour), then refitted.
- **Left and right.** Played as a diffuse field around a real (KEMAR) head, the ears are
  uncorrelated above 1 kHz but correlated in the bass, as in a real field: broadband 0.01 for
  white/blue/violet, 0.48 pink, 0.76 green, 0.86 red, 0.92 brown, 0.93 orange. The examples'
  independent ears are a stereo production choice. Matching them would need the straight-to-
  the-ears path (two independent streams, one per ear), not built.
- **Fade-in** 0.5 s (examples about 1 s).

## How it is made

White, pink, red and violet are sample-by-sample filters. Brown, blue, grey, green and
orange are shaped in blocks (`noise_colour/shaped.rs`): 32 768 samples of white noise, each bin
multiplied by the shape in hertz, transformed back, blocks cross-faded with a sine window. This
gives the same spectrum at 44.1, 48 and 96 kHz (biquads warp near the rate's limit); the cost is
two FFTs every 16 384 samples, landing in one audio callback, and about 1 MB per generator.

Measured (`src/synth/noise_colour/tests.rs`): white, pink, red, violet slopes within 0.08 dB/oct of target
at all three rates; every octave band within 0.42 dB across rates; grey and blue within 0.85 dB
of their examples in every third-octave band; brown, green within 0.4 dB of their models;
orange within 1.2 dB of the example. Grey's `reference_spl` is 57.2, blue's 53.4.

## Open

- Independent ears, via the straight-to-the-ears path.
- Slow variation (the harmonance "drift"): none in the examples, none built. How real noise
  moves (one driver, wind gusts, surf) and a proposed wind-speed driver:
  `tone-and-noise-movement.md`.
