# The analyser: one report for a recording or a render

Planned 2026-11-22, built the same day (`crates/analysis`, `src/bin/analyse.rs`); this
document was corrected to match what was built, with three amendments from research
(C-weighting, McDermott and Simoncelli's texture statistics as their paper defines them,
interaural envelope coherence) and librosa's onset method. The second of the tools ([generator-workflow.md](generator-workflow.md#the-tools-in-order)):
the offline renderer makes a file, the analyser measures it, and later a comparison
puts two reports side by side. Measurements report differences; only listening judges.

## What it is

```sh
cargo run --release --bin analyse -- <file.wav|mp3>... [--from <s>] [--to <s>] [--output <report.json>] [--picture[=<path>]] [--summary <path>]
```

- **Any input.** A reference recording (MP3 or WAV, mono or stereo) or a render
  (2-channel headphones or 6-channel 5.1). The same measures for both, so they can be
  compared.
- **It reads the render's sidecar** (`<file>.json` beside it) when there is one, for
  the channel names, the output, the probe, the seed and the gain `--normalise`
  applied. With it, levels are given in dB SPL as the engine means them; without it,
  in dBFS only, where an RMS of 1.0 is 0 dBFS (the engine's calibration is in RMS).
- **Output.** A JSON report (default `target/analysis/<file stem>.json`), versioned
  (`"report_version": 1`) so the comparison tool can read it, and a short printed
  summary.

## Made to be read, by Pascal and by Claude

Claude cannot listen. It reads text and looks at pictures, so the analyser gives both,
made for a reader who has only them:

- **A one-screen summary** (printed, and `--summary <path>` to write it): every
  number with its unit and what it is, the file and its sidecar settings in one line,
  then one line per group (level, colour, texture, events, space), then **a strip over
  time**: LAeq, onsets and (for two channels) width per 1/20th of the file, so the
  shape of the file is readable without a picture. Several files in one call give
  their summaries one after another, with the same layout, so they line up.
- **A picture** (`--picture`, default `target/analysis/<stem>.png`, 1600 px wide), in
  panels sharing one time axis, every axis labelled in text (a built-in 5×7 bitmap
  font; no font dependency):
  1. a spectrogram, log frequency 20 Hz–20 kHz with lines and labels at 100 Hz,
     1 kHz and 10 kHz, colour over a fixed 70 dB range below the loudest cell, its dB
     scale drawn beside it;
  2. LAF over time per channel, with L10, L50 and L90 as labelled lines;
  3. the onsets as ticks;
  4. for two channels: width (1 − IACC, 500 Hz–2 kHz) and ILD over 100 ms frames.
  The `png` crate writes it (pure Rust, no system libraries).
- **The JSON report** for the comparison tool, with the same over-time series in it.

## The measures, each from a published definition

| Group | Measure | Definition | Why |
|---|---|---|---|
| Level | Leq, unweighted, A- and C-weighted | IEC 61672-1 A- and C-weighting | The overall level; LCeq − LAeq is the low-frequency content (wind rumble), as ISO/TS 12913-3 reports it |
| | Level while it sounds | ITU-R BS.1770-4 gating, no K-weighting: the function presets are calibrated with, moved here so there is one. With a known calibration the absolute gate is 0 dB SPL instead of −70 dBFS | What `reference_spl` means |
| | LAFmax | FAST (125 ms) exponential time weighting, A-weighted, IEC 61672-1 | What bird papers report; peak over level while sounding |
| | L10, L50, L90 | Percentile levels of LAF (ISO 1996-1) | L90 is the bed under everything, L10 − L90 how eventful |
| | Peak, crest | Sample peak over RMS | |
| Colour | Third-octave bands, 25 Hz–20 kHz | IEC 61260-1 nominal centres, long-term level per band, absolute and relative to the total | The spectrum, as in the earlier tools |
| | Spectral centroid | Power-weighted mean frequency | Brightness in one number |
| Texture | Subband envelope moments M1–M4 (mean, variance over squared mean, skewness, kurtosis) | McDermott and Simoncelli 2011 (Neuron 71, Experimental Procedures): 30 half-cosine filters on the ERB-number scale, centres 52–8844 Hz, plus a lowpass and a highpass (32 subbands); Hilbert envelopes compressed by 0.3, lowpassed and sampled at 400 Hz; time averages weighted by a window with 1 s raised-cosine ramps | What makes rain rain and wind wind; the measures they showed people hear |
| | Modulation power, 20 filters 0.5–200 Hz, Q = 2 | Same paper: power in each half-cosine modulation filter over the envelope's variance | Gusting, flicker and roughness, by rate |
| | Cross-band envelope correlation C, subband offsets 1, 2, 3, 5, 8, 11, 16, 21 | Same paper (C) | Whether the bands move together (one gust) or apart |
| | C1: neighbouring subbands' modulation bands, octave filters 3.125–100 Hz, Q = √2 | Same paper (C1). C2 is left out | Whether the bands' fluctuations at one rate line up |
| Events | Onset rate per minute; intervals between onsets: median, 10th and 90th percentile | librosa 0.10 `onset_detect` with its defaults: spectral flux on a log mel spectrogram (128 bands, 2048/512), librosa's peak picker; decibels relative to the loudest cell so a gain changes nothing; a peak must also rise 0.75 dB above the median rise of the second around it, so steady noise has none | How often things happen, and how regularly |
| Space (2 or more channels) | IACC per octave band, 125 Hz–8 kHz, long-term and median of 100 ms frames | ISO 3382-1: the largest normalised cross-correlation within ±1 ms | How alike the ears are; diffuse sound is low, one source is high |
| | Apparent width | 1 − IACC averaged over the 500 Hz, 1 kHz and 2 kHz bands (Hidaka et al. 1995, IACC_E3) | The established single number for width |
| | Envelope coherence per octave band, 125 Hz–8 kHz | The correlation of the two ears' band envelopes (Hilbert, 400 Hz, uncompressed), long-term | Above about 1.5 kHz hearing follows envelopes, so waveform IACC of any diffuse sound is near 0 there; this says whether the ears' levels move together |
| | ILD per octave band: mean and standard deviation over 100 ms frames | Level difference, right minus left | Where sounds sit left and right |
| | ITD: median and interquartile range of the lag of the largest positive correlation, 88 Hz–1.4 kHz, frames where that peak exceeds 0.5 | | The same, from timing |
| | 5.1: level per channel, front (L C R) against surround (Ls Rs) | | Where the energy goes in the room |

Per-channel levels for every input. The space measures are left out for a mono file,
and the binaural ones (IACC, ILD, ITD, width) are given for a 2-channel file and for the
L/R pair of a 5.1 file.

## How it is built

A new crate, `crates/analysis` (`atrium-analysis`): pure functions on samples, with no
engine types, so it is tested without a scene and the measures cannot depend on the
engine they measure.

```text
crates/analysis/
  README.md           # design record: decisions that look wrong and are right, known gaps
  src/
    lib.rs            # facade: reexports only
    analyse.rs        # analyse(&Signal, &Settings) -> Report: each channel transformed once
    report.rs         # the Report types (serde), versioned
    signal.rs         # Signal, Settings; how channels combine; the calibration
    spectrum.rs       # the whole-file transform every filter is a gain on
    envelope.rs       # a band's Hilbert envelope at exactly 400 Hz
    level.rs          # Leq, the BS.1770 gate (moved from src/config/level.rs), percentiles
    weighting.rs      # A- and C-weighting and FAST time weighting
    bands.rs          # third-octave and octave bands
    texture/          # mod.rs, filters.rs (cochlear and modulation banks), statistics.rs
    onsets.rs         # librosa's spectral flux onsets
    space.rs          # IACC, width, envelope coherence, ILD, ITD
    over_time.rs      # the per-frame series the summary strip and the picture draw
    summary.rs        # the one-screen text
    picture/          # the PNG: mod.rs (layout), spectrogram.rs, curves.rs, canvas.rs, font.rs
    statistics.rs     # decibels, quantiles, correlation
```

Beyond the first layout: `analyse.rs` because `lib.rs` holds only the facade;
`spectrum.rs` and `envelope.rs` because the weightings, bands, texture and space all
filter the same whole-file transform; `signal.rs` and `statistics.rs` for what every
measure shares; `texture/` because the paper's model is two filter banks and a set of
statistics; `picture/canvas.rs` for the raster.

```rust
/// What is analysed: interleaved samples and what they are.
pub struct Signal<'a> {
    pub interleaved: &'a [f32],
    pub channels: usize,
    pub sample_rate: u32,
    /// L, R, C, LFE, Ls, Rs, or as the sidecar names them.
    pub channel_names: Vec<String>,
}

pub struct Settings {
    /// dB SPL of an RMS of 1.0 (0 dBFS here), when the calibration is known:
    /// the scene's spl_reference − 20·log10(target_rms), minus the gain
    /// --normalise applied. None for a recording.
    pub full_scale_db_spl: Option<f64>,
}

pub fn analyse(signal: &Signal, settings: &Settings) -> Report;
pub fn power_while_sounding(takes: &[(&[f32], u32)]) -> f64;
pub fn summary(heading: &str, report: &Report) -> String;
pub fn picture(heading: &str, signal: &Signal, report: &Report) -> Vec<u8>; // a PNG
```

The binary adds an `input` section (file, cut, sidecar) to the report's JSON.

Other changes:

- **The gated level moves.** `src/config/level.rs` and its tests move into the crate;
  `src/config/built_source.rs` calls `atrium_analysis::power_while_sounding`. No copy
  is left behind.
- **The renderer's sidecar gains the calibration**:
  `"calibration": { "full_scale_db_spl": 114.0 }`, from the scene's
  `normalization` (spl_reference − 20·log10(target_rms): 94 dB SPL at RMS 0.1 in the
  meadow; the code's default RMS is 0.5, 100.0 dB SPL). It is written before
  `--normalise`; the analyser subtracts `gain_decibels`.
- **`decode_channels` becomes `pub`**, for the binary.
- **`src/bin/analyse.rs`**: a thin binary that decodes, reads the sidecar, cuts
  `--from`/`--to`, calls `analyse`, writes the JSON and prints the summary.
- **The old research tools stay** (`analyze_sample`, `analyze_rain`, `analyze_synth`,
  `analyze_bubbles`, `spectrogram`). They answer narrower questions about single
  generators.
- Docs: a section in `run.md`, the tools list in `generator-workflow.md`, the crate
  README.

## How we know it is right

Each measure against a signal whose answer is known by hand, then the whole chain on
a real render:

| Check | Signal | Expected |
|---|---|---|
| Third-octave bands | Pink noise, 60 s | Every band from 100 Hz to 16 kHz within ±0.5 dB of the others |
| | 1 kHz sine at −20 dBFS RMS | The 1 kHz band at −20.0 ± 0.1 dBFS; bands two or more away at least 30 dB lower |
| A-weighting | Sines at 31.5 Hz, 100 Hz, 1 kHz, 10 kHz | LA − LZ = −39.4, −19.1, 0.0, −2.5 dB, each ±0.2 (IEC 61672-1) |
| C-weighting | The same sines | LC − LZ = −3.0, −0.3, 0.0, −4.4 dB, each ±0.2 |
| FAST | 4 kHz bursts of 200, 50 and 2 ms | LAFmax below the steady level by 10·log10(1 − e^(−T/0.125)): −1.0, −4.8, −18.0 dB, ±0.1 (IEC 61672-1 toneburst response) |
| Percentiles | 5 s at −20 dBFS and 5 s at −40, repeated for 60 s | L10 = −20 ± 0.5, L90 = −40 ± 0.5 |
| Texture | Gaussian noise, the cochlear subband nearest 300 Hz, uncompressed envelope | M2 = 4/π − 1 = 0.2732 ± 0.03 (Rayleigh) |
| | Noise fully modulated at 4 Hz, and at 64 Hz | Modulation power largest in the filter centred nearest that rate, in every subband from 1 to 8 kHz |
| | White noise; the same with one 2 Hz swing of depth 1 | C and C1 of subbands two apart near 0 (within the scatter 1/√(B·T)); with the swing C ≥ 0.778 − 0.05 (derived by hand from Rayleigh moments), C1 at 3.125 Hz near 1 |
| Onsets | 5 ms bursts every 0.5 s in noise 30 dB lower | 120 ± 2 per minute, median interval 0.500 ± 0.01 s |
| Space | The same noise in both channels | IACC 1.00 in every band, width 0.00 |
| | Independent noises, 30 s | IACC below 0.1 in every band |
| | Right = left delayed 0.5 ms | ITD 0.50 ± 0.03 ms |
| | Right = left × 0.5 | ILD −6.02 ± 0.05 dB |
| | Identical channels; independent noises | Envelope coherence 1.00; below 0.1 |
| | Independent noise carriers sharing one 2 Hz swing of depth 1 | Envelope coherence above 0.8 at 2 and 4 kHz (by hand 0.82 and 0.90, after the 200 Hz lowpass), IACC there below 0.1 |
| 5.1 | Front three at twice the surround's power each | Front − surround 4.77 ± 0.1 dB |
| End to end | The meadow's wind alone (`--probe ambient`, headphones, 60 s), through the sidecar | Leq 42 ± 1 dB SPL, the preset's level |
| | The same, `--normalise`d | The same dB SPL: the gain is undone |
| | The same | IACC above 0.8 at 125 Hz and below 0.4 at 2 kHz and up: a diffuse field at the ears |
| | A blackbird take (MP3, mono) | A report with no space section, and its LAFmax minus level while sounding within 0.3 dB of what was measured when the levels were set (7.6, 6.3 and 6.7 dB for its three takes) |
| Picture | A 1 kHz tone from 2 to 4 s in a 6 s file | The bright row sits at the 1 kHz line, from the 2 s to the 4 s tick (checked on the decoded PNG's pixels, not by eye), within the 85 ms window's half-length |
| | The meadow render | Claude reads the PNG and the summary, and describes the birds' songs and the wind bed from them alone; the description must match what the scene holds |
| Speed | 60 s of stereo at 48 kHz, release build, picture included | Under 5 s |

Then: `cargo fmt`, `cargo clippy` (no new warnings), `cargo test --workspace`,
`scripts/dep-cycles.py`, and the boundary check from AGENTS.md (no implementation
type of the crate named outside it).

## Not in this step

- **Comparing two reports.** The next tool; this report is its input.
- **Cutting events out of a recording.** The event cutter, after that.
- **Calibrating a recording to dB SPL.** A reference recording's level is unknown,
  so it is reported in dBFS; comparisons use shapes and differences, not absolute
  level.
- **Loudness models** (Zwicker, ISO 532). The A-weighted levels and the third-octave
  bands cover what the comparison needs first.
- **C2**, the paper's phase statistic between modulation bands an octave apart.
