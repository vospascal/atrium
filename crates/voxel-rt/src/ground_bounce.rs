//! C5: the ground-bounce aggregate the cloud deck reads.
//!
//! Clouds are lit from below as well as above. Without it, cloud undersides stay a flat grey
//! at sunset while the terrain beneath them goes orange — the single most obvious tell that a
//! sky and a world were rendered by two unrelated systems.
//!
//! # Why this is not a CAGI lookup
//!
//! The obvious implementation is to sample the light volume at each cloud sample. That cannot
//! work: CAGI's grid is deliberately clamped in Y to the terrain's occupied height plus
//! `SKY_MARGIN_CELLS`, and `cagi_cell_radiance` returns one constant for any
//! cell above it. A cloud sample hundreds of metres up therefore reads the same value
//! everywhere — not terrain bounce, not sunset warmth. The grid comment is right that
//! allocating that space would be "paying for a constant"; growing it to cloud altitude is not
//! the fix.
//!
//! What a cloud needs from the ground is low-frequency by nature, which is what makes an
//! aggregate sufficient. Order-1 spherical harmonics is the cheapest representation that still
//! carries a *direction* — which the packed RGB in a CAGI cell does not — so it goes to the GPU
//! as 16 floats on the environment request rather than as a texture binding.
//!
//! # What this version approximates
//!
//! The coefficients are derived analytically from the sun and sky state times a representative
//! ground albedo, NOT reduced from the CAGI volume. That captures the sunset and daylight
//! response, which is the visible win. It does NOT capture local emissive sources — lava, a lit
//! window, a torch — tinting the cloud above them. Doing that needs a GPU reduction over the
//! volume's top layer whose result stays on the GPU; the shader side is already written for it,
//! since it only reads these four coefficients.

use voxel_environment::EnvironmentFrame;

/// Fraction of incident light a representative terrain surface returns.
///
/// A single number rather than a material average because it multiplies an aggregate that is
/// already low-frequency: grass, rock and sand differ by less than the sun's own hourly
/// variation, and a weighted average over the visible world would be precision applied to the
/// wrong term.
pub const GROUND_ALBEDO: [f32; 3] = [0.28, 0.26, 0.21];

/// Share of the ground's outgoing radiance that reaches the deck.
///
/// The rest is absorbed by the air column between them or scattered out of the path. Folded in
/// here so the shader's SH evaluation stays a plain dot product.
const GROUND_TO_DECK_TRANSMITTANCE: f32 = 0.55;

/// Order-1 SH of upward ground radiance, in the layout
/// [`voxel_environment::CloudRequest::ground_bounce_sh`] expects.
///
/// `[0]` is the constant term and `[1..4]` the linear x/y/z terms; each `xyz` is RGB with `w`
/// unused. Basis constants are folded in here, so the shader evaluates
/// `constant + linear · direction` and nothing else.
pub fn ground_bounce_sh(frame: &EnvironmentFrame) -> [[f32; 4]; 4] {
    // Irradiance landing on the ground: the sun's own contribution, scaled by how high it is,
    // plus the sky's. At night the first term vanishes and only the second remains, which is
    // why a moonlit deck still has a faintly lit underside rather than a black one.
    let sun_elevation = frame.active_direction.y.max(0.0);

    let mut constant = [0.0f32; 4];
    let mut linear_y = [0.0f32; 4];
    for channel in 0..3 {
        // Ground colour comes from the light that hit it, so a warm low sun makes warm bounce
        // without a separate sunset tint anywhere in this file.
        let direct = frame.active_illuminance[channel] * sun_elevation;
        let ambient = frame.ambient_scale * frame.active_color[channel];
        let outgoing = (direct + ambient) * GROUND_ALBEDO[channel] * GROUND_TO_DECK_TRANSMITTANCE;
        constant[channel] = outgoing;
        // Negative Y: the radiance leaves the ground travelling UP, so it is strongest in the
        // downward-looking direction a cloud sample queries. The magnitude is deliberately
        // below the constant term — a Lambertian ground is a broad lobe, not a beam, and an
        // L1 term larger than L0 would make the SH go negative looking upward.
        linear_y[channel] = -outgoing * 0.45;
    }

    // X and Z stay zero: an analytic uniform ground has no horizontal asymmetry. They exist in
    // the layout because a real CAGI reduction would fill them, and the shader already reads
    // them — so that upgrade needs no shader change.
    [constant, [0.0; 4], linear_y, [0.0; 4]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use voxel_environment::state::SunSettings;

    fn frame_at(day_phase: f32) -> EnvironmentFrame {
        SunSettings {
            day_phase,
            ..SunSettings::default()
        }
        .environment_frame()
    }

    /// Evaluate the SH the way the shader does, so the test exercises the real contract rather
    /// than the coefficients in isolation.
    fn evaluate(sh: [[f32; 4]; 4], direction: Vec3) -> [f32; 3] {
        let mut result = [0.0f32; 3];
        for channel in 0..3 {
            result[channel] = (sh[0][channel]
                + sh[1][channel] * direction.x
                + sh[2][channel] * direction.y
                + sh[3][channel] * direction.z)
                .max(0.0);
        }
        result
    }

    #[test]
    fn noon_ground_bounce_is_brighter_than_night() {
        let noon = evaluate(ground_bounce_sh(&frame_at(0.5)), Vec3::NEG_Y);
        let midnight = evaluate(ground_bounce_sh(&frame_at(0.0)), Vec3::NEG_Y);
        assert!(
            noon[0] > midnight[0],
            "noon {noon:?} must exceed midnight {midnight:?}"
        );
    }

    /// The sunset case is the whole point of the feature: cloud undersides must go warm when
    /// the terrain does. Warm means red exceeds blue by more than it does at noon.
    #[test]
    fn sunset_bounce_is_warmer_than_noon() {
        let warmth = |phase| {
            let value = evaluate(ground_bounce_sh(&frame_at(phase)), Vec3::NEG_Y);
            value[0] / value[2].max(1e-6)
        };
        assert!(
            warmth(0.255) > warmth(0.5),
            "sunrise warmth {} must exceed noon {}",
            warmth(0.255),
            warmth(0.5)
        );
    }

    /// The L1 term must not exceed L0, or the reconstruction goes negative looking upward —
    /// which would subtract light from the top of the deck.
    #[test]
    fn the_reconstruction_stays_non_negative_in_every_direction() {
        for phase in [0.0, 0.25, 0.3, 0.5, 0.75, 0.9] {
            let sh = ground_bounce_sh(&frame_at(phase));
            for direction in [
                Vec3::Y,
                Vec3::NEG_Y,
                Vec3::X,
                Vec3::NEG_X,
                Vec3::Z,
                Vec3::NEG_Z,
            ] {
                // Evaluated WITHOUT the max(0) clamp the shader applies, so a negative value
                // fails here rather than being silently hidden.
                let basis = [1.0, direction.x, direction.y, direction.z];
                for channel in 0..3 {
                    let value: f32 = sh
                        .iter()
                        .zip(basis)
                        .map(|(coefficients, weight)| coefficients[channel] * weight)
                        .sum();
                    assert!(
                        value >= 0.0,
                        "phase {phase} direction {direction:?} channel {channel} went negative: {value}"
                    );
                }
            }
        }
    }

    /// Looking down at the ground must be brighter than looking up at the sky, or the lobe is
    /// pointing the wrong way and the deck would be lit from above twice.
    #[test]
    fn the_lobe_points_downward() {
        let sh = ground_bounce_sh(&frame_at(0.5));
        let below = evaluate(sh, Vec3::NEG_Y);
        let above = evaluate(sh, Vec3::Y);
        assert!(below[0] > above[0]);
    }
}
