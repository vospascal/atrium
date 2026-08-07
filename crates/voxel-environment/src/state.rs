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
    /// Sun intensity multiplier, scaling [`SUN_INTENSITY`]. 1.0 is the shipped look.
    ///
    /// Exposed by S2c, and it was a real gap rather than a nicety: **an emitter cannot
    /// be judged against a light you cannot turn down.** The sun was a hardcoded
    /// constant, so a glowing surface and the light it casts were both washed out by a
    /// fixed 2.2 of daylight, and there was no way to tell an emitter that worked from
    /// one that did nothing (Pascal, 2026-07-31: *"i cant realy see it emiting .. might
    /// be as well that we dont have the right sky or light conditions .. we have pretty
    /// crude over head light"*).
    ///
    /// Zero is a genuine night: the sun contributes nothing and only ambient, GI and
    /// emitters remain. Which is exactly the condition an emissive material is for.
    pub intensity_scale: f32,
    /// Hemisphere-ambient multiplier, scaling [`AMBIENT_STRENGTH`]. 1.0 is the shipped
    /// look, 0.0 removes the ambient floor entirely.
    ///
    /// Needed alongside the sun scale for the same reason: at sun zero the 0.4 ambient
    /// is still enough to read every surface, so an emitter's contribution stays
    /// invisible. Turning both down is what makes a dark room dark.
    pub ambient_scale: f32,
    /// Drive the light and sky from [`Self::day_phase`] instead of treating the
    /// azimuth/elevation fields as a completely manual directional light.
    pub day_night_enabled: bool,
    /// Advance [`Self::day_phase`] each frame. The Studio defaults to a frozen
    /// noon so opening a material never changes underneath the author.
    pub cycle_running: bool,
    /// Normalized time of day: 0/1 midnight, 0.25 sunrise, 0.5 noon, 0.75 sunset.
    pub day_phase: f32,
    /// Real seconds for one complete in-world day.
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
                sun_illuminance: [
                    SUN_COLOR[0] * SUN_INTENSITY * self.intensity_scale.max(0.0),
                    SUN_COLOR[1] * SUN_INTENSITY * self.intensity_scale.max(0.0),
                    SUN_COLOR[2] * SUN_INTENSITY * self.intensity_scale.max(0.0),
                ],
                moon_illuminance: [0.0; 3],
                active_illuminance: [
                    SUN_COLOR[0] * SUN_INTENSITY * self.intensity_scale.max(0.0),
                    SUN_COLOR[1] * SUN_INTENSITY * self.intensity_scale.max(0.0),
                    SUN_COLOR[2] * SUN_INTENSITY * self.intensity_scale.max(0.0),
                ],
                active_color: SUN_COLOR,
                direct_strength: 1.0,
                ambient_strength: 1.0,
                ambient_scale: AMBIENT_STRENGTH * self.ambient_scale.max(0.0),
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
        let intensity = SUN_INTENSITY * self.intensity_scale.max(0.0);
        // Keep the physical sun source alive through the low-sun twilight band so Hillaire can
        // colour the upper atmosphere, but remove it once the sun is well below the horizon. A
        // full-strength source at -45° still leaked enough high-altitude scattering to keep the
        // night sky bright even though direct lighting had correctly switched to the moon.
        let twilight_sun = smoothstep(-0.16, 0.04, sun_direction.y);
        let sun_illuminance = [
            // Whether that source reaches an atmospheric sample is still decided by the LUT light
            // ray, which rejects paths blocked by the planet. Direct surface/cloud lighting stays
            // on `active_illuminance` and therefore switches to the moon at night.
            sun_color[0] * intensity * twilight_sun,
            sun_color[1] * intensity * twilight_sun,
            sun_color[2] * intensity * twilight_sun,
        ];
        let moon_illuminance = [
            moon_color[0] * intensity * if is_day { 0.0 } else { direct_strength },
            moon_color[1] * intensity * if is_day { 0.0 } else { direct_strength },
            moon_color[2] * intensity * if is_day { 0.0 } else { direct_strength },
        ];
        let ambient_strength = 0.045 + daylight * 0.955 + moonlight * phase_brightness * 0.08;
        let ambient_scale = AMBIENT_STRENGTH * self.ambient_scale.max(0.0) * ambient_strength;

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
            sun_illuminance,
            moon_illuminance,
            active_illuminance: if is_day {
                sun_illuminance
            } else {
                moon_illuminance
            },
            active_color: if is_day { sun_color } else { moon_color },
            direct_strength,
            ambient_strength,
            ambient_scale,
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

    #[test]
    fn physical_and_active_illuminance_are_separate_at_night() {
        let night = SunSettings {
            day_phase: 0.0,
            ..SunSettings::default()
        }
        .environment_frame();
        assert!(night.sun_illuminance.iter().all(|value| *value == 0.0));
        assert!(night.active_illuminance.iter().any(|value| *value > 0.0));
        assert_eq!(night.active_illuminance, night.moon_illuminance);
    }

    #[test]
    fn twilight_keeps_a_low_sun_atmosphere_source_but_deep_night_is_dark() {
        let sunset = SunSettings {
            day_phase: 0.75,
            ..SunSettings::default()
        }
        .environment_frame();
        assert!(sunset.sun_direction.y.abs() < 1.0e-6);
        assert!(sunset.sun_illuminance.iter().all(|value| *value > 0.0));

        let night = SunSettings {
            day_phase: 0.0,
            ..SunSettings::default()
        }
        .environment_frame();
        assert!(night.sun_direction.y < -0.7);
        assert!(night.sun_illuminance.iter().all(|value| *value == 0.0));
    }
}
