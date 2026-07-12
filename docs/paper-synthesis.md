# Research Paper Synthesis (31 papers reviewed)

## Tier 1 — High Impact, Low-Medium Effort
1. **VbapReflectionsRenderer** — 7 independent VBAP inputs (direct + 6 walls). Validated by Yeoward 2021 FullSpat. See directional-reflections-plan.md.
2. **Per-speaker delay compensation** — `delay = (max_dist - speaker_dist) * sr / 343.0`. ~20 lines. Tightens phantom images.
3. **Pre-computed VBAP gain lookup** — 1-degree resolution table. Eliminates 7x matrix solves per source per buffer.
4. **Configurable DBAP rolloff** — `R` parameter: 6dB free-field, 3-5dB reverberant. Single f32.

## Tier 2 — High Impact, Medium Effort
5. **IRCAM Spat 4-segment room model** — direct/early/late/tail as composable stages. Architecture for ray-traced audio.
6. **Bilateral Ambisonics** — Two FOA streams at ear positions. 1st-order bilateral = 41st-order standard quality (Ben-Hur 2020).
7. **AllRAD decoder** — Virtual t-design ring → VBAP to real speakers. Gold-standard for irregular layouts.

## Tier 3 — Medium Impact, Medium-High Effort
8. **Multi-Delay SH reverb** — 4 FOA delay lines, source-count-independent. For reverb tail segment.
9. **Polarity inversion VBAP** — Negative gains extend ~40% beyond speaker boundary. Opt-in.
10. **QuickHull + Ray-Triangle VBAP** — Alternative to current triangle search. More robust for irregular layouts.

## Tier 4 — Future/Research
11. WFS (dense arrays only), 12. BCI (multi-listener), 13. DSHT (Ambisonics transforms), 14. PBAP (physics-based panning)

## Key Insight
Directional early reflections = biggest single perceptual upgrade for amplitude panning (Yeoward 2021, p<0.001).
Bilateral encoding beats higher-order for binaural (Ben-Hur 2020).
