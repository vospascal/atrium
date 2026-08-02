// Shared Material Graph ABI and helpers.
//
// ONE definition, two consumers: this file is concatenated into the DDA source
// (see passes/dda.rs) AND embedded by material_graph.rs as the prefix of every
// standalone generated program, so `naga` can validate a compiled graph without
// the rest of the renderer. The Rust side strips this prefix before injecting a
// function into the DDA source, which is why the two cannot drift.
//
// Host values these helpers read, declared elsewhere in the DDA source and
// stubbed by material_graph.rs for standalone validation:
//   lighting.animation_params  — the S3 clock (world.wgsl)
//   world_events               — the S3 event field (dda.wgsl)
//   brickmap.voxel_size_meters — voxel units to metres (world.wgsl)
//   BRICK_SIZE                 — detail cells per authored block (world.wgsl)
struct GraphMaterial {
    base_color: vec4<f32>,
    roughness: f32,
    emission: vec4<f32>,
    graph_active: bool,
    face_color_active: bool,
    face_roughness_active: bool,
    // S3 — per-pattern-slot gain and drift, in surface-chain order. Identity
    // when the graph connects nothing, so an un-animated material is unchanged.
    animation: PatternAnimation,
}

// Shared procedural helpers used by material graph functions.
fn graph_hash3(point: vec3<f32>) -> f32 {
    return fract(sin(dot(point, vec3<f32>(127.1, 311.7, 74.7))) * 43758.5453);
}

fn graph_value_noise(point: vec3<f32>) -> f32 {
    let cell = floor(point);
    let local = fract(point);
    let blend = local * local * (vec3<f32>(3.0, 3.0, 3.0) - 2.0 * local);
    let n000 = graph_hash3(cell + vec3<f32>(0.0, 0.0, 0.0));
    let n100 = graph_hash3(cell + vec3<f32>(1.0, 0.0, 0.0));
    let n010 = graph_hash3(cell + vec3<f32>(0.0, 1.0, 0.0));
    let n110 = graph_hash3(cell + vec3<f32>(1.0, 1.0, 0.0));
    let n001 = graph_hash3(cell + vec3<f32>(0.0, 0.0, 1.0));
    let n101 = graph_hash3(cell + vec3<f32>(1.0, 0.0, 1.0));
    let n011 = graph_hash3(cell + vec3<f32>(0.0, 1.0, 1.0));
    let n111 = graph_hash3(cell + vec3<f32>(1.0, 1.0, 1.0));
    let x00 = mix(n000, n100, blend.x);
    let x10 = mix(n010, n110, blend.x);
    let x01 = mix(n001, n101, blend.x);
    let x11 = mix(n011, n111, blend.x);
    return mix(mix(x00, x10, blend.y), mix(x01, x11, blend.y), blend.z);
}

fn graph_fbm(point: vec3<f32>, octaves: f32, roughness: f32) -> f32 {
    var total = 0.0;
    var amplitude = 1.0;
    var frequency = 1.0;
    var normalisation = 0.0;
    for (var octave = 0u; octave < 8u; octave = octave + 1u) {
        let enabled = select(0.0, 1.0, f32(octave) < max(octaves, 1.0));
        total = total + graph_value_noise(point * frequency) * amplitude * enabled;
        normalisation = normalisation + amplitude * enabled;
        frequency = frequency * 2.0;
        amplitude = amplitude * clamp(roughness, 0.0, 1.0);
    }
    return select(0.0, total / normalisation, normalisation > 0.0);
}

fn graph_safe_normalize(vector: vec3<f32>) -> vec3<f32> {
    let magnitude = length(vector);
    return select(vec3<f32>(0.0, 1.0, 0.0), vector / magnitude, magnitude > 0.000001);
}

fn graph_face_color(normal: vec3<f32>, base: vec4<f32>, top: vec4<f32>, side: vec4<f32>, bottom: vec4<f32>) -> vec4<f32> {
    if (normal.y > 0.5) { return top; }
    if (normal.y < -0.5) { return bottom; }
    return side;
}

fn graph_face_scalar(normal: vec3<f32>, base: f32, top: f32, side: f32, bottom: f32) -> f32 {
    if (normal.y > 0.5) { return top; }
    if (normal.y < -0.5) { return bottom; }
    return side;
}

// ---- S3: the animation clock -------------------------------------------------
//
// Mirrors src/animation_clock.rs exactly. The clock arrives split into whole
// epochs and a remainder inside one, because a single f32 second count loses
// the fraction an oscillator needs within hours of uptime, and any wrapped
// single clock steps every rate that is not harmonic with the wrap.
const ANIMATION_EPOCH_SECONDS: f32 = 64.0;

// Capacity of the event field. Kept in step with the `array<WorldEvent, 16>`
// binding in dda.wgsl and MAX_WORLD_EVENTS in src/world_event.rs; the array
// size there must stay a literal because this file is concatenated after it.
const MAX_WORLD_EVENTS: u32 = 16u;

// Monotone seconds since start. What `material.time` returns; never steps back.
fn graph_animation_seconds() -> f32 {
    return lighting.animation_params.y * ANIMATION_EPOCH_SECONDS
        + lighting.animation_params.x;
}

// An oscillator's phase in turns, [0, 1).
//
// The epoch term is `fract(rate * EPOCH) * epoch` rather than
// `rate * epoch * EPOCH`: the inner fract is a per-rate constant, so the phase
// is continuous across an epoch boundary instead of stepping there.
fn graph_oscillator_phase(rate_hz: f32) -> f32 {
    let per_epoch = fract(rate_hz * ANIMATION_EPOCH_SECONDS);
    return fract(rate_hz * lighting.animation_params.x
        + per_epoch * lighting.animation_params.y);
}

// How many world events are live. Sensors loop to THIS, never to the array
// capacity, so a world with no entities costs one comparison per sensor.
fn graph_world_event_count() -> u32 {
    return min(u32(max(lighting.animation_params.z, 0.0)), MAX_WORLD_EVENTS);
}

// Speed and two angles to a velocity vector.
//
// Azimuth around the vertical axis, 0 along +X and 90 along +Z; elevation above
// horizontal. The same meaning lighting.rs already gives those words, reused so
// there is one definition of an angle pair rather than two.
fn graph_direction(speed: f32, azimuth_degrees: f32, elevation_degrees: f32) -> vec3<f32> {
    let azimuth = radians(azimuth_degrees);
    let elevation = radians(elevation_degrees);
    let cos_elevation = cos(elevation);
    return vec3<f32>(
        cos_elevation * cos(azimuth) * speed,
        sin(elevation) * speed,
        cos_elevation * sin(azimuth) * speed,
    );
}

// ---- S3: oscillator ----------------------------------------------------------

const GRAPH_WAVE_SINE: u32 = 0u;
const GRAPH_WAVE_TRIANGLE: u32 = 1u;
const GRAPH_WAVE_SAW: u32 = 2u;
const GRAPH_WAVE_PULSE: u32 = 3u;
const GRAPH_WAVE_FLICKER: u32 = 4u;

const GRAPH_SYNC_GLOBAL: u32 = 0u;
const GRAPH_SYNC_PER_VOXEL: u32 = 1u;
const GRAPH_SYNC_PER_FACE: u32 = 2u;
const GRAPH_SYNC_PER_MATERIAL: u32 = 3u;

// Chris Wellons' lowbias32, the same integer hash pattern.wgsl uses — three
// multiplies and three shifts, expressible identically in Rust and WGSL.
fn graph_hash_u32(value: u32) -> u32 {
    var hashed = value;
    hashed = hashed ^ (hashed >> 16u);
    hashed = hashed * 0x7feb352du;
    hashed = hashed ^ (hashed >> 15u);
    hashed = hashed * 0x846ca68bu;
    hashed = hashed ^ (hashed >> 16u);
    return hashed;
}

fn graph_hash_to_unit(value: u32) -> f32 {
    return f32(graph_hash_u32(value) >> 8u) / 16777216.0;
}

// A phase offset in turns, so two blocks of one material either beat together
// or do not.
//
// `position` is in TRAVERSAL VOXEL units (12.5 cm), not metres and not authored
// blocks. `per_voxel` therefore divides by BRICK_SIZE first: hashing the raw
// coordinate would de-sync each sub-voxel detail cell rather than each
// one-metre block, which is not what "per voxel" means to anyone authoring it.
// pattern.wgsl's `pattern_coordinate` makes the same conversion.
fn graph_phase_offset(sync: u32, seed: u32, position: vec3<f32>, normal: vec3<f32>) -> f32 {
    if (sync == GRAPH_SYNC_GLOBAL) {
        return 0.0;
    }
    if (sync == GRAPH_SYNC_PER_MATERIAL) {
        // Golden-ratio conjugate: successive seeds land far apart in phase
        // rather than marching in a visible progression. Written to the same
        // digits as the Rust mirror so both parse to the identical f32.
        return fract(f32(seed) * 0.618034);
    }
    let block = vec3<i32>(floor(position / BRICK_SIZE));
    var mixed = u32(block.x) * 0x27d4eb2du
        ^ u32(block.y) * 0x9e3779b9u
        ^ u32(block.z) * 0x85ebca6bu
        ^ seed * 0xc2b2ae35u;
    if (sync == GRAPH_SYNC_PER_FACE) {
        // The face index 0..5, so a block's top and bottom differ too.
        var face = 0u;
        if (abs(normal.y) > 0.5) {
            face = select(2u, 3u, normal.y > 0.0);
        } else if (abs(normal.x) > 0.5) {
            face = select(0u, 1u, normal.x > 0.0);
        } else {
            face = select(4u, 5u, normal.z > 0.0);
        }
        mixed = mixed ^ (face + 1u) * 0x165667b1u;
    }
    return graph_hash_to_unit(mixed);
}

// One cycle of the wave, normalised to [0, 1] before the low/high remap.
fn graph_wave(wave: u32, phase: f32, duty: f32) -> f32 {
    let cycle = fract(phase);
    if (wave == GRAPH_WAVE_TRIANGLE) {
        return 1.0 - abs(cycle * 2.0 - 1.0);
    }
    if (wave == GRAPH_WAVE_SAW) {
        return cycle;
    }
    if (wave == GRAPH_WAVE_PULSE) {
        // A hard step, not a smoothed one: the interval shape is meant to
        // switch. `duty` is the fraction of the cycle spent high.
        return select(0.0, 1.0, cycle < clamp(duty, 0.0, 1.0));
    }
    if (wave == GRAPH_WAVE_FLICKER) {
        // Sample-and-hold: one random level per cycle, HELD until the next.
        // It snaps, which is what reads as a failing lamp. Interpolated noise
        // over time is just a wobblier sine and is deliberately not offered.
        return graph_hash_to_unit(u32(i32(floor(phase))) ^ 0x9e3779b9u);
    }
    return 0.5 - 0.5 * cos(cycle * 6.283185307179586);
}

fn graph_oscillator(
    wave: u32,
    sync: u32,
    seed: u32,
    rate_hz: f32,
    phase_offset: f32,
    duty: f32,
    low: f32,
    high: f32,
    position: vec3<f32>,
    normal: vec3<f32>,
) -> f32 {
    let sync_offset = graph_phase_offset(sync, seed, position, normal);
    let phase = graph_oscillator_phase(rate_hz) + phase_offset + sync_offset;
    return low + (high - low) * graph_wave(wave, phase, duty);
}

// ---- S3: the event sensor ----------------------------------------------------

const GRAPH_FALLOFF_SMOOTHSTEP: u32 = 0u;
const GRAPH_FALLOFF_LINEAR: u32 = 1u;
const GRAPH_FALLOFF_INVERSE_SQUARE: u32 = 2u;
const GRAPH_FALLOFF_STEP: u32 = 3u;

fn graph_falloff(kind: u32, normalised_distance: f32) -> f32 {
    let t = clamp(normalised_distance, 0.0, 1.0);
    if (kind == GRAPH_FALLOFF_LINEAR) {
        return 1.0 - t;
    }
    if (kind == GRAPH_FALLOFF_INVERSE_SQUARE) {
        // Normalised so it still reaches 0 at the radius, rather than trailing
        // a long invisible tail that never quite ends.
        let falloff = 1.0 / (1.0 + 8.0 * t * t);
        let edge = 1.0 / 9.0;
        return clamp((falloff - edge) / (1.0 - edge), 0.0, 1.0);
    }
    if (kind == GRAPH_FALLOFF_STEP) {
        return select(0.0, 1.0, t < 1.0);
    }
    let smooth_t = 1.0 - t;
    return smooth_t * smooth_t * (3.0 - 2.0 * smooth_t);
}

fn graph_ramp(value: f32, length_seconds: f32) -> f32 {
    if (length_seconds <= 0.0) {
        return select(0.0, 1.0, value >= 0.0);
    }
    return clamp(value / length_seconds, 0.0, 1.0);
}

// The attack/hold/release envelope of one event.
//
// Attack and release are MULTIPLIED rather than switched between, and that is
// what makes an impulse continuous: an event that opens and closes inside one
// frame ramps up and down simultaneously and yields a smooth shortened blip
// instead of a step. Switching on a phase would have produced the step.
fn graph_event_envelope(
    event_index: u32,
    attack_seconds: f32,
    hold_seconds: f32,
    release_seconds: f32,
) -> f32 {
    let event = world_events[event_index];
    let now_remainder = lighting.animation_params.x;
    let now_epoch = lighting.animation_params.y;
    let since_start = (now_epoch - event.started_epoch) * ANIMATION_EPOCH_SECONDS
        + (now_remainder - event.started_remainder_seconds);
    let attack_factor = graph_ramp(since_start, attack_seconds);
    if (event.open > 0.5) {
        return attack_factor;
    }
    let since_end = (now_epoch - event.ended_epoch) * ANIMATION_EPOCH_SECONDS
        + (now_remainder - event.ended_remainder_seconds);
    let release_factor = 1.0 - graph_ramp(since_end - hold_seconds, release_seconds);
    return attack_factor * release_factor;
}

// Sense the world-event field. Returns (signal, nearness, envelope).
//
// ONE winning event supplies all three components. Taking an independent
// maximum per output could report a nearness from one event and an envelope
// from another — a combination that never existed — and would break the
// `signal == nearness * envelope * strength` invariant callers rely on.
//
// `position` arrives in traversal voxel units; event positions are in metres.
fn graph_event_sensor(
    channel: u32,
    radius_meters: f32,
    falloff: u32,
    attack_seconds: f32,
    hold_seconds: f32,
    release_seconds: f32,
    invert: bool,
    position: vec3<f32>,
) -> vec3<f32> {
    let point_meters = position * brickmap.voxel_size_meters;
    let count = graph_world_event_count();
    var best_signal = 0.0;
    var best_nearness = 0.0;
    var best_envelope = 0.0;
    for (var index = 0u; index < count; index = index + 1u) {
        let event = world_events[index];
        if (event.channel != channel) {
            continue;
        }
        // The sensor's own radius intersected with the event's reach, so a
        // large creature is felt further away without re-authoring anything.
        let reach = min(radius_meters, event.radius_meters);
        if (reach <= 0.0) {
            continue;
        }
        // Squared-distance reject before any envelope maths: this loop runs per
        // sensor per shaded hit, including secondary rays.
        let offset = event.position_meters - point_meters;
        let distance_squared = dot(offset, offset);
        if (distance_squared >= reach * reach) {
            continue;
        }
        let nearness = graph_falloff(falloff, sqrt(distance_squared) / reach);
        let envelope = graph_event_envelope(
            index, attack_seconds, hold_seconds, release_seconds);
        let signal = nearness * envelope * clamp(event.strength, 0.0, 1.0);
        if (signal > best_signal) {
            best_signal = signal;
            best_nearness = nearness;
            best_envelope = envelope;
        }
    }
    // Invert applies to the SIGNAL only. Nearness and envelope keep their
    // literal meanings so they stay usable as diagnostics.
    let signal = select(best_signal, 1.0 - best_signal, invert);
    return vec3<f32>(signal, best_nearness, best_envelope);
}
