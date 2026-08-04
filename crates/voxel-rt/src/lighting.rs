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

use voxel_environment::{
    SunSettings, AMBIENT_STRENGTH, GROUND_AMBIENT_COLOR, SKY_AMBIENT_COLOR, SUN_INTENSITY,
};

/// Per-frame lighting data for the DDA compute shader, bindable as a uniform.
///
/// `#[repr(C)]` layout (80 bytes, 16-byte aligned — matches the WGSL
/// `Lighting` struct in `shaders/dda.wgsl`; the `vec3<f32>` is padded to 16
/// bytes with an explicit pad float):
///
/// | offset | field                 | WGSL type   | contents |
/// |--------|-----------------------|-------------|----------|
/// | 0      | `sun_direction`       | `vec3<f32>` | unit vector, surface → sun |
/// | 12     | `pad_a`               | `f32`       | |
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
/// | 208    | `animation_params`    | `vec4<f32>` | the scaled material clock — see [`AnimationParams`] |
/// | 224    | `event_params`        | `vec4<f32>` | the unscaled world clock + event count |
/// | 240    | `output_params`       | `vec4<f32>` | the display's measured HDR headroom — see [`OutputParams`] |
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightingUniform {
    pub sun_direction: [f32; 3],
    pub pad_a: f32,
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
    /// The animation clock and event count — see [`AnimationParams`].
    pub animation_params: [f32; 4],
    /// The unscaled simulation clock used by world-event envelopes.
    pub event_params: [f32; 4],
    /// The display's HDR headroom — see [`OutputParams`].
    pub output_params: [f32; 4],
}

// Manual impls instead of derive so we do not depend on bytemuck's `derive`
// feature flag: `#[repr(C)]`, all-f32 fields, no implicit padding (the pad
// is an explicit field).
unsafe impl bytemuck::Zeroable for LightingUniform {}
unsafe impl bytemuck::Pod for LightingUniform {}

impl LightingUniform {
    /// Attach the display's measured HDR headroom.
    ///
    /// **A builder rather than a seventh parameter to [`SunSettings::lighting_uniform`],
    /// deliberately.** Headroom is a property of a physical display, and of the thirteen
    /// callers of that constructor exactly one — the windowed app — has one. The
    /// benchmark, the CAGI probes and every unit test render offscreen and have nothing to
    /// report, so requiring the argument would mean thirteen edits to pass a value twelve
    /// of them would have to invent.
    ///
    /// The failure mode is what makes this safe rather than merely convenient: forgetting
    /// to call it leaves [`OutputParams::default`], which claims NO headroom and clips at
    /// white. That is the conservative direction — an over-claimed headroom is the bug
    /// this whole path exists to fix, and it cannot be reached by omission.
    pub fn with_output_params(mut self, output_params: OutputParams) -> LightingUniform {
        self.output_params = output_params.to_array();
        self
    }
}

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
    /// World metres one pixel spans at one metre of distance — the vertical FOV
    /// over the render height. Multiply by a hit distance and you have that hit's
    /// pixel footprint in metres.
    ///
    /// A RESOLUTION-dependent value in the frame uniform rather than a shader
    /// const, because the render scale moves with the quality preset and a const
    /// would silently describe the wrong screen. `PATTERN_OCTAVE_LOD` reads it to
    /// decide how many octaves of a fractal generator can still be resolved.
    pub pixel_footprint_at_one_meter: f32,
}

impl MaterialParams {
    fn to_array(self) -> [f32; 4] {
        [
            self.pattern_fade_start_meters.max(0.0),
            self.pattern_fade_end_meters.max(0.0),
            self.pixel_footprint_at_one_meter.max(0.0),
            0.0,
        ]
    }
}

/// The animation clock as the shader receives it, packed into
/// `Lighting.animation_params` (S3).
///
/// The clock is split into whole epochs and a remainder rather than shipped as
/// one monotonic float, because a single f32 second count loses the fraction
/// an oscillator needs within hours of uptime, and any wrapped single clock
/// steps every non-harmonic rate. See [`voxel_material::animation_clock`] for the full
/// argument — this struct only carries the result.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AnimationParams {
    /// `x` — seconds within the current epoch, `[0, EPOCH_SECONDS)`.
    pub remainder_seconds: f32,
    /// `y` — whole epochs elapsed, integer-exact to 2^24.
    pub epoch: f32,
    /// `z/w` — reserved for future material-wide values.
    pub reserved_flow: f32,
    pub reserved: f32,
}

impl AnimationParams {
    fn to_array(self) -> [f32; 4] {
        [
            self.remainder_seconds,
            self.epoch,
            self.reserved_flow,
            self.reserved,
        ]
    }
}

/// The unscaled clock for event timestamps and release tails. Material speed
/// deliberately cannot pause or accelerate the world simulation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EventParams {
    pub remainder_seconds: f32,
    pub epoch: f32,
    pub event_count: f32,
}

impl EventParams {
    fn to_array(self) -> [f32; 4] {
        [
            self.remainder_seconds,
            self.epoch,
            self.event_count.max(0.0),
            0.0,
        ]
    }
}

/// The display's HDR headroom, packed into `Lighting.output_params`.
///
/// **A UNIFORM RATHER THAN A SHADER CONST, and that is the change.** Headroom used to be
/// `const OUTPUT_HDR_HEADROOM: f32 = 4.0` in `dda.wgsl` — a number nothing ever checked
/// against the hardware. Real EDR headroom moves while the app runs: the brightness
/// slider changes it, thermal state changes it, dragging the window to another display
/// changes it. A const would mean a pipeline rebuild on every one of those, which is
/// stutter during a slider drag, so this is the one output knob that has to be uploaded
/// rather than patched.
///
/// It rides in the lighting uniform because that buffer is already written every frame
/// unconditionally, so a live headroom costs nothing — and because a new binding is
/// exactly what broke the HDR path repeatedly while it was being wired.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutputParams {
    /// Peak display luminance as a multiple of SDR reference white; 1.0 means no
    /// headroom, so `tonemap_hdr` degenerates to a hard clip at white.
    ///
    /// Already clamped to a believable band by
    /// [`voxel_color::DisplayHeadroom`](voxel_color::DisplayHeadroom), which is the only
    /// thing that should construct this — `tonemap_hdr` divides by `headroom - 1`, so a
    /// value below 1.0 would invert the highlight rolloff.
    pub hdr_headroom: f32,
    /// `y` — which tonemap the shading pass applies, as
    /// [`voxel_color::TonemapCurve::shader_index`]. A uniform rather than a shader const
    /// so the curves can be compared on the same frame instead of across a rebuild.
    pub tonemap: voxel_color::TonemapCurve,
    /// `z` — what BT.2390 assumes the scene's brightest pixel is, as a multiple of SDR
    /// reference white. Read by that curve only; see
    /// [`voxel_color::tonemap::DEFAULT_CONTENT_PEAK`] for why it is an assumption.
    pub content_peak: f32,
    /// `w` — scene exposure, applied BEFORE the tonemap.
    ///
    /// **The piece the whole output path was missing.** Without it the tonemap doubles as
    /// exposure: `SUN_INTENSITY` was tuned so a sunlit surface lands high on Reinhard's
    /// usable range (see its comment above), which means the curve — not the lighting —
    /// decides how bright the image is. That is why swapping to a curve that leaves
    /// mid-tones alone brightened the entire room, and why "what is the content peak?"
    /// had no answer: the scene's absolute scale was arbitrary, tuned to fit a curve.
    ///
    /// With exposure separate, the tonemap only shapes the highlight rolloff and the two
    /// can be judged independently.
    ///
    /// Defaults to 1.0, so nothing changes until someone moves it.
    pub exposure: f32,
}

impl Default for OutputParams {
    /// SDR: no headroom claimed. Matches `voxel_color::headroom::UNMEASURED_HEADROOM`, so
    /// a caller that never probes gets the conservative answer rather than the old
    /// optimistic 4.0.
    fn default() -> OutputParams {
        OutputParams {
            hdr_headroom: 1.0,
            tonemap: voxel_color::TonemapCurve::Reinhard,
            content_peak: voxel_color::tonemap::DEFAULT_CONTENT_PEAK,
            exposure: 1.0,
        }
    }
}

impl OutputParams {
    fn to_array(self) -> [f32; 4] {
        // Floored at 1.0 as a second line of defence. `DisplayHeadroom` already clamps,
        // but this struct is `pub` with a `pub` field and the shader has no way to
        // defend itself against a negative `room`.
        [
            self.hdr_headroom.max(1.0),
            self.tonemap.shader_index(),
            // Floored at 1.0 for the same reason as headroom: the EETF divides by the
            // content peak in PQ, and a zero would take the whole curve with it.
            self.content_peak.max(1.0),
            // Non-negative, but NOT floored at 1.0 — exposing down is as legitimate as
            // exposing up, and zero is a valid "black" for a fade.
            self.exposure.max(0.0),
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

/// This frame's GPU lighting data for `sun`.
///
/// A free function because [`SunSettings`] belongs to `voxel-environment` now — the mirror this
/// replaced was a field-for-field copy that existed only so this method could hang off it.
pub fn lighting_uniform(
    sun: &SunSettings,
    shading_params: ShadingParams,
    gi_params: GiParams,
    water_params: WaterParams,
    material_params: MaterialParams,
    animation_params: AnimationParams,
    event_params: EventParams,
) -> LightingUniform {
    let celestial = sun.environment_frame();
    LightingUniform {
        sun_direction: celestial.active_direction.to_array(),
        pad_a: 0.0,
        sun_color_intensity: [
            celestial.active_color[0],
            celestial.active_color[1],
            celestial.active_color[2],
            SUN_INTENSITY * sun.intensity_scale.max(0.0) * celestial.direct_strength,
        ],
        sky_ambient: [
            SKY_AMBIENT_COLOR[0] * (0.25 + 0.75 * celestial.daylight),
            SKY_AMBIENT_COLOR[1] * (0.25 + 0.75 * celestial.daylight),
            SKY_AMBIENT_COLOR[2],
            AMBIENT_STRENGTH * sun.ambient_scale.max(0.0) * celestial.ambient_strength,
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
            sun.moon_phase.clamp(0.0, 1.0),
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
        animation_params: animation_params.to_array(),
        event_params: event_params.to_array(),
        // SDR by default; the windowed app overrides via `with_output_params`.
        output_params: OutputParams::default().to_array(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    /// Every runtime knob vector, in one uniform, so the component-order tests
    /// below all read the same construction.
    fn probe_uniform(shadow_penumbra_scale: f32) -> LightingUniform {
        probe_uniform_for(SunSettings::default(), shadow_penumbra_scale)
    }

    /// The same, for a chosen sun — so a test about the sun's own knobs shares this
    /// construction rather than writing a second one that could drift from it.
    fn probe_uniform_for(sun: SunSettings, shadow_penumbra_scale: f32) -> LightingUniform {
        probe_uniform_full(
            sun,
            shadow_penumbra_scale,
            AnimationParams::default(),
            EventParams::default(),
        )
    }

    /// The one construction every probe funnels through, so a new vector cannot
    /// be added to the uniform without every existing component test seeing it.
    fn probe_uniform_full(
        sun: SunSettings,
        shadow_penumbra_scale: f32,
        animation_params: AnimationParams,
        event_params: EventParams,
    ) -> LightingUniform {
        crate::lighting::lighting_uniform(
            &sun,
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
                pattern_fade_start_meters: voxel_material::pattern::PATTERN_FADE_START_METERS,
                pattern_fade_end_meters: voxel_material::pattern::PATTERN_FADE_END_METERS,
                pixel_footprint_at_one_meter: 0.0,
            },
            animation_params,
            event_params,
        )
    }

    #[test]
    fn uniform_layout_is_gpu_ready() {
        assert_eq!(std::mem::size_of::<LightingUniform>(), 256);
        assert_eq!(std::mem::align_of::<LightingUniform>(), 4);
        assert_eq!(std::mem::size_of::<LightingUniform>() % 16, 0);
    }

    /// Headroom must default to NO headroom and must land in `x`.
    ///
    /// The default is the load-bearing half. Every offscreen caller — the benchmark, the
    /// CAGI probes, every test — skips `with_output_params`, so whatever this returns is
    /// what they render with. If it ever claimed headroom, they would all tone-map into
    /// range no display was asked about, which is the exact bug the measured probe
    /// replaced, reintroduced by omission.
    #[test]
    fn output_params_default_to_no_headroom_and_land_in_x() {
        let uniform = probe_uniform(115.0);
        assert_eq!(
            uniform.output_params,
            [
                1.0,
                voxel_color::TonemapCurve::Reinhard.shader_index(),
                voxel_color::tonemap::DEFAULT_CONTENT_PEAK,
                1.0
            ],
            "an unset headroom must be 1.0 (clip at white), never an optimistic guess; the \
             curve defaults to Reinhard, the content peak to the HDR10 baseline, and \
             exposure to 1.0 so the default look is unchanged"
        );

        let bright = uniform.with_output_params(OutputParams {
            hdr_headroom: 4.0,
            tonemap: voxel_color::TonemapCurve::HdrKnee,
            content_peak: 10.0,
            exposure: 2.0,
        });
        assert_eq!(
            bright.output_params,
            [
                4.0,
                voxel_color::TonemapCurve::HdrKnee.shader_index(),
                10.0,
                2.0
            ],
            "headroom in x, the curve index in y, the content peak in z"
        );
        // The builder must touch nothing else — it is bolted onto a finished uniform.
        assert_eq!(bright.material_params, uniform.material_params);
        assert_eq!(bright.event_params, uniform.event_params);
        assert_eq!(bright.animation_params, uniform.animation_params);

        // `tonemap_hdr` divides by `headroom - 1`, so a sub-1.0 value would invert the
        // highlight rolloff. `DisplayHeadroom` clamps, but this field is `pub` and the
        // shader cannot defend itself.
        let floored = uniform.with_output_params(OutputParams {
            hdr_headroom: -3.0,
            tonemap: voxel_color::TonemapCurve::HdrKnee,
            content_peak: 10.0,
            exposure: -1.0,
        });
        assert_eq!(floored.output_params[0], 1.0);
        // Exposure floors at ZERO, not one: exposing down is legitimate and a fade to
        // black wants 0. Only negative is nonsense.
        assert_eq!(floored.output_params[3], 0.0);
    }

    /// The clock must land in its own slots: swapping epoch and remainder
    /// would make every oscillator jump once a second instead of once an epoch.
    #[test]
    fn animation_params_keep_their_vector_components() {
        let uniform = probe_uniform(115.0);
        assert_eq!(uniform.animation_params, [0.0, 0.0, 0.0, 0.0]);

        let animated = probe_uniform_full(
            SunSettings::default(),
            115.0,
            AnimationParams {
                remainder_seconds: 12.5,
                epoch: 3.0,
                reserved_flow: 0.0,
                reserved: 0.0,
            },
            EventParams {
                remainder_seconds: 7.5,
                epoch: 4.0,
                event_count: 2.0,
            },
        );
        assert_eq!(animated.animation_params, [12.5, 3.0, 0.0, 0.0]);
        assert_eq!(animated.event_params, [7.5, 4.0, 2.0, 0.0]);
        // ...and the material knobs must be untouched by the new vector.
        assert_eq!(
            animated.material_params,
            [
                voxel_material::pattern::PATTERN_FADE_START_METERS,
                voxel_material::pattern::PATTERN_FADE_END_METERS,
                0.0,
                0.0
            ]
        );
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
