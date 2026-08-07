// cagi.wgsl — E4 CAGI v0: the cellular automaton that floods the integer light
// volume with sun + sky light. Concatenated after `world.wgsl` (the shared
// traversal core) and `cagi_volume.wgsl` (the shared volume bindings + packing),
// followed by the LUT-backed environment sampler.
//
// One thread per CELL (workgroup 4x4x4 — cubic, so the six neighbour loads land
// in the same cache lines as the workgroup's own cells). PING-PONG: reads
// binding 11, writes binding 12, and the Rust side swaps the two every iteration
// — the dossier records xima's explicit preference for double buffering over
// in-place updates ("more precise and less chaotic"), and it is also the only
// way this pass stays deterministic: an in-place update would make a cell's
// value depend on the dispatch order of its neighbours.
//
// The update, per cell, in the dossier's reconstructed form
// `L_{t+1}(x) = E(x) + Q( sum_n T(n->x) A(x) L_t(n) )`:
//
//   solid cell            -> 0 (absorbed; the v0 simplification, see below)
//   sun source            -> recomputed from LUT transmittance each iteration
//   otherwise             -> max(propagate(neighbours), inject(sky, sun))
//
// `max` rather than `+` for the emission term keeps a source cell from
// accumulating without bound while the cellular flood converges.
//
// V0 SIMPLIFICATIONS (all deliberate, all in the E4 report):
//   * Absorption is binary. A cell counts as solid at a quarter fill
//     (src/cagi.rs) and then absorbs EVERYTHING; there is no per-material
//     reflectance in the transport. Material colour enters only through the sun
//     bounce's albedo tint.
//   * Emission is sun + sky only. Emissive voxels and the lantern are E5, which
//     needs E2's edit API.
//   * The world is static: no dirty-region re-flood, only the global re-flood
//     the Rust side triggers on a sun change (or a resolution change).
//
// Own binding (group 0):
//  12  storage  light_volume_out — the BACK ping-pong buffer, read_write

@group(G_WORLD) @binding(B_LIGHT_VOLUME_BACK) var<storage, read_write> light_volume_out: array<u32>;

// ---- E4: CAGI propagation levers ---------------------------------------------
// Registry rows with the measured verdicts: `src/variants.rs::REGISTRY`,
// subsystem `Gi`. The two levers the shading pass also needs (CAGI_ENABLED,
// CAGI_SAMPLE_MODE) live in `cagi_volume.wgsl`.
//
// CAGI_RULE — the A/B the dossier asks for:
//   0  max-decrement flood (Minecraft-style): L = max_n(L_n) - attenuation.
//      Sharp, cheapest, and the attenuation is a straight line in radiance, so
//      the falloff reads as a hard-edged reach rather than a soft gradient.
//   1  diffusion over the 6 face neighbours: L = (sum_n L_n) * transmit / 6.
//      The dossier's reconstructed equation, softer and more GI-like.
// (A 26-neighbour diffusion variant was PRUNED 2026-08-07: 2.1-2.7x the cost
// for a mean 0.5/255 look change, and the banks layout owns directionality now.)
const CAGI_RULE_MAX_DECREMENT: u32 = 0u;
const CAGI_RULE_DIFFUSION_6: u32 = 1u;
const CAGI_RULE: u32 = 1u;
// CAGI_SKY_TEST — how a cell decides it can see the sky:
//   0  column max: one load of the per-XZ-brick-column max occupied brick Y
//      (binding 8, the traversal's own column-height data). O(1) and exact for
//      the vertical direction, but quantized to the 8-voxel brick column: a cell
//      beside a tree trunk shares the trunk's column and reads "occluded".
//   1  legacy alias: retained for preset compatibility, but resolved to the
//      same column-height test so CAGI never launches a per-cell ray.
const CAGI_SKY_TEST_COLUMN_MAX: u32 = 0u;
const CAGI_SKY_TEST_UPWARD_TRACE: u32 = 1u;
const CAGI_SKY_TEST: u32 = 0u;
// CAGI_SUN_CACHE — packed-volume compatibility flag. Sun visibility is now an
// atmosphere transmittance LUT lookup, so no per-cell world shadow ray is cached
// or retraced. The Rust side still clears both buffers on a sun move.
const CAGI_SUN_CACHE: bool = true;
// CAGI_TRANSMISSION (M2) — whether a SOLID cell passes its material's
// transmitted fraction on instead of absorbing everything.
//
// E4 v0 wrote 0 into every solid cell, which is right for stone and wrong for
// every leaf: a canopy became a light-proof wall and the ground under a tree
// went black. With this on, a solid cell still receives no emission (it cannot
// see the sky) but forwards `propagate(neighbours) * transmittance`, so light
// seeps through foliage while the canopy keeps casting a real shadow. Stone has
// transmittance 0, so opaque geometry is bit-identical either way.
//
// Off by default: the fix is UNMEASURED, and this repo's rule is that a lever's
// default follows a verdict. Flipping it is the point of the M2 app run.
const CAGI_TRANSMISSION: bool = false;
// CAGI_REFLECTANCE (E5b) — whether a SOLID cell returns the light reaching it
// back into the volume, tinted by its own albedo. This is what colour bleed IS.
//
// The v0 transport had no way to produce bleed at all, and the reason was subtle
// enough to be worth writing down. It was not that the bounce was missing:
// `cagi_sun_bounce` computes a correctly albedo-tinted term and injects it with a
// fraction of 0.35. It was that the bounce is GATED on the receiving air cell
// seeing the sun, so indirect light existed only in the thin shell the sun
// already lit and never appeared in shadow — which is the entire job of GI. In
// L0's corridor the floor five voxels out of the sunbeam rendered BLACK next to a
// brightly lit strip, between 0.8-albedo walls.
//
// This term is ungated and works on PROPAGATED light, which is the difference. It
// is the same move TooManyLimits' published kernel makes:
//
//   output[c][dir] = max(output, (input[c][(dir+2)%4] * color[c]).saturating_sub(1))
//
// — reflect whatever is flowing into a block back out, tinted, with no reference
// to any light source. Because it multiplies INCOMING light rather than injecting
// the surface's own colour, a white ceiling stays white only while white light
// reaches it: red arriving from the floor comes back red. That is the second half
// of the fix, and it falls out of the formulation rather than needing its own
// rule.
//
// Where it lives is dictated by the sampler. `cagi_sample_surface` walks OUT of
// solid cells before reading, and `shade` multiplies the result by the hit
// surface's own albedo — so the volume must carry the EMITTING surface's colour
// and must not pre-multiply by the receiving one. Storing the tinted light in the
// solid cell, where the neighbouring air picks it up as an ordinary propagation
// source, satisfies both and needs no new gather.
//
// Off by default, following the same rule CAGI_TRANSMISSION states above: the
// verdict comes from bench section 15, whose whole purpose is to score this.
const CAGI_REFLECTANCE: bool = false;
// CAGI_EMISSIVE (E5) — whether emissive materials inject their radiance into the
// volume. ON by default and, unusually for this registry, without a measured
// verdict first: the generated world contains no emissive voxel, so until one is
// PLACED this costs a shift, a mask and an indexed uniform load per cell and
// changes nothing. The bench measures a world with no emitters, which is exactly
// why its number cannot justify the default either way.
const CAGI_EMISSIVE: bool = true;
// CAGI_EMITTER_BOUNCE (E5c) — whether an AIR cell reads the emission of its solid
// face neighbours directly, instead of waiting for the propagation stencil to
// carry it.
//
// ON by default, and unlike most rows here the measurement argues FOR the default
// rather than being absent: the diffusion numerator is transmission/6, which is
// near-lossless for a uniform field (6V * 0.94/6 = 0.94V) and keeps 15.7% of a
// lone bright neighbour among five dark ones. Measured on the `wall + glow block`
// prop, the air cell in front of the emitter settles at 152/1023 under
// CAGI_RULE_MAX_DECREMENT (scale-free) and 45/1023 under the SHIPPED
// CAGI_RULE_DIFFUSION_6. So a point light worked only under a rule that is not the
// default, which is the bug this closes: with the bounce on, the neighbour reads
// the emitter's own mean under every rule.
//
// Off restores that rule-dependent behaviour exactly, which is what makes the
// before/after measurable.
const CAGI_EMITTER_BOUNCE: bool = true;
// CAGI_EVENT_LIGHT (S3b) — whether a cell whose material answers the world event
// field modulates the emission it injects, so a surface that lights up when you
// walk toward it also LIGHTS THE ROOM instead of being a decal.
//
// ON by default, and this one comes with an argument rather than a measurement,
// like CAGI_EMISSIVE did: with no event-responsive material in the world every
// cell's response index is 0 and this costs one shift-and-mask per emission
// read, changing nothing. What it costs when a material DOES respond is one
// `world_event_sense` per gated cell per iteration — bounded by the emitter's
// own surface area, not by the grid — and the bench row is the honest place to
// price it.
//
// The reason this works at all, and the reason S3b needed no re-flood: the CA is
// not a one-shot flood. It dispatches `iterations_per_frame` steps EVERY frame,
// and neither propagation rule reads a cell's own previous value, so the field
// both brightens and darkens on its own. A time-varying emitter is therefore
// just an emitter whose value changed; the global re-flood exists only to clear
// the source field when the SUN moves.
const CAGI_EVENT_LIGHT: bool = true;
// Fixed-point shift of both diffusion numerators (see CagiVolumeMeta).
const CAGI_DIFFUSION_SHIFT: u32 = 12u;

// ---- D2: directional-banks transport levers (CAGI_LAYOUT = banks6 only) -------
// docs/cagi-directional-banks-plan.md. All three are UNMEASURED defaults, tuned
// at the D2 app gate and given verdicts by D5's bench. Per-METER like every other
// transport constant here, so the resolution lever cannot change the physics.
//
// Subtractive loss per meter travelled along a bank's own direction. x1m4's rule:
// loss is subtractive, not multiplicative (`max(LOSS, L) - LOSS`), which is what
// lets a beam reach an exact 0 that dirty-region culling (D6) can test.
//
// This is the convergence EPSILON, not the falloff — the reference kernel's
// `saturating_sub(1)`. The D3 gate showed why the hierarchy matters: with the
// subtractive term as the primary loss, radiance decays as a straight line and
// the lit region ends at a hard visible terminator (the same artifact the
// isotropic max-decrement rule's verdict records). The multiplicative
// per-meter transmission below is the falloff; this just trims the tail to an
// exact 0.
const CAGI_BANKS_LOSS_PER_METER: f32 = 1.0;
// The lateral pull's loss, as a multiple of the direct loss. Light travelling +X
// also seeps into the +X banks of side neighbours — the heat-conduction spread —
// but with a steeper falloff, so a beam widens without dissolving.
const CAGI_BANKS_SIDE_LOSS_MULTIPLIER: f32 = 4.0;
// Per-METER fraction of a bank that scatters into the four PERPENDICULAR banks
// each step (a quarter each) — the direction-decay term of x1m4's per-axis
// diffusion, which D2 first shipped without: transport keeps a beam's label
// forever, so lava's upward column stayed "upward light" after wrapping over a
// wall and painted every bottom face behind it.
//
// PERPENDICULAR, deliberately NOT the opposite bank, and this is a measured
// deviation from the literal `mix(lightpy, lightny, x)` reading: opposite-bank
// mixing manufactures backward-travelling light along every beam, which is
// exactly the bank the surface behind a wall samples — the D4 gate showed it
// as light "coming through everywhere". Real air scattering is small-angle:
// sideways is common, full reversal is not. With perpendicular scatter a beam
// still forgets its direction (reversal takes two hops, mix^2 per meter
// squared — negligible), but no wall face ever gets fed directly.
const CAGI_BANKS_DIRECTION_MIX: f32 = 0.08;
// The corner-seal's PARTIAL tier (TML's DIAGONAL_PARTIAL_OCCLUDED_ATTENUATION):
// the fraction of a lateral seep that survives grazing a wall edge — the seep
// into cell C of bank d from lateral neighbour L cuts the diagonal bracketed
// by C-d and L; both solid seals it to ZERO (the classic flood-fill
// wall-join leak), exactly one solid pays this fraction. This is what stops
// the over-the-wall wrap band from re-seeding beams down a wall's shadow face
// at full strength — the D4 gate's "comes through everywhere".
const CAGI_BANKS_SEAL_PARTIAL: f32 = 0.25;
const CAGI_BANKS_SEAL_PARTIAL_NUMERATOR: u32 = u32(CAGI_BANKS_SEAL_PARTIAL * 4096.0);
// CAGI_BANKS_SKY_HORIZONTAL lives in cagi_volume.wgsl since D4: the sampler's
// above-the-volume sky reads must agree with what the CA injects.
// Multiplicative transmission per meter of air, applied to the transport ON TOP
// of the subtractive losses — the reference kernel carries both (its DECAY plus
// DIRECT/SIDE attenuations), and the D3 gate showed why: with subtractive loss
// alone, reach scales LINEARLY with injected energy, so a lava cell at the HDR
// ceiling (level 8184) out-reaches the sky (level ~1023) eight to one and eats
// the scene. Exponential decay caps bright point sources at a sane radius while
// open-air sky light, re-injected at every sky-seeing cell, is untouched.
//
// 0.7 is MEASURED (CPU probe, 2026-08-07, lava-vs-wall scene): the isotropic
// path's 0.884 was calibrated for a rule whose /6 averaging did the real
// attenuation; under max-transport banks it left the shadow behind a 10-cell
// wall at 1/4-1/10 of the lit side. At 0.7 the shadow floor is level <=30
// (a soft rim hugging the wall edges) while the lit side keeps levels
// 500-1800 at 6-7 m; at 0.6 the shadow is exactly 0 but the emitter radius
// visibly halves.
const CAGI_BANKS_TRANSMISSION_PER_METER: f32 = 0.7;
// D3: the propagated bounce's energy fraction, ON TOP of the surface albedo —
// the stand-in for the solid angle a bounce surface subtends (the isotropic
// E5b note records why interreflection must lose energy to geometry, not only
// to albedo: at snow's 0.92 albedo an un-penalized bounce turns a corridor
// into a light pipe). Loop gain = albedo * this, always below one, so the
// reflect-and-return flood contracts to a fixed point.
const CAGI_BANKS_BOUNCE: f32 = 0.5;
// Fixed-point form of the fractions the banks path multiplies with.
const CAGI_BANKS_SKY_HORIZONTAL_NUMERATOR: u32 =
    u32(CAGI_BANKS_SKY_HORIZONTAL * 4096.0);
const CAGI_BANKS_BOUNCE_NUMERATOR: u32 = u32(CAGI_BANKS_BOUNCE * 4096.0);
// 1/6 in the diffusion fixed point: an omnidirectional emitter splits its
// radiance evenly over the six banks, keeping "sum over banks ~= the isotropic
// word" true for every consumer that sums (fog, the interim sampler).
const CAGI_BANKS_SIXTH_NUMERATOR: u32 = 683u;

// Light in a neighbour cell as an integer level. One load per neighbour is the
// whole cost of the stencil: no attribute read is needed, because a solid cell's
// own light word already encodes what it contributes — 0 without
// CAGI_TRANSMISSION, and its transmitted fraction with it.
fn cagi_neighbour_light(cell: vec3<i32>) -> vec3<u32> {
    let grid = vec3<i32>(cagi_volume_meta.grid_size);
    if (cell.y >= grid.y) {
        return cagi_sky_light(cell); // above the clamped volume top: open sky
    }
    if (any(cell < vec3<i32>(0, 0, 0)) || any(cell >= grid)) {
        return vec3<u32>(0u, 0u, 0u);
    }
    return cagi_unpack(light_volume[cagi_cell_index(vec3<u32>(cell))]);
}

// Rule 0: the brightest neighbour minus a fixed attenuation. Deliberately does
// NOT include the cell's own previous value — including it would make the volume
// monotonically non-decreasing, which converges fine but can never DARKEN, so a
// sun change would leave stale light behind forever.
fn cagi_propagate_max_decrement(cell: vec3<i32>) -> vec3<u32> {
    var brightest = vec3<u32>(0u, 0u, 0u);
    brightest = max(brightest, cagi_neighbour_light(cell + vec3<i32>(1, 0, 0)));
    brightest = max(brightest, cagi_neighbour_light(cell - vec3<i32>(1, 0, 0)));
    brightest = max(brightest, cagi_neighbour_light(cell + vec3<i32>(0, 1, 0)));
    brightest = max(brightest, cagi_neighbour_light(cell - vec3<i32>(0, 1, 0)));
    brightest = max(brightest, cagi_neighbour_light(cell + vec3<i32>(0, 0, 1)));
    brightest = max(brightest, cagi_neighbour_light(cell - vec3<i32>(0, 0, 1)));
    let attenuation = vec3<u32>(cagi_volume_meta.attenuation);
    return max(brightest, attenuation) - attenuation;
}

// Rule 1: averaged 6-neighbour diffusion, all in u32. The sum of six 10-bit
// channels is at most 6138 and the numerator is under 2^11, so the product stays
// far inside u32 — no saturation anywhere in the transport.
fn cagi_propagate_diffusion_6(cell: vec3<i32>) -> vec3<u32> {
    var sum = vec3<u32>(0u, 0u, 0u);
    sum = sum + cagi_neighbour_light(cell + vec3<i32>(1, 0, 0));
    sum = sum + cagi_neighbour_light(cell - vec3<i32>(1, 0, 0));
    sum = sum + cagi_neighbour_light(cell + vec3<i32>(0, 1, 0));
    sum = sum + cagi_neighbour_light(cell - vec3<i32>(0, 1, 0));
    sum = sum + cagi_neighbour_light(cell + vec3<i32>(0, 0, 1));
    sum = sum + cagi_neighbour_light(cell - vec3<i32>(0, 0, 1));
    return (sum * cagi_volume_meta.diffusion_numerator) >> vec3<u32>(CAGI_DIFFUSION_SHIFT);
}

// The configured propagation rule (CAGI_RULE). Shared by the air path and the
// M2 transmitting-solid path so both always agree on the stencil.
fn cagi_propagate(cell: vec3<i32>) -> vec3<u32> {
    if (CAGI_RULE == CAGI_RULE_MAX_DECREMENT) {
        return cagi_propagate_max_decrement(cell);
    }
    return cagi_propagate_diffusion_6(cell);
}

// The center of a cell, in voxel units — the origin of its sun / sky rays.
fn cagi_cell_center_voxels(cell: vec3<i32>) -> vec3<f32> {
    return (vec3<f32>(cell) + vec3<f32>(0.5)) * cagi_volume_meta.cell_size_voxels;
}

// S3b: the emission this cell actually injects THIS iteration.
//
// Every read of a cell's emission goes through here — the solid-emitter pin, the
// thin-cover injection and the neighbour bounce — because a surface that lights
// up must do so consistently in all three, or the wall would brighten while the
// air in front of it did not.
//
// `attributes` is passed in rather than re-read: all three call sites already
// hold the word (they had to test CAGI_CELL_SOLID with it), and the emission
// read is the hot part of this pass.
//
// The interpolation is between the material's two ENDPOINTS, which is the
// documented approximation: the CA cannot run a material graph, so a graph of
// arbitrary shape between its sensor and its emission reaches the volume as the
// straight line through resting and triggered. The surface still shades the
// real curve. See `EmissionEventResponse` in src/material_graph.rs.
fn cagi_cell_emission_live(cell_index: u32, attributes: u32, cell: vec3<i32>) -> vec3<f32> {
    let stored = cagi_cell_emission(cell_index);
    if (!CAGI_EVENT_LIGHT) {
        return stored;
    }
    let response_index = cagi_cell_event_response(attributes);
    if (response_index == 0u) {
        return stored;
    }
    let response = cagi_volume_meta.event_responses[response_index];
    // The cell CENTRE, in metres — the same quantity the surface sensor uses,
    // one tier coarser. Sensing at the cell rather than at the voxel is what
    // makes this affordable: 0.5 m of position error against a light whose
    // radius is metres.
    let sensed = world_event_sense(
        cagi_cell_center_voxels(cell) * brickmap.voxel_size_meters,
        u32(response.channel),
        response.radius_meters,
        u32(response.falloff),
        response.attack_seconds,
        response.hold_seconds,
        response.release_seconds,
    );
    return stored * mix(response.resting_scale, response.triggered_scale, vec3<f32>(sensed.x));
}

// Whether this cell sees the sky (CAGI_SKY_TEST).
fn cagi_cell_sees_sky(cell: vec3<i32>) -> bool {
    if (CAGI_SKY_TEST == CAGI_SKY_TEST_COLUMN_MAX
        || CAGI_SKY_TEST == CAGI_SKY_TEST_UPWARD_TRACE) {
        // A cell never straddles two bricks (cell_voxels divides 8), so one
        // brick column covers it. Its bottom voxel row is clear of terrain when
        // the brick row holding it sits above the column's highest occupied
        // brick. The sentinel u32::MAX reads as -1, i.e. "empty column".
        let voxel_min = cell * i32(cagi_volume_meta.cell_voxels);
        let brick = voxel_min / 8;
        let column = u32(brick.x) + u32(brick.z) * brickmap.brick_grid_size.x;
        return brick.y > i32(column_max_brick_y[column]);
    }
    return false;
}

// Sun bounce injected into an AIR cell that touches sunlit geometry: the mean
// albedo of its solid face neighbours, weighted by each bounce surface's Lambert
// term (the surface normal is the direction from the solid neighbour toward this
// cell), times the sun's radiance and the runtime bounce fraction
// (gi_params.z) — and attenuated by the atmosphere transmittance LUT at the
// cell's world position. No world ray is traced here.
//
// Returns 0 when the cell touches no sunward-facing solid, so the (comparatively
// CAGI propagation remains a purely cellular neighbour operation; the cache
// parameter is retained only for packed-volume compatibility.
fn cagi_sun_bounce(cell: vec3<i32>) -> vec3<f32> {
    var albedo_sum = vec3<f32>(0.0, 0.0, 0.0);
    var lambert_sum = 0.0;
    for (var axis = 0u; axis < 3u; axis = axis + 1u) {
        for (var side = 0u; side < 2u; side = side + 1u) {
            var offset = vec3<i32>(0, 0, 0);
            let step = select(-1, 1, side == 0u);
            if (axis == 0u) {
                offset.x = step;
            } else if (axis == 1u) {
                offset.y = step;
            } else {
                offset.z = step;
            }
            let attributes = cagi_attributes_of(cell + offset);
            if ((attributes & CAGI_CELL_SOLID) == 0u) {
                continue;
            }
            // The bounce surface faces from the solid neighbour toward us.
            let normal = -vec3<f32>(offset);
            let lambert = max(dot(normal, lighting.sun_direction), 0.0);
            if (lambert <= 0.0) {
                continue;
            }
            albedo_sum = albedo_sum + cagi_cell_albedo(attributes) * lambert;
            lambert_sum = lambert_sum + lambert;
        }
    }
    if (lambert_sum <= 0.0) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    let cell_world_position = cagi_cell_center_voxels(cell) * brickmap.voxel_size_meters;
    // Atmosphere AND cloud deck. The `_with_clouds` form is the same lookup with the shadow
    // map folded in, so a cloud passing overhead dims what this cell injects — which is what
    // propagates the shadow down through the volume rather than only darkening the surface.
    let transmittance = environment_sun_transmittance_with_clouds(
        cell_world_position,
        lighting.sun_direction,
    );
    let sun = lighting.sun_color_intensity.rgb * lighting.sun_color_intensity.w;
    return sun * transmittance * lighting.gi_params.z * (albedo_sum / lambert_sum);
}

// E5c: the radiance an AIR cell receives from EMISSIVE solid face neighbours,
// injected directly rather than propagated.
//
// E5b, second half: read the REFLECTED radiance of solid face neighbours directly
// instead of waiting for the propagation stencil to carry it.
//
// Necessary for exactly the reason CAGI_EMITTER_BOUNCE is, and the first version
// of this lever proved it by omitting it. Storing `propagate * albedo` in the
// solid cell is the physically clean half, but the air cell then picks it up
// through `cagi_propagate`, whose shipped Diffusion6 numerator is transmission/6
// — near-lossless for a uniform field and a loss of 84% for a LONE bright
// neighbour among five dark ones. Measured on L0's corridor, reflectance stored
// but not read directly moved the ceiling readout from 22.8% to 25.7% of frame:
// real, and far too weak to see.
//
// Reading the neighbour's stored word directly bypasses the averaging entirely.
// The solid cell has already applied its own albedo, so this adds no tint of its
// own — it just refuses to divide the answer by six.
//
// Convergence: the solid cell's value is `propagate(its neighbours) * albedo`,
// and this cell is one of those neighbours, so the round-trip gain is albedo
// times that cell's share of the stencil — at most albedo, which is below one for
// every material. The loop is contracting under every rule.
// The light ARRIVING at a solid cell: one directional transport step, not the
// 6-neighbour average.
//
// Getting this term right took three attempts and both wrong ones are worth
// recording, because they bracket the answer.
//
// `cagi_propagate` (the shipped Diffusion6 average) is a stencil for a cell
// sitting IN a field, and averaging over six directions is correct there. A
// surface is not in a field: its irradiance arrives from the hemisphere in front
// of it, and five of its six neighbours are the opaque shell it belongs to. That
// discards ~84% of the incident light before albedo is applied, and on L0's
// corridor it was the difference between a bleed you can see and a
// two-percentage-point nudge (22.8% -> 25.7% of frame).
//
// The opposite error is just as easy: taking the bare brightest neighbour with no
// falloff at all. With snow at 0.92 albedo the round-trip gain becomes 0.92, the
// room turns into a nearly lossless light pipe, and every pixel in the corridor
// lights up (100% of frame) with almost no falloff over distance. Interreflection
// has to lose energy to the geometry — the solid angle a neighbour subtends —
// not only to the albedo.
//
// `cagi_propagate_max_decrement` is exactly the middle: brightest neighbour minus
// the volume's own tuned attenuation. It is directional rather than averaged, so
// a surface sees the light in front of it, and it carries the same distance
// falloff every other transport path in this volume uses, so a reflection cannot
// travel further than direct light would.
//
// Transmission deliberately keeps `cagi_propagate`: it is a separate lever with
// its own unmeasured M2 verdict, and changing what it computes here would
// silently re-baseline it.
fn cagi_incident(cell: vec3<i32>) -> vec3<u32> {
    return cagi_propagate_max_decrement(cell);
}

fn cagi_reflectance_bounce(cell: vec3<i32>) -> vec3<u32> {
    var brightest = vec3<u32>(0u, 0u, 0u);
    for (var axis = 0u; axis < 3u; axis = axis + 1u) {
        for (var side = 0u; side < 2u; side = side + 1u) {
            var offset = vec3<i32>(0, 0, 0);
            let step = select(-1, 1, side == 0u);
            if (axis == 0u) {
                offset.x = step;
            } else if (axis == 1u) {
                offset.y = step;
            } else {
                offset.z = step;
            }
            let neighbour = cell + offset;
            if ((cagi_attributes_of(neighbour) & CAGI_CELL_SOLID) == 0u) {
                continue;
            }
            brightest = max(brightest, cagi_neighbour_light(neighbour));
        }
    }
    return brightest;
}

// The same 6-neighbour walk as `cagi_sun_bounce`, and deliberately so — an emitter
// is a bounce surface whose radiance happens to be its own instead of the sun's.
// Two differences, both making this the cheaper of the two:
//
// * no shadow ray. The source is the neighbour itself, so there is nothing to
//   occlude — where the sun term has to prove the SUN reaches the cell.
// * no Lambert weighting. The neighbour's stored value is already a mean radiance
//   over its exposed area (E5b), i.e. what leaves that face in every direction, so
//   weighting it by a direction would be double-counting the area term.
//
// Several emissive neighbours take the MAX rather than the sum: the light word is a
// clamped 10-bit channel and the CA composes every source with `max` already (sky,
// sun, the cell's own emission), so summing here would be the one place in the
// transport that could overshoot what a channel can hold.
fn cagi_emitter_bounce(cell: vec3<i32>) -> vec3<f32> {
    var brightest = vec3<f32>(0.0, 0.0, 0.0);
    let grid = vec3<i32>(cagi_volume_meta.grid_size);
    for (var axis = 0u; axis < 3u; axis = axis + 1u) {
        for (var side = 0u; side < 2u; side = side + 1u) {
            var offset = vec3<i32>(0, 0, 0);
            let step = select(-1, 1, side == 0u);
            if (axis == 0u) {
                offset.x = step;
            } else if (axis == 1u) {
                offset.y = step;
            } else {
                offset.z = step;
            }
            let neighbour = cell + offset;
            // A cell outside the grid has no cell_data slot, so this bounds test is
            // the emission read's guard — not merely a correctness nicety like the
            // one in `cagi_attributes_of`, which can afford to answer "solid".
            if (any(neighbour < vec3<i32>(0, 0, 0)) || any(neighbour >= grid)) {
                continue;
            }
            let index = cagi_cell_index(vec3<u32>(neighbour));
            let attributes = cagi_cell_attribute(index);
            if ((attributes & CAGI_CELL_SOLID) == 0u) {
                continue; // not a surface: whatever light it holds the stencil carries
            }
            // The NEIGHBOUR's own gate, not this cell's: the neighbour is the
            // surface that lights up, and its centre is half a cell away.
            brightest = max(brightest, cagi_cell_emission_live(index, attributes, neighbour));
        }
    }
    return brightest;
}

// ---- D2: the directional-banks transport (CAGI_LAYOUT = banks6) ---------------
// docs/cagi-directional-banks-plan.md. Bank d holds light TRAVELLING in
// direction d. Transport per bank: the beam continues from the upstream
// neighbour's same bank (direct), and seeps in from the four lateral
// neighbours' same bank with a steeper loss (the heat-conduction spread). Both
// losses are SUBTRACTIVE — max(L, loss) - loss — per x1m4's rule, composed with
// max like everything else in this volume so the flood converges to a fixed
// point and can reach an exact 0.

// Per-step integer losses (direct, side), derived from the per-meter levers so
// the resolution lever cannot change the reach.
fn cagi_banks_losses() -> vec2<u32> {
    let cell_meters = cagi_volume_meta.cell_size_voxels * brickmap.voxel_size_meters;
    let direct = max(1u, u32(round(CAGI_BANKS_LOSS_PER_METER * cell_meters)));
    let side = max(direct + 1u,
        u32(round(CAGI_BANKS_LOSS_PER_METER * CAGI_BANKS_SIDE_LOSS_MULTIPLIER * cell_meters)));
    return vec2<u32>(direct, side);
}

// The multiplicative per-step transmission, in the diffusion fixed point.
fn cagi_banks_transmission_numerator() -> u32 {
    let cell_meters = cagi_volume_meta.cell_size_voxels * brickmap.voxel_size_meters;
    return u32(pow(CAGI_BANKS_TRANSMISSION_PER_METER, cell_meters) * 4096.0);
}

// The per-step opposing-bank mix, in the diffusion fixed point. Per meter like
// every other transport constant: the per-step retention is
// (1 - mix_per_meter)^cell_meters, so the direction half-life is resolution
// independent.
fn cagi_banks_direction_mix_numerator() -> u32 {
    let cell_meters = cagi_volume_meta.cell_size_voxels * brickmap.voxel_size_meters;
    let retained = pow(1.0 - CAGI_BANKS_DIRECTION_MIX, cell_meters);
    return u32((1.0 - retained) * 4096.0);
}

// Bank `bank` of a neighbour cell. Above the volume's clamped top is open sky:
// the downward bank reads the full sky, the four horizontal banks its horizon
// fraction, and the upward bank nothing — the directional split of what
// `cagi_neighbour_light` answers isotropically. Everywhere else out of grid is
// black, exactly like the isotropic rule.
fn cagi_banks_neighbour_light(cell: vec3<i32>, bank: u32) -> vec3<u32> {
    let grid = vec3<i32>(cagi_volume_meta.grid_size);
    if (cell.y >= grid.y) {
        if (bank == 3u) {
            return cagi_sky_light(cell);
        }
        if (bank != 2u) {
            return (cagi_sky_light(cell) * CAGI_BANKS_SKY_HORIZONTAL_NUMERATOR)
                >> vec3<u32>(CAGI_DIFFUSION_SHIFT);
        }
        return vec3<u32>(0u, 0u, 0u);
    }
    if (any(cell < vec3<i32>(0, 0, 0)) || any(cell >= grid)) {
        return vec3<u32>(0u, 0u, 0u);
    }
    let cell_index = cagi_cell_index(vec3<u32>(cell));
    return cagi_unpack(light_volume[cagi_light_index(cell_index, bank)]);
}

// The banks path of the CA, one cell per call. Writes all six bank words.
fn cagi_banks_main(index: u32, cell: vec3<i32>, attributes: u32) {
    if ((attributes & CAGI_CELL_SOLID) != 0u) {
        // An emissive solid radiates from every face: a sixth per bank keeps the
        // banks' sum equal to the isotropic word the E5 path would have pinned.
        var emission_sixth = vec3<u32>(0u, 0u, 0u);
        if (CAGI_EMISSIVE) {
            let emission = cagi_cell_emission_live(index, attributes, cell);
            if (any(emission > vec3<f32>(0.0))) {
                emission_sixth = (cagi_quantize(emission) * CAGI_BANKS_SIXTH_NUMERATOR)
                    >> vec3<u32>(CAGI_DIFFUSION_SHIFT);
            }
        }
        // D3: what a solid cell hands back to the volume, PER BANK — this is
        // where banks pay off. Its bank d holds light LEAVING it travelling d:
        //
        //   * E5b reflectance — TooManyLimits' kernel, `input[(dir+2)%4] *
        //     color`: the light that ARRIVED travelling d's reverse (read from
        //     the neighbour that light came through, at cell + dir(d), bank
        //     d^1), tinted by the albedo and cut by CAGI_BANKS_BOUNCE. The
        //     direction REVERSAL is what makes a bounce read as a bounce; air
        //     cells pick it up as their ordinary direct-upstream term at full
        //     strength, so no isotropic-style direct-read bypass is needed.
        //     At the volume top this reflects the sky's downward bank upward —
        //     ground bounce, directional, for free.
        //   * M2 transmission — bank d continues THROUGH from the upstream
        //     neighbour at cell - dir(d), scaled by the material's transmitted
        //     fraction (foliage), direction preserved.
        //
        // Combined per bank with max (a photon transmits or reflects, never
        // both) and paying the direct step loss, so a bounce can never outrun
        // direct light. Both factors are below one: the flood still contracts.
        var albedo_numerator = vec3<u32>(0u, 0u, 0u);
        if (CAGI_REFLECTANCE) {
            albedo_numerator = vec3<u32>(
                cagi_cell_albedo(attributes) * f32(CAGI_BANKS_BOUNCE_NUMERATOR));
        }
        var transmit_numerator = 0u;
        if (CAGI_TRANSMISSION) {
            transmit_numerator = u32(cagi_cell_transmittance(attributes) * 4096.0);
        }
        let direct_loss = vec3<u32>(cagi_banks_losses().x);
        for (var bank = 0u; bank < 6u; bank = bank + 1u) {
            var outgoing = vec3<u32>(0u, 0u, 0u);
            if (CAGI_REFLECTANCE && any(albedo_numerator > vec3<u32>(0u))) {
                let arriving = cagi_banks_neighbour_light(
                    cell + cagi_bank_direction(bank), bank ^ 1u);
                outgoing = (arriving * albedo_numerator) >> vec3<u32>(CAGI_DIFFUSION_SHIFT);
            }
            if (CAGI_TRANSMISSION && transmit_numerator > 0u) {
                let through = cagi_banks_neighbour_light(
                    cell - cagi_bank_direction(bank), bank);
                outgoing = max(outgoing,
                    (through * transmit_numerator) >> vec3<u32>(CAGI_DIFFUSION_SHIFT));
            }
            outgoing = max(outgoing, direct_loss) - direct_loss;
            light_volume_out[cagi_light_index(index, bank)] =
                cagi_pack(max(outgoing, emission_sixth));
        }
        return;
    }

    // ---- Injection, gathered per bank before the transport loop ----
    var injected: array<vec3<u32>, 6>;
    for (var bank = 0u; bank < 6u; bank = bank + 1u) {
        injected[bank] = vec3<u32>(0u, 0u, 0u);
    }
    if (cagi_cell_sees_sky(cell)) {
        let sky = cagi_sky_light(cell);
        injected[3u] = sky; // downward
        let horizontal = (sky * CAGI_BANKS_SKY_HORIZONTAL_NUMERATOR)
            >> vec3<u32>(CAGI_DIFFUSION_SHIFT);
        injected[0u] = horizontal;
        injected[1u] = horizontal;
        injected[4u] = horizontal;
        injected[5u] = horizontal;
    }
    // A thin-cover emitter is omnidirectional: a sixth per bank (sum ~= E).
    if (CAGI_EMISSIVE) {
        let own = cagi_quantize(cagi_cell_emission_live(index, attributes, cell));
        if (any(own > vec3<u32>(0u))) {
            let sixth = (own * CAGI_BANKS_SIXTH_NUMERATOR) >> vec3<u32>(CAGI_DIFFUSION_SHIFT);
            for (var bank = 0u; bank < 6u; bank = bank + 1u) {
                injected[bank] = max(injected[bank], sixth);
            }
        }
    }
    // The directional injections: both walk the six solid face neighbours, and
    // both inject into the bank pointing FROM the neighbour INTO this cell —
    // the bounce surface's normal. This is where banks EARN their memory: the
    // sun bounce and an emissive wall light the air directionally instead of
    // dissolving into an average.
    let sun = lighting.sun_color_intensity.rgb * lighting.sun_color_intensity.w;
    var sun_transmittance = vec3<f32>(-1.0, -1.0, -1.0); // lazily fetched once
    let grid = vec3<i32>(cagi_volume_meta.grid_size);
    // One attribute read per face neighbour, shared by this injection walk and
    // the transport loop's corner seal — which previously re-read the same six
    // cells up to 30 times per cell. Out of grid reads as solid, which is what
    // the seal wants; the injection paths below still bounds-check before
    // touching cell_data.
    var neighbour_attribute: array<u32, 6>;
    for (var direction = 0u; direction < 6u; direction = direction + 1u) {
        neighbour_attribute[direction] =
            cagi_attributes_of(cell + cagi_bank_direction(direction));
    }
    for (var bank = 0u; bank < 6u; bank = bank + 1u) {
        let offset = cagi_bank_direction(bank);
        let neighbour = cell + offset;
        if (any(neighbour < vec3<i32>(0, 0, 0)) || any(neighbour >= grid)) {
            continue;
        }
        let neighbour_attributes = neighbour_attribute[bank];
        if ((neighbour_attributes & CAGI_CELL_SOLID) == 0u) {
            continue;
        }
        let neighbour_index = cagi_cell_index(vec3<u32>(neighbour));
        // Light leaves the surface travelling opposite the offset that reached it.
        let into_cell = bank ^ 1u;
        // E5c, directional: the emissive neighbour's radiance enters ONE bank at
        // full strength — the directional version of `cagi_emitter_bounce`.
        if (CAGI_EMISSIVE && CAGI_EMITTER_BOUNCE) {
            let emission = cagi_cell_emission_live(
                neighbour_index, neighbour_attributes, neighbour);
            injected[into_cell] = max(injected[into_cell], cagi_quantize(emission));
        }
        // Sun bounce, directional: same LUT-transmittance injection as the
        // isotropic `cagi_sun_bounce`, but each surface feeds only the bank its
        // normal points along, and no lambert-mean is needed — banks keep the
        // per-surface terms separate by construction.
        let normal = -vec3<f32>(offset);
        let lambert = max(dot(normal, lighting.sun_direction), 0.0);
        if (lambert > 0.0) {
            if (sun_transmittance.x < 0.0) {
                let cell_world_position =
                    cagi_cell_center_voxels(cell) * brickmap.voxel_size_meters;
                sun_transmittance = environment_sun_transmittance_with_clouds(
                    cell_world_position, lighting.sun_direction);
            }
            let bounce = sun * sun_transmittance * lighting.gi_params.z
                * cagi_cell_albedo(neighbour_attributes) * lambert;
            injected[into_cell] = max(injected[into_cell], cagi_quantize(bounce));
        }
    }

    // ---- Transport, per bank ----
    let losses = cagi_banks_losses();
    let transmission = cagi_banks_transmission_numerator();
    var banks: array<vec3<u32>, 6>;
    for (var bank = 0u; bank < 6u; bank = bank + 1u) {
        let direction = cagi_bank_direction(bank);
        let direct = (cagi_banks_neighbour_light(cell - direction, bank) * transmission)
            >> vec3<u32>(CAGI_DIFFUSION_SHIFT);
        // Corner-seal bracket, half one: the cell's own upstream along the
        // beam (the hoisted attribute of the REVERSED direction's neighbour).
        // Solid means this cell sits in the beam's shadow pocket (just behind
        // a wall crest) and a lateral seep would be cutting the corner.
        let upstream_solid = (neighbour_attribute[bank ^ 1u] & CAGI_CELL_SOLID) != 0u;
        var side = vec3<u32>(0u, 0u, 0u);
        for (var lateral = 0u; lateral < 6u; lateral = lateral + 1u) {
            if ((lateral >> 1u) == (bank >> 1u)) {
                continue; // own axis: upstream is the direct term, downstream is behind the beam
            }
            // Bracket half two: the lateral source itself (solid only when its
            // light is a bounce hand-back).
            let lateral_solid = (neighbour_attribute[lateral] & CAGI_CELL_SOLID) != 0u;
            if (upstream_solid && lateral_solid) {
                continue; // sealed corner: light cannot cut the wall join
            }
            var seep = cagi_banks_neighbour_light(cell + cagi_bank_direction(lateral), bank);
            if (upstream_solid || lateral_solid) {
                seep = (seep * CAGI_BANKS_SEAL_PARTIAL_NUMERATOR)
                    >> vec3<u32>(CAGI_DIFFUSION_SHIFT);
            }
            side = max(side, seep);
        }
        side = (side * transmission) >> vec3<u32>(CAGI_DIFFUSION_SHIFT);
        let direct_loss = vec3<u32>(losses.x);
        let side_loss = vec3<u32>(losses.y);
        let propagated = max(
            max(direct, direct_loss) - direct_loss,
            max(side, side_loss) - side_loss,
        );
        banks[bank] = max(propagated, injected[bank]);
    }
    // ---- Direction decay + write ----
    // Each bank keeps (1 - mix) of itself and receives a quarter of the mix
    // fraction from each of its four PERPENDICULAR banks (see the lever note:
    // never the opposite — that feeds the faces behind walls). The perpendicular
    // sum is total - own - opposite. Conservative across the six banks, so the
    // bank SUM (fog, exposure) is untouched.
    let mix_numerator = cagi_banks_direction_mix_numerator();
    let keep_numerator = 4096u - mix_numerator;
    let quarter_mix = mix_numerator >> 2u;
    var total = vec3<u32>(0u, 0u, 0u);
    for (var bank = 0u; bank < 6u; bank = bank + 1u) {
        total = total + banks[bank];
    }
    for (var bank = 0u; bank < 6u; bank = bank + 1u) {
        let perpendicular = total - banks[bank] - banks[bank ^ 1u];
        let mixed = (banks[bank] * keep_numerator + perpendicular * quarter_mix)
            >> vec3<u32>(CAGI_DIFFUSION_SHIFT);
        light_volume_out[cagi_light_index(index, bank)] = cagi_pack(mixed);
    }
}

@compute @workgroup_size(4, 4, 4)
fn cagi_main(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let grid = cagi_volume_meta.grid_size;
    if (any(invocation >= grid)) {
        return;
    }
    let index = cagi_cell_index(invocation);
    let cell = vec3<i32>(invocation);

    let attributes = cagi_cell_attribute(index);
    if (CAGI_LAYOUT == CAGI_LAYOUT_BANKS6) {
        cagi_banks_main(index, cell, attributes);
        return;
    }
    if ((attributes & CAGI_CELL_SOLID) != 0u) {
        // An emissive solid is a SOURCE, not an absorber: it pins its own
        // radiance every iteration so the air cells around it read it as a
        // neighbour and diffuse it outward. Without this a glow block would be
        // lit-looking but light nothing, because the CA only ever reads
        // neighbours' light words.
        if (CAGI_EMISSIVE) {
            let emission = cagi_cell_emission_live(index, attributes, cell);
            if (any(emission > vec3<f32>(0.0))) {
                light_volume_out[index] = cagi_pack(cagi_quantize(emission));
                return;
            }
        }
        if (!CAGI_TRANSMISSION && !CAGI_REFLECTANCE) {
            light_volume_out[index] = 0u; // absorbed (v0: binary, total)
            return;
        }
        // What a solid cell hands back to the volume. Two independent mechanisms,
        // each behind its own lever, both reading the SAME propagated incident
        // light so they cannot disagree about what reached the surface:
        //
        //   * M2 transmission — `incident * transmittance`, light going THROUGH
        //     (foliage). No emission term: a solid cell sees neither sky nor sun.
        //   * E5b reflectance — `incident * albedo`, light coming BACK OFF,
        //     tinted. This is colour bleed.
        //
        // Combined per channel with `max` rather than a sum. A surface cannot both
        // transmit and reflect the same photon, so summing would manufacture
        // energy; and because both factors are below one, `max` keeps the cell
        // strictly DIMMER than what reaches it, which is what makes the flood
        // converge to a fixed point instead of ringing. The reflect-and-return
        // loop gain is albedo times the stencil's single-neighbour share — about
        // 0.8 * 0.157 under the shipped Diffusion6 — so it settles quickly.
        var outgoing = vec3<f32>(0.0, 0.0, 0.0);
        if (CAGI_TRANSMISSION) {
            outgoing = vec3<f32>(cagi_propagate(cell)) * cagi_cell_transmittance(attributes);
        }
        if (CAGI_REFLECTANCE) {
            let incident = vec3<f32>(cagi_incident(cell));
            outgoing = max(outgoing, incident * cagi_cell_albedo(attributes));
        }
        if (all(outgoing <= vec3<f32>(0.0, 0.0, 0.0))) {
            light_volume_out[index] = 0u;
            return;
        }
        light_volume_out[index] = cagi_pack(vec3<u32>(outgoing + vec3<f32>(0.5)));
        return;
    }
    // The source value is recomputed from the LUT and combined with propagated
    // neighbour radiance below.
    let propagated = cagi_propagate(cell);

    var emission = vec3<u32>(0u, 0u, 0u);
    if (cagi_cell_sees_sky(cell)) {
        emission = cagi_sky_light(cell);
    }
    // A thin-cover emitter (berries) rarely reaches the quarter-fill solid
    // threshold, so it lands here instead: same injection, clamped from below
    // exactly like the sky term.
    if (CAGI_EMISSIVE) {
        emission = max(emission, cagi_quantize(cagi_cell_emission_live(index, attributes, cell)));
        // E5c: and the emission of any emissive SOLID next door, which the stencil
        // cannot be relied on to carry — a lone bright neighbour survives
        // max-decrement but loses 84% per step under the shipped diffusion rule.
        if (CAGI_EMITTER_BOUNCE) {
            emission = max(emission, cagi_quantize(cagi_emitter_bounce(cell)));
        }
    }
    // E5b: light coming back off the solid neighbours, already albedo-tinted by
    // them. Placed with the emission terms rather than folded into `propagated`
    // because it must not be divided by the stencil — see the function's note.
    if (CAGI_REFLECTANCE) {
        emission = max(emission, cagi_reflectance_bounce(cell));
    }
    let bounce = cagi_sun_bounce(cell);
    if (any(bounce > vec3<f32>(0.0, 0.0, 0.0))) {
        emission = max(emission, cagi_quantize(bounce));
    }

    light_volume_out[index] = cagi_pack(max(propagated, emission));
}
