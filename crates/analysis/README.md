# atrium-analysis

One report for a recording or a render: level, colour, texture, events and space, each from
a published definition. The same measures for a field recording and for what the engine
renders, so the two can be compared. Measurements report differences; only listening judges.

```rust
let report = atrium_analysis::analyse(&Signal { interleaved, channels, sample_rate, channel_names }, &Settings { full_scale_db_spl });
let text = atrium_analysis::summary("heading", &report);      // one screen, every number with its unit
let png = atrium_analysis::picture("heading", &signal, &report); // 1600 px, panels on one time axis
```

And reports side by side, where other files differ from a reference:

```rust
let comparison = atrium_analysis::compare(&reference, &others);   // each a Compared: report, name, halves, timeline
let text = atrium_analysis::comparison_summary(&comparison);      // one screen: reference, each file, each difference
let png = atrium_analysis::comparison_picture(&reference, &others); // 1600 px of overlays, the reference black
```

And the same measures on a stream, for the app's Analysis lanes while a scene plays:

```rust
let mut live = atrium_analysis::Live::new(sample_rate, channels, full_scale_db_spl);
live.push(&block, clock_sample);       // what the output played, from that clock sample on
let columns = live.take_columns();     // 50 ms each: spectrum, LAeq and LAF, onset, width, ILD
```

`report` serialises to JSON (`report_version: 1`), which the comparison reads back as it is;
`Comparison` serialises to JSON too (`comparison_version: 1`). The binary that
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
  compare/         the comparison: mod.rs (Compared, compare), comparison.rs (the serde types),
                   one file per group (level, colour, texture, events, space, over_time),
                   summary.rs (its one-screen text), tests.rs (the acceptance checks)
  picture/         mod.rs (layout), spectrogram.rs, curves.rs, canvas.rs, font.rs;
                   comparison.rs (the comparison's picture) on axes.rs (overlay panels)
  frequency_rows.rs  the spectrogram's axis (20 Hz to 20 kHz, log) and window, picture and live
  live/            the stream: mod.rs (Live, Column), a_weighting.rs, spectrum.rs, onsets.rs
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

## The comparison

`compare` takes a reference and any number of other files, each a `Compared`: its report, a
short name, how it was made, its two halves analysed apart (read for the reference), what fixes
its timing (a render's scene, seed and stretch) and, for the picture, its samples. Every
difference is the other's value minus the reference's. It reports where files differ; only
listening says which is better.

| Group | Compared | As |
|---|---|---|
| Level shape | L10 − L90, LAFmax over the level while sounding, crest, LCeq − LAeq | Differences (dB) |
| Absolute level | LAeq, Leq, only when both files carry a calibration | Differences; marked beyond 1 dB, the JND ISO 3382-1 gives |
| Colour | Third octaves relative to each file's own Leq | The difference per band; its RMS over 50 Hz–16 kHz; the tilt (a least-squares line over octaves, 100 Hz–10 kHz) of each; the centroid ratio |
| Texture | M1, M2, M3, M4, C, modulation power, C1 | Per class the paper's SNR, 10·log10(Σ s² / Σ (s − s′)²), s the reference's; whether ≥ 20 dB (the paper's "converged"); a scatter floor (below); the three places (subbands, pairs, modulation rates) holding most of the error |
| Events | Onsets per minute, interval median, 10 % and 90 % | Differences and the rate's ratio |
| Space | Two two-channel files: IACC, envelope coherence, ILD spread per octave; width | Differences; width marked beyond 0.075, the JND of IACC (Cox, Davies & Lam 1993) |
| Over time | The 25 ms LAF series and the width per 100 ms frame | Distributions (L95 … L5; width exceeded 90, 50, 10 %), and for two files on one timeline the largest difference and when |

The picture overlays, on one set of axes per panel: the third-octave colour and the difference
from the reference; the modulation spectrum; the LAF distribution; the texture SNR per class
with its scatter floor; M2 and M4 per subband and C between neighbours; IACC and envelope
coherence per octave; and each file's spectrogram as a strip.

### Decisions that look wrong and are right, comparing

- **Each texture class carries a scatter floor, beyond the paper's 20 dB.** McDermott and
  Simoncelli imposed statistics on a synthetic sound until they matched; their SNR measured
  the imposing, not chance. Two independent takes of one sound differ by chance, and in
  statistics near zero that chance is most of Σ s²: two 30 s takes of the same white noise
  reach C 7.8 dB and C1 4.3 dB, M3 17.1 (the other classes 27 to 53 dB). By the paper's 20 dB
  alone every pair of noise recordings "differs in comodulation". The floor is the SNR two takes
  of the reference's sound would reach: the reference's halves are analysed apart, their squared
  error is 4c/T for a statistic whose estimate scatters with variance c/T, and two whole takes
  of T and T′ seconds would differ by c/T + c/T′, the halves' error times (1 + T/T′)/4. Between
  those two noise takes every class's SNR lay from 1.2 dB below its floor to 3.4 above, so a
  class counts as within the floor down to 3 dB below it. A class below 20 dB and below its
  floor is marked as a difference (`!`); below 20 dB and within it, as chance (`~`).
- **The floor comes from the reference's halves only.** The other file is taken to scatter as
  the reference does. The reference says what the sound is; and a file read from its report
  JSON has no halves, so needing the other's would make a report compare differently from the
  render it was written from (checked: through `analyse`, the same comparison to the bit, apart
  from the file's name).
- **M1 is compared after the overall level difference is taken out.** The envelope means follow
  the level: a gain g scales every compressed envelope, and so its mean, by g^0.3. The other's
  means are multiplied by 10^(−0.3·ΔL/20), ΔL the difference of the unweighted Leq in dBFS
  (the samples' level, whatever the calibration), as the colour is compared relative to Leq. The
  same noise 10 dB louder then has every class beyond 60 dB. What remains of M1 is the spectrum
  across subbands, so white against pink noise is 10.8 dB in M1 while their M2 and M4 converge.
- **The tilt is fitted on the bands' exact centres, 1000·10^(n/10) Hz, not the nominal ones.**
  White noise's third octaves rise 3.01 dB per octave only on the exact centres; on the
  nominal ones (125 for 125.9) the fit is off by a few hundredths.
- **A band more than 100 dB below its file's Leq counts as −100 dB.** An empty band (an MP3
  above its lowpass, a silent channel's) would otherwise put a hundred-odd dB into the RMS. No
  difference below that is one of colour.
- **The LAF distribution is relative to each file's L50 unless both are calibrated**, as all
  level is compared as shapes when either is not: a recording's dBFS says nothing of its SPL.
  It is taken from the report's 25 ms LAF series (the largest in each step), so its L10 can
  stand a tenth of a decibel above the report's own.
- **Two files line up only when both say so, alike.** The binary gives a render's timeline as its
  scene, seed, start, weather speed and the stretch cut from it; layout, mode, probe and a head
  turn change what is heard, not when. A recording has none and lines up with nothing, not even
  itself in another file. Only then is the over-time difference sample by sample.
- **Space is compared only between two two-channel files.** A 5.1 file's L/R pair is two
  loudspeakers, not two ears; its IACC against a pair of ears compares different things.
- **The spectrogram strips share one time scale, the longest file spanning the width, and each is
  relative to its own loudest cell.** A 3 s blackbird take is then a short strip beside a minute
  of wind, which is what they are; and an uncalibrated recording's cells have no level to share.

## Live

`Live` measures what the engine plays, a column every 50 ms, from the same pieces as the
report, so a live number and an `analyse` number of the same sound cannot disagree beyond what
a stream cannot know. `src/offline/tests/live_analysis.rs` (in the engine) renders the meadow and
reads it both ways: every 100 ms width and ILD identical, every 3 s LAeq identical to 0.0001 dB,
43 onsets live against 44 whole in the minute; the wind alone 40.82 dB SPL live against 40.82.

| Lane | Per column | Shared with the report |
|---|---|---|
| Spectrum | Band level (dB SPL), 96 rows a tenth of an octave, 20 Hz to 20 kHz | The picture's window and axis (`frequency_rows`) |
| Level | A-weighted, per channel and overall (as the report's overall level): LAeq over the column, the largest LAF in it | `a_weighting`, `Fast`, `Combination` |
| Onsets | Whether one fell in the column | `MelFrames`, `rise`, `is_peak`, `stands_clear` |
| Width, ILD | Over the 100 ms that end with the column | `PairFrames::width_and_ild` |

### Decisions that look wrong and are right, live

- **The A-weighting is a 4101-tap FIR, not an IIR filter.** The curve's own zero-phase impulse
  response, cut at ±43 ms, applied by overlap-save and read 43 ms late so it is zero-phase again.
  A bilinear IIR is a decibel low at 10 kHz (above); the FIR holds the analytic curve within
  0.05 dB from 31.5 Hz to 10 kHz, and a column's LAeq is the whole-file weighting's to 0.0001 dB.
  The cost is the delay: a column waits 43 ms for the samples after it.
- **Width and ILD are read by the report's own frame code**, split so live reads only the
  three width bands and the broadband ILD (three correlations of the eight per frame). Every
  other column ends where a report frame ends, and there they agree exactly. The same frame
  bias as the report's frames applies: the wind reads about 0.87, not the long-term 0.94.
- **The onset median is of the past second, not of the second around the frame.** A stream has
  no future second. The peak picker's moving mean does look 100 ms ahead, as librosa's; the
  detector waits for it. Two more things are "so far": decibels relative to the loudest mel cell
  heard so far, and the 0.07 of the flux scaled to the largest flux so far. On the meadow's
  minute this costs one onset in 44.
- **A spectrum row is the power of the bins it overlaps, in proportion to the overlap**, not the
  picture's mean of the bins whose centres it holds. The picture's cells are relative to the
  loudest; as absolute band levels the mean would be off by the row's width over its whole bins,
  3 dB where rows are one or two bins wide and up to 0.7 dB for a sine at 1 kHz. Shared by
  overlap, the rows add up to the power from 20 Hz to 20 kHz, and a sine's lobe to its level.
- **Spectrum rows combine channels as the overall level does** (ears averaged, speakers summed),
  not as a plain mean of the channels, so a row and a level of the same sound agree on 5.1 too.
- **FAST starts from the first column's level.** The report starts from the first 125 ms; a
  stream would have to hold its first column back for it.
- **A column is finished only when everything in it is**, about 0.15 s after it sounded (the
  onset picker's 100 ms ahead and its frame), so a column never changes once taken.
- **A push that does not follow on from the last starts afresh.** The tap drops whole blocks
  when nobody reads it; the gap shows on the clock, and joining across it would put silence or a
  splice into FAST and the flux. The columns still waiting are lost with it.

## Known gaps

- **The scatter floor's margin was set on one pair of white-noise takes** (SNR within 1.2 dB
  below to 3.4 dB above the floor, so 3 dB of margin). Takes of a recorded wind or rain whose
  statistics drift over the file scatter more than halves of a stationary noise show; a floor from
  halves then reads too high, and a real difference too often. More pairs of real takes would
  set it properly.
- **The floor assumes the other file scatters as the reference does.** Against a sound of
  another kind (a blackbird against wind) the floor is the wind's; that comparison is far below
  any floor anyway.
- **The comparison's texture compares subbands by index**, which is right for any two sample
  rates of 20 kHz and above (the cochlear bank is the same up to 10 kHz; only the highpass's top
  differs).

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
- **Live onsets in the first seconds of a stream** are held to a lower bar (the largest flux
  so far is small) and have no full second of median behind them.
- **Live 5.1 is tested on synthetic noise only**, as the report's is.
- **The spectrogram's window is 85 ms**: an edge is smeared by ±43 ms, and the lowest octaves'
  rows interpolate between 12 Hz bins.
