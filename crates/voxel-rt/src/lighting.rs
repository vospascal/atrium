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
        }
    }
}

impl SunSettings {
    /// Unit direction from a surface toward the sun.
    pub fn sun_direction(&self) -> Vec3 {
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
    ) -> LightingUniform {
        let sun_direction = self.sun_direction();
        LightingUniform {
            sun_direction: sun_direction.to_array(),
            _pad0: 0.0,
            sun_color_intensity: [
                SUN_COLOR[0],
                SUN_COLOR[1],
                SUN_COLOR[2],
                SUN_INTENSITY * self.intensity_scale.max(0.0),
            ],
            sky_ambient: [
                SKY_AMBIENT_COLOR[0],
                SKY_AMBIENT_COLOR[1],
                SKY_AMBIENT_COLOR[2],
                AMBIENT_STRENGTH * self.ambient_scale.max(0.0),
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
                ambient_floor: 0.25,
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
        )
    }

    #[test]
    fn uniform_layout_is_gpu_ready() {
        assert_eq!(std::mem::size_of::<LightingUniform>(), 128);
        assert_eq!(std::mem::align_of::<LightingUniform>(), 4);
    }

    /// The E4 knobs must land in their own slots of `gi_params` — swapping two
    /// would make the GI strength slider control the sun bounce fraction.
    #[test]
    fn gi_params_keep_their_vector_components() {
        let uniform = probe_uniform(115.0);
        assert_eq!(uniform.gi_params, [1.0, 0.25, 0.35, 0.0]);
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
        assert_eq!(uniform.gi_params, [1.0, 0.25, 0.35, 0.0]);
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
}
