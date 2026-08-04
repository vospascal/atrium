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

@group(0) @binding(12) var<storage, read_write> light_volume_out: array<u32>;

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
//   2  diffusion over all 26 neighbours (face 4 / edge 2 / corner 1 weights) —
//      the isotropy contender: 26 loads instead of 6, but the propagation front
//      is a rounded cube instead of an octahedron.
const CAGI_RULE_MAX_DECREMENT: u32 = 0u;
const CAGI_RULE_DIFFUSION_6: u32 = 1u;
const CAGI_RULE_DIFFUSION_26: u32 = 2u;
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

// Light in a neighbour cell as an integer level. One load per neighbour is the
// whole cost of the stencil: no attribute read is needed, because a solid cell's
// own light word already encodes what it contributes — 0 without
// CAGI_TRANSMISSION, and its transmitted fraction with it.
fn cagi_neighbour_light(cell: vec3<i32>) -> vec3<u32> {
    let grid = vec3<i32>(cagi_volume_meta.grid_size);
    if (cell.y >= grid.y) {
        return cagi_sky_light(); // above the clamped volume top: open sky
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

// Rule 2: the same diffusion over all 26 neighbours, weighted 4 / 2 / 1 by
// face / edge / corner adjacency (the integer stand-in for 1/distance), which
// is the isotropy contender against rule 1's axis-aligned stencil.
fn cagi_propagate_diffusion_26(cell: vec3<i32>) -> vec3<u32> {
    var sum = vec3<u32>(0u, 0u, 0u);
    for (var offset_z = -1; offset_z <= 1; offset_z = offset_z + 1) {
        for (var offset_y = -1; offset_y <= 1; offset_y = offset_y + 1) {
            for (var offset_x = -1; offset_x <= 1; offset_x = offset_x + 1) {
                let axis_count = abs(offset_x) + abs(offset_y) + abs(offset_z);
                if (axis_count == 0) {
                    continue;
                }
                // face 1 -> 4, edge 2 -> 2, corner 3 -> 1
                let weight = u32(select(select(1, 2, axis_count == 2), 4, axis_count == 1));
                sum = sum + cagi_neighbour_light(cell + vec3<i32>(offset_x, offset_y, offset_z))
                    * weight;
            }
        }
    }
    return (sum * cagi_volume_meta.diffusion_26_numerator) >> vec3<u32>(CAGI_DIFFUSION_SHIFT);
}

// The configured propagation rule (CAGI_RULE). Shared by the air path and the
// M2 transmitting-solid path so both always agree on the stencil.
fn cagi_propagate(cell: vec3<i32>) -> vec3<u32> {
    if (CAGI_RULE == CAGI_RULE_MAX_DECREMENT) {
        return cagi_propagate_max_decrement(cell);
    } else if (CAGI_RULE == CAGI_RULE_DIFFUSION_6) {
        return cagi_propagate_diffusion_6(cell);
    }
    return cagi_propagate_diffusion_26(cell);
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
    let transmittance = environment_sun_transmittance_at(
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

@compute @workgroup_size(4, 4, 4)
fn cagi_main(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let grid = cagi_volume_meta.grid_size;
    if (any(invocation >= grid)) {
        return;
    }
    let index = cagi_cell_index(invocation);
    let cell = vec3<i32>(invocation);

    let attributes = cagi_cell_attribute(index);
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
        emission = cagi_sky_light();
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
