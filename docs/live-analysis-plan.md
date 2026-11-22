# Live analysis: the analyser under the timeline, while the scene plays

Planned 2026-11-22, after the offline analyser ([analyser-plan.md](analyser-plan.md)).
Pascal's layout: the timeline stays on top of the bottom panel; under it two tabs,
**Tracks** (what is there today) and **Analysis** (new), both on the timeline's time
axis, as Adobe's bottom panels have tabs.

```text
┌ TIMELINE ─ ~~~~ wind ~~~~ rain ▁▁▂▂ ─────●──────── 3 h ─ [L▮▮▮ R▮▮] ─ 30 s │ 2 min │ 10 min ┐
│ ruler   -0:30           -0:20            -0:10            now            +0:07              │
├─[ Tracks ]─[ Analysis ]─────────────────────────────────────────────────────────────────────┤
│ LAeq 57 dB SPL │ L90 40 │ L10 62 │ width 0.69 │ onsets 44/min        (over the span shown) │
│  20k ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░│                ▌ 90 dB SPL  │
│   5k ░░▓▓▒░░░░░░░▓█▓▒░░░░░░░░▒▓▓░░░░░░░░▓▓▒░░░░░░░▓▓▒░░░│                ▌            │
│   1k ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░│                ▌            │
│  100 ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░│                ▌ 0          │
│ level 70    /\      /\_          /\      /\              │                             │
│       40 __/  \____/   \________/  \____/  \_______ L90  │                             │
│ onsets      |||      ||||          ||      |||           │                             │
│ width  ‾‾‾‾\_/‾‾‾‾‾‾\__/‾‾‾‾‾‾‾‾‾‾\_/‾‾‾‾‾‾\_/‾‾‾‾‾‾‾‾‾  │                             │
│ ear    ────\_/──────/‾\───────────\_/──────/‾\─────────  │                             │
└───────────────────────────────────────────────────────────┴─────────────────────────────┘
```

Right of "now" stays empty in Analysis: it can only show what has sounded.

## What it measures

The same as the offline analyser's picture, from the same crate, so a live number and an
`analyse` number of the same sound cannot disagree:

| Lane | Per column | As offline |
|---|---|---|
| Spectrum | Band level in dB SPL, 96 rows log-spaced 20 Hz–20 kHz, mean power of the channels | The picture's spectrogram, but on an absolute dB SPL scale (fixed range, so a colour means the same level all session) |
| Level | A-weighted level per channel: the column's Leq and its LAF (FAST) | `LAF` series, LAeq |
| Onsets | Whether an onset fell in the column | `onsets`, with its 0.75 dB rise threshold |
| Width | 1 − IACC, 500 Hz–2 kHz, over the last 100 ms (two channels, or L/R of 5.1) | Width per 100 ms frame |
| Ear | ILD R − L over the last 100 ms | ILD per 100 ms frame |

A column every 50 ms (20 a second). The header's numbers are over the span shown:
LAeq = energy mean of the columns' Leq, L10/L90 = percentiles of the columns' LAF,
width = mean, onsets = count per minute.

## How it is built

### Rust (first)

1. **An output tap on the audio thread.** `AudioScene::render` copies its finished output
   (after the probe and the pause fade; for an I16 device the float buffer before
   conversion) into a preallocated lock-free ring (`rtrb`, f32 samples), plus a small ring
   of block headers: the audio clock's sample count at the block's start, the frames and
   the channels. Samples first, header only if all of the block fit; a block that does not
   fit is dropped whole, and the reader sees the gap from the clock. Never blocks, never
   allocates: `tests/audio_thread_allocations.rs` attaches the tap and still asserts zero
   heap operations. Capacity: 2 s of the device's channels.
2. **A streaming analyser in `atrium-analysis`** (`live.rs`, or `live/` if it grows):
   `Live::new(sample_rate, channels, full_scale_db_spl)`, `push(interleaved, at_clock_sample)`,
   `take_columns() -> Vec<Column>`. Built from the crate's own pieces (weighting, bands,
   onsets' flux and threshold, IACC/ILD), causal where offline looks both ways (the onset
   median runs over the past second; say so in the README). Pure and tested without a
   device.
3. **The binding.** `crates/ffi`: `Engine::analysis(&self) -> Vec<AnalysisColumn>` drains the
   tap, feeds `Live`, and returns the new columns, each stamped in **scene seconds** (the
   same time `Engine::scene_time()` reports, so a column lands under the ruler's playhead;
   after a seek, columns carry the new times). `AnalysisColumn { scene_seconds: f64,
   spectrum: Vec<u8> /* 96 rows, 0–255 over SPECTRUM_FLOOR..SPECTRUM_TOP dB SPL */,
   leq_db: Vec<f32>, laf_db: Vec<f32> /* per channel */, onset: bool, width: Option<f32>,
   ild_db: Option<f32> }` plus `analysis_scale()` giving the spectrum's dB range and row
   frequencies, so the app labels its axis from the engine, not from a copy. The app polls
   it every display frame, as it polls telemetry; the work runs on that call.

### Compose (second, once the binding is generated)

4. **The bottom panel gets tabs.** The TIMELINE overview, the ruler, the meters and the zoom
   stay on top; under them `[ Tracks ] [ Analysis ]`. Tracks is today's weather lanes and
   voices, unchanged. The chosen tab is remembered (`SettingsStore`, `editor.` key).
5. **The Analysis tab** draws the lanes above on the ruler's span and zoom, from an
   `AnalysisHistory` that keeps 10 minutes (as `TrackHistory` does) and is cleared on seek.
   It is fed every frame whichever tab is shown, so switching shows the past. The spectrum is
   drawn as an image strip (one column of pixels per column, the offline picture's colour map),
   its dB scale beside it. Hovering shows the time and the values under the pointer.

## How we know it is right

| Check | Expected |
|---|---|
| The tap allocates nothing | `audio_callback_never_touches_the_heap` passes with the tap attached, in every mode |
| A full tap drops whole blocks | Never reads the ring: no panic, no block, and a later read sees the gap by its clock |
| Live equals offline | A render fed to `Live` in device-sized chunks (512, and a ragged 37): per-100 ms width within 0.03 and ILD within 0.3 dB of the offline report's frames; per-column Leq within 0.2 dB of the offline LAeq over the same 50 ms; onsets within ±1 of the offline count per minute (causal median) |
| Calibration | The meadow's wind alone, live: the columns' energy mean 41.5 ± 0.5 dB SPL, as `analyse` reads it |
| Time stamps | Columns are 50 ms apart in scene seconds; after a seek of the engine to 120 s, the next column is within 0.1 s of 120 s plus the time played since |
| Cost | `Engine::analysis` takes under 1 ms for 1/60 s of 5.1 at 48 kHz (release) |
| In the app | Pascal sees a bird's phrase in the spectrum lane at the same moment as its clip in Tracks |

## Not in this step

- A live picture of the future: nothing has sounded there.
- Recording the output to a file from the app (the offline renderer does files).
- The comparison of two reports (next, after this).
