# How to run things

Everything runs from the repository root. Stop any running engine with **Ctrl+C**.

## Listen on speakers

```sh
cargo run --release                                   # scenes/default.yaml
cargo run --release -- scenes/nature.yaml             # any scene file
cargo run --release -- scenes/wisp.yaml               # one wisp flying through the room
cargo run --release -- --ui scenes/nature.yaml        # with the terminal dashboard
```

## Listen on headphones

`--headphones` plays any scene binaurally on two channels, whatever speakers the scene
names. There is one file per scene, not a second one for headphones. In the app the same
choice is Settings → Output, and it is remembered.

```sh
cargo run --release -- --headphones scenes/nature.yaml  # the meadow, binaural
cargo run --release -- --headphones scenes/wisp.yaml    # one wisp flying around your head
cargo run --release -- scenes/frog.yaml                 # one frog, front-right; binaural by itself
```

## Control it from a browser or a phone

The engine listens for control on port 3333 whenever it runs. The page in
`web/` talks to it: switch render mode and speaker set, master gain, mute
sounds, see levels and where everything is.

```sh
cargo run --release -- scenes/nature.yaml   # terminal 1: the engine
npm --prefix web install                    # once
npm --prefix web run dev                    # terminal 2: the page, http://localhost:6006
```

On a phone on the same Wi-Fi, open `http://<this-machine>:6006`; the dev
server prints the address under "Network". The page finds the engine on the
same host. To point it elsewhere: `http://localhost:6006/?engine=nuc.local:3333`.

## Head tracking

Your head turn steers the listener, so the world stays put when you turn.

```sh
# Sony WH/WF-1000XM5/XM6 — one command, the engine starts the Sony bridge too
cargo run --release -- --head-tracking=sony scenes/frog.yaml
cargo run --release -- --head-tracking=sony --headphones scenes/wisp.yaml

# Any other tracker that sends OpenTrack UDP to 127.0.0.1:4242 — start it yourself
cargo run --release -- --head-tracking scenes/frog.yaml
```

Look straight ahead when it starts: that is "centre". Test: in the frog scene,
turn your head right and the frog moves to straight ahead.
The wisp scene is the harder test: it flies around and overhead, so turn your
head to follow it and it should stay where it is in the world.

The app in `examples/compose-scene` has the same two choices under
Settings → Head tracking, and switches them while a scene plays.

### Sony setup (once, and after firmware or setting changes)

1. Sony Sound Connect app: **Bluetooth Connection Quality → Prioritize Stable
   Connection**. "Prioritize Sound Quality" (LDAC) switches the sensor off.
2. Mac: **forget the headphones and pair again** after changing that setting or
   updating firmware, otherwise macOS does not see the sensor.
3. First run builds the bridge into `target/sony-head-tracker/` (needs Xcode).
   Then enable **SonyBridge** under System Settings → Privacy & Security →
   Input Monitoring.

```sh
sh scripts/sony-head-tracker.sh probe     # is the sensor visible? expect "verified Android tracker=yes"
sh scripts/sony-head-tracker.sh rebuild   # fresh build of the bridge
sh scripts/sony-head-tracker.sh           # bridge on its own (rarely needed)
```

The bridge log is `$TMPDIR/atrium-sony-bridge.log`.

## Render a scene to a file

`render` plays a scene into a WAV file instead of the speakers, through the real
engine, about twenty times faster than real time. The same arguments give the same
file, bit for bit. Files go to `target/renders/` unless `--output` says otherwise,
each with a `.json` sidecar beside it: the scene and a hash of it, the commit and
whether the tree was dirty, the seed and every setting, the channels in order, and
each channel's peak and RMS.

```sh
cargo run --release --bin render -- scenes/meadow-birds.yaml --headphones --seconds 20
cargo run --release --bin render -- scenes/passing-storm.yaml --seconds 20     # the scene's 5.1, in VBAP
cargo run --release --bin render -- scenes/passing-storm.yaml --mode dbap --weather-speed 60
cargo run --release --bin render -- scenes/nature.yaml --start 120 --seconds 10 # from 2 minutes in
cargo run --release --bin render -- scenes/frog.yaml --headphones --turn 45     # the head turning left
cargo run --release --bin render -- scenes/meadow-birds.yaml --headphones \
    --probe direct,no-air-absorption --format s16 --output target/renders/dry-birds.wav
```

| Option | Default | |
|---|---|---|
| `--seconds <n>` | 30 | How long to render |
| `--start <s>` | 0 | Start this far into the scene, as the editor's timeline seeks |
| `--sample-rate <hz>` | 48000 | |
| `--block <frames>` | 512 | Frames per engine call, as the audio callback's buffer (1 to 8192) |
| `--headphones` | | Binaural on two channels, as the engine's `--headphones` |
| `--mode <name>` | the scene's | `world_locked`, `vbap`, `hrtf`, `dbap` or `ambisonics` on the scene's speakers |
| `--seed <n>` | the scene's `seed:`, else 1 | The session seed |
| `--probe <list>` | everything | Parts alone and effects off; see below |
| `--turn <deg/s>` | 0 | The listener's head turning steadily, positive to the left |
| `--weather-speed <n>` | 1 | The weather this many times as fast, from the scene's beginning |
| `--format <f32\|s16>` | `f32` | 32-bit float for analysis; 16-bit for listening, and it says how many samples clipped |
| `--normalise[=<dBFS>]` | off | One gain on every channel so the loudest sample peaks at −1 dBFS, or the level given (`--normalise=-6`); the sidecar's `gain_decibels` says how much |
| `--output <path>` | `target/renders/<scene>-<output>-seed<seed>….wav` | |

The channels are the layout's, in the order the app names them: L, R, C, LFE, Ls, Rs
on 5.1 and quad (quad's C and LFE are silent), L, R on headphones and stereo.

`--probe` takes a comma-separated list, as the microscope (see "Listening to one
part" in the README):

| Word | |
|---|---|
| `direct`, `reflections`, `reverb`, `ambient` | Only the parts named reach the file |
| `none` | No part at all: digital silence |
| `no-air-absorption`, `no-moving-air`, `no-ground-effect`, `no-wall-colour`, `no-near-field`, `no-headphone-equalisation` | That effect off |
| `channel=<L\|R\|C\|LFE\|Ls\|Rs\|index>` | One channel alone |

The probe holds from the first sample, so the four parts rendered alone add up to the
whole render.

Levels are the world's, not a mix's: the meadow's birds, a dozen metres off, peak
around −55 dBFS. The sidecar's `calibration.full_scale_db_spl` is the SPL of an RMS of
1.0 as the engine renders it (the scene's `spl_reference` − 20·log10(`target_rms`), 114 dB
SPL for the meadow), so `analyse` can read the file in dB SPL. That is one reason float is the default; a 16-bit file of a quiet
scene keeps few bits of it. To listen, add `--normalise`: the file is raised to peak
at −1 dBFS, one gain for all channels so nothing moves, and `-normalised` is added to
the default name. The levels in its sidecar are the file's; the engine's own are those
minus `gain_decibels`. Leave it off for files you will measure against each other.

## Analyse a file

`analyse` measures recordings and renders the same way: level, colour, texture, events
and, for two or more channels, space. For each file it writes a JSON report
(`target/analysis/<stem>.json`, `report_version` 1, for the comparison tool), prints a
one-screen summary, and with `--picture` draws a 1600 px PNG: spectrogram, LAF with L10,
L50 and L90, onsets, and for two channels width and ILD, on one time axis. A render's
sidecar beside it gives the channel names and the calibration, so levels are in dB SPL
(the gain `--normalise` applied is undone); a recording without one is in dBFS, where
an RMS of 1.0 is 0 dBFS.

```sh
cargo run --release --bin analyse -- target/renders/meadow-birds-headphones-seed1.wav --picture
cargo run --release --bin analyse -- assets/birds/merel/*.mp3 --summary target/analysis/merel.txt
cargo run --release --bin analyse -- recording.wav --from 10 --to 40 --picture=target/rain.png
```

| Option | Default | |
|---|---|---|
| `<file>@<from>-<to>` | the whole file | Only this stretch of this file, in seconds: `take.mp3@30-60` |
| `--from <s>`, `--to <s>` | the whole file | Analyse only this stretch, for every file without its own `@` |
| `--output <path>` | `target/analysis/<stem>.json` | The report; one file only |
| `--picture[=<path>]` | off; `target/analysis/<stem>.png` | Draw the picture too |
| `--summary <path>` | printed only | Also write the summaries of every file given to this file |

Several files in one call give their summaries one after another in the same layout, so
they line up row for row. The measures and their definitions are in
`crates/analysis/README.md`. About 1.6 s for a minute of stereo at 48 kHz, picture
included.

## Compare files

`analyse --compare` puts a reference and one or more other files side by side and says where
the others differ: level shape (and absolute level when both are calibrated), colour, texture,
events, space and the distributions over time. The first file is the reference; each file is
audio (analysed as above, its sidecar read) or a report JSON `analyse` wrote. It prints a
one-screen comparison (the reference's value, each file's value and its difference), writes a
comparison JSON (`comparison_version` 1) and with `--picture` a 1600 px PNG of overlays, the
reference black and thick, each other file in its colour, each named in the legend. It reports
differences; only listening judges which version is better.

```sh
cargo run --release --bin analyse -- --compare "research papers/soft wind.mp3@0-28" \
    target/renders/wind-seed1.wav --picture --summary "target/analysis/soft wind-against-wind-seed1.txt"
cargo run --release --bin analyse -- --compare target/renders/wind-seed1.wav target/renders/wind-seed2.wav --picture
cargo run --release --bin analyse -- --compare target/renders/wind-seed1.wav target/analysis/wind-seed1-old.json
```

| Option | Default | |
|---|---|---|
| `<file>@<from>-<to>` | the whole file | A stretch of an audio file, so a part of a long field recording is compared with a render |
| `--output <path>` | `target/analysis/<reference>-against-<others>.json` | The comparison JSON |
| `--picture[=<path>]` | off; beside the JSON, `.png` | Draw the picture too |
| `--summary <path>` | printed only | Also write the comparison to this file |

Reading the texture rows: each statistic class of McDermott and Simoncelli gets the paper's
SNR, the reference's statistics as the signal; the paper took 20 dB as converged. Two takes of
one sound do not reach 20 dB in every class by chance alone (envelope correlations near zero
scatter), so each class also shows, in brackets, the SNR two takes of the reference's sound
would reach, from its two halves analysed apart. `!` marks a class below 20 dB and below that
floor, a difference; `~` one below 20 dB but within it, chance. Why, and what else looks wrong
and is right, is in `crates/analysis/README.md`, "The comparison".

Two renders of the same scene, seed, start, weather speed and stretch line up in time (their
layout, mode or probe may differ); for those the comparison also gives where the LAF and the
width differ most and when. Anything else is compared by distribution only. About 3 s for a
28 s recording against a minute of stereo.

## Cut a recording

`cut` takes a long field recording and finds its takes: each call or song with its attack and
its decay, the rests between them, their types by shape and the runs of one type, all measured
from the recording. It writes only `target/cut/<kind>/`, and refuses a folder that is already
there:

- `take-01.wav` …, each take as 32-bit float at the recording's rate and channels, with 5 ms
  fades in and out;
- `cut.json` (`cut_version` 1): every decision with the values behind it, and where each take
  lies in the recording;
- `summary.txt`, the one-screen summary it also prints;
- `preset.yaml`, a draft source with the takes, the measured gap and bout;
- with `--picture`, `cut.png`: the spectrogram a strip a minute with the takes boxed, the band
  level under it with its bed and thresholds, the gaps with the fitted bout criterion, and the
  takes' similarity matrix.

```sh
cargo run --release --bin cut -- assets/frog.mp3 --name frog --picture
cargo run --release --bin cut -- recording.wav@120-300 --name robin --band 2000-8000
```

| Option | Default | |
|---|---|---|
| `<recording>@<from>-<to>` | the whole file | Only this stretch, in seconds, as `analyse` reads it |
| `--name <kind>` | the file's stem | The folder's name. If `sources/<kind>.yaml` exists, the draft takes its `name`, `group`, `width`, `vary.pitch` and `follows` |
| `--band <low>-<high>` | chosen | Find the events between these frequencies (Hz) instead of in the third octaves where they stand furthest above the bed |
| `--picture` | off | Draw `cut.png` too |

The draft does not load until its `reference_spl` is filled in, on purpose: a recording's
level in dBFS says nothing of the sound's level in the world. Listen to the takes, fill it in
from the literature, then copy the takes to `assets/` and the preset to `sources/` with the take
paths changed to match. Flagged takes (under 20 dB above their bed, whose bed will gate in and
out with them) are written but left out of the draft. The summary says why nothing was joined
when the gaps do not show two kinds. The rules and what looks wrong and is right are in
`docs/cutter-plan.md` and `crates/analysis/README.md`, "The cutter". About half a second for
the frog's 78 s.

## Analysis tools

```sh
cargo run --release --bin analyze_sample -- "<recording>" [segment_start_s segment_end_s]...
cargo run --release --bin analyze_rain -- "research papers/soft_rain.mp3"
cargo run --release --bin analyze_synth
cargo run --release --bin analyze_bubbles
cargo run --release --bin spectrogram -- "<recording>" <out_dir> [wind|rain]
```

## Profiling

```sh
cargo run --release --features profiler -- scenes/default.yaml --profile fmt        # span timing in the terminal
cargo run --release --features profiler -- scenes/default.yaml --profile perfetto   # → trace.pftrace
cargo run --release --features profiler -- scenes/default.yaml --profile flame      # → tracing.folded
cargo run --release --features memprof -- scenes/default.yaml                       # allocation tracking
```

## Checks

```sh
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets
python3 scripts/dep-cycles.py             # module cycles in src/
```

## Ports

| Port | What |
|---|---|
| UDP 4242 (127.0.0.1) | head tracking in (OpenTrack) |
| UDP 4243 (127.0.0.1) | Sony bridge JSON debug stream |
| TCP 3333 | WebSocket telemetry + control (the page in `web/` is its client) |
| TCP 6006 | The control page's dev server (`npm --prefix web run dev`) |
