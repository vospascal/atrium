# Directional Reflections Plan (VbapReflectionsRenderer)

## Problem
Per-source reflections currently reuse the direct signal's VBAP gains (simplified panning).
Each reflection should be panned to its image-source direction instead.

The SourceStage trait's `process_sample() -> f32` is mono — can't output per-channel data.
So reflections must move into the renderer, which has access to both delay buffers and multichannel output.

## Approach: Option A — Renderer-Integrated Reflections

Remove `ReflectionsStage` from VBAP source_stages. Create `VbapReflectionsRenderer` that handles
both direct multichannel panning AND directional reflection panning.

### VBAP pipeline change
```
Before: source_stages: [AirAbsorption, GroundEffect, Reflections, VbapGains]
         renderer: MultichannelRenderer

After:  source_stages: [AirAbsorption, GroundEffect, VbapGains]
         renderer: VbapReflectionsRenderer
```

### VbapReflectionsRenderer structure
```
VbapReflectionsRenderer
  Per-source state (Vec<SourceReflectionState>)
    prev_direct_gains: [f32; MAX_CHANNELS]     // gain ramp for direct signal
    delay_buffer: Box<[f32; 4096]>              // mono delay line
    write_pos: usize
    tap_count: usize
    taps: [TapState; 6]                        // one per wall reflection
      delay_samples: usize
      distance_gain: f32                        // wall_absorption / image_dist
      prev_gains: [f32; MAX_CHANNELS]           // gain ramp for this tap
      target_gains: [f32; MAX_CHANNELS]         // VBAP gains toward image direction
  wet_gain: f32
  wall_absorption: f32
```

### Buffer-rate (per source, per buffer)
1. Compute 6 image sources (mirror source across each wall — same math as ReflectionCore::update)
2. For each valid image: compute VBAP gains via `layout.compute_gains_vbap()` with image position as source
3. Store tap params (delay, distance gain, target channel gains)

### Sample-rate (inner loop)
```
for each frame:
    raw = source.next_sample()
    sample = source_stages.process_sample(raw)  // air absorption, ground
    sample *= gain_modifier

    buffer[write_pos] = sample                  // feed delay line

    // Direct signal: same as MultichannelRenderer
    for ch: out[ch] += sample * lerp(prev_direct, target_direct, t)

    // Reflections: each tap panned independently
    for tap in taps:
        delayed = buffer[write_pos - tap.delay] * tap.distance_gain * wet_gain
        for ch: out[ch] += delayed * lerp(tap.prev_gains[ch], tap.target_gains[ch], t)

    write_pos = (write_pos + 1) & MASK
```

### Cost analysis
- Buffer-rate: +6 VBAP computations per source (tiny — VBAP is a 2x2 matrix solve)
- Sample-rate: +6 delay reads + 6*channels multiply-accumulates per sample
- Profile with `cargo run --features profiler -- --profile perfetto` before and after

### Open design decision: tap gain interpolation
(a) Linearly interpolate all 6 taps' channel gains (click-free but 6x more interpolation)
(b) Snap tap gains immediately (cheaper, could click on fast movement)
(c) Interpolate only direct signal, snap tap gains (compromise — reflections are diffuse, clicks masked)

### Key files
- `src/pipeline/stages/reflections.rs` — current ReflectionCore with image-source math (reuse)
- `src/pipeline/renderers/multichannel.rs` — current MultichannelRenderer (base pattern)
- `src/pipeline/mod.rs:295` — build_vbap() pipeline constructor
- `src/pipeline/renderer.rs` — Renderer trait
- `crates/core/src/speaker.rs:394` — compute_gains_vbap()

### Notes
- Image sources use omnidirectional directivity (reflections scatter)
- ReflectionCore::update() math can be extracted/reused for image source computation
- Don't forget to update build_vbap() to wire the new renderer
- Profile before implementing to establish baseline timing
