// The cloud view march and its scattering.
//
// PHYSICAL, like `hillaire.wgsl` and unlike `appearance.wgsl` — what this returns is shared by
// the camera, the sun-transmittance term CAGI reads, and the diffuse hemisphere. That sharing
// is the crate's invariant: the cloud you see and the cloud that shadows the ground are one
// object.
//
// The density model itself is in `clouds/density.wgsl`, spliced above this and shared with the
// shadow-map compute pass.
//
// Sources: density stack and multiple-scattering octaves from Nubis (Schneider & Vos 2015);
// logarithmic primary distribution from Spiri0; light-march threshold and early-out from
// Brucks; blue-noise offset from Heckel. The sigma_s/sigma_t split is ours, because the
// `Media` transparency class also covers smoke.

// Bind the group(1) density field to the shared core.
fn cloud_density_at(position_world: vec3<f32>, detail: bool) -> f32 {
    return cloud_density_sampled(
        cloud_density_field,
        cloud_field_sampler,
        position_world,
        detail,
    );
}

fn cloud_henyey_greenstein(cos_angle: f32, eccentricity: f32) -> f32 {
    let g_squared = eccentricity * eccentricity;
    let denominator = 1.0 + g_squared - 2.0 * eccentricity * cos_angle;
    return (1.0 - g_squared) / (4.0 * 3.141592653589793 * pow(max(denominator, 0.0001), 1.5));
}

// Dual-lobe phase: a forward lobe for the silver lining looking sunward, a weaker back lobe for
// the glow looking away. One lobe cannot do both, and the pair is most of what makes a cloud
// read as cloud rather than as fog.
fn cloud_primary_phase(cos_angle: f32) -> f32 {
    return cloud_henyey_greenstein(cos_angle, cloud_forward_scatter());
}

fn cloud_secondary_phase(cos_angle: f32) -> f32 {
    let backward = cloud_henyey_greenstein(cos_angle, cloud_back_scatter());
    let isotropic = 1.0 / (4.0 * 3.141592653589793);
    return mix(backward, isotropic, 0.35);
}

// Beer-Powder. Non-physical and aesthetic, as its originators say.
//
// Beer's law alone renders cloud edges DARK, because thin cloud transmits nearly everything and
// so scatters little back. The edge term restores a controlled bright rim. Applied ONLY to
// sun-facing in-scatter — folding it into transmittance is the usual way to get this wrong, and
// yields translucent cloud instead of rimmed cloud. The old implementation mixed in the powder
// value itself, which is below one at the edge and therefore made the rim darker.
fn cloud_powder(density: f32) -> f32 {
    let powder = 1.0 - exp(-density * 4.0);
    let edge = 1.0 - powder;
    return 1.0 + edge * cloud_powder_strength() * 0.75;
}

// A narrow, sun-facing edge is the visual part of Nubis' powder term. Keeping the angular
// response here is important: a density-only powder term brightens every silhouette, including
// the side facing away from the sun, and the cloud loses the cinematic silver-lining read.
fn cloud_sun_rim(density: f32, cos_angle: f32) -> f32 {
    let thin_edge = 1.0 - smoothstep(0.04, 0.55, density);
    let sunward = pow(clamp(cos_angle * 0.5 + 0.5, 0.0, 1.0), 3.0);
    return 1.0 + thin_edge * sunward * cloud_powder_strength() * 1.25;
}

struct CloudLight {
    transmittance: f32,
    multiple_scattering: f32,
}

// Order-1 SH evaluation of the ground-bounce aggregate (C5).
//
// What a cloud gets from the terrain below: sunset warmth, lava, lamps. It cannot come from
// sampling CAGI — that grid stops just above the terrain and returns a constant above it — so
// the renderer reduces its top layer to these four coefficients instead.
fn cloud_ground_bounce(direction: vec3<f32>) -> vec3<f32> {
    let constant_term = atmosphere.ground_bounce_sh[0].rgb;
    let linear_x = atmosphere.ground_bounce_sh[1].rgb;
    let linear_y = atmosphere.ground_bounce_sh[2].rgb;
    let linear_z = atmosphere.ground_bounce_sh[3].rgb;
    // Basis constants are folded into the coefficients by the producer, so this stays a dot
    // product rather than a basis evaluation.
    let linear = linear_x * direction.x + linear_y * direction.y + linear_z * direction.z;
    return max(constant_term + linear, vec3<f32>(0.0));
}

// The ground bounce reaching a SURFACE, from the same C5 aggregate the deck reads.
//
// A diffuse surface integrates the whole lower hemisphere, so the constant term dominates and the
// linear term only tilts the result toward whichever way the surface leans: a floor gets the
// average, a ceiling sees more of the lit ground beneath it. Evaluating at the full `-Y` for every
// surface would over-brighten upward-facing ones by about 45%.
fn environment_ground_bounce(normal: vec3<f32>) -> vec3<f32> {
    let downward = max(-normalize(normal).y, 0.0);
    return cloud_ground_bounce(vec3<f32>(0.0, -downward, 0.0));
}

// Broad, low-frequency cloud shadow used by the envelope lighting term.
//
// The local light march resolves the cloud's detailed silhouette. The shadow map supplies the
// larger-scale answer: is this world column under a broad cloud mass? Evolved uses the same kind
// of long-distance shadow sample for its envelope. It is deliberately only a fill/direct-lobe
// control here; the CAGI and terrain sun path continue to use `cloud_shadow_at` as their physical
// transmittance.
fn cloud_envelope_shadow(position_world: vec3<f32>) -> f32 {
    if (!cloud_enabled()) {
        return 1.0;
    }
    let extent = max(cloud_shadow_extent(), 1.0);
    let centre = atmosphere.camera_position.xz;
    let uv = (position_world.xz - centre) / extent + vec2<f32>(0.5);
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return 1.0;
    }
    return textureSampleLevel(cloud_shadow_map, atmosphere_lut_sampler, uv, 0.0).r;
}

// Evolved's envelope signal: clear, elevated cloud regions receive more sky fill, while a dense
// coarse mass and its broad shadow suppress the fill. This is intentionally separate from the
// detailed density used for extinction, so erosion cannot make ambient light sparkle.
fn cloud_envelope_ambient_factor(position_world: vec3<f32>) -> f32 {
    let coarse_density = clamp(cloud_density_at(position_world, false), 0.0, 1.0);
    let height = smoothstep(0.0, 1.0, cloud_height_fraction(position_world));
    let clear_envelope = pow(1.0 - coarse_density, 0.25);
    let broad_shadow = cloud_envelope_shadow(position_world);
    let shadow_weight = mix(0.55, 0.18, height);
    return clamp(
        clear_envelope * mix(0.35, 1.0, height) * mix(1.0, broad_shadow, shadow_weight),
        0.0,
        1.0,
    );
}

// Three upward taps for sky occlusion, at geometrically spaced distances.
//
// Brucks' ambient term, with two changes. His `SkyColor` is a constant; here it is the real
// sky-view LUT plus the ground-bounce aggregate, so the term is correct at sunset for free. And
// the taps are cone-spread rather than straight up, because pure +Y systematically
// under-occludes overhangs.
//
// No phase function on this term: ambient is isotropic by construction, and phase belongs to
// the directional light alone.
fn cloud_ambient_light(position_world: vec3<f32>) -> vec3<f32> {
    let spread = cloud_thickness() * 0.06;
    var occlusion_density = 0.0;
    occlusion_density += cloud_density_at(
        position_world + vec3<f32>(spread * 0.3, spread, -spread * 0.2),
        false,
    );
    occlusion_density += cloud_density_at(
        position_world + vec3<f32>(-spread * 0.5, spread * 2.0, spread * 0.4),
        false,
    );
    occlusion_density += cloud_density_at(
        position_world + vec3<f32>(spread * 0.2, spread * 4.0, spread * 0.6),
        false,
    );
    let occlusion = exp(-occlusion_density * cloud_ambient_density());

    // The sky is sampled at the ZENITH *and* toward the sun's horizon, then mixed.
    //
    // Zenith alone was a real bug and it is why sunsets looked wrong: at sunset the warm light
    // lives near the HORIZON while the zenith stays blue, so a deck filled from straight up got
    // cool ambient that cancelled the warm direct term. The result was grey cloud over orange
    // terrain — the exact tell that a sky and a world were rendered by two systems.
    //
    // Weighted toward the horizon because a cloud sees far more sky near the horizon than at the
    // zenith, and because that is where the interesting colour is.
    let zenith = environment_hillaire_sky(vec3<f32>(0.0, 1.0, 0.0));
    let sun_direction = normalize(atmosphere.sun_direction);
    let sunward_horizon = normalize(vec3<f32>(sun_direction.x, 0.12, sun_direction.z));
    let horizon = environment_hillaire_sky(sunward_horizon);
    let sky = mix(zenith, horizon, 0.65);

    let ground = cloud_ground_bounce(vec3<f32>(0.0, -1.0, 0.0));
    let height = cloud_height_fraction(position_world);
    let envelope = cloud_envelope_ambient_factor(position_world);
    // Keep a small baseline so dense interiors do not collapse to black, but let the envelope
    // control the cinematic separation between open tops and shaded under-bodies. Ground bounce
    // is strongest below the deck and fades toward the top where the sky is the dominant source.
    let sky_fill = mix(0.28, 1.0, envelope);
    let ground_fill = mix(0.45, 0.12, smoothstep(0.0, 1.0, height));
    return (sky * sky_fill + ground * ground_fill) * occlusion;
}

// Light reaching a point inside the deck from the sun.
//
// Accumulates LINEAR density rather than transmittance, so the inner loop is one add instead of
// two multiplies and a subtract. The threshold is converted to a distance by inverting Beer's
// law (Brucks), so the early-out is exact rather than approximate.
//
// Multiple-scattering octaves (Wrenninge) are applied to the result: without them a thick cloud
// interior goes black, which is the whole difference between cloud and volumetric fog.
fn cloud_sun_light(position_world: vec3<f32>, sun_direction: vec3<f32>) -> CloudLight {
    let taps = cloud_light_steps();
    // Step by the MEAN FREE PATH, not by the deck's thickness.
    //
    // `thickness / taps * 0.9` is 150 m at the shipped 1000 m deck and 6 taps. One tap of dense
    // cloud at that length is an optical depth of 0.08 * 150 = 12, i.e. transmittance 6e-6 — so the
    // first tap saturated and the 4.605/extinction early-out tripped immediately. Measured: the
    // MEDIAN number of taps actually used was 1 of 6, and the resulting sun term was capped at
    // 1-4% of the sun at EVERY elevation. The warm direct light that colours a sunset cloud was
    // effectively absent, which is why only the (neutral) ambient showed.
    //
    // `1 / extinction` is the distance over which transmittance falls to 1/e — 12.5 m at 0.08 — so
    // the taps land inside the cloud's own falloff instead of past it. The deck-relative term stays
    // as an upper bound for very thin or very transparent decks.
    let step_length = min(
        cloud_thickness() / f32(max(taps, 1)) * 0.9,
        1.0 / max(cloud_extinction(), 0.0001),
    );
    let extinction = max(cloud_extinction(), 0.0001);
    // -log(0.01) / extinction: the accumulated linear density at which transmittance falls
    // below 1%. Past that, remaining taps cannot change the pixel.
    let threshold = 4.605 / extinction;

    var accumulated = 0.0;
    for (var tap = 0; tap < taps; tap++) {
        // Widening cone: a march down a single ray misses the neighbourhood that actually
        // lights the point, and a cone approximates it for no extra taps.
        let cone = f32(tap) / f32(max(taps, 1));
        let spread = vec3<f32>(cone * 0.35, 0.0, cone * -0.25) * step_length;
        // Midpoint integration matters at a silhouette: starting a full step away skips the
        // short, bright segment immediately outside the boundary and turns a silver lining grey.
        let sample_position = position_world
            + sun_direction * ((f32(tap) + 0.5) * step_length)
            + spread;
        // Leaving the deck vertically means nothing further can occlude.
        if (sample_position.y < cloud_bottom() || sample_position.y > cloud_top()) {
            break;
        }
        accumulated += cloud_density_sampled(
            cloud_density_field,
            cloud_field_sampler,
            sample_position,
            false,
        ) * step_length;
        if (accumulated > threshold) {
            break;
        }
    }

    let optical_depth = accumulated * extinction;
    let transmittance = exp(-optical_depth);
    // Evolved separates direct scattering into primary transmittance and a secondary lobe. The
    // two attenuated octaves below approximate that secondary term and are normalised so they
    // contribute nothing when the point is outside the cloud.
    let secondary_raw = 0.5 * exp(-optical_depth * 0.55)
        + 0.25 * exp(-optical_depth * 0.3025);
    let multiple_scattering = max(secondary_raw - 0.75 * transmittance, 0.0) / 0.75;
    return CloudLight(transmittance, multiple_scattering);
}

struct CloudMarch {
    // In-scattered radiance along the marched segment.
    scattering: vec3<f32>,
    // Transmittance through the deck: 1 clear, 0 fully opaque.
    transmittance: f32,
    // Opacity-weighted mean distance to the cloud, in world units.
    //
    // Carried out of the march so the caller can apply aerial perspective. Without it every
    // cloud is equally crisp regardless of range, and a deck stretching to the horizon loses the
    // depth cue that makes a real cloudscape read as deep rather than as a wall — the distant
    // towers in a reference photo are always hazed toward the sky colour.
    distance: f32,
}

// Hashed per-pixel offset to break the banding low step counts produce.
//
// Golden-ratio increment (Heitz & Belcour) rather than the reference's `/sqrt(0.5)` — it
// decorrelates better. The frame index is not available to this crate, so this is spatial only;
// a temporal term belongs to the renderer, as a lever.
fn cloud_dither(position_world: vec3<f32>) -> f32 {
    let hashed = fract(
        sin(dot(position_world, vec3<f32>(12.9898, 78.233, 37.719))) * 43758.5453,
    );
    return fract(hashed + 0.6180339887);
}

// The longest a single primary step may be, in world units.
//
// Set by the noise, not by taste: the finest base octave has ~69.7 m features, so 35 m is its
// Nyquist limit. See the note at the step computation for what exceeding it looked like.
const CLOUD_MAX_STEP_WORLD: f32 = 35.0;

// March the deck along a view ray.
//
// `max_distance` is where the ray stops — a real scene depth, or a large number for sky.
// Passing the true depth is what makes clouds composite against terrain instead of drawing over
// it, and it is the one thing none of the sky-only references handle.
fn cloud_march_view(
    origin: vec3<f32>,
    direction: vec3<f32>,
    sun_direction: vec3<f32>,
    max_distance: f32,
) -> CloudMarch {
    var result: CloudMarch;
    result.scattering = vec3<f32>(0.0);
    result.transmittance = 1.0;
    result.distance = 0.0;
    if (!cloud_enabled()) {
        return result;
    }
    var opacity_weight = 0.0;

    let span = cloud_deck_span(origin, direction);
    let entry = span.x;
    let exit = min(span.y, max_distance);
    if (exit <= entry) {
        return result;
    }

    let steps = cloud_primary_steps();
    let extinction = max(cloud_extinction(), 0.0001);
    let scattering_coefficient = extinction * cloud_albedo();
    let cos_angle = dot(normalize(direction), normalize(sun_direction));
    let primary_phase = cloud_primary_phase(cos_angle);
    let secondary_phase = cloud_secondary_phase(cos_angle);
    let sun_radiance = atmosphere.active_light_illuminance;

    let travel = exit - entry;
    let dither = cloud_dither(origin + direction * entry);
    var previous_distance = entry;

    for (var step = 0; step < steps; step++) {
        // Logarithmic distribution (Spiri0): more samples near, fewer far. A uniform step
        // spends most of its budget on distant deck it cannot resolve. Applied by warping the
        // parameter rather than adapting the step, so the loop stays branch-free.
        let linear_t = (f32(step) + dither) / f32(steps);
        let warped = (exp(linear_t * 2.2) - 1.0) / (exp(2.2) - 1.0);
        // CAPPED IN WORLD UNITS, and this is what removes the horizon band.
        //
        // The deck is an infinite flat slab, so travel inside it explodes toward the horizon:
        // 1000 m looking up, 2924 m at 20 degrees, 11 474 m at 5, 57 299 m at 1. Spreading a fixed
        // 48 steps over that gave a LAST STEP of 0.0515 * travel — 2887 m at 1 degree, which is
        // 1.01 whole base-noise tile periods in a single step. Measured, the lag-1 autocorrelation
        // along a screen row collapsed from +0.945 at 30 degrees to -0.003 at 1 degree: adjacent
        // pixels became statistically independent. That is per-pixel aliasing, and it is exactly
        // the dense speckled band above the horizon. A LOWER base frequency would make it worse,
        // not better, which is why this is capped here rather than retuned there.
        //
        // 35 m is the Nyquist limit of the finest base octave (69.7 m features). Above ~20 degrees
        // the cap costs nothing — the log distribution already asks for shorter steps than this, and
        // 48 x 35 m = 1680 m still crosses the whole 1000 m deck. Below that the ray stops partway
        // through the slab, which is correct: by 1680 m of cloud the optical depth is far past the
        // transmittance early-out, so what is dropped could not have been visible.
        let target_distance = entry + travel * warped;
        let distance = min(target_distance, previous_distance + CLOUD_MAX_STEP_WORLD);
        let step_length = max(distance - previous_distance, 0.0001);
        previous_distance = distance;

        let position = origin + direction * distance;
        let density = cloud_density_at(position, true);
        if (density <= 0.001) {
            continue;
        }

        let sun_light = cloud_sun_light(position, sun_direction);
        let broad_shadow = cloud_envelope_shadow(position);
        // The local march owns the sharp silhouette. The broad shadow only modulates the
        // secondary/direct-fill lobe, matching Evolved's envelope lighting without double
        // attenuating the primary silver lining.
        let envelope_secondary = sun_light.multiple_scattering
            * mix(1.0, broad_shadow, 0.35)
            * secondary_phase;
        let direct = sun_radiance
            * (sun_light.transmittance * primary_phase
                + envelope_secondary)
            * cloud_powder(density)
            * cloud_sun_rim(density, cos_angle);
        let ambient = cloud_ambient_light(position);
        // sigma_s for in-scatter, sigma_t for extinction. Nearly equal for cloud
        // (albedo ~0.999) but NOT for smoke, which is why they are separate fields.
        let sample_extinction = max(density * extinction, 1.0e-6);
        let sample_scattering = density * scattering_coefficient;
        let step_transmittance = exp(-sample_extinction * step_length);

        // ENERGY-CONSERVING integration (Hillaire, Frostbite) — the analytic integral of
        // in-scatter across the step, not a first-order sample of it.
        //
        // This was `(direct + ambient) * sigma_s * density * step_length`, which is the
        // differential form and is only valid while per-step optical depth is much less than 1.
        // Measured, it is **1.6 overhead and about 33 near the horizon**, so it over-integrated
        // without bound: the same cloud came out at 0.99 radiance looking up and **3.72 looking at
        // the horizon**, and fell 1.18 -> 0.85 as the step count went 24 -> 192. Brightness was
        // being decided by how far the ray happened to travel rather than by the light, which is
        // why the deck read as a bright band toward the horizon that changed with view direction.
        //
        // `(1 - exp(-sigma_t * ds)) / sigma_t` is bounded by `1 / sigma_t`, so the result is
        // bounded by the albedo and cannot exceed the incoming radiance no matter how long the
        // step. Measured invariant: 0.80 at every step count and both travel distances.
        let integrated = sample_scattering * (1.0 - step_transmittance) / sample_extinction;
        result.scattering += result.transmittance * (direct + ambient) * integrated;
        // Weight the distance by how much this sample actually contributes to the pixel, so the
        // recorded range is where the cloud VISIBLY is rather than where the ray entered the deck.
        let contribution = result.transmittance * density * step_length;
        result.distance += distance * contribution;
        opacity_weight += contribution;
        // Transmittance uses the same per-step value the in-scatter was integrated against, so the
        // two cannot disagree. The exp form is step-size invariant; the comment here used to claim
        // that for the whole loop, but it was only ever true of this line.
        result.transmittance *= step_transmittance;
        if (result.transmittance < 0.01) {
            result.transmittance = 0.0;
            break;
        }
    }

    if (opacity_weight > 0.0) {
        result.distance = result.distance / opacity_weight;
    } else {
        result.distance = entry;
    }
    return result;
}

// Fade a cloud's own radiance into the atmosphere over distance.
//
// The cloud is treated as a surface at its opacity-weighted mean range and pushed through the same
// aerial-perspective LUT the terrain uses, so a tower 30 km out washes toward the sky colour while
// one overhead stays crisp. Without this every cloud is equally saturated at every range, and a
// deck reaching the horizon reads as a wall rather than as depth — in any reference cloudscape the
// distant towers are visibly hazed.
//
// Uses the SAME blend curve as `sky_color_at_distance`, so a cloud and the terrain behind it fade
// at one rate instead of two.
fn cloud_aerial_fade(radiance: vec3<f32>, direction: vec3<f32>, distance_world: f32) -> vec3<f32> {
    if (distance_world <= 0.0) {
        return radiance;
    }
    let distance_km = distance_world / atmosphere.from_kilometers_scale;
    let haze = clamp(1.0 - exp(-distance_km * 0.035), 0.0, 1.0);
    let aerial = environment_aerial_perspective(direction, distance_world);
    return mix(radiance, aerial, haze);
}

// Transmittance of the deck along the active direct-light axis for a world point, from the shadow map.
//
// World-anchored and camera-centred: the extent bounds how far a cloud shadow can be *seen*,
// and outside it the lookup returns unshadowed — which reads as distant sunlit ground rather
// than as a visible boundary.
fn cloud_shadow_at(position_world: vec3<f32>) -> f32 {
    if (!cloud_enabled()) {
        return 1.0;
    }
    let extent = max(cloud_shadow_extent(), 1.0);
    let centre = atmosphere.camera_position.xz;
    let uv = (position_world.xz - centre) / extent + vec2<f32>(0.5);
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return 1.0;
    }
    return textureSampleLevel(cloud_shadow_map, atmosphere_lut_sampler, uv, 0.0).r;
}

// Share of the light a deck removes from the direct beam that leaves its UNDERSIDE.
//
// Cloud has a single-scatter albedo of ~0.999: a deck is a diffuser, not an absorber. What it
// takes out of the beam is not lost, it is redirected — which is why an overcast day is bright
// but directionless rather than dark. Without this term, adding cloud attenuation to the sky
// would make an overcast sky darker than a clear one, and that is simply not what weather does.
const CLOUD_UNDERSIDE_SHARE: f32 = 0.34;

// How much of the sky a full deck occludes, at coverage 1.
//
// Not 1.0: even under thick overcast a surface still sees light arriving around the deck's edges
// and through its thin patches, and the shadow map is one transmittance sample rather than a
// hemisphere integral.
const CLOUD_SKY_OCCLUSION: f32 = 0.8;

// The downward sky radiance arriving at a world position, sunset-correct and cloud-aware.
//
// THE shared answer to "what light comes from above", and deliberately one function: it is read
// by the DDA hemisphere ambient (`environment_diffuse_radiance`) and by CAGI's sky injection
// (`cagi_sky_radiance`). Those two used to each sample `environment_hillaire_sky(vec3(0,1,0))`
// independently — straight up, so blue at sunset while the warm light sits at the horizon, and
// with no knowledge of the deck at all. An overcast sky injected full clear-sky radiance into the
// light volume.
//
// The non-monotonic behaviour of cloud ambient falls out of the physics here rather than being an
// authored curve: partial cloud replaces some directional sun with a bright diffuse underside, so
// scattered skies read BRIGHTER than clear ones, and only heavy overcast is net darker. An earlier
// version of this crate had that curve on the CPU (`CloudSettings::ambient_dimming`) and never
// applied it; deriving it here means it cannot be forgotten or double-counted.
fn environment_sky_ambient_at(position_world: vec3<f32>, up: vec3<f32>) -> vec3<f32> {
    // Zenith blended toward the sunward horizon. A surface sees more sky low down than at the
    // zenith, and low down is where the interesting colour is.
    let zenith = environment_hillaire_sky(normalize(up));
    let sun_direction = normalize(atmosphere.sun_direction);
    let sunward_horizon = normalize(vec3<f32>(sun_direction.x, 0.12, sun_direction.z));
    let sky = mix(zenith, environment_hillaire_sky(sunward_horizon), 0.4);
    if (!cloud_enabled()) {
        return sky;
    }
    let transmittance = cloud_shadow_at(position_world);
    let occlusion = mix(1.0, transmittance, cloud_coverage() * CLOUD_SKY_OCCLUSION);
    let underside = atmosphere.active_light_illuminance
        * (1.0 - transmittance)
        * cloud_coverage()
        * CLOUD_UNDERSIDE_SHARE;
    return sky * occlusion + underside;
}

// Atmospheric transmittance TIMES the cloud deck's.
//
// The C3 seam, and it needed no new entry point: DDA's direct sun term and CAGI's injection
// already call `environment_sun_transmittance_at`, so multiplying the deck in here gives both
// passes cloud shadows with no call-site change. One scalar covers all three channels because
// the deck is grey — Mie scattering is wavelength-neutral, as Brucks notes.
//
// Lives in this file rather than beside the function it wraps because WGSL has no forward
// declarations and `cloud_shadow_at` is spliced after `hillaire.wgsl`.
fn environment_sun_transmittance_with_clouds(
    position_world: vec3<f32>,
    direction: vec3<f32>,
) -> vec3<f32> {
    return environment_sun_transmittance_at(position_world, direction)
        * cloud_shadow_at(position_world);
}
