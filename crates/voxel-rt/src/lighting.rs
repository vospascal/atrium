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
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightingUniform {
    pub sun_direction: [f32; 3],
    pub _pad0: f32,
    pub sun_color_intensity: [f32; 4],
    pub sky_ambient: [f32; 4],
    pub ground_ambient: [f32; 4],
    pub shading_params: [f32; 4],
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
    pub fn lighting_uniform(&self, shading_params: ShadingParams) -> LightingUniform {
        let sun_direction = self.sun_direction();
        LightingUniform {
            sun_direction: sun_direction.to_array(),
            _pad0: 0.0,
            sun_color_intensity: [SUN_COLOR[0], SUN_COLOR[1], SUN_COLOR[2], SUN_INTENSITY],
            sky_ambient: [
                SKY_AMBIENT_COLOR[0],
                SKY_AMBIENT_COLOR[1],
                SKY_AMBIENT_COLOR[2],
                AMBIENT_STRENGTH,
            ],
            ground_ambient: [
                GROUND_AMBIENT_COLOR[0],
                GROUND_AMBIENT_COLOR[1],
                GROUND_AMBIENT_COLOR[2],
                0.0,
            ],
            shading_params: shading_params.to_array(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_layout_is_gpu_ready() {
        assert_eq!(std::mem::size_of::<LightingUniform>(), 80);
        assert_eq!(std::mem::align_of::<LightingUniform>(), 4);
    }

    /// The runtime knobs must land in their own slots of the shared
    /// `shading_params` vector — swapping two would silently make the AO slider
    /// control the penumbra width.
    #[test]
    fn shading_params_keep_their_vector_components() {
        let uniform = SunSettings::default().lighting_uniform(ShadingParams {
            ambient_occlusion_strength: 0.8,
            shadow_penumbra_scale: 4.0,
            ambient_occlusion_fade_start_voxels: 240.0,
            ambient_occlusion_fade_end_voxels: 480.0,
        });
        assert_eq!(uniform.shading_params, [0.8, 4.0, 240.0, 480.0]);
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

    #[test]
    fn straight_up_elevation_points_along_y() {
        let settings = SunSettings {
            azimuth_degrees: 123.0,
            elevation_degrees: 90.0,
        };
        let direction = settings.sun_direction();
        assert!((direction - Vec3::Y).length() < 1e-5);
    }
}
