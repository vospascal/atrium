//! Sun position, day/night cycle, and frame-state evaluation.

use std::f32::consts::TAU;

use glam::Vec3;

use crate::EnvironmentFrame;

/// Warm daylight colour in linear RGB, used as the top-of-atmosphere source colour.
pub const SUN_COLOR: [f32; 3] = [1.0, 0.96, 0.88];

/// Warm daylight strength used by the renderer's directional-light contract.
pub const SUN_INTENSITY: f32 = 2.2;
/// Sky-side colour retained for the diffuse fallback and CAGI compatibility path.
pub const SKY_AMBIENT_COLOR: [f32; 3] = [0.45, 0.65, 1.0];
/// Ground-side colour retained for the diffuse fallback and CAGI compatibility path.
pub const GROUND_AMBIENT_COLOR: [f32; 3] = [0.45, 0.36, 0.28];
/// Strength of the diffuse fallback environment.
pub const AMBIENT_STRENGTH: f32 = 0.4;

/// User-facing sun position and day/night clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunSettings {
    pub azimuth_degrees: f32,
    pub elevation_degrees: f32,
    /// Multiplier on [`SUN_INTENSITY`]. Kept as an environment control, not an HDR control.
    pub intensity_scale: f32,
    /// Multiplier on the current diffuse environment approximation.
    pub ambient_scale: f32,
    pub day_night_enabled: bool,
    pub cycle_running: bool,
    /// Normalized time of day: 0/1 midnight, 0.25 sunrise, 0.5 noon, 0.75 sunset.
    pub day_phase: f32,
    pub day_length_seconds: f32,
    /// 0/1 new moon, 0.5 full moon.
    pub moon_phase: f32,
}

impl Default for SunSettings {
    fn default() -> Self {
        let direction = Vec3::new(0.55, 0.8, 0.35).normalize();
        Self {
            azimuth_degrees: direction.z.atan2(direction.x).to_degrees(),
            elevation_degrees: direction.y.asin().to_degrees(),
            intensity_scale: 1.0,
            ambient_scale: 1.0,
            day_night_enabled: true,
            cycle_running: false,
            day_phase: 0.5,
            day_length_seconds: 240.0,
            moon_phase: 0.85,
        }
    }
}

fn mix_rgb(from: [f32; 3], to: [f32; 3], amount: f32) -> [f32; 3] {
    [
        from[0] + (to[0] - from[0]) * amount,
        from[1] + (to[1] - from[1]) * amount,
        from[2] + (to[2] - from[2]) * amount,
    ]
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl SunSettings {
    pub fn advance_day_cycle(&mut self, elapsed_seconds: f32) {
        if self.day_night_enabled && self.cycle_running {
            let day_length = self.day_length_seconds.max(1.0);
            self.day_phase =
                (self.day_phase + elapsed_seconds.max(0.0) / day_length).rem_euclid(1.0);
        }
    }

    pub fn clock_label(&self) -> String {
        let total_minutes = (self.day_phase.rem_euclid(1.0) * 24.0 * 60.0).round() as u32;
        format!("{:02}:{:02}", (total_minutes / 60) % 24, total_minutes % 60)
    }

    /// Evaluate the shared sun/sky state for this frame.
    pub fn environment_frame(&self) -> EnvironmentFrame {
        if !self.day_night_enabled {
            let direction = self.manual_sun_direction();
            return EnvironmentFrame {
                sun_direction: direction,
                moon_direction: -direction,
                active_direction: direction,
                active_color: SUN_COLOR,
                direct_strength: 1.0,
                ambient_strength: 1.0,
                daylight: 1.0,
                moonlight: 0.0,
                zenith: [0.08, 0.31, 2.55],
                horizon: [2.55, 1.37, 0.63],
                star_rotation: 0.0,
            };
        }

        let phase = self.day_phase.rem_euclid(1.0);
        let orbit = (phase - 0.25) * TAU;
        let sun_height = orbit.sin();
        let daylight = smoothstep(0.0, 0.25, sun_height);
        let moonlight = smoothstep(0.05, 0.35, -sun_height);
        let elevation = self.elevation_degrees.to_radians() * sun_height;
        let azimuth = (self.azimuth_degrees + (phase - 0.5) * 320.0).to_radians();
        let (sin_elevation, cos_elevation) = elevation.sin_cos();
        let (sin_azimuth, cos_azimuth) = azimuth.sin_cos();
        let sun_direction = Vec3::new(
            cos_elevation * cos_azimuth,
            sin_elevation,
            cos_elevation * sin_azimuth,
        )
        .normalize();
        let moon_direction = -sun_direction;
        let is_day = sun_height > 0.0;
        let active_direction = if is_day {
            sun_direction
        } else {
            moon_direction
        };
        let horizon_warmth = 1.0 - smoothstep(0.0, 0.35, sun_height.max(0.0));
        let sun_color = mix_rgb(SUN_COLOR, [1.0, 0.52, 0.24], horizon_warmth);
        let moon_color = [0.38, 0.50, 1.0];
        let phase_brightness = 0.15 + 0.85 * (0.5 - 0.5 * (self.moon_phase * TAU).cos());
        let direct_strength = if is_day {
            daylight * (0.75 + 0.25 * sun_height.max(0.0))
        } else {
            moonlight * phase_brightness * 0.045
        };
        let ambient_strength = 0.045 + daylight * 0.955 + moonlight * phase_brightness * 0.08;

        const NIGHT_ZENITH: [f32; 3] = [0.002, 0.004, 0.018];
        const DAY_ZENITH: [f32; 3] = [0.08, 0.31, 2.55];
        const NIGHT_HORIZON: [f32; 3] = [0.012, 0.020, 0.060];
        const DAY_HORIZON: [f32; 3] = [2.55, 1.37, 0.63];
        const TWILIGHT: [f32; 3] = [3.0, 0.55, 0.12];
        let twilight = (1.0 - (sun_height.abs() / 0.28).min(1.0)) * 0.55;
        let zenith = mix_rgb(NIGHT_ZENITH, DAY_ZENITH, daylight);
        let horizon = mix_rgb(
            mix_rgb(NIGHT_HORIZON, DAY_HORIZON, daylight),
            TWILIGHT,
            twilight,
        );

        EnvironmentFrame {
            sun_direction,
            moon_direction,
            active_direction,
            active_color: if is_day { sun_color } else { moon_color },
            direct_strength,
            ambient_strength,
            daylight,
            moonlight,
            zenith,
            horizon,
            star_rotation: phase * TAU,
        }
    }

    pub fn sun_direction(&self) -> Vec3 {
        self.environment_frame().active_direction
    }

    pub fn requires_light_reflood(&self, previous_settings: &Self) -> bool {
        let current = self.environment_frame();
        let previous = previous_settings.environment_frame();
        current.active_direction.dot(previous.active_direction) < 0.9994
            || (current.direct_strength - previous.direct_strength).abs() > 0.04
            || (self.intensity_scale - previous_settings.intensity_scale).abs() > f32::EPSILON
            || (self.ambient_scale - previous_settings.ambient_scale).abs() > f32::EPSILON
    }

    fn manual_sun_direction(&self) -> Vec3 {
        let azimuth = self.azimuth_degrees.to_radians();
        let elevation = self.elevation_degrees.to_radians();
        let (sin_elevation, cos_elevation) = elevation.sin_cos();
        let (sin_azimuth, cos_azimuth) = azimuth.sin_cos();
        Vec3::new(
            cos_elevation * cos_azimuth,
            sin_elevation,
            cos_elevation * sin_azimuth,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_environment_is_daylight_and_finite() {
        let frame = SunSettings::default().environment_frame();
        assert!(frame.sun_direction.is_normalized());
        assert!(frame.daylight > 0.99);
        assert!(frame
            .zenith
            .iter()
            .chain(frame.horizon.iter())
            .all(|v| v.is_finite()));
    }

    #[test]
    fn day_cycle_wraps_without_changing_the_contract() {
        let mut settings = SunSettings {
            cycle_running: true,
            day_length_seconds: 10.0,
            ..SunSettings::default()
        };
        settings.advance_day_cycle(11.0);
        assert!((settings.day_phase - 0.6).abs() < 1e-6);
    }
}
