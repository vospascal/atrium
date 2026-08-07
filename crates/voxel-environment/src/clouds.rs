//! The cloud deck: user-facing settings, and the per-frame payload the GPU reads.
//!
//! Clouds live in this crate for the same reason the sky does — the cloud a camera sees and
//! the cloud shadow that darkens the ground must be one object. Splitting them would let the
//! visible deck and the light it blocks drift apart, which is the failure the crate exists to
//! prevent.
//!
//! [`CloudSettings`] is the authored description and owns the wind integration.
//! [`CloudRequest`] is the flattened result stated to the GPU each frame, and lives on
//! [`crate::EnvironmentRequest`] beside the sun.

/// Authored cloud-deck description.
///
/// Split from [`CloudRequest`] because these are the knobs a panel edits, while the request
/// is what the uniform reads. The wind *offset* is integrated here rather than derived from a
/// clock in the shader, so the deck's position is reproducible from a frame sequence and a
/// paused app has genuinely still clouds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloudSettings {
    /// Skip the march entirely. The density field and shadow map are still allocated —
    /// this is a per-frame decision, not a configuration one.
    pub enabled: bool,
    /// Fraction of sky covered, 0 clear to 1 overcast. Drives the visible deck, the shadow map
    /// the ground reads, and the sky attenuation in `environment_sky_ambient_at`.
    pub coverage: f32,
    /// Extinction coefficient σₜ scale, per world unit of cloud traversed.
    pub extinction: f32,
    /// Single-scatter albedo σₛ/σₜ.
    ///
    /// Deliberately a separate field from [`Self::extinction`] rather than the usual
    /// conflation. Cloud sits at ~0.999, where the two coincide and every reference writes
    /// one symbol for both — but the `Media` transparency class also covers smoke at
    /// ~0.2–0.5, and a shared coefficient would render smoke as a glowing cloud.
    pub albedo: f32,
    /// Deck base altitude in renderer world units.
    pub bottom_world: f32,
    /// Deck thickness in renderer world units.
    pub thickness_world: f32,
    /// 0 stratus (flat), 0.5 cumulus (billowing), 1 cumulonimbus (towering).
    ///
    /// One lerp across the height-density gradient rather than three code paths, so a
    /// weather transition can cross cloud types continuously.
    pub cloud_type: f32,
    /// Strength of the high-frequency Worley erosion that carves the base shape.
    pub detail_strength: f32,
    /// How much coverage varies across the sky, 0 uniform to 1 fully patchy.
    ///
    /// Without this the deck reads as ONE continuous mass: a single coverage number applied to the
    /// whole sky gives no large-scale organisation, so the 3D noise only modulates an unbroken
    /// slab. Nubis states it plainly — coverage is "a FUNCTION of our weather system", a 2D map
    /// over the world rather than a scalar. This is the cheap stand-in for that map.
    pub weather_variation: f32,
    /// Weather-map precipitation channel, 0 fair to 1 rain-bearing storm. It thickens cloud
    /// extinction so the same shape can distinguish a dry cumulus from a dark rain shaft.
    pub precipitation: f32,
    /// Multiplier applied to the shaped density so cloud cores saturate.
    ///
    /// Exists because the density chain's natural output tops out around 0.37 — measured — and a
    /// deck whose densest point is a third opaque renders as haze at any extinction you can
    /// reasonably set. Values above 1 are the normal case, not a hack: the noise supplies the
    /// *shape*, this supplies the *substance*, and clamping at 1 keeps cores from going
    /// non-physical.
    pub density_scale: f32,
    /// Extinction applied to the three-tap upward sky-occlusion term.
    pub ambient_density: f32,
    /// Strength of the Beer–Powder rim term.
    ///
    /// Non-physical and aesthetic, as its originators say. Beer's law alone renders cloud
    /// edges dark because thin cloud transmits nearly everything; powder restores the bright
    /// rim. Applied only to sun-facing in-scatter — applying it to transmittance is the usual
    /// way to get this wrong.
    pub powder_strength: f32,
    /// Forward Henyey–Greenstein lobe eccentricity, the silver lining looking sunward.
    pub forward_scatter: f32,
    /// Back lobe eccentricity, negative. The pair is what makes a cloud read as cloud
    /// rather than as fog.
    pub back_scatter: f32,
    /// Primary view-ray sample budget, distributed logarithmically in depth.
    pub primary_steps: u32,
    /// Cone light-march taps toward the sun per in-cloud primary sample.
    pub light_steps: u32,
    /// Horizontal wind velocity in world units per second, from the weather model.
    pub wind: [f32; 2],
    /// Integrated advection offset. Advanced by [`Self::advance`]; never read from a clock.
    pub wind_offset: [f32; 3],
}

impl Default for CloudSettings {
    fn default() -> Self {
        Self {
            // OFF by default (2026-08-07): the per-sky-pixel march is the DDA
            // pass's largest native-resolution cost in-app (it took the frame
            // to ~60 fps on an M3 Max laptop display), and with the deck off
            // every consumer degrades cleanly — `cloud_shadow_at` returns 1.0,
            // so sun/sky injection (CAGI included) reads clear-sky values.
            // The panel toggle re-enables per session; named weather
            // conditions still bring their own deck when selected.
            enabled: false,
            // Pascal's authored baseline: a medium, broken cumulus deck that remains readable
            // under the physical Hillaire sky without flattening it. These are the panel values
            // shipped for the hand-dialled mode; named weather conditions still replace the shape
            // fields when selected.
            coverage: 0.70,
            extinction: 0.015,
            albedo: 1.0,
            bottom_world: 600.0,
            thickness_world: 300.0,
            cloud_type: 0.55,
            detail_strength: 0.50,
            weather_variation: 0.75,
            precipitation: 0.0,
            density_scale: 1.8,
            ambient_density: 0.38,
            powder_strength: 0.5,
            forward_scatter: 0.8,
            back_scatter: -0.2,
            primary_steps: 35,
            light_steps: 7,
            wind: [5.0, 1.5],
            wind_offset: [0.0, 0.0, 0.0],
        }
    }
}

impl CloudSettings {
    /// Integrate the wind offset. Call once per frame with the frame's elapsed seconds.
    ///
    /// Only the horizontal components move: a deck that drifts vertically would slide out of
    /// its own altitude band, and real advection at this scale is horizontal.
    pub fn advance(&mut self, elapsed_seconds: f32) {
        let elapsed = elapsed_seconds.max(0.0);
        self.wind_offset[0] += self.wind[0] * elapsed;
        self.wind_offset[2] += self.wind[1] * elapsed;
    }

    /// Deck top altitude in world units.
    pub fn top_world(&self) -> f32 {
        self.bottom_world + self.thickness_world.max(1.0)
    }

    /// How much this deck dims light reaching the ground, 0 none to 1 total.
    ///
    /// Deliberately *not* a light multiplier. An overcast sky still delivers a lot of light —
    /// it arrives diffusely instead of directionally — and how that plays out is derived in the
    /// shader (`environment_sky_ambient_at`) from the deck's own transmittance, not from a curve
    /// authored here. Two such curves USED to live on this type and were never applied to
    /// anything; deriving it in one place is what stops that recurring.
    pub fn overcast(&self) -> f32 {
        self.coverage.clamp(0.0, 1.0)
    }

    /// Flatten to the payload the GPU reads.
    pub fn request(&self) -> CloudRequest {
        CloudRequest {
            coverage: self.overcast(),
            extinction: self.extinction.max(0.0),
            albedo: self.albedo.clamp(0.0, 1.0),
            bottom_world: self.bottom_world,
            thickness_world: self.thickness_world.max(1.0),
            cloud_type: self.cloud_type.clamp(0.0, 1.0),
            detail_strength: self.detail_strength.max(0.0),
            weather_variation: self.weather_variation.clamp(0.0, 1.0),
            precipitation: self.precipitation.clamp(0.0, 1.0),
            density_scale: self.density_scale.clamp(0.0, 8.0),
            ambient_density: self.ambient_density.max(0.0),
            powder_strength: self.powder_strength.max(0.0),
            forward_scatter: self.forward_scatter.clamp(-0.95, 0.95),
            back_scatter: self.back_scatter.clamp(-0.95, 0.95),
            primary_steps: if self.enabled {
                self.primary_steps.clamp(1, 256)
            } else {
                0
            },
            light_steps: self.light_steps.clamp(1, 32),
            wind_offset: self.wind_offset,
            ground_bounce_sh: CloudRequest::NO_GROUND_BOUNCE,
        }
    }
}

/// The per-frame cloud payload, stated by the renderer and read by the uniform.
///
/// Plain data with clamps already applied, so the shader never defends against a negative
/// thickness and the uniform-writing code has no policy in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloudRequest {
    pub coverage: f32,
    pub extinction: f32,
    pub albedo: f32,
    pub bottom_world: f32,
    pub thickness_world: f32,
    pub cloud_type: f32,
    pub detail_strength: f32,
    pub weather_variation: f32,
    pub precipitation: f32,
    pub density_scale: f32,
    pub ambient_density: f32,
    pub powder_strength: f32,
    pub forward_scatter: f32,
    pub back_scatter: f32,
    /// Zero disables the march for this frame without disturbing any allocation.
    pub primary_steps: u32,
    pub light_steps: u32,
    pub wind_offset: [f32; 3],
    /// Order-1 spherical harmonics of upward radiance leaving the ground, per channel.
    ///
    /// `[0]` is the constant term, `[1..4]` the linear x/y/z terms; `xyz` of each is RGB and
    /// `w` is unused padding so the WGSL side is a plain `array<vec4<f32>, 4>`.
    ///
    /// This is the C5 ground-bounce aggregate, and it is *data on the request* rather than a
    /// texture binding for a specific reason: CAGI's grid stops just above the terrain, so a
    /// cloud sample kilometres up cannot read the volume at all. What a cloud needs from the
    /// ground is low-frequency by nature, and SH-L1 is the cheapest representation that still
    /// carries a direction — which plain packed RGB in a CAGI cell does not.
    pub ground_bounce_sh: [[f32; 4]; 4],
}

impl CloudRequest {
    /// No light returning from the ground. The correct value before C5 supplies a real one.
    pub const NO_GROUND_BOUNCE: [[f32; 4]; 4] = [[0.0; 4]; 4];

    /// Whether the deck should be marched at all this frame.
    pub fn marches(&self) -> bool {
        self.primary_steps > 0 && self.coverage > 0.0
    }

    /// What a transition from `previous` invalidates in cloud-derived state.
    ///
    /// Only the shadow map is cached, so this is one bit. Note what is *absent*: coverage does
    /// not invalidate any atmosphere LUT. The deck attenuates the sky on read instead of
    /// being integrated into the tables, which keeps the LUTs pure atmosphere and makes a
    /// coverage change free.
    pub fn invalidates_shadow_since(&self, previous: &Self) -> bool {
        self != previous
    }
}

impl Default for CloudRequest {
    fn default() -> Self {
        CloudSettings::default().request()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wind_advances_horizontally_only() {
        let mut settings = CloudSettings {
            wind: [4.0, -2.0],
            ..CloudSettings::default()
        };
        settings.advance(2.0);
        assert_eq!(settings.wind_offset, [8.0, 0.0, -4.0]);
    }

    #[test]
    fn a_paused_frame_does_not_move_the_deck() {
        let mut settings = CloudSettings::default();
        settings.advance(0.0);
        assert_eq!(settings.wind_offset, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn disabling_stops_the_march_without_changing_shape() {
        let settings = CloudSettings {
            enabled: false,
            ..CloudSettings::default()
        };
        let request = settings.request();
        assert!(!request.marches());
        assert_eq!(request.thickness_world, settings.thickness_world);
    }

    /// Nubis' density chain, pinned in the shader's structure.
    ///
    /// The height gradient shapes the procedural modeling profile before coverage remapping, and
    /// the profile then feeds the Nubis up-resolve. This keeps the weather-map meaning intact
    /// while preserving the vertical base/top profile.
    #[test]
    fn height_gradient_precedes_modeling_coverage_remap() {
        let source = crate::hillaire::shaders::WGSL;
        assert!(
            source.contains("cloud_modeling_profile(base_field, gradient, coverage)"),
            "the height gradient must shape the modeling profile first"
        );
        let remap = source
            .find("cloud_remap(modeling_profile, 1.0 - coverage")
            .expect("coverage must remap the shaped base");
        let coverage = source
            .find("fn cloud_modeling_profile")
            .expect("coverage must enter the modeling profile");
        assert!(coverage < remap, "modeling coverage must precede the remap");
    }

    /// Cores have to be able to saturate, or the deck is haze at any extinction.
    #[test]
    fn the_density_scale_reaches_the_shader() {
        let source = crate::hillaire::shaders::WGSL;
        assert!(source.contains("fn cloud_density_scale()"));
        assert!(source.contains("fn cloud_modeling_density_scale(ndf: vec4<f32>)"));
        assert!(source.contains("let powered_scale = pow(modeling_density_scale, 4.0)"));
        // Above 1 is the normal case: the noise supplies shape, this supplies substance.
        assert!(CloudSettings::default().density_scale > 1.0);
    }

    #[test]
    fn evolved_density_keeps_the_close_detail_and_camera_fades() {
        let source = crate::hillaire::shaders::WGSL;
        assert!(source.contains("fn cloud_ndf_at"));
        assert!(source.contains("const CLOUD_NDF_EXTENT_WORLD: f32 = 16384.0"));
        assert!(source.contains("fn cloud_noise_mip_level"));
        assert!(
            source.contains("textureSampleLevel(field, field_sampler, base_uvw, noise_mip_level)")
        );
        assert!(source.contains("fn cloud_evolved_high_frequency_noise"));
        assert!(source.contains("cloud_value_remap(distance_world, 50.0, 150.0, 0.9, 1.0)"));
        assert!(source.contains("cloud_value_remap(sample_distance, 10.0, 120.0, 0.25, 1.0)"));
    }

    #[test]
    fn evolved_light_keeps_primary_secondary_and_edge_terms_separate() {
        let source = crate::hillaire::shaders::WGSL;
        assert!(source.contains("struct CloudLight"));
        assert!(source.contains("fn cloud_primary_phase"));
        assert!(source.contains("fn cloud_secondary_phase"));
        assert!(source.contains("fn cloud_envelope_shadow"));
        assert!(source.contains("fn cloud_envelope_ambient_factor"));
        assert!(source.contains("let clear_envelope = pow(1.0 - coarse_density, 0.25)"));
        assert!(source.contains("fn cloud_sun_rim"));
        assert!(source.contains("sun_light.transmittance * primary_phase"));
        assert!(source.contains("let envelope_secondary = sun_light.multiple_scattering"));
        assert!(source.contains("let edge = 1.0 - powder"));
        assert!(source.contains("(f32(tap) + 0.5) * step_length"));
    }

    #[test]
    fn shipped_cloud_baseline_matches_the_hand_dialled_preset() {
        let settings = CloudSettings::default();
        assert_eq!(settings.coverage, 0.70);
        assert_eq!(settings.cloud_type, 0.55);
        assert_eq!(settings.bottom_world, 600.0);
        assert_eq!(settings.thickness_world, 300.0);
        assert_eq!(settings.extinction, 0.015);
        assert_eq!(settings.weather_variation, 0.75);
        assert_eq!(settings.density_scale, 1.8);
        assert_eq!(settings.detail_strength, 0.50);
        assert_eq!(settings.powder_strength, 0.50);
        assert_eq!(settings.forward_scatter, 0.80);
        assert_eq!(settings.back_scatter, -0.20);
        assert_eq!(settings.ambient_density, 0.38);
        assert_eq!(settings.albedo, 1.0);
        assert_eq!(settings.primary_steps, 35);
        assert_eq!(settings.light_steps, 7);
    }

    /// The base field must be FLATTENED to ~uniform, or `coverage` does not mean coverage.
    ///
    /// The consumer computes `remap(base, 1.0 - coverage, 1.0, 0.0, 1.0)`, which only yields "a
    /// `coverage` fraction of the sky has cloud" when the field is uniformly distributed. It is not,
    /// naturally: measured, the shaped Perlin–Worley comes out p05 0.001 / p50 0.330 / p95 0.688.
    ///
    /// This has now caused two visible failures. First, coverage 0.45 set a floor of 0.55 that most
    /// of the field could not clear. Then the weather map lowered *local* coverage to ~0.31, making
    /// the floor 0.693 against a p95 of 0.688 — **no clouds at all**, which is exactly what the app
    /// showed. Neither was visible in any test, because nothing related the field's distribution to
    /// the meaning of the coverage parameter.
    #[test]
    fn the_base_field_is_flattened_so_coverage_means_coverage() {
        let source = crate::hillaire::shaders::LUT_WGSL;
        assert!(
            source.contains("remap_range(shaped, 0.001, 0.688, 0.05, 0.95)"),
            "the distribution-flattening stretch must remain in the noise generator"
        );
    }

    /// In-scattered radiance must not depend on the step count or on how far the ray travels.
    ///
    /// This is the bug that made the deck look wrong once it was finally visible. The march used the
    /// differential form `(direct + ambient) * sigma_s * density * ds`, valid only while per-step
    /// optical depth is far below 1. It is **1.6 overhead and about 33 near the horizon**, so the
    /// integral ran away: the same cloud measured 0.99 looking up and **3.72 at the horizon**, and
    /// drifted 1.18 -> 0.85 as steps went 24 -> 192. Brightness was set by ray geometry, not light.
    ///
    /// Ported to CPU rather than asserted as a string match, because the defect was arithmetic —
    /// the shader read plausibly and its comment described the intent correctly. Every prior cloud
    /// bug this session was found by measuring and missed by structural tests.
    #[test]
    fn in_scattered_radiance_is_invariant_to_step_count_and_travel() {
        let settings = CloudSettings::default();
        let extinction = settings.extinction;
        let scattering_coefficient = extinction * settings.albedo;

        // The shader's accumulation over a uniform-density interior — the case that matters, since
        // a varying profile would confound step count with where the samples happen to land.
        let march = |steps: u32, travel: f32| -> f32 {
            let mut scattering = 0.0f32;
            let mut transmittance = 1.0f32;
            let mut previous = 0.0f32;
            for step in 0..steps {
                let linear = (step as f32 + 0.5) / steps as f32;
                let warped = ((linear * 2.2).exp() - 1.0) / (2.2f32).exp_m1();
                let distance = travel * warped;
                let step_length = (distance - previous).max(0.0001);
                previous = distance;
                let density = 1.0f32;
                let incoming = 1.0f32;
                let sample_extinction = (density * extinction).max(1.0e-6);
                let step_transmittance = (-sample_extinction * step_length).exp();
                let integrated = density * scattering_coefficient * (1.0 - step_transmittance)
                    / sample_extinction;
                scattering += transmittance * incoming * integrated;
                transmittance *= step_transmittance;
                if transmittance < 0.01 {
                    break;
                }
            }
            scattering
        };

        // A deck crossed overhead, and the same deck crossed at a grazing angle toward the horizon.
        let reference = march(48, settings.thickness_world);
        for steps in [24u32, 48, 96, 192] {
            for travel in [settings.thickness_world, 20_000.0] {
                let measured = march(steps, travel);
                assert!(
                    (measured - reference).abs() < 0.02,
                    "{steps} steps over {travel} m gave {measured}, not {reference}: in-scatter \
                     depends on ray geometry, so the integration is not energy-conserving"
                );
            }
        }

        // And it must stay bounded by the albedo: an optically thick deck cannot scatter back more
        // than arrives. This is the property the differential form lacked entirely.
        assert!(
            reference <= settings.albedo + 1.0e-3,
            "in-scatter {reference} exceeds the single-scatter albedo {}",
            settings.albedo
        );
    }

    /// Octave frequencies must stay integers, because the field tiles.
    ///
    /// Heckel drifts his lacunarity (`factor += 0.21`) to decorrelate octaves, and that is right
    /// for a 2D texture lookup that never wraps a lattice. Here `cloud_value_3d` wraps on the
    /// frequency itself, so a fractional ratio would put seams across the whole sky.
    #[test]
    fn perlin_octave_frequencies_are_integers_so_the_field_tiles() {
        let source = crate::hillaire::shaders::LUT_WGSL;
        assert!(
            source.contains("array<f32, 4>(4.0, 9.0, 19.0, 41.0)"),
            "octave frequencies must be the integer, decorrelated set"
        );
        assert!(
            !source.contains("lacunarity += 0.21"),
            "a drifting lacunarity yields non-integer periods and breaks tiling"
        );
    }

    #[test]
    fn albedo_and_extinction_are_independent_fields() {
        // Smoke: strongly absorbing. If these were one coefficient it would glow.
        let smoke = CloudSettings {
            albedo: 0.3,
            extinction: 0.9,
            ..CloudSettings::default()
        };
        let request = smoke.request();
        assert!(request.albedo < 0.5 && request.extinction > 0.5);
    }
}
