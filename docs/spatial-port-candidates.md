# Port Candidates from the `spatial` Project (TypeScript WebAudio Garden)

First-pass survey of `/Users/pascal.vos/Documents/spatial` — flagging what is worth
re-engineering in atrium. Graded by value for the atrium goal (a living, non-static
acoustic environment) and by effort given what atrium already has.

**Legend**: value ★–★★★ · effort S/M/L · status = where it stands today.

---

## 0. Headline findings

1. **Half the DSP port already happened.** `src/synth/` (noise, wind, rain, rain_v2,
   wave) is a clean Rust port of the AudioWorklet processors — allocation-free,
   tested, implements `SoundSource`, and correctly drops the inline Freeverb in
   favor of the existing FDN. **But nothing uses it live**: only the `analyze_*`
   binaries. The generators cannot appear in a scene, the scene maker, or the
   16-slot RT pool.
2. **Thunder was never ported.** The TS project has two polished thunder generators
   (strike + rumble) that were orphaned even there — complete DSP, no node wiring.
   They are the missing piece of the weather set.
3. **A weather state machine already existed in atrium** — `weather.rs` (346 lines,
   Clear/Rain/Wind/Storm with smooth parameter transitions, rain particles) was
   added in commit `dba6163` and deleted in the 2D migration. It was visual-only,
   but it is a ready blueprint for an *audio-driving* weather system.
4. **The TS project's single biggest lesson**: the things that made it feel alive
   were (a) randomized-interval event scheduling, (b) slow intensity "breathing"
   LFOs, and (c) per-sample stochastic synthesis (gust envelopes, drop atoms).
   All three are cheap to implement in Rust. The elaborate environmental-physics
   model, by contrast, was mostly aspirational — and atrium's physics are already
   better.

---

## 1. Dynamic behavior — the "aliveness" layer  ← the core ask

### 1.1 Scheduled random events ★★★ · S–M
Status TS: fully working for bird calls; designed-but-never-wired for ambient
events (wind gusts, campfire crackle, distant thunder claps). Status atrium: absent.

The recipe from `ScheduledBehaviorComponent` + `CallFrequencyComponent`:
- Per source: draw the next trigger time as `min + rand * (max - min)`
  (uniform random inter-event gap, redrawn after every trigger; bird default 3–8 s).
- On trigger: pick a **random clip from a repertoire** (never the same sound twice
  in a row feel), play one-shot, hold an "active" window, reschedule.
- This uniform-random gap + random repertoire pick is the single biggest
  contributor to the non-mechanical feel. No phrase grammar needed.

Atrium fit: a `ScheduledEvent` behavior on scene sources (main thread), pushing
one-shot plays through the existing SceneEdit channel into the RT pool. The TS
project never finished this for non-bird events — finishing it (thunder claps,
gusts, crackle) is new value, not just a port.

### 1.2 Intensity "breathing" (slow gain LFO) ★★★ · S
Status TS: working (`IntensityVariationProcessor`). Status atrium: absent.

Slow sine LFO on source gain: `gain = min + (max - min) * (sin(phase·2π)·0.5 + 0.5)`
with pattern durations of 15 s (defaults 0.2–0.8) up to 90–120 s for rain beds.
Makes any loop — sampled or synthesized — stop sounding static.
Improvement over TS: use two incommensurate LFO rates or filtered noise instead of
one pure sine so the cycle never audibly repeats (same trick as the FDN delay LFOs).

### 1.3 Species / environment data tables ★★ · S
Status TS: working data model. Status atrium: absent (sources/*.yaml are single-clip).

- Bird species (Dutch garden set: robin, blackbird, great tit, dunnock, blue tit,
  sparrow): per-species song repertoire (multiple clips with semantic labels),
  call-frequency range, volume, attenuation defaults.
- Ambient environments (9 defined): wind-breeze, wind-gusts, rain-drizzle,
  rain-downpour, water-stream, urban-traffic, fire-campfire, thunder-distant,
  insects-cicadas — each = repertoire + event-frequency range + intensity-variation
  block + looping flag.

Atrium fit: extend `sources/*.yaml` from single `path:` to an optional repertoire
list + event-timing block. Pairs directly with 1.1/1.2. The user already prefers
real samples — this multiplies their value.

### 1.4 Moving sources (orbital / path motion) ★★ · S
Status TS: working (`OrbitalMotionComponent`, TestNode only). Status atrium:
position updates already flow live through SceneEdit; no motion generator.

Circular orbit: `angle += 2π / seconds_per_revolution * dt`, radius in meters,
around a center (listener or fixed point). TS defined but never used
'one-time' and 'interval' motion patterns. A bevy-ui system driving source
positions (bird flying an arc, bee circling) would be nearly free and very
audible with VBAP/HRTF. Long-term hook for the two-box/game-engine plan.

### 1.5 Time-of-day activity ★ · S (idea only)
Status TS: `active: dawn|day|dusk|night` metadata on every species/environment —
**never read by any logic**. Cheap to actually implement in atrium as an event-rate
multiplier (dawn chorus, dusk cicadas, night owl). Flag: nice-to-have, not first wave.

---

## 2. Weather system — the glue that ties it together

### 2.1 Global weather state driving everything ★★★ · M
Status TS: plumbing complete (`EnvironmentalContext` observable + `AcousticsEngine`
cache invalidation), but `update()` was **never called at runtime** — weather was
frozen at Netherlands defaults (18 °C, 70 %, 5 m/s). Status atrium: physics
parameters exist (temperature-dependent speed of sound, multi-band air absorption)
but are static per scene; the deleted bevy `weather.rs` had Clear/Rain/Wind/Storm
states with smooth transitions, visual-only.

The idea worth engineering (the TS project's best unrealized design):
one `WeatherState` (wind speed, rain intensity, storm level, temperature, humidity)
that simultaneously drives:
- **Synth parameters**: wind speed → wind generator speed/gustiness; rain
  intensity → drop rate/drop-size mix; storm → thunder event rate. (In the TS wind
  worklet, speed alone crossfades rumble→hiss: `hiss_mix = speed / 25`.)
- **Event scheduling** (1.1): gust bursts and thunder claps become more frequent
  as the storm level rises.
- **Physics**: temperature/humidity → the existing air-absorption and
  speed-of-sound parameters (atrium already computes these — just feed them).
- **Visuals**: resurrect the `weather.rs` transition machinery from git `dba6163`
  so the 2D map (or future 3D view) shows the same state the ears hear.

Smooth parameter transitions over tens of seconds (weather.rs already did this
for visuals) prevent audible steps. This directly answers "static background
breaks the experiment".

### 2.2 Per-sound-type propagation modifiers ★ · S (cherry-pick only)
Status TS: worked at source creation. Humidity factor 0.7–1.3 (peak at 70 %),
temperature 0.9–1.1, wind per-type (birds carry −30 %, rain +40 %, wind +60 %),
habitat lookup tables. The richer legacy model (4-band frequency profiles,
volumetric factor, "advanced" processor) was demo-only and buggy (`^` used as
exponent = XOR bug in the air-absorption formula).

Atrium verdict: **do not port the model** — atrium's ISO-9613-style multi-band air
absorption and per-source distance models are already more correct. Worth
cherry-picking only the *concept* that weather nudges per-source audibility
(e.g. wind extends rain/wind sources' reach, dry air shortens birdsong reach) as
simple multipliers on max-distance/rolloff, driven by 2.1. The TS "volumetric
factor" (rolloff exponent morphing toward line-source, 1/r^n with n in 0.5–2)
is already ~covered by atrium's spread + per-source distance model.

---

## 3. Procedural synthesis (DSP)

### 3.1 Wire existing synth sources into the live engine ★★★ · M
Status: **the blocker for everything above.** Generators exist in `src/synth/`,
implement `SoundSource`, but:
- `sources/*.yaml` only supports `path:` (sample file) — needs a synth variant,
  e.g. `synth: field_wind` + parameter block, so scenes/scene-maker can place them.
- No live parameter editing: synth knobs (wind speed, rain intensity…) need to
  flow through the SceneEdit channel like SPL/spread/directivity already do.
- Scene maker UI needs a synth-source type in the add-source flow + a parameter
  panel per generator.

Per the no-compat principle: make the source definition an enum
(sample | synth), don't bolt a magic string onto `path`.

### 3.2 Port thunder strike + rumble ★★★ · S–M
Status TS: polished DSP, orphaned. Status atrium: not ported.

- **Strike**: ~40 ms crack = half-sine-gated white noise + three decaying
  resonators (500/1700/3000 Hz), then a 0.5–3 s tail = 0.7·brown + 0.3·hiss with
  quadratic fade; distance parameter controls a one-pole LP on the tail
  (far strikes = dull rumble, near = bright crack). Self-terminating one-shot.
- **Rumble**: infra noise (<25 Hz ground shake) + brown body + colored hiss with a
  distance HF shelf (−12 dB at distance 1.0); raised-cosine 10 % fade in/out;
  5–20 s duration. Presets near/mid/far/massive for both.

These are one-shot `SoundSource`s — exactly what the scheduled-event system (1.1)
fires. Wind+rain+thunder+waves = complete weather voice. LFE-relevant: the infra
layer finally gives the .1 channel real program material.

### 3.3 Verify the Rust ports against known TS bugs ★★ · S
The TS versions shipped with wiring bugs the Rust port may or may not have
inherited — worth a one-pass audit + tests (matches the verification-tests
principle):
- **Waves**: in TS, the UI params (period/speed/gustiness) never reached the
  worklet — only `crashProb` was live; the intended mapping was
  `roar_level = speed/50`, `hiss_level = gustiness/10`. The Rust `WaveSource` has
  real fields (checked — it is the fixed interface), but decide whether the
  wind-style speed→mix mapping should exist at the weather layer.
- **Rain v2**: TS declared but never used `bubbleDamping`, `distanceAttenuation`,
  `airAbsorption` (the latter two belong to atrium's pipeline anyway — drop them);
  right-channel EQ skipped the low shelf (L/R mismatch). Check `rain_v2.rs`
  for both.
- Confirm each Rust generator actually consumes every public field it exposes
  (a `params-reach-the-DSP` test per generator).

### 3.4 Wind DSP reference (for the weather mapping) ★ · —
Already ported; recorded here because 2.1 needs the mapping logic:
gust envelope state machine (Rise 35 % / Hold 20 % / Fall 25 % / Rest 20 %,
random cycle duration mean±jitter), turbulence puffs only when envelope > 0.7
(probability ∝ gustiness), `hiss_mix = speed/25` crossfading brown rumble → pink/gray
hiss. Rain v2: physically derived drop atoms — Marshall-Palmer-ish size
distribution, terminal-velocity polynomials, Minnaert bubble resonance
(f₀ = 3240/r_mm Hz) for water surfaces, 15–35-mode "thock" for solid, filtered
noise burst for leaves; deterministic LCG for repeatability.

---

## 4. UI ideas (bevy-ui 2D schematic)

Atrium already has: node dragging, per-source SPL/spread/directivity editing,
gamepad + bindings menu, save/load, telemetry over rtrb.

### 4.1 Live metering ★★★ · M
Master + per-speaker-channel peak/RMS dB meters with activity labels, and a
per-speaker debug readout (spatial gain, final gain, distance, attenuation dB).
The TS channel visualizer was one of its most polished panels. Atrium's telemetry
ring buffer is exactly the transport for this — likely the highest-value UI port
for a 5.1 tool.

### 4.2 Attenuation & range visualization ★★★ · S–M
- Range circles (ref-distance + max-distance rings) shown **only for the selected
  or playing source** — keeps the map clean.
- Distance-model-specific field rendering (linear = evenly spaced rings, inverse =
  radial gradient, exponential = rings bunching near the source) so the chosen
  model is visible at a glance.
- Concentric meter-labeled distance grid around the listener.

### 4.3 Audio-reactive map feedback ★★ · S
Playing sources pulse (radius ×(1 + 0.1·sin t)); speakers draw wave arcs whose
count/intensity scales with current gain; a listener→selected-source line
annotated with live distance (m) and attenuation (dB). Cheap in bevy, big
"the map is alive" payoff — and it doubles as debugging.

### 4.4 Listener hearing cone + preset bundles ★★ · S
The TS radar panel let the user edit the **listener's** directional hearing
(inner/outer angle, side/rear gain, hearing range) separately from source
emission cones, with one-click presets (Narrow Beam / Voice / Ambient / Omni…)
bundling distance-model + cone settings. Atrium edits source directivity but has
no listener-side directional model in the UI. Also port the *idea* of preset
bundles for source settings (bird / ambient / effect) instead of raw-number entry.

### 4.5 Groups / layers with real mute–solo ★★ · M
TS grouped nodes by type with per-layer play/stop and visibility — but visibility
didn't mute audio (footgun) and there was no mute/solo. Port the grouping +
per-group play/stop, and add proper mute/solo (trivially expressible through the
existing active_mask / gain path).

### 4.6 Viewport & editing ergonomics ★ · S
Zoom anchored on the listener (listener stays put on screen while zooming);
zoom-to-fit that includes each source's audible range, not just its position;
smart clone placement (expanding spiral, no overlap, in bounds).

### 4.7 Standalone generator lab ★ · S
TS had a separate debug page: pick one generator, auto-generate sliders from its
parameter metadata, tune in isolation. Atrium's `analyze_synth` binaries are the
offline half; a small interactive lab (TUI crate or a bevy debug panel with
auto-generated sliders from a parameter-descriptor table) would make sound-design
iteration much faster. The parameter-metadata-driven UI idea also feeds 3.1's
per-generator panels.

---

## 5. Anti-patterns observed (do not port)

- **Two parallel environmental models** (rich demo-only one + lean working one)
  — decide once; atrium already decided (its own physics).
- **Declared-but-unwired parameters** everywhere (wave panel dead knobs, rain v2
  unused params, dead radar cache, empty overlay component). Countermeasure:
  end-to-end "knob reaches DSP" tests (3.3).
- **Mixed event systems** (typed bus 1/6 adopted, rest DOM CustomEvents) — atrium's
  single command-channel design already avoids this; keep it that way.
- **Dual attenuation components per node** kept in sync manually — redundant state.
- **Visibility ≠ mute** confusion in layers (4.5).
- Hard-coded 16.67 ms animation delta instead of measured dt.

---

## 6. Suggested first wave (order of attack)

1. **3.1** Synth sources into scene schema + RT pool + scene-maker editing
   (unblocks everything).
2. **3.2** Port thunder strike + rumble (completes the weather voice, feeds LFE).
3. **1.1 + 1.3** Scheduled random events + repertoire tables (birds, gusts,
   crackle, thunder claps — the aliveness core).
4. **2.1** WeatherState driving synth params + event rates + existing physics
   (resurrect `weather.rs` transition machinery from git `dba6163` for the visual
   half).
5. **1.2** Intensity breathing on ambient beds.
6. UI wave: **4.1** meters → **4.2** attenuation viz → **4.3** reactive feedback.
