# Working on generators

Written 2026-11-21, after thunder went three rounds of tuning and still sounded wrong. This is
how every sound generator is made and tuned from now on: wind, rain, river, waves, thunder and
whatever comes next.

## Why this exists

Thunder was tuned by measurement in a loop. Each round brought its colour, flatness, flicker and
length closer to a reference recording, until they matched within a few dB, and each round
Pascal listened and said it was bad: crackly, then echoing, then "like a gunshot but very hollow,
thunder has way more depth". The numbers said "matches" while the ear said no. Rain has the same
problem. There was no shared way to judge a generator, so the work had no end.

## The workflow

### 1. References first, chosen by Pascal

Before any DSP, gather 3–5 recordings of what the sound should be, in different conditions:
light and heavy rain, thunder near and far, a calm and a gusty wind. Keep them together with a
note of what each one is:

```text
assets/reference/<sound>/
  <name>.wav|mp3
  notes.md        # what each recording is: distance, conditions, what is good about it
```

One recording is not enough. Thunder was tuned against a single clap of a single kind.

### 2. A listening brief, in words

Pascal writes, in his own words, what makes the sound real and what would make it wrong:
"depth, it rolls in, never a gunshot"; "rain is a space full of drops, not hiss". This goes in
`notes.md` next to the references. It is the test a version is judged by. The spectrum is not.

### 3. Decide early: synthesis, recordings, or both

| Route | Suits | Example |
|---|---|---|
| Synthesis | Sounds that never repeat and change smoothly with the weather | Wind |
| Recordings | Sounds whose character is hard to build | Thunder and rain, probably |
| Both | Recorded takes, with a generator filling in between | Rain bed recorded, drops generated |

Recordings already fit the model: a kind plays its takes with rests, chosen by the plan, and
follows the weather like any other sound (`sources/blackbird.yaml`).

**A limit.** If a generator still does not convince after three listening rounds, switch it to
recordings instead of tuning further.

### 4. Listen in place

A generator is judged in a scene, through the real render path: its distance, the space, the
5.1 speakers or headphones, head tracking. Not as a mono file on its own. "Hollow" is partly what
one point source in a dry file sounds like.

### 5. The generator lab

A place in the editor to work on one generator:

- **Live controls.** Every setting of the generator on screen, changed while it plays (feature 27,
  generator panels; the engine already takes the changes through `SetSynthParam`).
- **A/B against a reference.** Switch between a reference recording and the generator at matched
  loudness. Blind when it matters (A/B/X): the listener does not know which is which.
- **Versions side by side.** The last version and this one, each reproducible from its seed, so
  every change is heard against the one before it.
- **Presets with a reason.** A setting that works is saved as a preset, with a line about why.

### 6. Measurements help, they do not judge

The analysis stays (colour in third-octave bands, onset, crest, level flicker, length, spatial
width) as a report of what changed between versions and where a version differs from the
references. It suggests where to look. Only listening decides whether a version is better, and no
generator is called "matching" from numbers alone.

### 7. One change per round

Each listening round changes one thing and gets one verdict. Slower per step, and no more
three-round detours.

## Order of work

1. Write the listening briefs for thunder and rain, and gather their references.
2. Build the lab: generator panels (feature 27), then A/B against references and versions.
3. Run thunder and rain through it, deciding route first.
4. Then the others, starting with whichever Pascal finds weakest.

## The tools, in order

The lab is built from tools, each needing the one before:

1. **The offline renderer** (`render`, built 2026-11-22): a scene to a WAV file
   through the real render path, faster than real time and the same file for the
   same seed, with a sidecar saying how it was made. A version of a generator is
   heard and measured in place (step 4) from these files, and two versions are
   compared from two files of the same seed. `--probe` keeps one part of the render
   alone (the ambient field a wind makes, say), `--weather-speed` runs a storm past in a minute. See
   [run.md](../run.md#render-a-scene-to-a-file).
2. An analyser that reports the same measures for a reference recording and for a
   render, reading the sidecar to know what it is reading.
3. A comparison of two such reports, an event cutter, and the lab in the app.

## Where things are

- Thunder's research, history and current model: `docs/thunder-research.md`.
- Listening samples of thunder:
  `THUNDER_OUT=target/thunder-samples cargo test --release --lib write_listening_samples`.
- The generators: `src/synth/`. Their presets: `sources/`.
- A scene to a file: `cargo run --release --bin render -- <scene.yaml>`; the code is
  `src/offline/`.
