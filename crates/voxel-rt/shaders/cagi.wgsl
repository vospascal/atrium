// cagi.wgsl — E4 CAGI v0: the cellular automaton that floods the integer light
// volume with sun + sky light. Concatenated after `world.wgsl` (the shared
// traversal core — this pass calls `trace_shadow_visibility` for its per-cell sun
// test) and `cagi_volume.wgsl` (the shared volume bindings + packing).
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
//   pinned sun source     -> kept verbatim (CAGI_SUN_CACHE)
//   otherwise             -> max(propagate(neighbours), inject(sky, sun))
//
// `max` rather than `+` for the emission term so a source cell is CLAMPED FROM
// BELOW to its injected value instead of accumulating without bound — with
// integer channels and a fixed iteration budget, a pinned source is what makes
// the flood converge to a fixed point rather than oscillate.
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
//   1  upward trace: a real vertical shadow ray through the brickmap. Exact per
//      voxel, one traversal per candidate cell.
const CAGI_SKY_TEST_COLUMN_MAX: u32 = 0u;
const CAGI_SKY_TEST_UPWARD_TRACE: u32 = 1u;
const CAGI_SKY_TEST: u32 = 0u;
// CAGI_SUN_CACHE — amortization: a cell that found the sun sets bit 30 of its
// light word, and on every later iteration that bit STANDS IN FOR the shadow ray
// — the cell still propagates and still recomputes its (cheap, six attribute
// loads) bounce colour, it just does not re-trace. Caching the ray RESULT rather
// than the cell's value matters: an earlier version pinned the value itself and
// froze source cells at their injected level, losing the diffusion they should
// also receive (measured: 26% of the frame, mean 0.6/255 and up to 38/255 too
// dark). With the ray result cached the output is bit-identical to re-tracing.
// Correct only because the world and the sun are static between re-floods, which
// is exactly E4's scope; the Rust side clears both buffers — and therefore every
// flag — whenever the sun moves.
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

// Whether this cell sees the sky (CAGI_SKY_TEST).
fn cagi_cell_sees_sky(cell: vec3<i32>) -> bool {
    if (CAGI_SKY_TEST == CAGI_SKY_TEST_COLUMN_MAX) {
        // A cell never straddles two bricks (cell_voxels divides 8), so one
        // brick column covers it. Its bottom voxel row is clear of terrain when
        // the brick row holding it sits above the column's highest occupied
        // brick. The sentinel u32::MAX reads as -1, i.e. "empty column".
        let voxel_min = cell * i32(cagi_volume_meta.cell_voxels);
        let brick = voxel_min / 8;
        let column = u32(brick.x) + u32(brick.z) * brickmap.brick_grid_size.x;
        return brick.y > i32(column_max_brick_y[column]);
    }
    return trace_shadow_visibility(cagi_cell_center_voxels(cell), vec3<f32>(0.0, 1.0, 0.0))
        > 0.5;
}

// Sun bounce injected into an AIR cell that touches sunlit geometry: the mean
// albedo of its solid face neighbours, weighted by each bounce surface's Lambert
// term (the surface normal is the direction from the solid neighbour toward this
// cell), times the sun's radiance and the runtime bounce fraction
// (gi_params.z) — and gated by ONE shadow ray from the cell center.
//
// Returns 0 when the cell touches no sunward-facing solid, so the (comparatively
// expensive) shadow ray is only traced for real candidates: the surface shell of
// the world, a few percent of the volume. `sun_already_found` is the cached ray
// result (CAGI_SUN_CACHE): when set, the trace is skipped and everything else is
// recomputed exactly as if it had returned "lit".
fn cagi_sun_bounce(cell: vec3<i32>, sun_already_found: bool) -> vec3<f32> {
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
    if (!sun_already_found
        && trace_shadow_visibility(cagi_cell_center_voxels(cell), lighting.sun_direction) <= 0.5) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    let sun = lighting.sun_color_intensity.rgb * lighting.sun_color_intensity.w;
    return sun * lighting.gi_params.z * (albedo_sum / lambert_sum);
}

// E5c: the radiance an AIR cell receives from EMISSIVE solid face neighbours,
// injected directly rather than propagated.
//
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
            if ((cagi_cell_attribute(index) & CAGI_CELL_SOLID) == 0u) {
                continue; // not a surface: whatever light it holds the stencil carries
            }
            brightest = max(brightest, cagi_cell_emission(index));
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
            let emission = cagi_cell_emission(index);
            if (any(emission > vec3<f32>(0.0))) {
                light_volume_out[index] = cagi_pack(cagi_quantize(emission));
                return;
            }
        }
        if (!CAGI_TRANSMISSION) {
            light_volume_out[index] = 0u; // absorbed (v0: binary, total)
            return;
        }
        // M2: pass the transmitted fraction on. No emission term — a solid cell
        // sees neither sky nor sun — so a transmitting cell can only ever be
        // DIMMER than what reaches it, and the flood still converges.
        let transmittance = cagi_cell_transmittance(attributes);
        if (transmittance <= 0.0) {
            light_volume_out[index] = 0u;
            return;
        }
        let through = vec3<f32>(cagi_propagate(cell)) * transmittance;
        light_volume_out[index] = cagi_pack(vec3<u32>(through + vec3<f32>(0.5)));
        return;
    }
    // The cached shadow-ray result of this cell (CAGI_SUN_CACHE): "this cell has
    // already been proven to see the sun", which saves the trace but nothing else.
    let sun_already_found =
        CAGI_SUN_CACHE && (light_volume[index] & CAGI_SUN_SOURCE_FLAG) != 0u;

    let propagated = cagi_propagate(cell);

    var emission = vec3<u32>(0u, 0u, 0u);
    if (cagi_cell_sees_sky(cell)) {
        emission = cagi_sky_light();
    }
    // A thin-cover emitter (berries) rarely reaches the quarter-fill solid
    // threshold, so it lands here instead: same injection, clamped from below
    // exactly like the sky term.
    if (CAGI_EMISSIVE) {
        emission = max(emission, cagi_quantize(cagi_cell_emission(index)));
        // E5c: and the emission of any emissive SOLID next door, which the stencil
        // cannot be relied on to carry — a lone bright neighbour survives
        // max-decrement but loses 84% per step under the shipped diffusion rule.
        if (CAGI_EMITTER_BOUNCE) {
            emission = max(emission, cagi_quantize(cagi_emitter_bounce(cell)));
        }
    }
    var is_sun_source = false;
    let bounce = cagi_sun_bounce(cell, sun_already_found);
    if (any(bounce > vec3<f32>(0.0, 0.0, 0.0))) {
        emission = max(emission, cagi_quantize(bounce));
        is_sun_source = true;
    }

    var word = cagi_pack(max(propagated, emission));
    if (CAGI_SUN_CACHE && is_sun_source) {
        word = word | CAGI_SUN_SOURCE_FLAG;
    }
    light_volume_out[index] = word;
}
