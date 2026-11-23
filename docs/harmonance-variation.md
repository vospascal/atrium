# How Harmonance varies its sounds over time

Research record, 2026-11-23. Question from Pascal: "what variation do they bring in their
frequencies?" Answer in one line: **very little, and almost none of it is random.** The tones
themselves never change pitch. What moves is level (a slow rotation in Harmonics, an optional
±15 % random walk called Drift, a breath-paced dip), a pre-recorded 180 s strings loop, and a
handful of sine LFOs and timers in the synthesised ambient beds.

Sources, all public and read as text only (no script was executed, no audio downloaded):

| Short name | What it is | Where we read it |
|---|---|---|
| `Index` | Studio main chunk (`/studio/assets/Index-CnDmehGI.js`, 423 KB, minified) | scratchpad `research-variation/` |
| `StudioChrome` | Studio strings adapter + player domain (`StudioChrome-Wsd-U-y2.js`) | same |
| `BinauralBuilder`, `JourneyModal`, `useJourney` | Studio side chunks | same |
| `player.js` | per-frequency page player (`frequency-player.js`) | scratchpad `page/player.js` |
| `programme-player.js`, `player-domain/binaural/index.mjs` | binaural "programme" generator (readable, commented) | scratchpad `research-variation/binaural/` |
| strings `manifest.json` (×5), breath cue `manifest.json` | JSON metadata only | scratchpad `research-variation/` |

The `js` file saved next to the Studio HTML (545 KB) is Google Tag Manager, not their audio code;
the audio lives in the Vite chunks above. Minified names (`V`, `Xs`, `Re`, …) are as they appear
in those builds and will change with their next deploy.

Where this record says **(inferred)** it is our reading of behaviour that the code implies but
does not state; everything else is read directly.

---

## 1. Pure tone

**Studio** (`Index`, `B`): one `OscillatorNode`, sine (waveform switchable to other oscillator
types by `ve`), fixed frequency, set once with `setValueAtTime`. No detune, no vibrato, no pitch
drift anywhere in the Studio. The only things that ever move a pure tone are its gain (fades,
Drift, Breathe) and the master low-pass (Breathe).

**Frequency pages** (`player.js`): the pure tone is a **20 s mastered `.ogg` file looped by an
`<audio>` element** (`data-player-audio-src`, `data-player-duration="20"`, `a.loop=!0`), mastered
to −22 dBFS RMS. When the file cannot play, the fallback is a live sine oscillator. We did not
download a file, so whether the 20 s render itself carries any movement is unknown — see Known
gaps.

**Level per frequency, static** (same in both players): every tone's gain comes from the
**ISO 226:2003 equal-loudness table** — 29 third-octave centre frequencies 20 Hz–12.5 kHz with
the three ISO columns α_f, L_U, T_f (`yd` in `Index`, `s` in `player.js`). That is the "table of
third-octave values with three columns". It is used for one thing: compute the SPL that a tone
of that frequency needs to sound as loud as **38.35 phon** (`vd=38.35` in `Index`; the constant
0.03551 in `player.js` is the same 38.35 phon pre-computed), interpolated in log-frequency,
mapped to dBFS as `SPL − 86.01` (so they assume 0 dBFS ≈ 86 dB SPL), capped at −12 dBFS, then
divided by the default master volume 0.18 so that at default volume the tone sits exactly at its
equal-loudness level. The Studio subtracts −3.01 dB (`xd`, RMS of a unit sine) and divides by the
default tone volume 0.5 too (`Td`). Nothing in this is time-varying.

## 2. Harmonics (`longform-octaves-moving`)

Already reproduced in Atrium; recorded here for completeness (`player.js`, the function built
inside the `harmonicsId` branch of `K`, helpers `Ne`, `He`, `Te`).

- Data comes from a JSON block on each page (`data-harmonics-listens`): `lines` (1–8 lines,
  each `hz` + `rms_dbfs`), `rotation.period_s` (allowed 1–600) and `rotation.depth_db`
  (allowed 0–24). Both pages we read use **period 24 s, depth 8 dB**.
- Each line is one sine oscillator at a fixed frequency. Its gain is driven by a second
  oscillator at 1/period Hz whose wave is a 12-harmonic `PeriodicWave` built from modified
  Bessel functions I_n (`Ne` is the I_n series). That makes the gain exactly
  `exp(a·cos(2πt/P − φ))` with `a = (depth/2)·ln10/20`, i.e. **a sine in decibels, ±depth/2 =
  ±4 dB**, not a sine in amplitude.
- Phase offset per line `φ = 2π·index/lines`: the tone (index 0) peaks first and the emphasis
  walks down the octaves. With 3 lines and 24 s the peaks are 8 s apart; with 4 lines 6 s.
- Line levels: `√2·10^((rms_dbfs − total − 22)/20)`, so the whole set is normalised to −22 dBFS
  RMS (`He` is the power sum of the lines).
- Deterministic, no randomness. Repeats exactly every `period_s`.
- A "Harmonics set" can chain several listens and advance by elapsed seconds per segment
  (`M.harmonicsSet.segments[].seconds`, checked in the timer around `D(1)`).

## 3. Strings ("Evolving strings")

`StudioChrome` (`Re`, the arrangement adapter; `we` validates the manifest) and the manifests.

- **One pre-rendered stereo recording per frequency**, 32 frequencies (`_` lists them: 111, 126.22,
  136.1, 140.25, …, 963, 999). Files `strings-v1-<hz>/`, manifest label "Evolving strings".
- **Loop length 180 s** (loopStartFrame 48 000 → loopEndFrame 8 688 000 at 48 kHz, i.e. the loop
  starts 1 s into the file). It repeats exactly every 180 s. All variation is baked into the
  recording; the code adds no modulation to the strings. Measured −24 LUFS integrated, true peak
  −10.3 to −11.7 dBTP (manifest `measuredLoudness`, `measuredPeak`).
- Streaming: 20 segments of 2.5, 5, 10 × 17, 2.5 s (Opus, AAC fallback, each with ≥3840 lead and
  ≥960 trail frames, boundaries on a 160-frame grid). While loading, segments are chained with
  **70 ms linear crossfades** (`Oe=.07`, in `me`); once all are decoded the adapter switches to
  one looping buffer. Playback position is persisted (`positionSeconds`), so a resume continues
  where it stopped instead of restarting the loop.
- The pure tone is mixed under the strings by a separate oscillator into the `tone` gain. Default
  tone gain under strings ≈ 0.0011 (≈ −59 dB, manifest `defaultToneGain`), so at the default
  setting the strings carry the frequency and the sine is a trace. Pure mode uses
  `pureToneGain` 0.0794 (−22 dB).
- Per-arrangement loudness trims, static: Studio `Xu` +9.2 to +13.3 dB (applied to the strings gate
  with a 70 ms raised-cosine curve, `ed`/`Zu`); page player `u` −4.6 to −8.7 dB.
- Level sliders (`se`): below 0.5 the gain is `base·(2x)²`; above 0.5 it is `base·m^(2x−1)` with
  m = 2 (+6 dB) for strings and pure tone, m = 8 (+18 dB) for the tone under strings.
- Switching strings↔pure: equal-power 0.35 s curve (`Le`, `ke=.35`); other setting changes
  0.07 s linear (`Ie`).
- Output: soft limiter (`Pe`), linear to 0.794 (−2 dBFS), tanh knee up to 0.977 (−0.2 dBFS).

### Warmth (strings only)

A **high-shelf biquad at 1500 Hz** on the strings music (and therefore on the reverb feed);
gain `(0.5 − warmth)·12` dB, so **+6 dB at "Brighter" (0), 0 dB at 0.5, −6 dB at "Warmer" (1)**.
The tone oscillator bypasses it. Static control, no movement. (`StudioChrome`, `te`; filter made in
`j`.)

### Space (strings only)

Two mechanisms split at 0.5 (`te`):

- **Below 0.5 "Closer" = narrower stereo.** A 2×2 matrix with direct gain `(1+s)/2` and cross gain
  `(1−s)/2`, `s = 0.5 + space`. At 0, s = 0.5 → direct 0.75, cross 0.25. At ≥ 0.5 full width.
- **Above 0.5 "More space" = convolution reverb.** `ConvolverNode` (normalize off) with a measured
  or rendered impulse `space.ogg` (loaded lazily the first time space passes 0.5). Wet amount
  `u = 2·space − 1`, wet gain `0.19585·u` (−14.2 dB at full).
- Both paths are **loudness-compensated**: narrowing by `1/√(1 − 0.2496·(1 − s²))`, reverb by
  `1/√(1 + 2·0.1861·u + 0.1766·u²)` (`Ne`, `Me`, `je`). (inferred) The three coefficients are
  fitted correlation/energy terms so the perceived level stays constant while the control moves.

## 4. Binaural

**Studio** (`Index`, `B`, `ke`; `BinauralBuilder`): per tone two oscillators at `f − offset`
(hard left, `StereoPanner −1`) and `f + offset` (hard right). Offset default 2 Hz (`od`), clamped
0.5–44; the builder passes **offset = beat/2**, so the beat is 2·offset. Builder bands: delta
0.5–4 (default 2), theta 4–8 (6), alpha 8–13 (10), beta 13–30 (18), gamma 30–44 (40); carriers
offered 174, 200, 136.1, 432. Changing the beat rebuilds the oscillators: 0.2 s fade-out, 250 ms
wait, 0.2 s fade-in. Static otherwise.

**Frequency pages, "programme" entries** (`programme-player.js`, `binaural/index.mjs`) — the only
place where a frequency itself moves:

- Carrier fixed at **108 Hz left**; right = 108 + beat. Duration **32 400 s (9 h)**.
- Beat either fixed (1.618, 2, 4, 6, 7.83, 10, 20, 40 Hz) or a **single linear sweep over the nine
  hours**: DELTA 3.5→1.5, THETA 7→5, ALPHA 12→9, BETA 16→24, GAMMA 32→40 Hz.
- Right phase is the analytic integral `(108 + b0)·t + (b1 − b0)·t²/(2T)`, so the sweep is
  phase-continuous; pause/seek reopen a source at the exact analytic phase (30 ms crossfade on
  seek). Envelope: 20 s raised-cosine fade in and out (the web player replaces the opening fade
  by full level). Level matched to catalogue tones: unit sine × 10^(−22/20)·√2.
- Never loops; position persisted in `sessionStorage`, saved every 5 s.
- Page-player non-programme binaural: two fixed sines, 108 Hz left and 108 + beat right
  (`data-binaural-left/right`).

## 5. Drift (Studio "randomDrift")

`Index`, function `V` (scheduler), `de`/`fe` (cancel), toggled by `De`.

- Per layer (each tone and each ambient sound), an independent chain of `setTimeout`s.
- First move after **1–2 s** (uniform). Each move: new target = **base ± U(−1, 1)·15 % of base**,
  clamped to 0–1; **linear ramp of 3–5 s** (uniform) to that target; the next move is scheduled
  after the same 3–5 s, so the gain is a piecewise-linear random walk that always returns towards
  the user's level (it is anchored on the base, not on the previous target).
- At the default volume 0.5 the range is 0.425–0.575, i.e. **−1.4 dB to +1.2 dB**.
- `Math.random`, unseeded. No period; never repeats exactly.
- No pitch, filter or pan movement — gain only.
- (inferred) Drift targets the raw base level, ignoring the mix-count scaling `pe` (see §8), so with
  three or more layers Drift also raises each layer back towards its unscaled level.
- Six of the thirteen ready-made mixes have Drift on (see §8).

## 6. Breathe

`Index`: patterns `Wr`, pace `Br`, phase maths `qr`/`Qr`/`Jr`, pacer `va` (and `Sa` for the
power-breathing method), audio hook `Ee` (`setBreathModulation`).

**Patterns** (seconds): Box 4-4-4-4; 4-7-8; Coherence 5-5; Energizing 2-2; Deep Calm 6-2-8-2.
Pace multiplier: calm ×1.467 (2.2/1.5), steady ×1, quick ×0.667. 3 s count-in. Power breathing:
1.5 s in + 1.5 s out × pace, 20/30/40 breaths per round, 1–5 rounds, a timed retention, 15 s
recovery.

**What it does to the sound** (only while "Sync tone to breath" is on, default on, and audio is
playing): a breath value 0…1 — inhale rises 0→1 with ease-in-out-quad, exhale falls 1→0 with
the same curve, holds keep the previous value, the very first inhale rises from 0.5 with
ease-out-cubic — drives the whole Studio mix:

- master gain `0.85 + 0.15·v` → **−1.4 dB on empty lungs, 0 dB on full**, linear ramp 0.12 s;
- master low-pass (biquad, Q 0.707) `6500 + 13 500·v` Hz → **6.5 kHz on empty, 20 kHz on full**,
  exponential ramp 0.12 s (`ud=.85`, `fd=6500`, `dd=2e4`, `pd=.12`).

Updated every animation frame. Deterministic: it follows the pattern clock exactly.

**Voice cues**: TTS recordings (the manifest names the voice), 3 takes each for inhale/hold/exhale,
6 each for in/out, one gong. Takes are chosen by a **seeded shuffle bag** (`ra` hash with the
golden-ratio constant 0x9E3779B9, `ia` Fisher–Yates, `aa`): bag *k* is a deterministic
permutation of the takes, and if a new bag would start with the take that ended the previous bag
the first two are swapped — so no take is heard twice in a row, and the same session always
gives the same sequence. Scheduled 15 s ahead (`pa.topUp`).

## 7. Ambient sounds (Studio, all synthesised live)

`Index`, `$s` (one `switch` per sound), helpers `qs`/`Ys` (noise), `Xs` (LFO), `Zs` (crackle
shaper), `Js` (bowl reverb IR).

**Shared noise source** (`qs`, `Ys`): **one 12 s mono white-noise buffer** (`Ks=12`,
`Math.random`) per sample rate, cached and shared by every noise layer of every sound. Each
layer plays it looped at a random rate **0.85–1.15** from a random start offset, so each layer
repeats every ≈ 10.4–14.1 s, and all layers are the same noise at different speeds.

**LFO** (`Xs`): a sine oscillator into a gain of `(max − min)/2`, added to the parameter whose
own value is set to `(max + min)/2`; i.e. a plain sine between min and max. Deterministic.

| Sound | Layers and movement |
|---|---|
| Rain | noise → low-pass 2400 Hz Q 0.5, cutoff swept **1900–3200 Hz at 0.1 Hz (10 s)**, gain 0.5; plus noise → high-pass 5 kHz, gain swept **0.04–0.12 at 0.17 Hz (5.9 s)** |
| Ocean Waves | noise → band-pass 400 Hz Q 0.5, gain with two LFOs **0.071 Hz (14.1 s) 0.1–0.65** and **0.029 Hz (34.5 s) 0.85–1.15**; plus noise → low-pass 200 Hz, gain **0.05–0.35 at 0.037 Hz (27 s)** |
| Wind | noise → band-pass Q 2, centre with two LFOs **0.07 Hz 150–400 Hz** and **0.13 Hz 200–500 Hz**; gain with two LFOs **0.05 Hz 0.2–0.5** and **0.021 Hz 0.85–1.2** |
| Fire | noise → low-pass 700 Hz Q 0.7, gain 0.28; crackle: noise → high-pass 1800 Hz → dead-zone shaper (passes only samples beyond ±0.82, tanh ×4) → gain **0.25–0.7 at 0.15 Hz** |
| Forest | noise → low-pass 400 Hz, gain 0.05; **bird chirps**: first after 1–2 s, then every **4–10 s**, a burst of **2–4 sine notes 90 ms apart**, base 2200–3400 Hz, each note ±120 Hz up or down (direction random per burst) ±40 Hz jitter, glide ×1.15 in 40 ms then ×0.95 at 80 ms, 12 ms attack to 0.03, decay to silence by 90 ms |
| Space | sines 55 Hz (0.045), 55.33 Hz (55×1.006, 0.03; beats at 0.33 Hz), 82.5 Hz (0.022); noise → band-pass Q 12 swept **2200–4200 Hz at 0.02 Hz (50 s)**, gain 0.012 |
| Singing Bowl | sines 256, 257.5 (256×1.006, beats at 1.54 Hz), 512, 768, 1024 Hz (0.3, 0.12, 0.09, 0.045, 0.024); tremolo **0.9–1.1 at 0.3 Hz**; strike envelope 12 ms attack, exponential decay to 0.18 at 9 s, **re-struck every 12 s exactly**; strike noise band-pass 1600 Hz Q 0.9, 4 ms attack, 220 ms decay; reverb: synthetic 2 s stereo noise IR shaped `(1 − t/T)³`, wet 0.3, dry 0.7 |
| Night | noise → low-pass 200 Hz, gain 0.025; **crickets**: 40 Hz square → band-pass 4 kHz Q 10 → amplitude sine **0–1 at 6.2 Hz** → gain 0.02; switched on/off with 0.5 s ramps — first off at 2.5 s, then **off for 6–13 s, on for 9–16 s** |

(inferred) Where two LFOs drive the same parameter (Ocean and Wind), the second `setValueAtTime`
overwrites the first centre value and the two oscillations add rather than multiply: Ocean gain
ends up moving about 0.575–1.425, Wind gain 0.7–1.35, Wind centre 75–625 Hz — wider than either
stated range. Probably unintended.

Randomness: `Math.random`, unseeded, in the noise, playback rates and offsets, and the Forest and
Night timers. Everything else is fixed sine LFOs; their periods (5.9–50 s) are not integer
multiples of each other, so the combined pattern does not audibly cycle, but each noise layer
does loop every 10–14 s.

## 8. Mixes: combining, presets, shuffle

`Index`: `pe`/`me` (count scaling), `R`/`le` (ramps), `Me` (`applyPreset`), presets `An`,
shuffle `Pi`, nature shuffle near `Qn`; `useJourney`, `JourneyModal`.

**Signal chain**: tones → per-tone equal-loudness gain → per-tone fader → master low-pass;
ambient sounds → dynamics compressor (threshold −50 dB, knee 40, ratio 12, attack 0, release
0.25 s) → the same low-pass; then breath gain → master volume (**slider squared**, default 0.18)
→ soft clipper (linear to 0.7, tanh to 0.891) → output. (inferred) Tones bypass the compressor;
the ambient beds are heavily compressed.

**Count scaling** (`pe`), applied 300 ms after any add/remove: 1 layer ×1, 2 ×0.85, 3 ×0.75, 4+
`max(0.4, 1/√n)`. Max 12 tones (6 on low-end devices).

**Fades**: add/remove a tone 0.2 s linear; fader moves 0.05 s; play/pause 0.07 s raised cosine
(33-point curve); ambient sound volume and stop 0.2 s linear.

**Ready-made mixes** (13, `An`): fixed frequencies and levels plus optional sound, and a Drift flag.
Examples: Morning Energy = 528 (0.4) + 285 (0.3) + 126.22 (0.5) + Forest (0.3), Drift **off**;
Deep Sleep = 174 + 432 + 194.18 + Rain, Drift on; Calm & Soothe = 396 + 432 + 194.18 + Ocean,
Drift on. Drift on in 6 of 13. Order: sorted by the sign-up purpose tag; time-of-day hints
(morning/afternoon/evening/night) pick suggestions. Switching mixes: old layers fade out over
**1.5 s**, new ones start 450 ms later and fade in over 1.5 s (0.35 s and no wait when chosen by
hand).

**Shuffle / next**:
- Studio shuffle (`Pi`): one frequency drawn uniformly (`Math.random`) from the library, excluding
  the ones already playing; played alone at 0.5, Drift off.
- "Add nature": a random sound not yet in the mix replaces the current nature sound.
- Previous/next: walk the library sorted by frequency.
- Page player shuffle (`z`): weighted random (`Math.random`) over candidates scored by shared
  themes/context with the current track, excluding the last 5 played.
- Player-session queue shuffle (`StudioChrome`, `te`): **seeded** — FNV-1a hash of
  `seed:id:index` sorts the queue; the seed is the session revision.

**Journeys** (6, e.g. Deep Sleep Journey, Morning Awakening): a timeline of mix steps of
**180–540 s** each (Deep Sleep: five 360 s steps, 30 min), each step applied as a preset with the
1.5 s crossfade, Drift forced off. The nearest thing they have to our "timeline mode".

---

## Parameter table

| Sound type | Parameter | Value | Source |
|---|---|---|---|
| Pure tone (Studio) | pitch movement | none; fixed `setValueAtTime` | `Index` `B` |
| Pure tone (pages) | loop | 20 s `.ogg`, `<audio loop>` | `player.js` media branch, `data-player-duration` |
| Pure tone (pages) | mastering level | −22 dBFS RMS | `player.js` `W` (`dn(e,−22)`), `programme-player.js` comment |
| All tones | equal-loudness target | 38.35 phon, ISO 226:2003, 20 Hz–12.5 kHz | `Index` `yd`,`vd`,`wd`; `player.js` `s`, `dn` |
| All tones | dBFS mapping | SPL − 86.01, cap −12 dBFS | `Index` `wd`, `bd=-12` |
| All tones | default master volume | 0.18, applied squared in Studio | `Index` `gd`, `hd`; `player.js` `un`,`fn` |
| Harmonics | rotation period | 24 s (allowed 1–600) | page JSON `rotation.period_s`; `player.js` `Te` |
| Harmonics | rotation depth | 8 dB peak-to-peak (±4 dB), dB-domain sine | page JSON `depth_db`; `player.js` (`Ne`, Bessel `PeriodicWave`) |
| Harmonics | phase per line | 2π·index/lines | `player.js` (`m=2*Math.PI*u/r`) |
| Harmonics | set level | −22 dBFS RMS total | `player.js` (`l.rms_dbfs−o−22`) |
| Strings | loop length | 180 s, starts 1 s into file | manifests `loopStartFrame`/`loopEndFrame` |
| Strings | segments | 20: 2.5, 5, 10×17, 2.5 s | manifests |
| Strings | segment crossfade | 70 ms linear | `StudioChrome` `Oe`, `me` |
| Strings | loudness | −24 LUFS, −10.3 to −11.7 dBTP | manifests |
| Strings | tone under strings | ≈ −59 dB default, up to +18 dB by slider | manifests `defaultToneGain`; `StudioChrome` `se` (m=8) |
| Strings | pure tone gain | 0.0794 (−22 dB) | manifests `pureToneGain` |
| Strings | mode switch | equal-power 0.35 s | `StudioChrome` `Le`, `ke` |
| Warmth | filter | high-shelf 1500 Hz, +6 dB … −6 dB | `StudioChrome` `j`, `te` |
| Space < 0.5 | stereo width | direct (1+s)/2, cross (1−s)/2, s = 0.5+space | `StudioChrome` `te` |
| Space > 0.5 | reverb | convolver `space.ogg`, wet 0.19585·(2·space−1) | `StudioChrome` `ee`, `te`, `Ae` |
| Space | loudness compensation | coefficients 0.2496, 0.1861, 0.1766 | `StudioChrome` `Ne`,`Me`,`je` |
| Binaural (Studio) | offset | 2 Hz default, 0.5–44, beat = 2·offset | `Index` `od`, `ke`; `BinauralBuilder` (`x(n/2)`) |
| Binaural (programme) | carrier | 108 Hz left | `binaural/index.mjs` `CARRIER_HZ` |
| Binaural (programme) | beat sweep | linear over 32 400 s, e.g. THETA 7→5 Hz | library page JSON specs |
| Binaural (programme) | fades | 20 s raised cosine | `index.mjs` `FADE_SECONDS` |
| Drift | step | ±15 % of base, uniform | `Index` `V` (`r*.15`) |
| Drift | ramp / interval | 3–5 s linear, uniform; first after 1–2 s | `Index` `V` |
| Drift | randomness | `Math.random`, unseeded | `Index` `V` |
| Breathe | patterns | 4-4-4-4, 4-7-8, 5-5, 2-2, 6-2-8-2 s | `Index` `Wr` |
| Breathe | pace | ×1.467 / ×1 / ×0.667 | `Index` `Br` |
| Breathe | gain range | 0.85–1 (−1.4 dB) | `Index` `ud`, `Ee` |
| Breathe | low-pass range | 6.5–20 kHz, Q 0.707 | `Index` `fd`, `dd`, `Ee` |
| Breathe | ramp | 0.12 s | `Index` `pd` |
| Breathe cues | take choice | seeded shuffle bag, no immediate repeat | `Index` `ra`,`ia`,`aa` |
| Ambient | noise buffer | 12 s white, shared, rate 0.85–1.15, random offset | `Index` `Ks`, `qs`, `Ys` |
| Rain | cutoff LFO | 1900–3200 Hz at 0.1 Hz | `Index` `$s` rain |
| Ocean | swell LFOs | 0.071 Hz and 0.029 Hz | `Index` `$s` ocean |
| Wind | filter LFOs | 0.07 Hz and 0.13 Hz; gain 0.05 Hz and 0.021 Hz | `Index` `$s` wind |
| Fire | crackle | dead-zone ±0.82, gain LFO 0.15 Hz | `Index` `Zs`, `$s` fire |
| Forest | chirp bursts | every 4–10 s, 2–4 notes, 2.2–3.4 kHz | `Index` `$s` forest |
| Space (sound) | sweep | 2200–4200 Hz at 0.02 Hz | `Index` `$s` space |
| Singing Bowl | strike period | 12 s fixed | `Index` `$s` singing-bowl |
| Singing Bowl | partials | 256 × {1, 1.006, 2, 3, 4} Hz | `Index` `$s` singing-bowl |
| Night | cricket gating | on 9–16 s, off 6–13 s, 6.2 Hz pulse | `Index` `$s` night |
| Mix | count scaling | 1, 0.85, 0.75, max(0.4, 1/√n) | `Index` `pe` |
| Mix | preset crossfade | 1.5 s (0.35 s manual) | `Index` `je`, `Me` |
| Mix | compressor | −50 dB, knee 40, 12:1, 0 / 0.25 s | `Index` `oe` |
| Mix | soft clip | linear to 0.7, ceiling 0.891 | `Index` `Ed`, `Sd`, `Cd` |
| Journeys | step length | 180–540 s | `JourneyModal` `durationSeconds` |
| Shuffle | Studio | uniform `Math.random`, excludes playing | `Index` `Pi` |
| Shuffle | queue | FNV-1a seeded order | `StudioChrome` `ee`, `te` |

## Known gaps

- **The strings recordings, the 20 s pure-tone `.ogg` files and the `space.ogg` impulse were not
  analysed** — we read only their manifests. Whatever "evolving" happens inside the strings is in
  the audio, not the code. Downloading one arrangement (≈ 20 segments) and one pure-tone file and
  running our `analyse` tool on them would answer it; not done because audio downloads need
  Pascal's go-ahead.
- The Studio's per-frequency page "Harmonics" voice exists only on the frequency pages; we saw two
  listens (333 Hz and 528 Hz), both 24 s / 8 dB. Other voices may exist on other pages.
- Minified names are build-specific.

## What Atrium could take from this

Measured against our rules (real physical size, compute before judging, nothing arbitrary):

1. **Equal-loudness levelling per tone is worth copying, and it is real.** ISO 226:2003 at a
   chosen phon level gives each focus tone the level that sounds equally loud. We would state our
   own calibration (they assume 0 dBFS ≈ 86 dB SPL; ours depends on the speaker chain and should
   be measured), and the phon target is a decision to record, not a constant to borrow.
2. **Their "variation" is small and mostly deterministic.** Drift is ±15 % gain (≈ ±1.3 dB) over
   3–5 s; Harmonics is a ±4 dB rotation every 24 s; Breathe is −1.4 dB and a 6.5 kHz low-pass.
   No pitch moves except the 9-hour binaural sweep. If we add gain movement to a tone, ±1–2 dB
   over seconds is the scale they found acceptable — but in Atrium it should come from a physical
   cause (source or listener motion, air turbulence on a long path), computed, not a random gain.
3. **Seeded shuffle bag without back-to-back repeats** (their breath cues) fits our recorded takes
   from the cutter exactly: deterministic per seed, so offline renders and comparisons are
   reproducible, and no take plays twice in a row.
4. **Phase-continuous chirp** for any slowly evolving beat or focus tone: integrate the frequency
   analytically so pause, seek and offline render agree sample-for-sample.
5. **Loudness compensation when a "space" control moves** — keep perceived level constant while
   width or reverb changes. In Atrium the equivalent is energy-correct direct/reverb balance from
   the room model, not fitted coefficients.
6. **Do not copy**: the shared 12 s looped noise buffer (audible repetition; we generate
   continuously), harmonic singing-bowl partials (real bowls are inharmonic) and its fixed 12 s
   strike, the additive double-LFO on one parameter, the ±6 dB "Warmth" shelf and the breath
   low-pass as tone controls (they are mixing effects with no physical counterpart), and the heavy
   −50 dB / 12:1 compressor on the nature beds.
7. **A 180 s pre-rendered loop is their answer to "not static"** for the strings. That is the
   opposite of our generative direction; it repeats every three minutes. Our aliveness goal needs
   variation that does not cycle at all.
