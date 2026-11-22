# atrium-analysis

One report for a recording or a render: level, colour, texture, events and space, each from
a published definition. The same measures for a field recording and for what the engine
renders, so the two can be compared. Measurements report differences; only listening judges.

```rust
let report = atrium_analysis::analyse(&Signal { interleaved, channels, sample_rate, channel_names }, &Settings { full_scale_db_spl });
let text = atrium_analysis::summary("heading", &report);      // one screen, every number with its unit
let png = atrium_analysis::picture("heading", &signal, &report); // 1600 px, panels on one time axis
```

`report` serialises to JSON (`report_version: 1`) for the comparison tool. The binary that
decodes files, reads a render's sidecar and writes all three is `src/bin/analyse.rs`; see
run.md, "Analyse a file". `power_while_sounding` is the gated level the engine calibrates
presets with; it lives here so there is one.

Pure functions on samples: no engine type, no file, no I/O. The measures cannot depend on the
engine they measure, and every one is tested against a signal whose answer is known by hand.

## The measures

| Group | Measure | Definition |
|---|---|---|
| Level | Leq, LAeq, LCeq | IEC 61672-1 A and C weightings |
| | Level while it sounds | ITU-R BS.1770-4 gating, no K-weighting |
| | LAFmax, L10, L50, L90 | FAST (125 ms) time weighting; percentiles of LAF (ISO 1996-1) |
| | Peak, crest | Sample peak, peak over Leq |
| Colour | Third octaves 25 Hz–20 kHz, centroid | IEC 61260-1 base-ten bands |
| Texture | M1–M4, C, modulation power, C1 | McDermott and Simoncelli 2011, Experimental Procedures |
| Events | Onsets, their rate and intervals | librosa 0.10 `onset_detect` defaults |
| Space | IACC per octave, long-term and per 100 ms | ISO 3382-1 |
| | Width | 1 − IACC over 500 Hz, 1 and 2 kHz (Hidaka et al. 1995) |
| | Envelope coherence per octave | Correlation of the two ears' Hilbert band envelopes |
| | ILD per octave, ITD 88 Hz–1.4 kHz | Level difference; lag of the correlation peak |
| | 5.1 front against surround | L C R against Ls Rs |

## Layout

```text
src/
  lib.rs           facade: reexports only
  analyse.rs       the orchestration: each channel transformed once, the measures, the report
  report.rs        the Report types (serde), versioned
  signal.rs        Signal, Settings; how channels combine; the calibration
  spectrum.rs      the whole-file transform every filter works on
  envelope.rs      a band's Hilbert envelope at 400 Hz
  weighting.rs     A, C, FAST
  level.rs         Leq, the BS.1770 gate (power_while_sounding), percentiles
  bands.rs         third octaves and octaves
  texture/         McDermott and Simoncelli: filters.rs (cochlear and modulation banks), statistics.rs
  onsets.rs        spectral flux and librosa's peak picker
  space.rs         IACC, width, envelope coherence, ILD, ITD
  over_time.rs     the series the strip and the picture draw
  summary.rs       the one-screen text
  picture/         mod.rs (layout), spectrogram.rs, curves.rs, canvas.rs, font.rs
  statistics.rs    decibels, quantiles, correlation
  test_signals.rs  noise, pink noise, sines (tests only)
```

## Decisions that look wrong and are right

- **Every filter is a gain on one Fourier transform of the whole file, not an IIR filter.** The
  bilinear-transform A-weighting every audio library uses is 1 dB low at 10 kHz at 48 kHz (it
  maps 11.7 kHz to 10 kHz, and its two extra poles become zeros at Nyquist); IEC 61672-1's
  tolerance there is generous, the acceptance check (±0.2 dB) is not. On the spectrum the
  analytic curve holds to the hundredth of a decibel and the filter is zero-phase. The blackbird
  levels were set with A-weighting done the same way, and the analyser reproduces their LAFmax
  over the gated level (7.6, 6.3, 6.6 dB against 7.6, 6.3, 6.7). The cost is memory: a few
  copies of each channel at once.
- **The transform is not a power of two.** Its length is a multiple of
  `sample_rate / gcd(sample_rate, 400)` (120 at 48 kHz, 441 at 44.1 kHz) times a power of two,
  so 400 Hz is a whole number of bins and every envelope comes out at exactly 400 Hz from an
  inverse transform, with no interpolation. rustfft handles the factors 3, 5 and 7 efficiently.
- **0 dBFS is an RMS of 1.0, so a full-scale sine reads −3.01 dBFS**, not 0 as AES17 has it.
  The engine is calibrated in RMS (`target_rms` at `spl_reference`), and the sidecar's
  `full_scale_db_spl` is the SPL of RMS 1.0; with the sine convention every level would carry a
  3 dB offset to explain.
- **Two channels are averaged, three or more summed.** Two channels are ears: a source in front
  reaches each ear at its full level, and the level a person hears is that, not twice it. Speaker
  channels add at the listener, and a source panned with power-preserving gains then reads its
  own level. A stereo music file is read as ears too.
- **The level while sounding gates at 0 dB SPL when the calibration is known, not at −70 dBFS.**
  BS.1770's absolute gate is for broadcast levels. A render at the engine's own level puts the
  meadow's 42 dB SPL wind at −72 dBFS: the −70 dBFS gate called the whole file silence, and a
  `--normalise`d copy of the same render read 42 dB. In dB SPL the gate is the same at any gain.
  `power_while_sounding`, for takes, keeps −70 dBFS: takes are recordings at a recording's level.
- **The onset spectrogram is in dB relative to its loudest cell, not to 1 as librosa's.** The
  flux is a difference of decibels, so a constant reference changes nothing, except where
  librosa's 10⁻¹⁰ power floor bites, which it does at the engine's quiet level. Relative to the
  loudest cell, a render and its normalised copy give the same onsets.
- **An onset must also rise 0.75 dB above the median rise of the second around it**, beyond
  librosa's method. librosa scales the flux to 0–1, so steady noise is picked at its own
  wobble: the meadow's wind alone gave 67 onsets a minute. Measured in dB, 99.9 % of that
  wind's frames rise at most 0.5 dB above the median rise, while a bird's syllable starting
  stands 2–8 dB above it (blackbird, robin, dunnock, blue tit takes). With the threshold the
  wind gives none and the meadow mix about 50 a minute, its syllables starting through the wind.
- **Texture is measured per channel and the statistics averaged, weighted by power; the
  channels are not mixed down first.** At the ears a diffuse wind is decorrelated above
  500 Hz; summing the ears would partly cancel and partly add the two envelopes and make the
  wind's statistics more Gaussian than either ear's. The weights make a silent channel (quad's
  C and LFE) count for nothing.
- **Width in the strip and the picture (frame mean) is lower than the long-term width.** The
  wind alone reads 0.94 over the whole file and 0.87 per 100 ms frame. A correlation over
  100 ms of a band 700 Hz wide has a positive bias of the order of 1/√(B·T) ≈ 0.1 for
  uncorrelated signals, and IACC takes its absolute value; over 60 s that bias is gone. Both
  are reported, each named for what it is.
- **Envelope coherence uses the texture's envelope, lowpassed at 200 Hz.** Hand-derived, the
  coherence of two independent noises sharing one 2 Hz swing of depth 1 is 0.55 with the full
  Hilbert envelope and about 0.82 at 2 kHz with the lowpass, because the lowpass removes most of
  each ear's own Rayleigh fluctuation. Hearing smooths envelopes too; the number is what the
  acceptance check (above 0.8) asks of it, with little margin at 2 kHz.
- **ITD is the lag of the largest positive correlation, IACC of the largest absolute one.**
  ISO 3382-1 defines IACC on |IACF|; a timing difference read from a negative peak would be a
  polarity inversion, not a delay.
- **FAST starts from the mean power of the first 125 ms, not from silence.** From silence a
  file opens with a quarter second of rising level that pulls L90 down.
- **The LAF series keeps the largest value in each 25 ms**, not a sample: a 2 ms event's peak
  is then in the series however coarsely it is drawn.
- **Kurtosis is not excess kurtosis** (a Gaussian's is 3), as the paper writes M4.
- **C2 is left out.** It needs analytic modulation bands and phase doubling for every
  subband; it separates crackling from noise more than it describes wind or rain. Add it when
  a comparison needs it.
- **The picture draws text with a 5×7 bitmap font in the crate**, not a font crate or file: no
  dependency, the same pixels everywhere, and readable at twice its size in a 1600 px image.

## Known gaps

- **The onset threshold is ours, set from four bird takes and one wind.** 0.75 dB above the
  local median rise keeps no onset in the meadow's wind and keeps the birds' syllables. Labelled
  reference recordings (onsets marked by ear) would set it properly; until then the rate
  compares files of a kind, and it may miss soft events in a loud bed.
- **5.1 is tested only on synthetic noise.** The front/surround split and the L/R pair have
  hand-derived tests; no 5.1 render has been analysed yet.
- **A recording's level is in dBFS.** Its calibration is unknown; comparisons use shapes and
  differences.
- **The whole file is in memory**, several copies per channel. Minutes are fine; hours are not.
- **Below 20 kHz sample rate** the cochlear bank's top bands lie above Nyquist and read as
  zeros.
- **The spectrogram's window is 85 ms**: an edge is smeared by ±43 ms, and the lowest octaves'
  rows interpolate between 12 Hz bins.
