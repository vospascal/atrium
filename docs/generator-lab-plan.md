# The generator lab

Planned 2026-11-22; stage 1 built the same day. Where the build departs from the plan, the
plan below is corrected and "As built" at the end says why. This is the fifth and last tool ([generator-workflow.md](generator-workflow.md#5-the-generator-lab)),
after the renderer, the analyser, the comparison and the cutter. It is a place in the app
for working on one sound: hearing it against a reference at the same loudness, without
knowing which is which when that matters, against its own last version, and with its
settings under your hands while it plays. The other four tools report differences. This one
is where listening judges them ("Measurements help, they do not judge").

## The listening workspace

**Lab** is a third tab beside Tracks and Analysis. The timeline stays above it. Each round
starts with a question from the sound's listening brief, such as *Does the gust fade naturally,
or does the air drop away too soon?* The loop, the settings you try, and the verdict all refer
back to that question.

Before a reference is loaded:

```text
┌ Scene timeline ────────────────────────────────────────────────────────────────────────┐
├ Tracks │ Analysis │ [Lab] ─────────────────────────────────────────────────────────────┤
│ Soft wind    [Open recording…]                                                         │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

With a marked gust:

```text
┌ Scene timeline ────────────────────────────────────────────────────────────────────────┐
├ Tracks │ Analysis │ [Lab] ─────────────────────────────────────────────────────────────┤
│ Soft wind   Does the gust fade naturally?                                              │
│ dune.wav · Gust 1                                     [Open recording…] [Mark passage] │
│ Reference waveform  ▁▂▂▁▂│▂▃▅▆▇▆▆▅▃▂▁│▁▁▂           Lab playhead 0:03 / 0:07           │
│                          ▲ Start     ▲ End                                             │
│ Hearing Reference   [● Reference] [Previous] [Working]   Trim held · rematch needed    │
│ [Pause] [✓ Loop] [Replay gust]   [Match levels] [✓ Hold level match]                   │
├ Mix · Editing Working (scrolls) ───────────────────────────────────────────────────────┤
│ Low        ━━━●━━━━   0 dB  [Solo] [Mute]                                              │
│ Body       ━━●━━━━━  −2 dB  [Solo] [Mute]                                              │
│ Mid        ━━━●━━━━   0 dB  [Solo] [Mute]                                              │
│ Presence   ━━━●━━━━   0 dB  [Solo] [Mute]                                              │
│ Air        ━━━●━━━━   0 dB  [Solo] [Mute]                                              │
│ ▸ Behaviour                                                                            │
│ [Pin note] [Save version…]                                                             │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

Keep **Reference**, **Previous**, and **Working** one click apart while the mixer is open.
The reference gives you the target, the previous version is the baseline for this round, and
the working version takes live edits. Pin observations to the reference or generator moment
you heard, and save the setting change and verdict together. Use a blind round when there is
a decision to make. Use measurements to investigate what you heard.

- **Reference** starts with **Open recording…**. Drag the start and end markers on the
  waveform, then use **Mark passage** to name and keep the selection. A note says what to
  listen for, such as the airy texture, the shape of a gust, or the irregular spacing between
  gusts. Short loops help with texture and shape. Longer passages help with timing and
  variation.
- **Playback** uses one Lab bar with **Play/Pause**, **Loop**, and **Replay gust**. The
  audition has its own labeled playhead below the scene ruler. The candidate buttons and
  playback bar stay visible while the mixer scrolls.
- **Mix** always shows **Editing Working**. The candidate bar shows what you hear in text,
  and switching candidates leaves the Working controls in place. Each layer has a fader,
  solo, and mute. Undo steps back through the session's mixer changes while listening. The audition's trim is shown
  beside its candidate and stays fixed while the match is held.
- **Behaviour** is collapsed by default. Open it to see the current generator's settings
  and actions for how its sound changes over time. The Lab uses the generator's labels,
  ranges, and actions to draw the same kinds of controls for any generator. In the wind
  session this includes gust brightness and **Run freely**.
- **Keys** work when the Lab has focus (a press anywhere in it gives it focus). Space plays
  and pauses the audition, not the scene. R, P and W switch to Reference, Previous and
  Working. Text fields keep their typing keys.
- **Version** is the one word for a saved state of a generator (not "snapshot").

## The stages

Try each stage before starting the next. The first session determines which controls and
captured details are worth carrying into later stages.

### 1. Tune soft wind in the Lab

Start with `sources/soft-wind.yaml` and a real wind recording chosen by Pascal. Put the
recording and listening brief in `assets/reference/soft-wind/` and choose a second recording
for the follow-up listen.

1. **Mark a gust in your recording.** Use **Open recording…**, drag the start and end
   markers around the gust with enough lead-in and tail to hear it in context, and use
   **Mark passage** to name it.
   Then choose a comparable gust in a seeded generator run. The Lab renders the run offline
   (fast, and the same for the same seed) and draws its waveform under the recording's, with
   markers dragged the same way. The gust driver knows where its own gusts begin, so those
   are offered as places for the markers to snap to (every move of the wind's repertoire:
   gust, swell or lull, within 12 px). The recording and generator
   regions can start at different places. They share a loop length and playhead offset so
   switching keeps your place. Each wrap recalls the same generator
   passage, including the state leading into it. Previous and Working start from the same
   seed and cue. Loop edges, and a replay, fade over 5 ms to avoid clicks.

   How each wrap recalls that state: the generator runs once from its seed up to the cue, off
   the audio thread, and a copy of its whole state at that moment is kept. Every wrap, and every
   **Replay gust**, swaps the copy back in, which costs almost nothing. A level or colour
   change applies on top of the copy and is heard at once; a timing change needs the run from
   the seed to the cue again, which is when the Lab says the marked gust no longer lines up.

   The same seed gives the same gusts only while the gusts themselves are not changed. The
   soft wind draws its gust driver and each noise layer from streams of their own
   (`src/synth/soft_wind.rs`, the seeds at `WindDriver::new` and each `PinkNoiseFull`), so a
   level or a colour setting such as `gust_brightness` leaves every gust where it was. A
   timing setting (gust duration, the rest between moves, turbulence time) makes different
   gusts. Each setting's description says which kind it is; after a timing change the Lab
   says that the marked gust no longer lines up, and the comparison moves to **Run freely**
   over a longer stretch, judged on spacing and variation rather than on one gust.
2. **Switch at matched levels.** Listen in place with the rest of the scene muted. Put the
   reference through the same render path as the wind so the match includes spatial rendering.
   Keep the candidates running in step, all three in the one voice the lab plays from, and
   use a 10 ms fade when switching: between the reference and a generator, two independent
   noises, one that keeps the power (sine and cosine); between the two generators, which share
   their seed and so their noise, one that keeps the amplitude (straight lines). Measure each
   candidate alone at the output tap using the analyser's gated A-weighted level and trim the
   generator candidates to the reference. Accumulate at least 10 seconds of sounding audio
   across loop passes, with a 60-second cap. Report the measured difference and how much the
   level varies between the first and second half of the passage. The target is 0.1 dB when the measurement
   supports it. The match applies to this excerpt. Turn on **Hold level match** to keep
   the measured trims fixed while moving faders or changing a behaviour setting. After an
   edit that could affect a candidate's level, show **Trim held · rematch needed** beside
   that candidate. With the match not held, that edit drops the candidate's trim back to
   0 dB, unmatched. Use **Match levels** to recompute the trims before judging the edit.
   Trims belong to the audition.

   - **Alone** means every other sound in the scene is muted, fading over 10 ms like any
     mute; the candidate still plays in place, through its own distance, space and head
     model. (Solo in the mixer is something else: one layer of a generator.)
   - **The trim is the candidate's own gain** in the lab's voice, before the voice's level
     (the soft wind's `reference_spl`). The render follows a source's level exactly, except
     in two stages that depend on level: the output's master stage and the reverb's feedback
     clamp. A test checks that at listening levels a candidate 6 dB louder comes out 6.00 dB
     louder.
   - **What makes a match stale**: a mixer level, solo or mute; any generator setting; moving
     or changing the loop; switching reference. The indicator is per candidate.
3. **Adjust the working wind while it plays.** The soft wind already has five layers with
   their own gains: low, body, mid, presence and air. Start with their live levels, plus solo
   and mute for listening to each. A fader at 0 dB is the layer at the level the generator
   gives it today, so the untouched mix is the baseline, sample for sample. Undo keeps a
   stack of mixer changes (levels, solo, mute) for the session. Change one setting per round
   and keep the seed fixed while exploring.
4. **Replay a seeded gust on demand.** Press **Replay gust** in the playback bar. Each
   press restarts the audition from the reference markers and the saved generator cue,
   even during playback. Previous and Working use the same cue and seed with their
   respective settings and lead-in state. The selected candidate remains audible with its
   current trim. Play the passage once with **Loop** off or repeat it with **Loop** on.
   Open Behaviour to try the existing `gust_brightness` setting and hear how strongly the
   wind brightens during the gust.
5. **Hear whether the change holds.** Compare Working with Previous and Reference at matched
   levels. Then try fresh seeds and the second recording. Choose **Run freely** in Behaviour
   to compare the spacing and variation of wind gusts with a longer stretch of the recording.
   Pin a note to the loop and record the verdict in your own words, such as “the fade sounds
   better, but the onset is still too
   sharp.” Save the session (`lab/<sound>/session.json`, rewritten on each save): the
   reference file and passage, the baseline (Previous) and working settings with what
   changed, the generator seed and cue, the trims, the question, the notes and the verdict,
   so the session can be reopened. Numbered versions and the round log follow in stages 2
   and 3.

The milestone is complete after Pascal has used the Lab to try a real soft-wind change and
reopened its session and listening notes, with the loop, seed and settings restored. Any verdict is useful, including keeping the
baseline. Promotion to the shipped preset is a separate decision.

### 2. Blind decisions and the log

- **ABX: can I hear a difference at all?** Each trial plays A, B and X, where X is A or B at
  random; you say which. 16 trials, fixed before starting, because stopping when the score
  looks good biases the result. The result is the number correct and the one-sided binomial
  p, so 12 of 16 is p 0.038, the usual 5 % line. For a change between versions, "no audible
  difference detected" is a possible result.
- **Which version answers the listening question better?** When a change is ready for a
  decision, carry the question into the blind round. Each trial plays the reference, then
  Previous and Working in random order under neutral names. The reference is the anchor, not
  a candidate. Use 16 trials, and report the preference count and a two-sided binomial
  p-value. Two-sided, 12 of 16 is p 0.077, not a result; **13 of 16** (p 0.021) is the 5 %
  line.
- **What a fail means.** 16 trials is a weak test: a listener who hears the difference in 70 %
  of trials reaches 12 of 16 only 45 % of the time. A fail is reported as "no difference heard
  in 16 trials", never as "no difference". Many tests in one session raise the chance that one
  passes by luck; the log counts the session's tests so this shows.
- **How a trial runs.** A, B, X and the reference can be replayed as often as you like
  before answering. No answer is marked right or wrong until the round ends. Trials are
  separated by 0.5 s of silence, so the change of trial gives nothing away.
- **No blind round on stale levels.** If any candidate shows **rematch needed**, starting a
  blind round first runs **Match levels**; a blind round always runs on fresh trims.
- **The log** is `lab/<sound>/log.jsonl`, one line per completed round, appended and never
  rewritten. Your words are the verdict; tags (onset, tail, spacing, texture, level,
  brightness) are optional, for finding rounds later. One line, shown wrapped:

  ```json
  {"log_version": 1, "round": 7, "at": "2026-11-23T10:42:00+01:00", "sound": "soft-wind",
   "question": "Does the gust fade naturally?",
   "reference": {"file": "assets/reference/soft-wind/dune.wav", "passage": "gust 1", "from": 12.4, "to": 19.0},
   "candidates": {"previous": {"version": 3, "seed": 1, "cue": 41.2},
                  "working": {"version": null, "parent": 3, "seed": 1, "cue": 41.2,
                              "changed": {"gust_brightness": [0.18, 0.30]}}},
   "trims_db": {"previous": -1.8, "working": -1.6}, "mode": "in place, alone",
   "test": {"kind": "preference", "trials": 16, "order_seed": 90211,
            "assignments": ["working", "previous"], "answers": ["working", "working"],
            "count": 13, "p": 0.021, "tests_this_session": 2},
   "verdict": "the fade sounds better, but the onset is still too sharp", "tags": ["tail"]}
  ```
  (`assignments` and `answers` hold one entry per trial; two are shown.) A missing field
  is read as absent, so later fields never make old lines unreadable.

### 3. Version history and playback options

- **A version** is a generator preset (the `sources/*.yaml` form) plus its seed, with a
  number, the version it came from, a line saying why, and what changed from that version,
  written by the Lab. Versions are saved as `lab/<sound>/v<N>.yaml`, numbered upwards and
  never overwritten:

  ```yaml
  version: 4
  parent: 3
  seed: 1
  saved: 2026-11-23T10:45:00+01:00
  why: the gust fades more slowly, so it no longer drops away
  changed:
    gust_brightness: [0.18, 0.30]
  preset:          # the whole source, as in sources/soft-wind.yaml
    name: Soft wind
    # …
  ```

  A version and its seed give the sound; hearing the exact passage again also needs the
  cue, which is in the log.
- **Save as v4…** asks only for the line saying why; the number, parent, seed and changes are
  filled in. Any saved version can become Previous or the starting point for Working.
- **Promote** copies a version over the source in `sources/`. It is a separate, deliberate
  step, never automatic: it shows the settings before and after, warns when the version never
  won a blind round, checks that the result loads, asks to confirm, and writes the promotion
  in the log. Git keeps the old source.
- References sit beside them in `assets/reference/<sound>/` with `notes.md` (the workflow's
  step 1). The lab lists them as candidates.
- **As recorded** plays binaural or stereo references straight to the ears so they keep the
  space already in the recording. Mix a stereo bus after the scene render and before the
  output tap, so level matching and live analysis hear it too; a mono reference goes to both
  ears alike. In place stays the default and the fair comparison, since both candidates then
  share one path; as recorded is a way of hearing the reference's own space.
- **With the scene** lets you hear how a change sits among the other sounds. Match levels
  with the candidates alone.

### 4. Extend the mixer to other generators (feature 27)

Decided with Pascal 2026-11-22: every generator's algorithm is different (drops with bubble
resonance, gusting wind, thunder rolling through space). Use the soft-wind session to decide
which controls belong in the shared mixer. The Lab draws each generator's own settings from
its description, like a plugin host drawing a panel from a parameter list.

- **The contract.** A generator gives its sound as named layers (the soft wind: low, body, mid,
  presence and air; rain: hiss and drops), names its drivers (what moves several layers at once: gusts,
  intensity), and describes its own settings (name, layer, unit, range, value, one line on
  what it does, and whether it changes timing, so the Lab knows when a marked passage stops
  lining up). It also describes any behaviour settings and audition actions it supports,
  so the collapsible Behaviour panel can show them using the same controls across generators.
- **The mix is shared code, the same for every generator**:
  - level per layer, where 0 dB is the layer at the level the generator gives it, so the
    untouched mix is the generator as it sounded before;
  - solo and mute per layer, so one layer is heard, and measured by the live analysis,
    on its own;
  - how much each layer follows each driver.
  A generator never mixes its own layers; it hands them over and the mixer sums them.
- **The ingredients stay each generator's own.** Its settings fold out under the layer they
  belong to, drawn from the description. The FFI gets `generator(slot)` (the description)
  and `set_generator_setting(slot, name, value)` on the existing `SetSynthParam` command,
  plus `set_layer_mix(slot, layer, level, solo, mute)`. The description carries the current
  values, so reading back is `generator(slot)` again; an unknown layer or setting name is an
  error, not ignored.
- **Which generators.** Today only the four winds and the river accept changes while playing.
  The others (rain, waves, the wisp, thunder; feature 26) are mixed as one layer until they
  hand over their layers, and show their settings read-only until they take changes.
- **Nothing is shared before it is needed.** A building block (one gust driver for wind and
  rain, say) becomes common code when a second generator needs it, as `AGENTS.md` asks.
- **Later: space per layer.** Rain's hiss overhead and its drops close around you means each
  layer is its own source in the scene. That is an engine change of its own, planned when a
  generator first needs it.

### 5. Measured in the lab

- **Measure** captures the same stretch of A and B at the output tap (30 s each, alone and
  matched) and runs the comparison (`atrium_analysis::compare`) on them. Its one-screen
  summary shows in the lab. Its JSON (`lab/<sound>/measure-<time>.json`) and the two
  captured WAVs are saved next to the log, with everything that made them: the versions and
  seeds, the loop and cues, the trims, and the mode (in place or as recorded, alone or with
  the scene). Any number in it can be made again from that.
- The numbers come from the same code as `analyse --compare`, and are heard in the same
  place as you hear them.

## How we know it is right

| Check | Expected | Measured (test) |
|---|---|---|
| First soft-wind session | Pascal marks a gust in his recording, switches among all three candidates with the live mixer open, holds the match while changing one setting, replays a seeded gust on demand, and reopens the versions and verdict with the loop, seed and settings restored. | Not yet: built for Pascal's first session |
| Loop and switch timing | Every wrap recalls the same seeded generator passage with its lead-in state. Switching keeps the shared playhead offset. Neither action produces an audible click. | Every pass is the first again, bit for bit, and so is a replay mid-pass (`a_wrap_recalls_the_run_at_the_cue_sample_for_sample`); the three candidates advance every sample whichever is heard, so a switch keeps the offset; clicks, see the two switching rows |
| Opening and marking a recording | The empty Lab offers Open recording. After loading, drag the waveform markers and use Mark passage to name the loop. | Built (Open recording… is the macOS file dialog; Mark passage takes the name field); not yet tried by Pascal |
| Marking the generated gust | The seeded run's waveform is drawn under the recording's; its markers snap to the gust starts the driver reports | Seed 1's driver reports moves at 46, 110, 170, 220, 265, 340, 381, 491 and 562 s of a ten-minute run; the run's start marker snaps to them within 12 px (`a_timing_setting_is_flagged_and_level_or_colour_keep_the_gusts`) |
| Recalling the state at the cue | A wrap that swaps the kept state back in sounds sample-identical to running from the seed to the cue; swapping costs no allocation on the audio thread | The first pass equals the run from the seed through the cue, sample for sample, past the 10 ms fade-in and the 5 ms edges (`a_wrap_recalls_the_run_at_the_cue_sample_for_sample`); 0 heap operations in 600 callbacks of a 0.5 s loop with wraps, replays, switches, trims and live settings (`the_labs_voice_never_touches_the_heap`) |
| Replay gust | The Lab playback bar restarts the marked passage with the saved cues and seed, the selected candidate and its current trim. It plays once or repeats according to Loop. The scene ruler keeps its own playhead. | A replay mid-pass fades out over 5 ms and then plays the pass from its start bit for bit (same test); the voice plays on its own playhead, the scene's ruler untouched |
| Compact Lab | Hearing and editing labels remain clear. The candidate buttons and playback bar stay visible while scrolling through the five faders and Behaviour. | Built: the candidate and playback bars sit above the scrolling mixer; not yet looked at by Pascal |
| Switching between two independent noises at the same level | No sample step at the switch larger than the noises' own largest step; the output level holds within 0.1 dB across it | 1 120 switches between the wind at seed 2 (as the reference) and seed 1: largest step 0.410 against the noises' own 0.447; level across the switches +0.007 dB (`switching_between_two_independent_noises_neither_clicks_nor_dips`). Between the two generators the switch is heard as one signal, within 10⁻⁵ of its peak (`switching_between_the_generators_keeps_their_shared_noise_whole`) |
| The trim is exact | A candidate 6 dB louder at its source reads 6.00 ± 0.01 dB louder at the output tap, at listening levels | 6.0000 dB (38.563 against 44.563 dB SPL, A-weighted while sounding, the same 11 s of a 12 s loop) (`a_trim_of_six_decibels_reads_six_decibels_at_the_output_tap`) |
| Matching two noises 6 dB apart, in place and alone | Trim 6.0 ± 0.1 dB; then the output tap reads both within 0.1 dB | Trims −6.06 (Previous) and −6.09 dB (Working); matched again, +0.009 and −0.004 dB from the reference; halves 0.43 and 0.49 dB apart (`matching_two_noises_six_decibels_apart_trims_by_six`) |
| A timing setting | Changing a soft-wind gust timing setting is flagged as no longer lining up; changing a layer level or `gust_brightness` leaves the gusts sample-aligned | Gust brightness 0.30 and the body layer −6 dB leave every move at its sample, and the lab says they line up; the rest between moves halved moves the first from 46 to 23 s, and every timing setting is flagged (`a_timing_setting_is_flagged_and_level_or_colour_keep_the_gusts`) |
| A blind round on stale levels | Starting one after a fader move runs Match levels first | Stage 2 |
| Preference threshold | A simulated random listener reaches 13 or more of 16 in 2.1 % of 10 000 runs (two-sided, either way) | Stage 2 |
| Saving and promoting | A saved version records its parent and exactly the settings that changed; promoting shows that change, refuses a preset that does not load, and logs the promotion | Stage 3 |
| Reopening a round | Loop, seed, settings and trims come back as logged, and an offline render of them matches the original sample for sample | Stage 1 has the session, not the round: question, passage, seed and cue, settings and mixes, loop, trims held, notes and verdict come back (`a_session_reopens_as_it_was_saved`), and the reopened working wind renders its passage sample for sample as before (`reopening_a_session_restores_loop_seed_and_settings`) |
| Editing a matched loop or working mix with the match held | Listening trims stay fixed and the Lab indicates that a rematch is needed. Match levels updates the trims and level readings. | A fader marks only Working stale, the loop both generators, the trim unchanged; not held, the edit drops it (`edits_make_trims_stale_per_candidate`) |
| Behaviour panel | Starts collapsed. Opening it shows the selected generator's declared behaviour settings and audition actions. Closing it leaves playback and mixing available. | Built: closed until opened; it shows gust brightness and Run freely |
| Matching a sparse recording (a bird) against a noise | Gated levels within 0.1 dB, and the reported uncertainty covers the halves | Not tested |
| As recorded | The stereo file reaches the ears sample-exact apart from the trim (a test through the engine), and the tap hears it | Stage 3 |
| ABX with a listener who answers at random (simulated, 10 000 runs) | 12 or more of 16 in 3.8 % of runs | Stage 2 |
| Blind order | Over 10 000 trials, A comes first or is X half the time within 1 % | Stage 2 |
| A saved version | Played again with its seed, sample-identical to the first time (offline render) | Stage 3 |
| The log | Every round written and read back the same; a round is never rewritten | Stage 2 |
| The mixer | A generator's layers at their mix levels sum to the generator as it sounded before (sample-identical at 0 dB each); a soloed layer is that layer alone; a muted layer is absent from the output | Stage 1's part: the untouched mix renders the preset's wind sample for sample; a muted layer's gain is 0 and a solo leaves only the soloed layers (`the_untouched_mix_is_the_generator_as_it_was`) |
| Ingredients | Each described setting round-trips: set, read back, and heard (the render differs) | Stage 4 |
| Measure | The same comparison as `analyse --compare` on the two captured files | Stage 5 |

## Deferred work

- Broader generator tuning after the first soft-wind session.
- **Focus tones.** A standalone generator for sustained pitches with overtones and a slow
  shimmer. The nine Solfeggio pitches span 174–963 Hz.
  [UI design plan, Beyond nature](ui-design-plan.md#beyond-nature)
- MUSHRA, and listening panels of several people.
- Changing settings in the middle of a run (from the cue on), to hear a timing setting on the
  same gust. The separate random streams cover most cases without it.
- Timeline storytelling mode.

Considered and left out: matching by broadcast loudness (LUFS), so that "matched" means the
one measure the analyser and the engine's calibration use; a confidence rating on every
trial; a glossary, state diagrams and multi-listener or remote listening, which a tool with one
listener does not need.

## As built

Stage 1, built 2026-11-22. Where the build departs from the plan above, and why. The engine
side is `src/lab/` (decisions and the voice), the app's surface `crates/ffi/src/engine/lab.rs`,
the screen `examples/compose-scene/.../editor/Lab.kt` with `session/LabBench.kt`.

- **The three candidates play in one voice.** The plan had each candidate its own source,
  trimmed by `set_source_level` and switched by muting. Built so, the reference and the two
  generators would each go through their own voice's renderer state (a diffuse field's
  decorrelation filters are per voice), so the match would not have included one render path;
  and a switch would be two mute commands that can land in different blocks. The lab's voice
  (`atrium::lab::LabVoice`) holds the recording and both generators, advances all three every
  sample, mixes them by its fade and trims, and sits in the scene as the soft wind would
  (`placed_like`: the wind's level, shape and place). A trim is therefore a gain inside that
  voice, before the soft wind's `reference_spl`. It also stays out of the perceptual masking
  layer, which only scores what is heard; the 6 dB check reads 6.0000 dB.
- **The switch has two fade laws.** A 10 ms straight-line fade between two independent noises
  dips by 1.8 dB at its middle (its mean of (1 − t)² + t² is 2/3); one that keeps the power
  (sine and cosine) holds the level within 0.01 dB, which is the check's 0.1 dB with room.
  Previous and Working share their seed, so they are nearly one signal: a power-keeping fade
  between them would swell by 3 dB, and the straight lines keep it whole. The law is chosen by
  the pair.
- **Every mute now fades over 10 ms.** "Alone" mutes the rest of the scene when the Lab plays
  and gives the listener's mutes back when it pauses; a mute cut in one step, as before, would
  click at the start of every audition. `src/audio/mute_fade.rs` is shared by the three scene
  voices (recording, takes, generator). A voice muted before it has played anything, built
  again in a voice the listener silenced, stays silent from its first sample.
- **Recalling the state is a clone that never allocates.** The soft wind, its driver, noise
  streams and filters are `Clone`; the wind's repertoire is shared by reference (`Arc`), so
  putting the kept state back at a wrap copies numbers and counts a reference, and frees
  nothing. A timing change, a new seed, cue or passage builds a new voice off the audio thread
  (the old one fades out first) and hands it over through the ring that adds sounds.
- **Levels are matched from the live analysis's columns.** Each 50 ms column's A-weighted
  level (`atrium_analysis::Live`) is turned back into power and gated as
  `power_while_sounding` gates a file: 400 ms blocks every 100 ms, the threshold of hearing
  as the absolute gate (the tap is calibrated in dB SPL), 10 dB below the mean as the relative
  one. A candidate is counted from the first sample it is heard alone, which the voice reports
  on the tap's clock. A consequence: at a volume where the wind is below the threshold of
  hearing nothing sounds, and the match keeps the trims it had. The halves are the first and
  second half of the passage, by the voice's place in it, over every pass. Before any match the
  recording is brought to the wind's level by their levels while sounding (the recording
  against a five-minute run), so the trims start near 0 dB.
- **Not holding the match drops a stale trim.** The plan said what holding does, not what not
  holding does; without it an edit that reaches a candidate puts its trim back to 0 dB,
  unmatched.
- **Undo of the mixer is the lab's own.** It lives in the engine (`lab_undo_mix`), apart from the
  scene's ⌘Z, and moving the same layer's level again straight after is one step (a drag sends
  many values); a solo or a mute is always a step of its own.
- **Stage 1 saves a session, not versions or a log.** `lab/soft-wind/session.json` is rewritten
  on every save, with the log line's fields where they exist (question, reference and passage,
  each candidate's seed, cue and settings, what changed, trims, mode, notes, verdict) plus the
  mixes and whether the match was held. Previous is the preset as it is, or a reopened
  session's; making Working the next Previous is a version, stage 3.
- **Run freely is the passage made long.** It plays the recording from the passage's start to
  its end, and the generator from the cue for as long, looping or once as Loop says; the
  gusts come as they come.
- **The run's markers snap to every move of the wind.** The soft wind's repertoire has gusts,
  swells and lulls; all three are reported, and the start marker snaps to any within 12 px.
  Both of the run's markers move the passage, whose length is the recording's.
- **Gust duration is flagged as timing, as the plan lists it, but does not move the soft wind's
  gusts.** With a repertoire, the wind's pushes come from its moves (`WindMoves`); the gust
  trajectory whose duration this sets is used only by a wind without one. The rest between
  moves is the timing setting that moves them (the test halves it). Turbulence time changes the
  ruffles, not the gusts. The engine's table (`atrium::lab::SETTINGS`) holds these four and
  gust brightness, the only one the Behaviour panel shows.
- **The Lab plays from the audition voice.** The last of the engine's 16 voices, like a preset
  heard from the library; hearing a preset there stops the Lab's voice, and a scene whose
  sounds take all 16 voices has no room for the Lab. Its voice plays on the scene's clock, so
  playing the Lab starts a paused scene, with every other sound muted.
- **Open recording… is the desktop's file dialog.** It opens on `assets/reference/` when there
  is one, else `research papers/`. iOS has none yet.
