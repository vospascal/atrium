# Comparing sounds: a reference against ours

Planned and built 2026-11-22; where the build departs from the plan, the plan below is
corrected and "As built" at the end says why. The third tool ([generator-workflow.md](generator-workflow.md#the-tools-in-order)),
after the renderer and the analyser. It puts two or more reports side by side and says
where they differ. It reports differences; only listening judges whether a version is
better ("Measurements help, they do not judge").

The first real case, measured by hand with `analyse` the day it was planned: our soft wind
against `research papers/soft wind.mp3`. The colour is within a few dB above 200 Hz (both
bright, centroid 5.9–6.0 kHz); the reference has more below 100 Hz, about twice the slow
swelling (0.5 Hz modulation 2.7 % against 1.4 % of envelope variance), and bands that move
together more (envelope correlation 0.26 against 0.13). The tool should show that at a glance.

## What it is

```sh
cargo run --release --bin analyse -- --compare <reference> <other>... [--picture]
```

- **Part of `analyse`**, not a second binary: the same reading of files and sidecars, and
  the first file is the reference. Each input is an audio file (analysed as now) or a report
  JSON (`target/analysis/*.json`, read as it is).
- **A range per file**: `file.mp3@30-60` takes 30 s to 60 s of it, so a stretch of a long
  field recording is compared with a render.
- **Output**: a one-screen comparison (reference column, then one column per other file,
  each value with its difference), a comparison JSON (`comparison_version: 1`), and with
  `--picture` one PNG of overlays.

## What is compared, and how

Level is compared only as shapes when either file is uncalibrated (a recording's dBFS says
nothing of its SPL); absolute level only when both carry a calibration.

| Group | Compared | How the difference is given |
|---|---|---|
| Level shape | L10 − L90, LAFmax over level while sounding, crest, LC − LA | Difference in dB |
| Absolute level | LAeq, Leq (both calibrated only) | Difference in dB; marked where beyond 1 dB, the JND ISO 3382-1 gives for level |
| Colour | Third-octave levels relative to Leq | The difference curve per band; its RMS over 50 Hz–16 kHz; the spectral tilt difference (dB per octave, a line fitted to each curve over 100 Hz–10 kHz); centroid ratio |
| Texture | McDermott & Simoncelli's statistic classes: envelope mean (after the overall level difference is removed), M2, M3, M4, C, modulation power, C1 | Per class the paper's own error measure: SNR = 10·log10(Σ s² / Σ (s − s′)²) over the class's statistics, reference s. The paper took 20 dB as "converged" and imposed to 30 dB. Beside it a scatter floor: the SNR two takes of the reference's sound reach by chance, from its two halves. The summary says which classes are below 20 dB and below the floor (a difference) or within it (chance), and for the differences the subbands, pairs or modulation rates that differ most |
| Events | Onsets per minute; interval median, 10 % and 90 % | Ratio and differences |
| Space (both two-channel) | IACC, envelope coherence and ILD spread per octave; width | Differences; width marked where beyond 0.075, the JND of IACC (Cox, Davies & Lam 1993) |
| Over time | The LAF and width series | Not compared sample by sample (different recordings do not line up); their distributions are (LAF L95 … L5, relative to each file's L50 unless both are calibrated; width exceeded 90, 50, 10 %). Two renders of the same scene, seed, start, weather speed and stretch line up, and for those the summary also gives the largest difference over time and when |

## The picture

One PNG, panels of overlays, the reference in one colour and each other file in its own,
every axis labelled:

1. third-octave colour curves (relative to Leq), with the difference below;
2. the modulation spectrum (mean over subbands), the LAF distributions (L95 … L5), and the
   texture SNR per class with its scatter floor;
3. M2 and M4 per subband (sparsity), and C at offset 1 per subband (comodulation);
4. IACC and envelope coherence per octave band (two-channel files);
5. the spectrograms, one strip each, on the same frequency axis, time scale and 70 dB range,
   each below its own loudest cell.

## How it is built

- In `crates/analysis`: `compare(reference: &Compared, others: &[Compared]) -> Comparison`,
  pure; `comparison_summary(&Comparison)` and `comparison_picture(&Compared, &[Compared])`, as
  `summary` and `picture` are. A `Compared` is a report with its name, how it was made, its
  halves (for the scatter floor), its timeline and its samples (for the spectrogram strip).
  New private modules (`compare/`, one file per group; `picture/comparison.rs` on
  `picture/axes.rs`); the report stays as it is.
- In `src/bin/analyse.rs`: `--compare`, reading a report JSON as input, and the `@from-to`
  range.
- Docs: `run.md`, the crate README, `generator-workflow.md`.

## How we know it is right

| Check | Expected | Measured (test) |
|---|---|---|
| A file against itself | Every difference 0; every texture class "identical" (no finite SNR) | Every `difference` in the JSON 0 (116 of them), every class identical (`a_file_against_itself`) |
| The same noise at a gain of +10 dB, both uncalibrated | Shape, colour, texture and space identical; LAeq not compared (uncalibrated) | Differences below 10⁻⁴; every texture class above 60 dB once M1's 10 dB is taken out; no absolute level (`the_same_noise_ten_decibels_louder`) |
| The same, both calibrated | LAeq difference 10.0 dB | 10.0 dB, marked beyond the 1 dB JND |
| White against pink noise | Tilt difference −3.0 ± 0.1 dB per octave; M2 and M4 ≥ 20 dB (both Gaussian); M1 below 20 dB and beyond the floor (M1 is the spectrum) | Tilt −3.00; M2 29.7, M4 34.6 dB; M1 10.8 dB; M3 15.5 dB against a floor of 19.3 (`white_against_pink_noise`) |
| Two takes of one white noise | C and C1 below 20 dB, every class within the scatter floor | C 7.8, C1 4.3 dB; each class 1.2 dB below to 3.4 above its floor (`two_takes_of_one_noise_differ_only_by_scatter`) |
| Noise against noise modulated at 4 Hz | Modulation-power SNR below 20 dB, and 4.5 Hz the rate that differs most | Below 20 dB; 4.55 Hz first (`noise_against_noise_modulated_at_four_hertz`) |
| Our wind, seed 1 against seed 2 | Every texture class at 20 dB or above | 38.8 dB at the lowest (C) (`our_wind_at_two_seeds_has_one_texture`); but the wind barely follows the session seed, see "As built" |
| Our wind's first half against its second | Every class converged or within the floor | M3 19.0 (floor 17.6), C 13.7 (13.1), C1 9.7 (9.9) within; the rest ≥ 25 dB (`our_wind_against_itself_later_differs_only_by_chance`) |
| Our wind against a blackbird take | Most classes far below 20 dB | All seven, from 4.1 dB (M1) to −15.4 (M2), beyond the floor (`our_wind_against_a_blackbird`) |
| A render against its report JSON | The same comparison as against the render itself | Equal to the bit in the crate (`a_report_against_its_json`); through `analyse`, the same JSON apart from the file's name |
| `soft wind.mp3@0-28` against our wind | The summary and picture show the differences found by hand (more below 100 Hz, more slow modulation, more comodulation) | Shown: 25–63 Hz 6 to 16 dB more in the recording; 0.5 Hz modulation 2.7 % against 1.4 %; C at offset 1 0.26 against 0.13, C beyond its floor; L10−L90 2.6 against 2.0 dB; centroid 5939 against 6019 Hz (`soft_wind_against_ours_shows_the_hand_findings`, ignored by default: the recording is not in git) |

## Not in this step

- Saying which version is better: only listening does.
- Aligning two different recordings in time.
- Comparing inside the app (the live view shows one sound; a reference beside it in the app
  is part of the generator lab).

## As built

Where the build departs from the plan above, and why.

- **A scatter floor per texture class.** Two independent takes of one white noise reach only
  C 7.8 dB and C1 4.3 dB by the paper's SNR: the paper measured how well it had imposed
  statistics, not how far two takes lie apart by chance, and statistics near zero are mostly
  chance. With the 20 dB rule alone every pair of noise recordings would "differ in
  comodulation". The floor (from the reference's halves) separates a difference from chance;
  `crates/analysis/README.md` gives its derivation and its 3 dB margin.
- **`compare` takes `Compared`, not bare reports.** The summary and picture name each file,
  the floor needs the reference's halves, lining up needs what fixes a render's timing (in the
  sidecar, not the report), and the spectrogram strips need samples. The report itself is
  unchanged; `REPORT_VERSION` became public so `analyse` can refuse a report of another
  layout.
- **"Texture marginal classes ≥ 20 dB" for white against pink was wrong for M1 and M3.** M1, the
  envelope means across subbands, is the spectrum; white and pink differ there by design. M3 of
  most subbands is near zero and shifts with the spectrum within a band; measured 15.5 dB.
- **Our wind at seed 1 and seed 2 is nearly one waveform** (their difference is −44 dB of their
  power): the soft wind takes a slot-stable seed, not the session seed. The row passes, but
  tests nothing about randomness, so the wind's two halves are compared as well. Whether the
  wind should follow the session seed is an engine question, not changed here.
- **The picture has a texture SNR panel**, and the LAF distribution shares a row with the
  modulation spectrum. The spectrogram strips are each relative to their own loudest cell, on
  one time scale (the longest file spans the width).
- **The comparison does not write each file's report.** `analyse` without `--compare` does;
  a report JSON can then be compared later.

