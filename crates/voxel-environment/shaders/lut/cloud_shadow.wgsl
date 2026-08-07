// Top-down cloud shadow map: transmittance of the deck along the active direct-light axis.
//
// This is the "ground under it" half of cloud lighting, and it cannot come from the sky-view
// LUT: that table is indexed by DIRECTION, and a cloud shadow is a function of POSITION. One
// 512x512 R-channel map, camera-centred and world-anchored, regenerated whenever the deck or
// the sun moves — which is most frames while the wind blows, and is why it is a cheap pass
// rather than a nested march.
//
// Its density comes from `clouds/density.wgsl`, the same fragment the view march uses. If the
// two could disagree, shadows would land where there is no cloud.

@group(0) @binding(1) var cloud_shadow_target: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var cloud_shadow_field: texture_3d<f32>;
@group(0) @binding(3) var cloud_shadow_sampler: sampler;

@compute @workgroup_size(8, 8, 1)
fn cloud_shadow_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let edge = u32(max(atmosphere.cloud_march.z, 1.0));
    if (id.x >= edge || id.y >= edge) {
        return;
    }

    // Fully transparent when the deck is off, so a consumer needs no branch of its own.
    if (!cloud_enabled()) {
        textureStore(cloud_shadow_target, vec2<i32>(id.xy), vec4<f32>(1.0));
        return;
    }

    // Texel centre -> world XZ, centred on the camera column. Matches `cloud_shadow_at`.
    let extent = max(atmosphere.cloud_march.y, 1.0);
    let uv = (vec2<f32>(id.xy) + vec2<f32>(0.5)) / f32(edge);
    let ground_xz = atmosphere.camera_position.xz + (uv - vec2<f32>(0.5)) * extent;

    let light_direction = normalize(atmosphere.active_light_direction);
    // A direct light at or below the horizon casts no usable vertical shadow — the ray would run
    // along the deck for an unbounded distance. Report unshadowed; its illuminance already fades
    // at the horizon.
    if (light_direction.y < 0.02) {
        textureStore(cloud_shadow_target, vec2<i32>(id.xy), vec4<f32>(1.0));
        return;
    }

    // March from the ground point toward the sun, through the deck only.
    let origin = vec3<f32>(ground_xz.x, 0.0, ground_xz.y);
    let span = cloud_deck_span(origin, light_direction);
    if (span.y <= span.x) {
        textureStore(cloud_shadow_target, vec2<i32>(id.xy), vec4<f32>(1.0));
        return;
    }

    // Fixed modest budget: this integrates a single straight line through the deck, and the
    // result is a low-frequency quantity that a half-metre of accuracy cannot show.
    let steps = 24;
    let travel = span.y - span.x;
    let step_length = travel / f32(steps);
    let extinction = max(cloud_extinction(), 0.0001);
    // Linear-density accumulation with the inverted-Beer threshold (Brucks): one add per step,
    // exact early-out.
    let threshold = 4.605 / extinction;

    var accumulated = 0.0;
    for (var step = 0; step < steps; step++) {
        let distance = span.x + (f32(step) + 0.5) * step_length;
        // No erosion detail: this is a low-frequency shadow, and detail is 3 of 4 channel
        // reads for a difference the map cannot resolve.
        accumulated += cloud_density_sampled(
            cloud_shadow_field,
            cloud_shadow_sampler,
            origin + light_direction * distance,
            false,
        ) * step_length;
        if (accumulated > threshold) {
            break;
        }
    }

    let transmittance = exp(-accumulated * extinction);
    textureStore(cloud_shadow_target, vec2<i32>(id.xy), vec4<f32>(transmittance));
}
