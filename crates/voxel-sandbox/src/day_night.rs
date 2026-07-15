//! Day/night cycle.
//!
//! One resource, [`DayNightCycle`], holds the time of day as a fraction
//! (0.0 = midnight, 0.25 = sunrise, 0.5 = noon, 0.75 = sunset). A single
//! system advances it and drives everything lighting-related: the sun
//! (which becomes the moon after sunset), global ambient light, sky color,
//! and the distance-fog tint. Emissive props (campfire flames) become the
//! dominant light sources at night.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use bevy::light::GlobalAmbientLight;
use bevy::pbr::DistanceFog;
use bevy::prelude::*;

use crate::noise::smoothstep;

/// Seconds of real time for one full in-world day.
const DAY_LENGTH_SECONDS: f32 = 240.0;
/// Holding `N` runs time this much faster.
const FAST_FORWARD_FACTOR: f32 = 40.0;

const DAY_SKY: [f32; 3] = [0.80, 0.82, 0.79];
const NIGHT_SKY: [f32; 3] = [0.030, 0.045, 0.095];
const HORIZON_GLOW: [f32; 3] = [0.87, 0.54, 0.36];

#[derive(Resource)]
pub struct DayNightCycle {
    /// 0.0 = midnight, 0.25 = sunrise, 0.5 = noon, 0.75 = sunset.
    pub time_fraction: f32,
}

impl Default for DayNightCycle {
    fn default() -> Self {
        // Mid-morning, or wherever `VOXEL_TIME` says (0..1).
        let time_fraction = std::env::var("VOXEL_TIME")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(0.40);
        Self {
            time_fraction: time_fraction.rem_euclid(1.0),
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

#[allow(clippy::too_many_arguments)]
pub fn advance_day_night(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut cycle: ResMut<DayNightCycle>,
    mut sun_query: Query<(&mut DirectionalLight, &mut Transform), With<SunLight>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut clear_color: ResMut<ClearColor>,
    mut fog_query: Query<&mut DistanceFog>,
) {
    let speed = if keyboard.pressed(KeyCode::KeyN) {
        FAST_FORWARD_FACTOR
    } else {
        1.0
    };
    cycle.time_fraction =
        (cycle.time_fraction + time.delta_secs() * speed / DAY_LENGTH_SECONDS).rem_euclid(1.0);

    // −1 at midnight, 0 at sunrise/sunset, +1 at noon.
    let sun_height = ((cycle.time_fraction - 0.25) * TAU).sin();
    let daylight = smoothstep(0.0, 0.25, sun_height);

    // The same directional light plays sun by day and moon by night,
    // mirrored to the opposite side of the sky.
    let is_day = sun_height > 0.0;
    let apparent_height = sun_height.abs();
    let elevation = apparent_height * 1.1;
    let azimuth_swing = ((cycle.time_fraction - 0.25) * TAU).cos();
    let azimuth = 0.8 + azimuth_swing * 1.2 + if is_day { 0.0 } else { PI };

    let Ok((mut sun_light, mut sun_transform)) = sun_query.single_mut() else {
        return;
    };
    *sun_transform = Transform::from_rotation(Quat::from_euler(
        EulerRot::YXZ,
        azimuth,
        -elevation.min(FRAC_PI_2),
        0.0,
    ));

    if is_day {
        let horizon_warmth = 1.0 - smoothstep(0.0, 0.35, sun_height);
        let sun_color = lerp_rgb([1.0, 0.96, 0.87], [1.0, 0.62, 0.36], horizon_warmth);
        sun_light.color = Color::srgb(sun_color[0], sun_color[1], sun_color[2]);
        sun_light.illuminance = 400.0 + 9_600.0 * daylight;
    } else {
        let moonlight = smoothstep(0.05, 0.35, apparent_height);
        sun_light.color = Color::srgb(0.55, 0.66, 1.0);
        sun_light.illuminance = 20.0 + 160.0 * moonlight;
    }

    ambient.brightness = 35.0 + 615.0 * daylight;
    let ambient_color = lerp_rgb([0.45, 0.55, 0.90], [0.85, 0.88, 0.90], daylight);
    ambient.color = Color::srgb(ambient_color[0], ambient_color[1], ambient_color[2]);

    // Sky: night ↔ day base, with a warm bump when the sun grazes the horizon.
    let horizon_bump = (1.0 - (sun_height.abs() / 0.28).min(1.0)) * 0.45;
    let sky = lerp_rgb(
        lerp_rgb(NIGHT_SKY, DAY_SKY, daylight),
        HORIZON_GLOW,
        horizon_bump,
    );
    let sky_color = Color::srgb(sky[0], sky[1], sky[2]);
    clear_color.0 = sky_color;
    for mut fog in &mut fog_query {
        fog.color = sky_color;
    }
}
