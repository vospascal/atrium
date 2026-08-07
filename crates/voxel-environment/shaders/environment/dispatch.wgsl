// The renderer-facing entry points.
//
// These four functions are the whole contract: DDA, water and CAGI call these names and
// nothing else in this module. Which implementation answers them — physical LUT reads,
// celestial presentation, cloud deck, or a blend — is decided here and only here, so a consumer
// never branches on the environment provider.
//
// This file reads nothing but `atmosphere`, which is this crate's own uniform. It used to
// multiply by `lighting.sky_ambient.w` — the RENDERER's uniform — meaning this crate could not
// be compiled without a binding it does not own. Concatenating every shader into one module hid
// that; building the same set as an import graph rejected it as a cycle, which is how it was
// found. The scale now arrives as `EnvironmentRequest::ambient_scale`.
//
// The cloud deck enters in TWO places, deliberately. `sky_color*` is what the camera sees;
// `environment_diffuse_radiance` is what lights a surface. A deck that appeared in only the
// first would be a painting of weather that cast no light — which is the failure this crate
// exists to prevent.

// The hemisphere ambient at a world position.
//
// Takes a POSITION as well as a normal, and that is the contract change clouds forced: once a
// deck exists, "how much sky reaches this surface" is a question about where the surface *is*,
// not only which way it faces. A position-free version could only ever apply cloud cover
// globally, which is the difference between an overcast slider and a cloud shadow crossing a
// field.
fn environment_diffuse_radiance(position_world: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    let up = normalize(vec3<f32>(
        normal.x * 0.35,
        0.35 + max(normal.y, 0.0) * 0.65,
        normal.z * 0.35,
    ));
    // Keep celestial decoration out of diffuse lighting; this is the physical
    // environment contribution shared with CAGI, not camera-only celestial decoration.
    //
    // Sky *and* deck, through the one shared function. Previously this sampled
    // `environment_hillaire_sky(up)` directly, which for flat ground is exactly the zenith — blue
    // at sunset while the warm light sits at the horizon, and blind to the deck entirely.
    let sky = environment_sky_ambient_at(position_world, up);
    // The ground bounce is the C5 aggregate rather than a hardcoded colour. The constant it
    // replaced (`vec3(0.45, 0.36, 0.28)`, scaled by daylight) could not warm at sunset no matter
    // what the terrain did, which is the same class of bug as the zenith sample above. Evaluated
    // in the direction the surface leans: a floor sees the aggregate's average, a ceiling sees
    // more of the ground below it.
    let ground = environment_ground_bounce(normal);
    return (sky * 0.12 + ground) * atmosphere.ambient_scale;
}

fn sky_color(direction: vec3<f32>) -> vec3<f32> {
    let backdrop = environment_sky_radiance(direction);
    if (!cloud_enabled()) {
        return backdrop;
    }
    // Sky rays have no depth limit, so the deck is marched to its own far side.
    let march = cloud_march_view(
        atmosphere.camera_position,
        normalize(direction),
        normalize(atmosphere.active_light_direction),
        1.0e7,
    );
    // The cloud's radiance hazed by its own range, so distant towers wash toward the sky.
    let cloud = cloud_aerial_fade(march.scattering, normalize(direction), march.distance);
    return backdrop * march.transmittance + cloud;
}

fn sky_color_at_distance(direction: vec3<f32>, distance_world: f32) -> vec3<f32> {
    if (distance_world <= 0.0) {
        return sky_color(direction);
    }
    // Near misses retain the Hillaire sky plus celestial sources; farther rays progressively use
    // the aerial-perspective LUT. Distant stars and resolved discs receive the same atmospheric
    // transmittance as the physical sky rather than floating on top of the haze.
    let visual = environment_sky_radiance(direction);
    let aerial = environment_aerial_perspective(direction, distance_world);
    let celestial = environment_celestial_detail(direction);
    let physical = aerial + celestial * environment_aerial_transmittance(direction, distance_world);
    let distance_km = distance_world / atmosphere.from_kilometers_scale;
    let atmosphere_blend = 1.0 - exp(-distance_km * 0.035);
    let sky = mix(visual, physical, clamp(atmosphere_blend, 0.0, 1.0));
    if (!cloud_enabled()) {
        return sky;
    }
    // The deck is marched UNBOUNDED, even though this function takes a distance.
    //
    // `distance_world` here is the tracer's give-up sentinel, not a depth: this function is only
    // ever reached when the ray hit nothing, and a ray that hits something is shaded by the hit
    // path instead. So bounding the deck by it clamps the cloud march to the trace radius, which
    // is *shorter than the cloud base*: at 2048 world units against a base at 1500, entry
    // (1500 - eye) / direction.y exceeded the bound past ~44 degrees off vertical and the march
    // returned empty. No clouds anywhere but a narrow cone straight up — and truncated even there.
    //
    // This used to claim it made cloud occlude a mountain. It cannot: when there is a mountain,
    // `shade_hit` runs and never calls this. The bound only ever removed sky.
    //
    // `distance_world` still bounds the ATMOSPHERE blend above, which is a genuine use of it.
    let march = cloud_march_view(
        atmosphere.camera_position,
        normalize(direction),
        normalize(atmosphere.active_light_direction),
        1.0e7,
    );
    let cloud = cloud_aerial_fade(march.scattering, normalize(direction), march.distance);
    return sky * march.transmittance + cloud;
}

fn ambient_light(position_world: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    return environment_diffuse_radiance(position_world, normal);
}
