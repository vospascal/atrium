# Synth Audit — Rain (and warmth across all generators)

Status: **verification complete** (2026-07-12). Four-angle audit workflow, 62/62
agents, **24 findings confirmed by both a code lens and a physics lens**; 5
"contested" were true-with-a-caveat, not refutations. **Round 1 of fixes
applied** (see §4). This doc records (1) the user's listening feedback — the
ground truth — (2) findings, and (3) the reconciliation where audit and ears
disagreed.

**Key reconciliation — warm vs. bright:** the audit's headline rain fix is to
make the sound *brighter* (restore 2–16 kHz sizzle, proper −3 dB/oct pink). The
user asked for *warmer*. The ears win. What both agree on is the real bug: the
shared pink generator ran ~6× hot, over-weighting every hiss layer. Round 1
fixes the level (→ warmer everywhere) but deliberately keeps the darker tone and
does NOT apply the brighten-it recommendations.

---

## 1. User listening feedback (ground truth — outranks the analysis)

From listening to `scenes/synth-test.yaml`:

- **Rain**: v1 and v2 *combined* work well — v2 gives a nice background rain
  wash, v1 gives the closer detail. But v1 is "so high pitch or something."
  → don't rewrite rain wholesale; the two-layer combination is the direction.
  Warm v1 down.
- **Intensity should be controllable** (live). → Phase 1 step 3
  (`SetSynthParam`) is wanted for the tuning loop, not just later.
- **Wind sounds too much like a wave**: too much fluctuation. Should ramp up
  slower, last longer, fluctuate less most of the time.
- **Waves have a "sissing" (hissing) sound that shouldn't be there.**
- **Everything should be warmer** overall.

Three of these — wave sissing, rain-v1 high pitch, "warmer overall" — point at
one shared root cause (see §2).

## 2. Confirmed by direct code reading (independent of the workflow)

- **`PinkNoise` is only a 3-pole filter** (`src/synth/noise.rs:59-84`; poles
  0.997/0.985/0.95). A 3-pole pink filter holds ≈−3 dB/oct only to ~1 kHz, then
  flattens toward white — i.e. too much high-frequency energy. This is the
  shared "sizzle/brightness" source feeding wind hiss, wave hiss, and rain
  beds. It is the single highest-leverage fix and matches the user's "warmer /
  sissing / high-pitch" trio. The full 7-pole Paul Kellet filter holds the
  slope to ~20 kHz. The audit also measured the current pink RMS at ~1.56
  (hot) — a controlled-level rewrite also stops rain v1 clipping.
- **rain_v2 `drop_rate` field is dead code** (`rain_v2.rs`): `new()` sets
  `drop_rate: 600.0`, but `next_sample()` recomputes a *local* `drop_rate` from
  a hard-coded piecewise curve (15/s at i≤0.2 → 50/s at i=0.5 → **75/s max** at
  i≥0.9) and never reads `self.drop_rate`. The public knob does nothing. (This
  is why the YAML def deliberately omits it.)
- **rain_v2 bubbles clamped 250–2500 Hz** (`bubble_params`): small
  `clamp(500,2500)`, medium `clamp(400,2000)`, large `clamp(250,1200)`.
- **rain_v2 bubble level ≈ 22 dB below impacts**: `impact_gain 0.6`,
  `bubble_gain 0.08`; bubble written at `amp*bubble_gain`. The header comment
  calls the bubble "the main rain sound" — it is buried.
- **rain_v2 triple low-pass**: per-impact 2-pole LP at `5500−4200·intensity`
  Hz (so *higher intensity = darker*), plus ring `env_smooth 0.65` (≈3.3 kHz
  one-pole), plus whole-mix LP at `800+6000·intensity` Hz.
- **rain_v1 bed defaults**: `hiss_gain 0.4`, `brown_gain 0.3`, `impact_gain
  0.6`, `env_smooth 0.7` — a continuous pink/brown bed that (per the audit)
  buries the drops.

## 3. Cross-angle convergence (pending final verification)

Four independent methods — TS-port diff, physics recalculation, rendered-signal
measurement, literature — agreed:

- **rain v1 ≈ wind**: bed-dominated, drops inaudible under the continuous
  pink/brown bed; measured centroid ~290 Hz (≈ the retired legacy wind control). The
  Rust port copied the *untuned* AudioParam descriptor defaults instead of the
  shipped tuned TS config (which had hiss at 0, brown at 1.0, impacts at 1.0,
  env_smooth 0.9).
- **rain v2 = event-starved + spectrally strangled**: ~75 drops/s max
  (silent 60–75% of the time → sparse/"machine-gun") and centroid ~0.65–1.4 kHz
  where real medium rain sits at ~2.7 kHz; heavy rain comes out *darkest*
  (brightness wired backwards).
- **v2 vs the paper it cites** (Liu/Cheng/Tong, SIGGRAPH 2019): the paper draws
  each impact click's frequency uniformly across 1–16 kHz, makes the Minnaert
  bubble *stronger* than the impact and pitched at 2–20 kHz, and renders
  100–2000 events/s. The code inverts or attenuates each of these.
- **Fix is retuning, not rewrite** — the two-layer architecture is sound:
  raise event density (or let a bright shaped-noise wash carry the body),
  restore 2–16 kHz content, lift and re-pitch bubbles, keep at most one LP
  stage (the pipeline already does distance/air coloration downstream — the
  source should be a *dry close-mic* sound).

## 4. Fixes

### Round 1 — APPLIED 2026-07-12 (warmth + wind/wave feel)

| Feedback | Change | Where |
|---|---|---|
| warmer overall / wave sissing / rain-v1 high pitch | `PinkNoise` output normalized ×0.19 (RMS 1.56→~0.30, no clip); kept darker 3-pole tone, did NOT brighten | `noise.rs` (+`pink_noise_is_level_normalized` test) |
| legacy wind "too wavy" (retired) | min_intensity 0.2→0.5 (shallower swells), mean_duration 8→18 s, jitter 3→5, rise ratio 35→50 %, turb_gain 0.4→0.2, yaml gustiness 3→2 | removed legacy synth and preset |
| waves sissing | hiss_level 0.3→0.15, crash_gain 0.6→0.45 | `wave.rs` |
| rain (both) | benefit passively from the pink fix; no drop-logic change (user likes the v1+v2 layering) | — |

Loudness is preserved automatically: each synth source is SPL-calibrated from a
preview render, so a warmer/quieter generator just gets more gain to hit the
same dB (observed: Wind amp 0.011→0.024, Rain v1 0.0035→0.015, no longer clips).
309 tests green.

### Round 2 — APPLIED 2026-07-12 (measured with `cargo run --bin analyze_synth`)

After round 1 the user reported: wind envelope better but tone still "not wind";
rain "a bit better, still too high"; waves "still overdrive sissing". Measuring
all four generators (new general `analyze_synth` binary) showed the brightness
was NOT the pink bed (that's dark) but the raw white-noise transients and mid
bursts. A dedicated wind-synthesis research pass (cited below) drove the fix.

| Generator | Change | Centroid before→after |
|---|---|---|
| **Wind** | full rebuild: HP the brown rumble (70 Hz, gain 0.8→0.25), band-passed pink "presence" bed (200 Hz + gust-modulated LP 400→3000 Hz), **aeolian resonator bank** (3 biquad BPs, centers from Strouhal f=0.2·speed/d), envelope now fans out to level+brightness+whistle pitch, subtle speed-scaled HF hiss | 136 → **417 Hz** (presence band 250–1000 Hz now populated) |
| **Waves** | lowpass the white-noise crash (1.4 kHz) — that was the "sissing" | 387 → **172 Hz** (air band −18→−34 dB) |
| **Rain v1** | burst LP 3500→1800 Hz; fixed bubble ping 3300→1600 Hz (the "metallic" artifact) | 459 → **398 Hz** |
| **Rain v2** | untouched — user likes it as the background layer | 561 (unchanged) |

New DSP primitive: `BiquadBP` (RBJ band-pass) in `noise.rs`, plus `set_cutoff`
on the one-pole filters. Regression test `wind_has_presence_band_energy`
(>25% of energy above 250 Hz). 310 tests green.

**Wind research — corrected conclusion (important):** the SC-Wind-Noise /
Mirabilii 2022 model our docs cite is a *microphone-buffeting* model (a 20–250 Hz
rumble by construction) — porting it would have made wind *worse*, not better.
What reads as wind is the **aeolian** sound: moving band-pass resonances in
200 Hz–2 kHz whose pitch tracks the gust (Strouhal f=0.2·U/d). Sources: Farnell
*Designing Sound* Ch.41; Selfridge/Reiss real-time aeolian tone
(https://intelligentsoundengineering.wordpress.com/2016/05/19/); Moffat/Selfridge/Reiss
"Sound Effect Synthesis" (https://www.eecs.qmul.ac.uk/~josh/documents/2019/Sound_Effect_Synthesis.pdf);
Mirabilii SC-Wind (https://www.audiolabs-erlangen.de/resources/2022-IWAENC-SWN,
useful only for "spectrum brightens with speed"); Nemisindo/Wwise layer practice.

### Round 3 — pending user's next listen

- **Live intensity control**: `Command::SetSynthParam { index, param, value }`
  + `SoundSource::set_param` hook (Phase 1 step 3). Wanted for the tuning loop.
- Wind whistle/presence *levels* (`whistle_gain`, `presence_gain`) are first-pass
  guesses — tune by ear.
- **rain_v2 dead `drop_rate` field**: honor it or delete it (confirmed critical).
  Deferred until the user decides whether v2 should stay as-is or get denser.
- rain v1 still slightly high? can drop burst LP further.

### Confirmed-but-deferred (from the audit, NOT yet applied — conflict with
"warmer" or with "keep rain as-is"):

- Raise rain event density 10–20× / raise bubble frequencies to 2–16 kHz /
  strip the LP cascade — all *brighten* rain. Revisit only if the user wants
  rain brighter after hearing round 1.
- rain v1 ring off-by-one (burst sample 0 replays ~171 ms late) — real, minor;
  fold into a future rain pass.
