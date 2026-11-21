// Directivity patterns for sources and listeners.
//
// Both source emission and listener reception are directional. A directivity
// pattern maps an angle (from the entity's forward direction) to a gain (0–1).
// The same math applies to both ends:
//
//   perceived_energy = source_directivity(emission_angle)
//                    × path_transfer(distance, reflections, occlusion)
//                    × receiver_directivity(arrival_angle)
//
// See docs/directivity-and-energy-transfer.md for the full derivation.

use crate::types::Vec3;

/// A directivity pattern: a function from angle to gain.
#[derive(Clone, Copy, Debug)]
pub enum DirectivityPattern {
    /// Gain = 1.0 everywhere. Campfire, ambient, subwoofer.
    Omni,

    /// Three-parameter cone (WebAudio PannerNode semantics).
    /// `inner` and `outer` are half-angles in radians.
    /// Full gain inside inner, linear interpolation to outer_gain, floor beyond outer.
    Cone {
        inner: f32,
        outer: f32,
        outer_gain: f32,
    },

    /// Polar pattern: gain = alpha + (1 - alpha) * cos(θ).
    /// alpha=1.0 → omni, 0.5 → cardioid, 0.37 → supercardioid, 0.25 → hypercardioid.
    Polar { alpha: f32 },

    /// A beam of any width with some sound behind it, the way a sound's
    /// shape is set: `gain = behind + (1 − behind) · ((1 + cos θ) / 2)^k`.
    /// `k` puts the point where the gain is halfway between front and back
    /// at `half_width`. A half width of π is omni; π/2 with nothing behind is
    /// exactly a cardioid. Build it with [`DirectivityPattern::beam`].
    Beam {
        /// Half the beam's width (radians).
        half_width: f32,
        /// Gain straight behind, 0–1.
        behind: f32,
        /// The exponent that gives `half_width`.
        sharpness: f32,
    },
}

/// A level this far below the front, or lower, is silence behind (dB).
pub const SILENT_BEHIND_DB: f32 = -60.0;

impl DirectivityPattern {
    pub const OMNI: DirectivityPattern = DirectivityPattern::Omni;

    pub fn cardioid() -> Self {
        DirectivityPattern::Polar { alpha: 0.5 }
    }

    pub fn supercardioid() -> Self {
        DirectivityPattern::Polar { alpha: 0.37 }
    }

    /// A beam `width_degrees` wide, from a few degrees up to 360 (all round),
    /// with the sound straight behind it `behind_db` below the front: 0 dB
    /// as loud, [`SILENT_BEHIND_DB`] or lower silent. The width is where the
    /// level is halfway between front and back.
    pub fn beam(width_degrees: f32, behind_db: f32) -> Self {
        let half_width = (width_degrees.clamp(1.0, 360.0) / 2.0).to_radians();
        let behind = if behind_db <= SILENT_BEHIND_DB {
            0.0
        } else {
            10f32.powf(behind_db.min(0.0) / 20.0)
        };
        // ((1 + cos w) / 2)^k = 1/2 at the half width w.
        let at_edge = (1.0 + half_width.cos()) / 2.0;
        let sharpness = if at_edge <= 1e-6 {
            0.0
        } else {
            0.5f32.ln() / at_edge.ln()
        };
        DirectivityPattern::Beam {
            half_width,
            behind,
            sharpness,
        }
    }

    /// Return the polar alpha coefficient (1.0 for omni, 0.5 for cardioid, etc.).
    /// Used for serialization to the browser UI.
    pub fn alpha(&self) -> f32 {
        match self {
            DirectivityPattern::Omni => 1.0,
            DirectivityPattern::Polar { alpha } => *alpha,
            DirectivityPattern::Cone { .. } => 1.0,
            // A polar pattern's alpha is its gain at 90°.
            DirectivityPattern::Beam { .. } => self.gain_at_angle(std::f32::consts::FRAC_PI_2),
        }
    }

    /// Evaluate the pattern at a given angle (radians, 0 = forward, π = behind).
    pub fn gain_at_angle(&self, angle: f32) -> f32 {
        let angle = angle.abs();
        match self {
            DirectivityPattern::Omni => 1.0,
            DirectivityPattern::Cone {
                inner,
                outer,
                outer_gain,
            } => {
                if angle <= *inner {
                    1.0
                } else if angle >= *outer {
                    *outer_gain
                } else {
                    let t = (angle - inner) / (outer - inner);
                    1.0 + t * (outer_gain - 1.0)
                }
            }
            DirectivityPattern::Polar { alpha } => {
                // gain = alpha + (1 - alpha) * cos(theta)
                // Clamp to 0 — hypercardioid can go slightly negative at rear
                (alpha + (1.0 - alpha) * angle.cos()).max(0.0)
            }
            DirectivityPattern::Beam {
                behind, sharpness, ..
            } => {
                let lobe = ((1.0 + angle.cos()) / 2.0).max(0.0).powf(*sharpness);
                behind + (1.0 - behind) * lobe
            }
        }
    }
}

/// Compute the directivity factor γ for a radiation pattern.
///
/// γ measures how focused the pattern is relative to omnidirectional:
///   γ = 2 / ∫₀^π g(θ)² sin(θ) dθ
///
/// where g(θ) is the pattern's gain at angle θ. The integral is evaluated
/// using composite Simpson's rule with 64 steps (sufficient for smooth patterns).
///
/// Known values:
///   - Omni → 1.0
///   - Cardioid (α=0.5) → 3.0
///   - Supercardioid (α=0.37) → ~3.7
///   - Hypercardioid (α=0.25) → ~4.0
///
/// Used in the critical distance formula: d_c = 0.057 × √(γ × V / RT60).
/// A higher γ means the source's direct sound dominates further from the source.
pub fn directivity_factor(pattern: &DirectivityPattern) -> f32 {
    if matches!(pattern, DirectivityPattern::Omni) {
        return 1.0;
    }

    // Composite Simpson's rule: ∫₀^π g(θ)² sin(θ) dθ
    const N: usize = 64; // must be even
    let h = std::f32::consts::PI / N as f32;
    let mut sum = 0.0f32;

    for i in 0..=N {
        let theta = i as f32 * h;
        let g = pattern.gain_at_angle(theta);
        let y = g * g * theta.sin();

        let weight = if i == 0 || i == N {
            1.0
        } else if i % 2 == 1 {
            4.0
        } else {
            2.0
        };
        sum += weight * y;
    }
    let integral = sum * h / 3.0;

    if integral < 1e-6 {
        return 1.0; // degenerate pattern — fallback to omni
    }
    2.0 / integral
}

/// Compute directivity gain from an entity (source or listener) toward a target point.
///
/// Works identically for source emission and listener reception:
/// - For sources: entity_pos = source position, entity_facing = source forward, target = listener
/// - For listeners: entity_pos = listener position, entity_facing = listener forward, target = source
pub fn directivity_gain(
    entity_pos: Vec3,
    entity_facing: Vec3,
    target_pos: Vec3,
    pattern: &DirectivityPattern,
) -> f32 {
    if matches!(pattern, DirectivityPattern::Omni) {
        return 1.0;
    }

    let to_target = target_pos - entity_pos;
    if to_target.length() < 1e-6 {
        return 1.0; // target at entity position — degenerate
    }
    let to_target = to_target.normalize();
    let cos_angle = entity_facing.normalize().dot(to_target).clamp(-1.0, 1.0);
    let angle = cos_angle.acos();
    pattern.gain_at_angle(angle)
}

#[cfg(test)]
mod tests;
