//! Day/night cycle.
//!
//! One resource, [`DayNightCycle`], holds the time of day as a fraction
//! (0.0 = midnight, 0.25 = sunrise, 0.5 = noon, 0.75 = sunset). A single
//! system advances it and drives everything lighting-related: the sun
//! (which becomes the moon after sunset), global ambient light, sky color,
//! and the distance-fog tint. Emissive props (campfire flames) become the
//! dominant light sources at night.
//!
//! The derived per-frame values (sun/moon directions, palette, star
//! rotation) are published in [`CelestialState`] so the sky dome shader and
//! the weather systems stay in perfect sync with the actual light.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use bevy::light::GlobalAmbientLight;
use bevy::pbr::DistanceFog;
use bevy::prelude::*;

use crate::weather::WeatherState;
use voxel_core::noise::smoothstep;

/// Seconds of real time for one full in-world day.
const DAY_LENGTH_SECONDS: f32 = 240.0;
/// Holding `N` runs time this much faster.
const FAST_FORWARD_FACTOR: f32 = 40.0;

const DAY_SKY: [f32; 3] = [0.80, 0.82, 0.79];
const NIGHT_SKY: [f32; 3] = [0.030, 0.045, 0.095];
const DAY_ZENITH: [f32; 3] = [0.33, 0.51, 0.78];
const NIGHT_ZENITH: [f32; 3] = [0.006, 0.011, 0.032];
const HORIZON_GLOW: [f32; 3] = [0.87, 0.54, 0.36];
/// What thick fog washes the sky toward, by day and by night.
const DAY_FOG: [f32; 3] = [0.74, 0.76, 0.77];
const NIGHT_FOG: [f32; 3] = [0.045, 0.055, 0.075];

#[derive(Resource)]
pub struct DayNightCycle {
    /// 0.0 = midnight, 0.25 = sunrise, 0.5 = noon, 0.75 = sunset.
    pub time_fraction: f32,
    /// When false the clock is frozen (panel slider still sets the time).
    pub run_clock: bool,
    /// 0 = new moon, 0.5 = full moon, 1 = new again (waxing in between).
    pub moon_phase: f32,
    /// Multiplier on the sun/moon directional-light strength (panel slider).
    /// 1.0 = default; lower softens the harsh midday contrast.
    pub sun_intensity: f32,
}

impl Default for DayNightCycle {
    fn default() -> Self {
        // Golden hour (17:40), or wherever `VOXEL_TIME` says (0..1).
        let time_fraction = std::env::var("VOXEL_TIME")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(17.0 / 24.0 + 40.0 / (60.0 * 24.0));
        let moon_phase = std::env::var("VOXEL_MOON")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(0.85);
        Self {
            time_fraction: time_fraction.rem_euclid(1.0),
            // Off by default: the light stays where you set it; the panel
            // checkbox (or holding N) runs the cycle when wanted.
            run_clock: false,
            moon_phase: moon_phase.clamp(0.0, 1.0),
            sun_intensity: 1.0,
        }
    }
}

/// Per-frame celestial values derived from the cycle, shared by the sky
/// dome shader, fog, and precipitation tinting. Colors are LINEAR rgb.
#[derive(Resource)]
pub struct CelestialState {
    /// Unit vector pointing at the sun (may be below the horizon).
    pub sun_direction: Vec3,
    /// Unit vector pointing at the moon.
    pub moon_direction: Vec3,
    /// 0 at night, 1 in full daylight.
    pub daylight: f32,
    /// 0 by day, up to 1 with the moon high.
    pub moonlight: f32,
    /// Color of whichever body currently lights the scene.
    pub light_color: Vec3,
    pub zenith_color: Vec3,
    pub horizon_color: Vec3,
    /// Star-field rotation angle (radians) — one revolution per day.
    pub star_rotation: f32,
}

impl Default for CelestialState {
    fn default() -> Self {
        Self {
            sun_direction: Vec3::Y,
            moon_direction: -Vec3::Y,
            daylight: 1.0,
            moonlight: 0.0,
            light_color: Vec3::ONE,
            zenith_color: linear_rgb(DAY_ZENITH),
            horizon_color: linear_rgb(DAY_SKY),
            star_rotation: 0.0,
        }
    }
}

/// Marker for the sun/moon directional light.
#[derive(Component)]
pub struct SunLight;

fn lerp_rgb(from: [f32; 3], to: [f32; 3], amount: f32) -> [f32; 3] {
    [
        from[0] + (to[0] - from[0]) * amount,
        from[1] + (to[1] - from[1]) * amount,
        from[2] + (to[2] - from[2]) * amount,
    ]
}

/// sRGB triple → linear rgb vector (the shader works in linear light).
fn linear_rgb(srgb: [f32; 3]) -> Vec3 {
    let linear = Color::srgb(srgb[0], srgb[1], srgb[2]).to_linear();
    Vec3::new(linear.red, linear.green, linear.blue)
}

/// Unit direction toward a body at the given azimuth/elevation (radians).
fn celestial_direction(azimuth: f32, elevation: f32) -> Vec3 {
    Quat::from_euler(EulerRot::YXZ, azimuth, -elevation, 0.0) * Vec3::Z
}

#[allow(clippy::too_many_arguments)]
pub fn advance_day_night(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut cycle: ResMut<DayNightCycle>,
    weather: Res<WeatherState>,
    mut celestial: ResMut<CelestialState>,
    mut sun_query: Query<(&mut DirectionalLight, &mut Transform), With<SunLight>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut clear_color: ResMut<ClearColor>,
    mut fog_query: Query<&mut DistanceFog>,
) {
    if cycle.run_clock {
        let speed = if keyboard.pressed(KeyCode::KeyN) {
            FAST_FORWARD_FACTOR
        } else {
            1.0
        };
        cycle.time_fraction =
            (cycle.time_fraction + time.delta_secs() * speed / DAY_LENGTH_SECONDS).rem_euclid(1.0);
    }

    // −1 at midnight, 0 at sunrise/sunset, +1 at noon.
    let sun_height = ((cycle.time_fraction - 0.25) * TAU).sin();
    let daylight = smoothstep(0.0, 0.25, sun_height);
    // Aesthetic sun-strength curve (user-dialed): softer when the sun is high
    // and harsh, warmer at the golden hours. ~0.35 at noon (sun_height 1.0)
    // rising to ~0.75 near 06:30/17:30 (sun_height ≈ 0.13). Shapes the direct
    // light on top of the daylight fade; the panel slider is a master on top.
    let sun_curve = 0.81 - 0.46 * sun_height.clamp(0.0, 1.0);

    // The same directional light plays sun by day and moon by night; the
    // moon rises where the sun set (mirrored azimuth).
    let is_day = sun_height > 0.0;
    let apparent_height = sun_height.abs();
    let azimuth_swing = ((cycle.time_fraction - 0.25) * TAU).cos();
    let sun_azimuth = 0.8 + azimuth_swing * 1.2;
    let moon_azimuth = sun_azimuth + PI;
    let sun_elevation = (sun_height * 1.1).min(FRAC_PI_2);
    let moon_elevation = (-sun_height * 1.1).min(FRAC_PI_2);

    // Overcast skies dim the direct light long before they block it.
    let coverage = weather.effective_cloud_coverage();
    let cloud_dim = 1.0 - 0.62 * coverage;

    let Ok((mut sun_light, mut sun_transform)) = sun_query.single_mut() else {
        return;
    };
    let (active_azimuth, active_elevation) = if is_day {
        (sun_azimuth, sun_elevation)
    } else {
        (moon_azimuth, moon_elevation)
    };
    *sun_transform = Transform::from_rotation(Quat::from_euler(
        EulerRot::YXZ,
        active_azimuth,
        -active_elevation,
        0.0,
    ));

    let moonlight = if is_day {
        0.0
    } else {
        smoothstep(0.05, 0.35, apparent_height)
    };
    if is_day {
        let horizon_warmth = 1.0 - smoothstep(0.0, 0.35, sun_height);
        let sun_color = lerp_rgb([1.0, 0.96, 0.87], [1.0, 0.62, 0.36], horizon_warmth);
        sun_light.color = Color::srgb(sun_color[0], sun_color[1], sun_color[2]);
        sun_light.illuminance =
            (400.0 + 9_600.0 * daylight) * cloud_dim * sun_curve * cycle.sun_intensity;
        celestial.light_color = linear_rgb(sun_color);
    } else {
        sun_light.color = Color::srgb(0.55, 0.66, 1.0);
        sun_light.illuminance =
            (20.0 + 160.0 * moonlight) * (1.0 - 0.5 * coverage) * cycle.sun_intensity;
        // Moon brightness follows its phase: a crescent barely lights clouds.
        let phase_brightness = 0.15 + 0.85 * moon_lit_fraction(cycle.moon_phase);
        celestial.light_color = linear_rgb([0.55, 0.66, 1.0]) * phase_brightness;
    }

    ambient.brightness = (35.0 + 615.0 * daylight) * (1.0 - 0.18 * coverage);
    let ambient_color = lerp_rgb([0.45, 0.55, 0.90], [0.85, 0.88, 0.90], daylight);
    ambient.color = Color::srgb(ambient_color[0], ambient_color[1], ambient_color[2]);

    // Sky palette: night ↔ day base, a warm bump when the sun grazes the
    // horizon, and everything washed toward gray as fog thickens.
    let fog_amount = weather.current.fog;
    let fog_wash = lerp_rgb(NIGHT_FOG, DAY_FOG, daylight);
    let horizon_bump = (1.0 - (sun_height.abs() / 0.28).min(1.0)) * 0.45;
    let horizon = lerp_rgb(
        lerp_rgb(
            lerp_rgb(NIGHT_SKY, DAY_SKY, daylight),
            HORIZON_GLOW,
            horizon_bump,
        ),
        fog_wash,
        fog_amount * 0.65,
    );
    let zenith = lerp_rgb(
        lerp_rgb(NIGHT_ZENITH, DAY_ZENITH, daylight),
        fog_wash,
        fog_amount * 0.40,
    );

    let sky_color = Color::srgb(horizon[0], horizon[1], horizon[2]);
    clear_color.0 = sky_color;
    for mut fog in &mut fog_query {
        fog.color = sky_color;
    }

    celestial.sun_direction = celestial_direction(sun_azimuth, sun_elevation);
    celestial.moon_direction = celestial_direction(moon_azimuth, moon_elevation);
    celestial.daylight = daylight;
    celestial.moonlight = moonlight;
    celestial.zenith_color = linear_rgb(zenith);
    celestial.horizon_color = linear_rgb(horizon);
    celestial.star_rotation = cycle.time_fraction * TAU;
}

/// Fraction of the moon disc that is lit for a phase in 0..1.
fn moon_lit_fraction(phase: f32) -> f32 {
    0.5 - 0.5 * (phase * TAU).cos()
}
