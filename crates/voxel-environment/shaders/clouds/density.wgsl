// The cloud density model, shared by the sampling module and the shadow-map pass.
//
// This fragment is spliced into BOTH WGSL modules, which is why it declares no bindings of its
// own: the density field lives at `@group(1)` for the sampler and `@group(0)` for the compute
// pass, so it arrives as a function parameter instead. The uniform *is* read directly, because
// both modules declare `atmosphere` — at their own binding — with the same layout.
//
// One density model, one file. If the deck the camera sees and the deck the shadow map
// integrates could disagree, cloud shadows would land where there is no cloud.

fn cloud_coverage() -> f32 { return atmosphere.cloud_shape.x; }
fn cloud_precipitation() -> f32 { return atmosphere.cloud_weather.y; }
fn cloud_extinction() -> f32 {
    // Rain-bearing cells absorb more light without changing the authored silhouette. This is
    // the useful part of the Nubis weather-map G channel until a simulated rain field exists.
    return atmosphere.cloud_shape.y * (1.0 + 0.75 * cloud_precipitation());
}
fn cloud_albedo() -> f32 { return atmosphere.cloud_shape.z; }
fn cloud_bottom() -> f32 { return atmosphere.cloud_shape.w; }
fn cloud_thickness() -> f32 { return atmosphere.cloud_detail.x; }
fn cloud_top() -> f32 { return cloud_bottom() + cloud_thickness(); }
fn cloud_type_blend() -> f32 { return atmosphere.cloud_detail.y; }
fn cloud_detail_strength() -> f32 { return atmosphere.cloud_detail.z; }
fn cloud_ambient_density() -> f32 { return atmosphere.cloud_detail.w; }
fn cloud_powder_strength() -> f32 { return atmosphere.cloud_scatter.x; }
fn cloud_forward_scatter() -> f32 { return atmosphere.cloud_scatter.y; }
fn cloud_back_scatter() -> f32 { return atmosphere.cloud_scatter.z; }
fn cloud_wind_offset() -> vec3<f32> { return atmosphere.cloud_wind.xyz; }
fn cloud_primary_steps() -> i32 { return i32(atmosphere.cloud_wind.w); }
fn cloud_light_steps() -> i32 { return i32(atmosphere.cloud_march.x); }
fn cloud_shadow_extent() -> f32 { return atmosphere.cloud_march.y; }
fn cloud_density_scale() -> f32 { return atmosphere.cloud_scatter.w; }
fn cloud_weather_variation() -> f32 { return atmosphere.cloud_weather.x; }

fn cloud_enabled() -> bool {
    return cloud_primary_steps() > 0 && cloud_coverage() > 0.0 && cloud_thickness() > 0.0;
}

fn cloud_remap(value: f32, from_low: f32, from_high: f32, to_low: f32, to_high: f32) -> f32 {
    let span = max(from_high - from_low, 0.0001);
    return to_low + (value - from_low) / span * (to_high - to_low);
}

fn cloud_value_remap(value: f32, from_low: f32, from_high: f32, to_low: f32, to_high: f32) -> f32 {
    return clamp(cloud_remap(value, from_low, from_high, to_low, to_high), 0.0, 1.0);
}

// Evolved's sky model has two explicit world domains: a 16 km x 16 km NDF and a tileable
// up-res noise volume. Keep their conversions named so the macro weather scale cannot silently
// become the repeat period of the 128^3 erosion field.
const CLOUD_NDF_EXTENT_WORLD: f32 = 16384.0;
const CLOUD_BASE_SAMPLE_SCALE: f32 = 0.00035;
const CLOUD_DETAIL_SAMPLE_SCALE: f32 = 0.00234375;
const CLOUD_NOISE_MIP_DISTANCE_SCALE: f32 = 0.004;
const CLOUD_NOISE_MIP_MAX_LEVEL: f32 = 5.0;

fn cloud_noise_mip_level(distance_world: f32) -> f32 {
    // Evolved's log-distance mip rule: retain close breakup, then progressively box-filter the
    // 128^3 field so the far sky sees cloud masses instead of individual erosion voxels.
    return clamp(
        log2(1.0 + abs(distance_world) * CLOUD_NOISE_MIP_DISTANCE_SCALE),
        0.0,
        CLOUD_NOISE_MIP_MAX_LEVEL,
    );
}

fn cloud_ndf_at(position_world: vec3<f32>, field_sampler: sampler) -> vec4<f32> {
    let advected = position_world + cloud_wind_offset();
    let uv = fract(advected.xz / CLOUD_NDF_EXTENT_WORLD + vec2<f32>(0.5));
    return textureSampleLevel(cloud_ndf_field, field_sampler, uv, 0.0);
}

// Coverage AT a position, rather than one number for the whole sky.
//
// This is the piece whose absence made the deck read as a single continuous mass. Nubis is explicit
// that "cloud coverage and cloud type are a FUNCTION of our weather system" — a 2D map over the
// world, not a scalar. With a uniform scalar there are no larger governing shapes, so the 3D noise
// modulates an unbroken slab and every direction looks equally cloudy; the paper makes the same
// criticism of naive fBM.
//
// The R channel is the NDF's regional coverage/layout signal. The procedural NDF pass supplies it
// today; an authored NDF upload can replace that texture without changing this density or lighting
// path. The deck advects with the wind because the lookup is offset by the same wind history.
fn cloud_coverage_at(
    field: texture_3d<f32>,
    field_sampler: sampler,
    position_world: vec3<f32>,
) -> f32 {
    let mean = cloud_coverage();
    // A fully clear or fully overcast sky has no variation to add: at the extremes the authored
    // value IS the answer, and blending noise in would put holes in an overcast.
    let headroom = 1.0 - abs(mean * 2.0 - 1.0);
    // Patchiness scales with CONVECTION, not uniformly.
    //
    // Stormscapes Table 1 lists no Perlin frequency or persistence at all for the fog and stratus
    // scenes, and enables them only from stratocumulus onward (0.07 / 0.50 for cumulus, rising to
    // 0.15 / 0.75 for cumulonimbus). That is physical: a stratus sheet is a continuous layer, and
    // separated cloud structure is what convection produces. Scaling by cloud type keeps a stratus
    // deck closed while letting cumulus break up.
    let convective = 0.25 + 0.75 * cloud_type_blend();
    let variation_strength = cloud_weather_variation() * convective;
    if (headroom <= 0.001 || variation_strength <= 0.0) {
        return mean;
    }
    let signal = cloud_ndf_at(position_world, field_sampler).r;
    // Centred on the authored coverage so the map both adds and removes cloud. Scaled by the
    // headroom so the variation shrinks as the sky approaches either extreme.
    let variation = (signal - 0.5) * 2.0 * variation_strength * headroom;
    return clamp(mean + variation, 0.0, 1.0);
}

// Normalized height within the deck, 0 at the base and 1 at the top.
fn cloud_height_fraction(position_world: vec3<f32>) -> f32 {
    return clamp(
        (position_world.y - cloud_bottom()) / max(cloud_thickness(), 0.0001),
        0.0,
        1.0,
    );
}

// Vertical density profile per cloud type — the single lerp that replaces three code paths.
//
// Stratus is a thin slab low in the deck; cumulus bulges mid-deck over a defined flat base;
// cumulonimbus fills nearly the whole depth. Blending continuously is what lets weather change
// cloud *type* without popping.
fn cloud_height_gradient(height: f32) -> f32 {
    let blend = cloud_type_blend();
    let stratus = smoothstep(0.0, 0.09, height) * (1.0 - smoothstep(0.16, 0.36, height));
    let cumulus = smoothstep(0.0, 0.18, height) * (1.0 - smoothstep(0.62, 1.0, height));

    // Cumulonimbus: a narrow rising column that FLARES into a flat anvil at the top.
    //
    // Stormscapes §6.1 explains where the shape comes from: *"the temperature inversion at z1 acts
    // as an obstacle for the rising thermal, causing the characteristic flat anvil top of a
    // cumulonimbus to form"*, with the paper's atmosphere using z1 = 8 km — which is exactly the
    // 8000 m top its Fig. 5e cumulonimbus reaches, and the top the storm preset now sets.
    //
    // So the anvil is not decoration: it is a hard ceiling. Modelled as a narrow column over most
    // of the depth that widens sharply in the last fifth and is then cut flat, rather than the
    // plain tall box this used to be.
    let column = smoothstep(0.0, 0.10, height) * (1.0 - smoothstep(0.92, 1.0, height));
    let anvil_flare = 1.0 + 1.6 * smoothstep(0.72, 0.95, height);
    // The cut: density stops dead at the inversion instead of tapering, which is what reads as a
    // flat top rather than a rounded one.
    let inversion_cut = 1.0 - smoothstep(0.95, 0.99, height);
    let towering = column * anvil_flare * inversion_cut;

    let low = mix(stratus, cumulus, clamp(blend * 2.0, 0.0, 1.0));
    return mix(low, towering, clamp(blend * 2.0 - 1.0, 0.0, 1.0));
}

// Nubis separates an authored modeling field from the noise used to up-resolve it. The pack's
// modeling channels are (in order) dimensional profile, local cloud type, and local density
// scale. We do not have an imported VDB/TGA asset bound yet, so the low-frequency R channel is
// the procedural modeling-field fallback. Keeping this as a named stage is important: an asset
// loader can replace these three values later without changing lighting, shadows, CAGI, or the
// camera march.
fn cloud_modeling_profile(base_field: vec4<f32>, height_gradient: f32, coverage: f32) -> f32 {
    return clamp(base_field.r * height_gradient, 0.0, 1.0) * coverage;
}

fn cloud_modeling_type(base_field: vec4<f32>, ndf: vec4<f32>) -> f32 {
    // A real modeling field owns this channel. Until one is supplied, let the weather signal
    // introduce a restrained local variation so a deck does not become one repeated cloud type.
    let authored_type = mix(cloud_type_blend(), ndf.g, cloud_weather_variation());
    let local_variation = (base_field.r - 0.5) * 0.35 * cloud_weather_variation();
    return clamp(authored_type + local_variation, 0.0, 1.0);
}

fn cloud_modeling_density_scale(ndf: vec4<f32>) -> f32 {
    // The Evolved modeling-data channel is authored in 0..1. Our hand-dialled density control is
    // intentionally wider (the shipped preset is 1.8), so normalize it for the Evolved powered
    // response and apply the remaining user gain after that response.
    return clamp(ndf.b * cloud_density_scale() * 0.5, 0.0, 1.0);
}

fn cloud_user_density_gain() -> f32 {
    return max(cloud_density_scale(), 1.0);
}

fn cloud_value_erosion(value: f32, erosion: f32) -> f32 {
    // Nubis' ValueErosion: move the erosion threshold through the value field and clamp the
    // result. It is deliberately not a multiplication; multiplication leaves a grey veil where
    // a threshold remap produces a readable cloud edge.
    return clamp((value - erosion) / max(1.0 - erosion, 0.0001), 0.0, 1.0);
}

fn cloud_upres_noise(noise: vec4<f32>, modeling_profile: f32, modeling_type: f32) -> f32 {
    // Nubis' wispy/billowy composition. The profile controls how much of the base shape survives
    // into the wispy branch; the fourth-root profile bias keeps billowy tops from disappearing in
    // thin parts of the modeling field.
    let wispy_noise = mix(noise.r, noise.g, modeling_profile);
    let billowy_type_gradient = pow(max(modeling_profile, 0.0), 0.25);
    let billowy_noise = mix(noise.b * 0.3, noise.a * 0.3, billowy_type_gradient);
    return mix(wispy_noise, billowy_noise, modeling_type);
}

fn cloud_evolved_high_frequency_noise(noise: vec4<f32>, modeling_type: f32) -> f32 {
    // Evolved folds the highest-frequency billowy channels around their midpoint instead of
    // asking for another texture fetch. This makes close clouds gain breakup without changing
    // the large-scale authored profile.
    let folded_wispy = 1.0 - pow(abs(abs(noise.g * 2.0 - 1.0) * 2.0 - 1.0), 4.0);
    let folded_billowy = pow(abs(abs(noise.a * 2.0 - 1.0) * 2.0 - 1.0), 2.0);
    return clamp(mix(folded_wispy, folded_billowy, modeling_type), 0.0, 1.0);
}

fn cloud_evolved_density(
    noise: vec4<f32>,
    modeling_profile: f32,
    modeling_type: f32,
    detail_strength: f32,
    modeling_density_scale: f32,
    distance_world: f32,
    high_frequency_details: bool,
) -> f32 {
    var composite = mix(
        0.0,
        cloud_upres_noise(noise, modeling_profile, modeling_type),
        clamp(detail_strength, 0.0, 1.0),
    );

    if (high_frequency_details) {
        // The Evolved sampler blends folded HF detail out over the 50–150 m range. The
        // saturating remap mirrors its ValueRemap helper; an unclamped linear remap would make
        // distant samples amplify noise rather than remove it.
        let hf_distance_blend = cloud_value_remap(distance_world, 50.0, 150.0, 0.9, 1.0);
        let high_frequency = cloud_evolved_high_frequency_noise(noise, modeling_type);
        composite = mix(high_frequency, composite, hf_distance_blend);
    }

    // Evolved's modeling-density stage: ValueErosion, powered authored density scale, then a
    // profile-dependent sharpen. The user gain is separate so Pascal's 1.8 hand-dial remains
    // meaningful while imported modeling volumes can use their native 0..1 density channel.
    let eroded = cloud_value_erosion(modeling_profile, composite);
    let powered_scale = pow(modeling_density_scale, 4.0);
    var density = eroded * powered_scale * cloud_user_density_gain();
    density = pow(max(density, 0.0), mix(0.3, 0.6, max(powered_scale, 0.0001)));

    if (high_frequency_details) {
        let sharpen_distance = cloud_value_remap(distance_world, 50.0, 150.0, 0.0, 1.0);
        density = pow(max(density, 0.0), mix(0.5, 1.0, sharpen_distance));
        density *= mix(0.666, 1.0, sharpen_distance);
    }

    return clamp(density, 0.0, 1.0);
}

// Sample the deck's density at a world position.
//
// `detail` false skips the three erosion octaves. The light march and the shadow map both use
// that: erosion changes a silhouette, which matters for what you see, but contributes almost
// nothing to how much light reaches a point — and it is 3 of the 4 channel reads.
fn cloud_density_sampled(
    field: texture_3d<f32>,
    field_sampler: sampler,
    position_world: vec3<f32>,
    detail: bool,
) -> f32 {
    let height = cloud_height_fraction(position_world);
    let gradient = cloud_height_gradient(height);
    if (gradient <= 0.0) {
        return 0.0;
    }

    // Advected sample position. The NDF, base shape and erosion all share this world-space
    // history. The Evolved temporal path reprojects this moving sample; until a history target is
    // available, keeping the coordinates coherent prevents the detail layer from shimmering
    // against a rigid base.
    let advected = position_world + cloud_wind_offset();
    let base_uvw = advected * CLOUD_BASE_SAMPLE_SCALE;
    let sample_distance = distance(position_world, atmosphere.camera_position);
    let noise_mip_level = cloud_noise_mip_level(sample_distance);
    let base_field = textureSampleLevel(field, field_sampler, base_uvw, noise_mip_level);
    let ndf = cloud_ndf_at(position_world, field_sampler);

    // Nubis' chain: first resolve the modeling field, then up-resolve it with wispy/billowy noise.
    // The remap erodes the cloud edge; the final multiplier keeps a weather cell's coverage
    // meaningful instead of making every surviving core equally opaque.
    let coverage = cloud_coverage_at(field, field_sampler, position_world);
    var modeling_profile = cloud_modeling_profile(base_field, gradient, coverage);
    modeling_profile = cloud_remap(modeling_profile, 1.0 - coverage, 1.0, 0.0, 1.0);
    modeling_profile = clamp(modeling_profile, 0.0, 1.0);
    if (modeling_profile <= 0.0) {
        return 0.0;
    }

    var upres_field = base_field;
    if (detail) {
        // Nubis fades high-frequency detail in from 50 to 150 metres. This is the procedural
        // equivalent of choosing a finer voxel mip near the camera while keeping distant clouds
        // stable and cheap. `textureSampleLevel` is explicit because this volume has one authored
        // mip today; the same fade remains valid when imported assets provide a mip pyramid.
        let high_frequency_fade = 1.0 - smoothstep(50.0, 150.0, sample_distance);

        // 0.00234375, NOT 0.0021 — the ratio to the base frequency must not be a small integer.
        //
        // 0.0021 / 0.00035 is exactly 6.000000, so the erosion lattice realigned with the base
        // lattice every single base tile and the two repeated in LOCKSTEP. Measured, transect
        // autocorrelation at the 2857.14 m base period was r = +0.622, rising to +0.680 at three
        // periods — the erosion was reinforcing the repeat instead of hiding it.
        //
        // 0.00234375 is 6.696... times the base, and is still an exact multiple of the 128-texel
        // tile (period 426.67 m) so the field still wraps seamlessly. The shared wind translation
        // slides both domains together, which is the temporal-safe fallback without history.
        let detail_uvw = advected * CLOUD_DETAIL_SAMPLE_SCALE;
        let erosion_field = textureSampleLevel(field, field_sampler, detail_uvw, noise_mip_level);
        upres_field = mix(base_field, erosion_field, high_frequency_fade);
    }

    let modeling_type = cloud_modeling_type(base_field, ndf);
    let modeling_density_scale = cloud_modeling_density_scale(ndf);
    let uprezzed_density = cloud_evolved_density(
        upres_field,
        modeling_profile,
        modeling_type,
        cloud_detail_strength(),
        modeling_density_scale,
        sample_distance,
        detail,
    );

    // Scale so cores SATURATE. Without it the deck's densest points sit near 0.37 and every ray
    // sees a translucent haze; a cloud needs an interior that is actually opaque before the
    // sun-side / shadow-side contrast that reads as "cloud" can exist at all.
    let distance_density = cloud_value_remap(sample_distance, 10.0, 120.0, 0.25, 1.0);
    return clamp(uprezzed_density * distance_density, 0.0, 1.0);
}

// Slab intersection of a ray with the deck, returning entry and exit distances.
//
// Analytic rather than marched-until-inside (Brucks): no per-step bounds test, no steps spent
// outside the deck, and a ray that misses costs nothing. `y` negative means no intersection.
fn cloud_deck_span(origin: vec3<f32>, direction: vec3<f32>) -> vec2<f32> {
    // A near-horizontal ray runs along the deck indefinitely; clamp rather than divide by
    // zero and let the caller's far limit bound it.
    if (abs(direction.y) < 0.000001) {
        if (origin.y >= cloud_bottom() && origin.y <= cloud_top()) {
            return vec2<f32>(0.0, 1.0e7);
        }
        return vec2<f32>(0.0, -1.0);
    }
    let to_bottom = (cloud_bottom() - origin.y) / direction.y;
    let to_top = (cloud_top() - origin.y) / direction.y;
    return vec2<f32>(max(min(to_bottom, to_top), 0.0), max(to_bottom, to_top));
}
