//! Sun + ambient lighting parameters: pure math, no wgpu, no windowing.
//!
//! Two layers, mirroring `camera.rs` (plan architecture rule):
//!
//! - [`LightingUniform`] — the per-frame GPU lighting data for the DDA pass.
//!   Stage 3 (CAGI) injects the same sun into the light volume, so the sun
//!   living in a uniform (not a shader constant) is a justified seam.
//! - [`SunSettings`] — the user-facing sun position (azimuth + elevation,
//!   degrees, mutated by the overlay sliders) that *produces* a
//!   `LightingUniform` each frame.
//!
//! Sun color/intensity and the hemisphere-ambient colors are fixed constants
//! for now (Stage 4's look pass decides whether they become settings too).

use std::f32::consts::TAU;

use glam::Vec3;

/// Sun color, linear RGB — slightly warm daylight.
const SUN_COLOR: [f32; 3] = [1.0, 0.96, 0.88];

/// Sun intensity multiplier (linear radiance). Chosen so a fully sunlit,
/// sun-facing surface lands near the top of the Reinhard curve's usable
/// range without clipping saturated palette colors to white.
const SUN_INTENSITY: f32 = 2.2;

/// Hemisphere ambient, sky side: cool blue, linear RGB. Upward faces receive
/// this in full; it is what keeps shadowed areas readable instead of black.
const SKY_AMBIENT_COLOR: [f32; 3] = [0.45, 0.65, 1.0];

/// Hemisphere ambient, ground side: warm bounce tint, linear RGB. Downward
/// faces receive this in full; side faces get the 50/50 mix.
const GROUND_AMBIENT_COLOR: [f32; 3] = [0.45, 0.36, 0.28];

/// Overall ambient strength applied to the hemisphere mix.
const AMBIENT_STRENGTH: f32 = 0.4;

/// Per-frame lighting data for the DDA compute shader, bindable as a uniform.
///
/// `#[repr(C)]` layout (80 bytes, 16-byte aligned — matches the WGSL
/// `Lighting` struct in `shaders/dda.wgsl`; the `vec3<f32>` is padded to 16
/// bytes with an explicit pad float):
///
/// | offset | field                 | WGSL type   | contents |
/// |--------|-----------------------|-------------|----------|
/// | 0      | `sun_direction`       | `vec3<f32>` | unit vector, surface → sun |
/// | 12     | `_pad0`               | `f32`       | |
/// | 16     | `sun_color_intensity` | `vec4<f32>` | rgb = linear sun color, w = intensity |
/// | 32     | `sky_ambient`         | `vec4<f32>` | rgb = linear sky ambient, w = ambient strength |
/// | 48     | `ground_ambient`      | `vec4<f32>` | rgb = linear ground bounce, w = unused |
/// | 64     | `shading_params`      | `vec4<f32>` | the runtime quality knobs — see [`ShadingParams`] |
/// | 80     | `gi_params`           | `vec4<f32>` | the runtime CAGI knobs — see [`GiParams`] |
/// | 96     | `water_params`        | `vec4<f32>` | the runtime E6 water knobs — see [`WaterParams`] |
/// | 112    | `water_optics`        | `vec4<f32>` | the E6 water look knobs — see [`WaterParams`] |
/// | 128    | `celestial_sun`       | `vec4<f32>` | physical sun direction + daylight |
/// | 144    | `celestial_moon`      | `vec4<f32>` | moon direction + phase |
/// | 160    | `sky_zenith`          | `vec4<f32>` | zenith radiance + star rotation |
/// | 176    | `sky_horizon`         | `vec4<f32>` | horizon radiance + moonlight |
/// | 192    | `material_params`     | `vec4<f32>` | runtime material knobs |
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightingUniform {
    pub sun_direction: [f32; 3],
    pub _pad0: f32,
    pub sun_color_intensity: [f32; 4],
    pub sky_ambient: [f32; 4],
    pub ground_ambient: [f32; 4],
    pub shading_params: [f32; 4],
    pub gi_params: [f32; 4],
    pub water_params: [f32; 4],
    pub water_optics: [f32; 4],
    /// xyz = physical sun direction (including below the horizon), w = daylight.
    pub celestial_sun: [f32; 4],
    /// xyz = moon direction, w = moon phase (0/1 new, 0.5 full).
    pub celestial_moon: [f32; 4],
    /// rgb = sky zenith radiance, w = star-field rotation in radians.
    pub sky_zenith: [f32; 4],
    /// rgb = sky horizon radiance, w = moonlight strength.
    pub sky_horizon: [f32; 4],
    /// x = absolute pattern fade start distance in metres.
    pub material_params: [f32; 4],
}

// Manual impls instead of derive so we do not depend on bytemuck's `derive`
// feature flag: `#[repr(C)]`, all-f32 fields, no implicit padding (the pad
// is an explicit field).
unsafe impl bytemuck::Zeroable for LightingUniform {}
unsafe impl bytemuck::Pod for LightingUniform {}

/// The RUNTIME quality knobs, packed into `Lighting.shading_params` — the
/// levers a preset switch can change WITHOUT a pipeline rebuild (E1c). The
/// field order IS the vector's component order, and
/// `crate::variants::REGISTRY` marks exactly these levers as
/// `LeverKind::Runtime`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadingParams {
    /// `x` — AO attenuation scale in [0, 1] (E1).
    pub ambient_occlusion_strength: f32,
    /// `y` — soft-shadow penumbra scale, the reciprocal of the light's angular
    /// radius (E1b). Ignored in hard-shadow mode.
    pub shadow_penumbra_scale: f32,
    /// `z` — start of the AO distance-fade ramp, voxel units (E1b lever 2,
    /// moved out of the shader consts in E1c). Ignored when the fade is
    /// compiled off.
    pub ambient_occlusion_fade_start_voxels: f32,
    /// `w` — end of the AO distance-fade ramp, voxel units; past it the
    /// estimator is skipped entirely.
    pub ambient_occlusion_fade_end_voxels: f32,
}

impl ShadingParams {
    fn to_array(self) -> [f32; 4] {
        [
            self.ambient_occlusion_strength,
            self.shadow_penumbra_scale,
            self.ambient_occlusion_fade_start_voxels,
            self.ambient_occlusion_fade_end_voxels,
        ]
    }
}

/// The RUNTIME CAGI knobs (E4), packed into `Lighting.gi_params`. Both passes
/// read this vector: the CA pass injects with `sun_bounce`, the shading pass
/// composes with `strength` and `ambient_floor`. Compile-time CAGI levers (the
/// master switch, the propagation rule, the sky test, the sampling mode, the
/// sun-source cache) are shader consts instead — see [`crate::cagi`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GiParams {
    /// `x` — multiplier on the sampled light volume.
    pub strength: f32,
    /// `y` — share of the hemisphere ambient kept under CAGI (0 = the volume is
    /// the only indirect light).
    pub ambient_floor: f32,
    /// `z` — share of the sun's radiance a sunlit surface injects into the volume.
    pub sun_bounce: f32,
    /// `w` — E5's emissive scale: a multiplier on every emitter's authored
    /// radiance, so a placed light can be dimmed without re-authoring the
    /// material table.
    pub emissive_scale: f32,
}

impl GiParams {
    fn to_array(self) -> [f32; 4] {
        [
            self.strength,
            self.ambient_floor,
            self.sun_bounce,
            self.emissive_scale,
        ]
    }
}

/// Runtime material controls carried by the shared frame uniform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialParams {
    pub pattern_fade_start_meters: f32,
    pub pattern_fade_end_meters: f32,
}

impl MaterialParams {
    fn to_array(self) -> [f32; 4] {
        [
            self.pattern_fade_start_meters.max(0.0),
            self.pattern_fade_end_meters.max(0.0),
            0.0,
            0.0,
        ]
    }
}

/// The RUNTIME water knobs (E6), packed into `Lighting.water_params`. The
/// compile-time water levers (the optics mode and the bounce budget) are shader
/// consts instead — see [`crate::water`], which also owns the physical constants
/// these two knobs scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterParams {
    /// `water_params.x` — multiplier on the medium's per-metre **absorption**
    /// coefficients: light the water destroys. The clarity/darkening axis.
    /// 1.0 = the authored coefficients.
    pub absorption_scale: f32,
    /// `water_params.y` — multiplier on the medium's per-metre **scattering**
    /// coefficients: light the water redirects, and therefore the light a ray picks
    /// up along its path. The brightness axis, and (with absorption) what the
    /// medium's colour is derived FROM. 0 makes the depths go black.
    pub scattering_scale: f32,
    /// `water_params.z` — the E6 ray cutoff: the smallest Fresnel weight worth a
    /// secondary ray. Below it the cheap analytic stand-in is substituted, so a
    /// head-on water pixel does not pay a full traced mirror for 2% of its colour.
    pub ray_cutoff: f32,
    /// `water_params.w` — unused, reserved for B6's fluid flow.
    pub reserved_flow: f32,
    /// `water_optics.x` — how far the medium's authored index of refraction is
    /// pulled toward 1.0, i.e. how WIDE Snell's window is (half-angle
    /// `asin(1 / n)`). 1.0 is the physical index and the shipped value; E6 step 3
    /// exposes it as a registry lever so it can be dialled in-app.
    pub refraction_strength: f32,
}

impl WaterParams {
    fn to_array(self) -> [f32; 4] {
        [
            self.absorption_scale,
            self.scattering_scale,
            self.ray_cutoff,
            self.reserved_flow,
        ]
    }

    fn optics_to_array(self) -> [f32; 4] {
        [self.refraction_strength, 0.0, 0.0, 0.0]
    }
}

/// User-facing sun position. The overlay mutates the angles; the platform
/// layer converts to a [`LightingUniform`] once per frame.
///
/// Conventions: azimuth is degrees around +Y with 0° along +X and 90° along
/// +Z (matching the camera's yaw convention); elevation is degrees above the
/// horizon.
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
    /// The Stage 1 shader constant, `normalize(vec3(0.55, 0.8, 0.35))`,
    /// expressed as angles (~32.5° azimuth, ~50.8° elevation).
    fn default() -> SunSettings {
        let stage_one_direction = Vec3::new(0.55, 0.8, 0.35).normalize();
        SunSettings {
            azimuth_degrees: stage_one_direction
                .z
                .atan2(stage_one_direction.x)
                .to_degrees(),
            elevation_degrees: stage_one_direction.y.asin().to_degrees(),
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

#[derive(Clone, Copy, Debug)]
struct CelestialState {
    sun_direction: Vec3,
    moon_direction: Vec3,
    active_direction: Vec3,
    active_color: [f32; 3],
    direct_strength: f32,
    ambient_strength: f32,
    daylight: f32,
    moonlight: f32,
    zenith: [f32; 3],
    horizon: [f32; 3],
    star_rotation: f32,
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

    /// Human-readable 24-hour clock for the Studio overlay.
    pub fn clock_label(&self) -> String {
        let total_minutes = (self.day_phase.rem_euclid(1.0) * 24.0 * 60.0).round() as u32;
        format!("{:02}:{:02}", (total_minutes / 60) % 24, total_minutes % 60)
    }

    fn celestial_state(&self) -> CelestialState {
        if !self.day_night_enabled {
            let direction = self.manual_sun_direction();
            return CelestialState {
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
        // A 160-degree east-to-west sweep between sunrise and sunset, while the
        // authored azimuth remains the noon direction and preserves the old look.
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

        CelestialState {
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

    fn manual_sun_direction(&self) -> Vec3 {
        let azimuth_radians = self.azimuth_degrees.to_radians();
        let elevation_radians = self.elevation_degrees.to_radians();
        let (sin_elevation, cos_elevation) = elevation_radians.sin_cos();
        let (sin_azimuth, cos_azimuth) = azimuth_radians.sin_cos();
        Vec3::new(
            cos_elevation * cos_azimuth,
            sin_elevation,
            cos_elevation * sin_azimuth,
        )
    }

    /// Unit direction from a surface toward the sun.
    pub fn sun_direction(&self) -> Vec3 {
        self.celestial_state().active_direction
    }

    /// Whether the CAGI volume has drifted far enough from its last celestial
    /// solution to warrant a new flood. Direct light and the sky still update
    /// every frame; this threshold prevents an animated clock from resetting
    /// the iterative volume sixty times per second.
    pub fn requires_light_reflood(&self, previous: &Self) -> bool {
        let current = self.celestial_state();
        let previous_state = previous.celestial_state();
        current
            .active_direction
            .dot(previous_state.active_direction)
            < 0.9994
            || (current.direct_strength - previous_state.direct_strength).abs() > 0.04
            || (self.intensity_scale - previous.intensity_scale).abs() > f32::EPSILON
            || (self.ambient_scale - previous.ambient_scale).abs() > f32::EPSILON
    }

    /// This frame's GPU lighting data. `shading_params` carries the
    /// experiments' runtime knobs (see [`ShadingParams`]);
    /// `crate::variants::RenderQuality::shading_params` produces it from the
    /// live quality settings, and each field is ignored by the shader when its
    /// lever is compiled off.
    pub fn lighting_uniform(
        &self,
        shading_params: ShadingParams,
        gi_params: GiParams,
        water_params: WaterParams,
        material_params: MaterialParams,
    ) -> LightingUniform {
        let celestial = self.celestial_state();
        LightingUniform {
            sun_direction: celestial.active_direction.to_array(),
            _pad0: 0.0,
            sun_color_intensity: [
                celestial.active_color[0],
                celestial.active_color[1],
                celestial.active_color[2],
                SUN_INTENSITY * self.intensity_scale.max(0.0) * celestial.direct_strength,
            ],
            sky_ambient: [
                SKY_AMBIENT_COLOR[0] * (0.25 + 0.75 * celestial.daylight),
                SKY_AMBIENT_COLOR[1] * (0.25 + 0.75 * celestial.daylight),
                SKY_AMBIENT_COLOR[2],
                AMBIENT_STRENGTH * self.ambient_scale.max(0.0) * celestial.ambient_strength,
            ],
            ground_ambient: [
                GROUND_AMBIENT_COLOR[0],
                GROUND_AMBIENT_COLOR[1],
                GROUND_AMBIENT_COLOR[2],
                0.0,
            ],
            shading_params: shading_params.to_array(),
            gi_params: gi_params.to_array(),
            water_params: water_params.to_array(),
            water_optics: water_params.optics_to_array(),
            celestial_sun: [
                celestial.sun_direction.x,
                celestial.sun_direction.y,
                celestial.sun_direction.z,
                celestial.daylight,
            ],
            celestial_moon: [
                celestial.moon_direction.x,
                celestial.moon_direction.y,
                celestial.moon_direction.z,
                self.moon_phase.clamp(0.0, 1.0),
            ],
            sky_zenith: [
                celestial.zenith[0],
                celestial.zenith[1],
                celestial.zenith[2],
                celestial.star_rotation,
            ],
            sky_horizon: [
                celestial.horizon[0],
                celestial.horizon[1],
                celestial.horizon[2],
                celestial.moonlight,
            ],
            material_params: material_params.to_array(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every runtime knob vector, in one uniform, so the component-order tests
    /// below all read the same construction.
    fn probe_uniform(shadow_penumbra_scale: f32) -> LightingUniform {
        probe_uniform_for(SunSettings::default(), shadow_penumbra_scale)
    }

    /// The same, for a chosen sun — so a test about the sun's own knobs shares this
    /// construction rather than writing a second one that could drift from it.
    fn probe_uniform_for(sun: SunSettings, shadow_penumbra_scale: f32) -> LightingUniform {
        sun.lighting_uniform(
            ShadingParams {
                ambient_occlusion_strength: 0.8,
                shadow_penumbra_scale,
                ambient_occlusion_fade_start_voxels: 240.0,
                ambient_occlusion_fade_end_voxels: 480.0,
            },
            GiParams {
                strength: 1.0,
                ambient_floor: 0.0,
                sun_bounce: 0.35,
                emissive_scale: 0.0,
            },
            WaterParams {
                absorption_scale: 1.0,
                scattering_scale: 1.0,
                ray_cutoff: 0.04,
                reserved_flow: 0.0,
                refraction_strength: 1.0,
            },
            MaterialParams {
                pattern_fade_start_meters: crate::pattern::PATTERN_FADE_START_METERS,
                pattern_fade_end_meters: crate::pattern::PATTERN_FADE_END_METERS,
            },
        )
    }

    #[test]
    fn uniform_layout_is_gpu_ready() {
        assert_eq!(std::mem::size_of::<LightingUniform>(), 208);
        assert_eq!(std::mem::align_of::<LightingUniform>(), 4);
    }

    /// The E4 knobs must land in their own slots of `gi_params` — swapping two
    /// would make the GI strength slider control the sun bounce fraction.
    #[test]
    fn gi_params_keep_their_vector_components() {
        let uniform = probe_uniform(115.0);
        assert_eq!(uniform.gi_params, [1.0, 0.0, 0.35, 0.0]);
        // ...and the E1 knobs must be untouched by the new vector.
        assert_eq!(uniform.shading_params, [0.8, 115.0, 240.0, 480.0]);
    }

    /// E6: the water knobs must land in their own slots too, and must not
    /// disturb the two vectors that were already there.
    #[test]
    fn water_params_keep_their_vector_components() {
        let uniform = probe_uniform(115.0);
        assert_eq!(uniform.water_params, [1.0, 1.0, 0.04, 0.0]);
        assert_eq!(uniform.water_optics, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(uniform.gi_params, [1.0, 0.0, 0.35, 0.0]);
        assert_eq!(uniform.shading_params, [0.8, 115.0, 240.0, 480.0]);
    }

    /// The runtime knobs must land in their own slots of the shared
    /// `shading_params` vector — swapping two would silently make the AO slider
    /// control the penumbra width.
    #[test]
    fn shading_params_keep_their_vector_components() {
        assert_eq!(probe_uniform(4.0).shading_params, [0.8, 4.0, 240.0, 480.0]);
    }

    #[test]
    fn default_sun_matches_stage_one_constant() {
        let direction = SunSettings::default().sun_direction();
        let stage_one_direction = Vec3::new(0.55, 0.8, 0.35).normalize();
        assert!(
            (direction - stage_one_direction).length() < 1e-5,
            "default sun {direction:?} drifted from the Stage 1 constant {stage_one_direction:?}"
        );
    }

    #[test]
    fn sun_direction_is_unit_length_across_angles() {
        for azimuth_degrees in [0.0_f32, 90.0, 180.0, 275.0, 360.0] {
            for elevation_degrees in [0.0_f32, 15.0, 50.0, 90.0] {
                let settings = SunSettings {
                    azimuth_degrees,
                    elevation_degrees,
                    ..SunSettings::default()
                };
                let length = settings.sun_direction().length();
                assert!(
                    (length - 1.0).abs() < 1e-5,
                    "non-unit sun direction at azimuth {azimuth_degrees}, \
                     elevation {elevation_degrees}: length {length}"
                );
            }
        }
    }

    /// S2c — the sun must be dimmable to nothing, and the ambient with it.
    ///
    /// The property an emissive material needs in order to be judgeable at all: a light
    /// you cannot turn down is a light that hides every emitter behind it.
    #[test]
    fn the_sun_and_the_ambient_floor_can_both_reach_zero() {
        let day = probe_uniform(1.0);
        let night = probe_uniform_for(
            SunSettings {
                intensity_scale: 0.0,
                ambient_scale: 0.0,
                ..SunSettings::default()
            },
            1.0,
        );

        // Daylight is the shipped look and must not have moved.
        assert_eq!(day.sun_color_intensity[3], SUN_INTENSITY);
        assert_eq!(day.sky_ambient[3], AMBIENT_STRENGTH);
        // Night is genuinely dark: nothing left but GI and emitters.
        assert_eq!(night.sun_color_intensity[3], 0.0);
        assert_eq!(night.sky_ambient[3], 0.0);
        // The direction is untouched by the scales — dimming is not moving the sun.
        assert_eq!(day.sun_direction, night.sun_direction);
        // And a negative scale cannot invert the light.
        let clamped = probe_uniform_for(
            SunSettings {
                intensity_scale: -5.0,
                ambient_scale: -5.0,
                ..SunSettings::default()
            },
            1.0,
        );
        assert_eq!(clamped.sun_color_intensity[3], 0.0);
        assert_eq!(clamped.sky_ambient[3], 0.0);
    }

    #[test]
    fn straight_up_elevation_points_along_y() {
        let settings = SunSettings {
            azimuth_degrees: 123.0,
            elevation_degrees: 90.0,
            ..SunSettings::default()
        };
        let direction = settings.sun_direction();
        assert!((direction - Vec3::Y).length() < 1e-5);
    }

    #[test]
    fn day_cycle_advances_only_when_enabled_and_running() {
        let mut settings = SunSettings::default();
        let noon = settings.day_phase;
        settings.advance_day_cycle(60.0);
        assert_eq!(settings.day_phase, noon, "paused Studio clock moved");

        settings.cycle_running = true;
        settings.advance_day_cycle(settings.day_length_seconds * 0.75);
        assert!(
            (settings.day_phase - 0.25).abs() < 1e-5,
            "day phase did not wrap"
        );

        settings.day_night_enabled = false;
        let stopped = settings.day_phase;
        settings.advance_day_cycle(60.0);
        assert_eq!(settings.day_phase, stopped, "disabled cycle moved");
    }

    #[test]
    fn noon_and_midnight_produce_distinct_sky_states() {
        let noon = probe_uniform_for(
            SunSettings {
                day_phase: 0.5,
                ..SunSettings::default()
            },
            1.0,
        );
        let midnight = probe_uniform_for(
            SunSettings {
                day_phase: 0.0,
                ..SunSettings::default()
            },
            1.0,
        );
        assert!(noon.celestial_sun[3] > midnight.celestial_sun[3]);
        assert!(midnight.celestial_moon[1] > 0.0);
        assert!(noon.celestial_moon[1] < 0.0);
        assert!(noon.sky_zenith[2] > midnight.sky_zenith[2]);
        assert_eq!(
            SunSettings {
                day_phase: 0.5,
                ..SunSettings::default()
            }
            .clock_label(),
            "12:00"
        );
        assert_eq!(
            SunSettings {
                day_phase: 0.0,
                ..SunSettings::default()
            }
            .clock_label(),
            "00:00"
        );
    }
}
