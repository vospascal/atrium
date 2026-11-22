# Cutting a long recording into takes

Planned 2026-11-22, built the same day; where the build departs from the plan, the plan is
corrected and "As built" at the end says why. The fourth tool ([generator-workflow.md](generator-workflow.md#the-tools-in-order)),
after the renderer, the analyser and the comparison; item 18 of
[ui-design-plan.md](ui-design-plan.md). It takes a long field recording and gives back what a
sound with a repertoire needs: its takes as files, the rests between them, its bouts, and a
preset that uses them, all measured from the recording, not guessed.

Why it matters now: every `rests` rule in `sources/` has its gap "a first guess by ear, not
measured" (the blackbird's own comment), and `sources/frog.yaml` plays its whole 78 s chorus as
one take with a guessed gap of 5–20 s.

## The first real case, measured by hand

`assets/frog.mp3` (78.6 s, stereo), through `analyse` and its LAF series (25 ms):

- **Eventful**: L10 − L90 53 dB; LAFmax 7.3 dB over the level while sounding.
- **Calls**: at 15 or 20 dB above the bed the count holds steady (17 and 16 events) at
  1–2.5 s each. The gaps between calls run mostly 1.8–5.6 s, so the preset's guessed gap
  of 5–20 s is too long.
- **The bed moves**: the 10th percentile over 10 s windows is −67 dB until 40 s and −78 dB
  after. One bed for the whole file (−77 dB) made the first 41 s a single "take". The bed
  has to be local.
- **Intervals come in two kinds**: at 10 dB above the bed the gaps split into pulses inside a
  call (0.06–0.3 s) and rests between calls (1–3 s), with little in between. That split is
  what separates "one take" from "a rest".

## What it is

```sh
cargo run --release --bin cut -- <recording>[@from-to] [--name <kind>] [--band <low>-<high>] [--picture]
```

- **A binary of its own**, not part of `analyse`: it writes audio and a preset, and
  `analyse` only measures. It reads files the way `analyse` does, including `@from-to`.
- **It writes only under `target/cut/<name>/`**. It never touches `assets/` or `sources/`;
  you copy the takes and the preset across once you have listened to them. It refuses to
  overwrite a folder that is already there.
  - the takes, `take-01.wav` …, 32-bit float at the recording's rate and channels;
  - `cut.json` (`cut_version: 1`): every decision with the values behind it, and where each
    take lies in the recording;
  - a one-screen summary, written for Claude to read like `analyse`'s;
  - `preset.yaml`, a draft source;
  - with `--picture`, one PNG.

## How it decides

The decision is a pure function in `crates/analysis` (`cut(signal, settings) -> Cut`, a
private `cut/` module, one file per step below). The binary only reads and writes files. That
way every rule is testable on synthetic signals without touching the disk.

| Step | What | From |
|---|---|---|
| 1. Band | Detect in the third-octave bands where the recording's events stand furthest above its bed: L1 − L90 per band, the bands within 6 dB of the largest. `--band` overrides. A bird at 2–8 kHz is then found under wind that sits below 500 Hz. The level is of the channels' mean (as the engine plays a take), a reading every 1 ms, each over a frame of B·T = 5 (ten degrees of freedom, B the band's width) | Raven's band-limited energy detector (Cornell Lab), which detects in a chosen band for the same reason |
| 2. Local bed | The band level's 10th percentile over the 10 s before the moment and over the 10 s after it; of the two, the one the level around the moment (its median over 1 s) reaches: the larger if both, else the smaller | Raven's noise estimate (a percentile over a block); the frog's 11 dB step at 40 s |
| 3. Events | An event starts when the band level rises 15 dB above the local bed and ends when it falls below bed + 6 dB (hysteresis, so a decaying call is not cut in two) | The frog: the count is steady from +15 to +20 dB and breaks apart at +10. Set from that measurement, not a publication, and said so in the code, as the onset threshold is |
| 4. One take or a rest | Gaps shorter than the bout criterion interval join two events into one take. The criterion is fitted to the gaps as two Poisson processes (within a take, between takes) by maximum likelihood. Nothing is joined, and the summary says why, if one process fits as well as two for their cost (Schwarz's BIC) or if the fitted distribution of log gaps has one peak, not two | Sibly, Nott & Fletcher 1990 (Anim. Behav. 39: 63–69), after Slater & Lester 1982; fitted by maximum likelihood as Langton, Collett & Sibly 1995 and `diveMove`'s `fitMLEbouts` do. ohun's `hold.time` is the same join with a fixed value |
| 5. Edges | The start moves back down the attack, while the level falls and is above bed + 3 dB, at most 0.3 s. The end moves on until it reaches bed + 3 dB, at most 2 s, to keep the decay and the recorded space's reverb. An edge is the outer end of its reading's frame. Fades are 5 ms raised-cosine in and out, outside the edges, so they lie over material at the bed | Clicks, tested |
| 6. What is dropped | Takes shorter than 50 ms (before the edges and gaps: they are not the animal), takes cut off by the recording's start or end, and takes that clip (three samples running at full scale) | |
| 7. Per take | Duration, level while sounding (`power_while_sounding` of the channels' mean), level relative to the loudest take, SNR over the local bed (band level while its events sound), centroid. A take with an SNR below 20 dB is flagged: its bed will gate in and out with it | |
| 8. Types and bouts | Similarity of two takes: the largest normalised cross-correlation of their log-mel spectrograms (the detection band, 20 ms windows) over a time shift. Takes are grouped by average linkage. A run of one type in the recording is a bout | Spectrogram cross-correlation as in Raven and warbleR's `cross_correlation`. The grouping threshold is chosen on an annotated recording (see the checks); until there is one it is 0.40, provisional, set from synthetic songs, and the matrix is always reported |

## What goes into the preset

| Preset | From the cut | Note |
|---|---|---|
| `takes` | Every take that is not flagged, named by type and number, at its file in `target/cut/<name>/` | The paths are changed when the takes are copied to `assets/` |
| `rule.rests.gap` | The 10th and 90th percentiles of the measured gaps | The engine's gap runs from the end of one take to the start of the next (`src/plan/voice.rs`). The cutter measures between the same edges. The rule draws evenly inside the range; the full distribution stays in `cut.json` |
| `rule.rests.bout` | The shortest and longest runs of one type | Only when there are at least two types and some run is longer than one |
| `level_spread` | Left out | The engine sets one level for all takes together (`src/config/built_source.rs`), so the takes keep their measured differences; adding `utterance` on top would count the spread twice. The measured SD is in the summary. One caveat: in the field it also holds the bird's turning and moving, not only its song |
| `reference_spl` | Not written; the preset will not load until it is filled in | A recording's dBFS says nothing of SPL. It comes from the literature, as the birds' did (Brumm et al. 2025) |
| `vary.pitch`, `follows`, `width` | Not written | Not in a recording; copied from the kind's existing preset if `--name` matches one, with its `name` and `group` |

## The picture

1. The recording's spectrogram (strips per minute for a long file), each take boxed and
   labelled with its number and type, the detection band marked.
2. Under it, the band level with the local bed and both thresholds.
3. The log-gap histogram with the fitted two processes and the bout criterion.
4. The similarity matrix of the takes, ordered by type.

## How we know it is right

Synthetic checks in `crates/analysis/src/cut/tests.rs`; those that need a recording or the
engine in `src/offline/tests/cut.rs`.

| Check | Expected | Measured (test) |
|---|---|---|
| Synthetic: 20 chirps in pink noise at known times, SNR 30, 20 and 12 dB | Every chirp found; starts within 10 ms, ends within 20 ms; no other takes | Every chirp one take, no other; the band 2.2–3.5 kHz (the chirps sweep 2–4 kHz); worst start 7.9, 7.9 and 5.9 ms, worst end 10.9 ms at all three (`chirps_in_pink_noise_are_found`). The same chirps under a wind below 500 Hz 10 dB louder than they are: the same band, every chirp found (`a_bird_under_wind`) |
| The same with the bed 11 dB louder for the first half (the frog's step) | The same takes; the first half is not one take | The same 20 takes; worst start 5.1 and 5.9 ms, worst end 5.1 and 10.9 ms at SNR 30 and 12 dB (`a_bed_that_steps_down_halfway`) |
| 60 s of pink noise; 60 s of our soft wind | No takes | No event in either (`pink_noise_alone_has_no_takes`, `our_soft_wind_has_no_takes`) |
| Songs of five 80 ms syllables 60 ms apart, songs 3–8 s apart | One take per song. The bout criterion lies between 0.06 and 3 s, and the two fitted processes match the drawn rates within 10 % | 20 takes of 5 events each; criterion 0.347 s; fast 17.24/s against 16.67 (+3.4 %), slow 0.187/s against 0.182 (+3.0 %) (`syllables_join_into_songs`) |
| A tone burst in noise, cut | No click: each take's first and last 5 ms below bed + 3 dB, and no sample step at the edges larger than the signal's own | Each take starts and ends at 0; its fades lie 24–26 ms before the burst and after it, over the bed alone; the largest step in a take's first and last 5 ms 0.0020–0.0034 against the recording's own 0.0152 (`a_cut_take_does_not_click`) |
| Two song shapes in the order A A A B B A A | Two types; bouts of 2–3 | Types A A A B B A A, bouts 3, 2, 2; two songs of one shape at least 0.68 alike, of two shapes at most 0.12 (`two_song_shapes_make_two_types_and_bouts`) |
| `assets/frog.mp3` | 15–18 calls of 1–2.5 s (the hand count above), the first 41 s not one take, gaps mostly 1.8–5.6 s | 19 takes, none flagged, 3 clicks dropped; 18 of 0.5–1.4 s and one of 4.3 s (four chorus calls, see "As built"); 7 takes in the first 41 s; criterion 0.09 s; gaps median 2.9 s, 11 of 18 within 1.8–5.6 s, 10–90 % 0.11–5.2 s; one type (`the_frog_recording`). The hand count was low: see "As built" |
| Round trip: the frog's preset (with reference_spl filled) rendered for 2 minutes and `analyse --compare`d with the recording | Onsets per minute and the LAF distribution relative to L50 within the recording's own chance between its halves. This checks the whole chain, with the comparison tool | Onsets: 81.5 a minute against the recording's 51.9, within its halves' 32.0 and 85.4. The LAF distribution is not: the render is silent between takes where the recording has its bed, so its L95 to L50 are one level and its L10 stands 116 dB over L50 against the recording's 42 (L45 +1.7 dB, L25 +40, L5 +82 from the recording, against the halves' +6.4, +7.1, −16.3) (`the_frog_round_trip`). See "As built" |
| An annotated field recording with each call's start and end marked | Event F-score with a 200 ms onset collar (Mesaros, Heittola & Virtanen 2016, as `sed_eval` scores DCASE), reported. The type threshold is chosen here. This needs a download, so I will name the dataset, its licence and its size and ask before fetching it | Pending: no recording fetched yet |
| The takes load | A scene with the written preset builds and plays in the engine (test) | As written the draft does not load; with `reference_spl: 65.0` it reads with 19 takes, the frog scene with it builds, and 20 s of it sound (`the_written_preset_loads_once_filled`) |

## Separation (Meta SAM Audio): not in this step

SAM Audio (Meta, Dec 2025, arXiv 2512.18099) isolates a sound named in text ("bird singing")
from a mix and returns the target and the residual. Released in small, base and large sizes,
under the SAM License.

- **What it needs**: Python ≥ 3.11 and PyTorch. Meta recommends a CUDA GPU; on a Mac it is
  untested. The checkpoints are gated on Hugging Face (request access, then `hf auth login`).
- **Why it is worth it**: it turns one recording into two layers. The bird becomes takes with
  no bed gating in and out, and the residual becomes an ambient bed in its own right.
- **How it fits later**: the cutter cuts any file, so a stem SAM Audio made can be cut like
  any recording, with no change here. Whether to set it up, and on which machine, is your
  call once the cutter works.

Google's bird MixIT model (Denton et al. 2022, trained on Xeno-canto) is an open alternative
trained on birdsong only. It sometimes splits one song across outputs.

## Not in this step

- Cutting in the app: item 18 asks for it there too. That belongs with the generator lab,
  where a take can be heard as it is cut.
- Registers (song, call, alarm as separate sets; item 20). Types are grouped by shape, not by
  meaning.
- Denoising a take. The flag says where the bed will be heard; separation is the remedy.
- Cutting sounds without silences (a stream, rain): they have no takes, and loops or
  generators serve them.

## As built

Where the build departs from the plan above, and why.

- **L1 − L90 picks the band, not L10 − L90.** L10 sees the events only when they sound more
  than a tenth of the time. The chirp check's 150 ms calls every 4 s sound 4 % of the time; by
  L10 − L90 every third octave was as eventful as the next (the bed's own scatter), and the whole
  spectrum was detected in. With L1 the chirps' bands are chosen, and the chirps are found
  under a wind 10 dB louder than they are. The frog's band moved from 800 Hz + 2 kHz to
  1.8–2.8 kHz.
- **Each level reading averages B·T = 5.** With frames of one length for every band, the narrow
  low bands scatter most by chance and pure noise looks eventful there. At ten degrees of
  freedom the noise's 10th percentile lies 3.1 dB under its mean and a reading of noise alone
  reaches the start threshold with a chance under 10⁻¹⁰. A reading is 2 ms in a wide band,
  4.8 ms in the frog's, 22 ms in one third octave at 1 kHz.
- **The bed is taken behind and ahead of the moment, not over a window centred on it.** A
  centred 10th percentile steps down as soon as a tenth of its window lies past a step: for
  4 s before the step-down check's step the loud half was measured against the quiet bed, and
  its noise reached the start threshold. Of the two one-sided beds, the one the level around
  the moment reaches is the bed there, which is the right side of a step either way.
- **The criterion is fitted by maximum likelihood**, the method's authors' own refinement
  (Langton, Collett & Sibly 1995), not by least squares on a binned histogram, whose answer moves
  with the bins. **And two processes must give two peaks in the log gaps**, besides fitting
  better by Schwarz's BIC. Rests are not Poisson: rests spread evenly over 1–4 s fit two
  exponentials better than one, and the BIC alone would join calls across the shorter rests.
- **The start walks back only while the level falls.** At ten degrees of freedom a reading of
  the bed alone is above bed + 3 dB nearly half the time, so walking back to the first reading
  under it wandered on into the bed: one song start in twenty came 11 ms early. The end still
  moves on to bed + 3 dB, through the decay's own wobble.
- **Short takes are dropped first; takes cut off or clipping last.** A click must not hold back
  a neighbour's edge or split a rest in two, so it goes before the edges and gaps; a cut-off or
  clipped call is still the animal resting around it, so its gaps count.
- **The frog's hand count was low.** It counted on the A-weighted FAST level, whose 125 ms
  smoothing runs together calls less than 0.3 s apart. The spectrogram shows them apart (45.2
  and 46.4 s, 71.0 and 71.9 s), and the cut keeps them apart: 19 takes, not 15–18. One take is
  4.3 s and holds four chorus calls (51.8–56.1 s): they follow each other within 0.1 s, their
  reverb fills the gaps, and the gaps left (4–70 ms) are as short as the brief drops below
  bed + 6 dB inside a call (4–33 ms), so the criterion (0.09 s) joins them. Splitting events
  where the level dips 15 dB and rises 15 dB again was tried: it split every pulse of every call
  (about 30 a second) and the criterion joined the same takes back. Timing alone cannot tell
  these apart; listening, or the type matrix, may.
- **The round trip shows the bed missing, not the takes wrong.** Onsets come out within the
  recording's chance. The level distribution does not: between takes the render is silent where
  the recording has its bed, so the render's L50 is silence. The takes carry their bed only while
  they sound. The remedy is a bed under them, the scene's own ambience or the residual a
  separation leaves (see "Separation" above), not a change to the cutter.
- **The draft is written by `atrium::config::draft_preset`**, not by the binary: it knows the
  preset's shape, and the engine's tests write, fill, build and render it. The binary decodes,
  cuts and writes files. `file@from-to` and the stretch it gives moved from `analyse` to
  `atrium::audio::file_stretch`, read by both binaries; `write_wav` became public for the takes.
- **The draft also copies `name` and `group`** from the kind's existing preset, besides
  `vary.pitch`, `follows` and `width`, and lists the flagged takes it leaves out in a comment.
- **The type threshold is 0.40, provisional**, halfway between the most alike pair of two shapes
  (0.12) and the least alike of one (0.68) in the synthetic check. The annotated recording sets it.
- **The summary prints the similarity matrix up to 24 takes**; beyond that it is in cut.json and
  the picture. The picture's strips are a minute each, every one with its band level beneath.

