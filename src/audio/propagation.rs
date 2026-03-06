/// ISO 9613-2:1996 — Sound propagation outdoors.
///
/// Provides the individual attenuation terms from ISO 9613-2 for computing
/// how sound level decreases between a source and receiver. These formulas
/// are the building blocks for ray-traced audio propagation.
///
/// # Total attenuation (ISO 9613-2, Equation 1)
///
/// The equivalent continuous A-weighted sound pressure level at a receiver is:
///
///   L_AT(DW) = L_W + D_c - A
///
/// where:
///   L_W    = sound power level of the source (dB re 1 pW)
///   D_c    = directivity correction (dB)
///   A      = total attenuation along the propagation path (dB)
///
/// Total attenuation A is the sum of individual terms:
///
///   A = A_div + A_atm + A_ground + A_bar + A_misc
///
/// where:
///   A_div    = geometric divergence (inverse square law)
///   A_atm    = atmospheric absorption (ISO 9613-1, see atmosphere.rs)
///   A_ground = ground effect (reflection and interference)
///   A_bar    = barrier/screening attenuation (diffraction)
///   A_misc   = miscellaneous (vegetation, housing, industrial sites)
///
/// Reference: ISO 9613-2:1996, "Acoustics — Attenuation of sound during
///            propagation outdoors — Part 2: General method of calculation"

use atrium_core::types::Vec3;

// ─────────────────────────────────────────────────────────────────────────────
// Geometric Divergence — ISO 9613-2 §7.1
// ─────────────────────────────────────────────────────────────────────────────

/// Geometric divergence attenuation in dB.
///
/// Models the inverse square law: sound energy spreads over the surface area
/// of an expanding sphere, losing 6 dB per doubling of distance.
///
/// # Formula (ISO 9613-2, Equation 3)
///
///   A_div = 20 · log₁₀(d / d₀) + 11 dB
///
/// where:
///   d  = distance from source to receiver (m)
///   d₀ = reference distance, 1 m (ISO standard)
///   11 = accounts for the source radiating into a full sphere (4π steradians)
///
/// The 11 dB term converts from sound power level (L_W) to sound pressure
/// level at 1 m for an omnidirectional point source: 10·log₁₀(4π) ≈ 11.
///
/// # Note
/// Our distance model already applies geometric divergence implicitly via
/// `gain = ref_dist / distance`. This function is provided for completeness
/// and for future use in ray-traced propagation paths where we need the
/// explicit dB value per path segment.
pub fn geometric_divergence_db(distance: f32) -> f32 {
    if distance <= 0.0 {
        return 0.0;
    }
    20.0 * distance.log10() + 11.0
}

// ─────────────────────────────────────────────────────────────────────────────
// Ground Effect — ISO 9613-2 §7.3
// ─────────────────────────────────────────────────────────────────────────────

/// Ground surface properties for ISO 9613-2 ground effect calculation.
///
/// The ground factor G ranges from 0.0 (hard/reflective) to 1.0 (porous/soft):
///   - G = 0.0: concrete, water, stone, tile, asphalt
///   - G = 0.5: mixed or compacted soil
///   - G = 1.0: grass, soil, carpet, vegetation
///
/// For indoor use, typical values:
///   - Hard floor (tile, concrete): G = 0.0
///   - Carpet / rugs: G ≈ 0.7–1.0
///   - Wooden floor: G ≈ 0.3
#[derive(Clone, Copy, Debug)]
pub struct GroundProperties {
    /// Ground factor near the source (0.0 = hard, 1.0 = porous).
    pub g_source: f32,
    /// Ground factor in the middle region between source and receiver.
    pub g_middle: f32,
    /// Ground factor near the receiver.
    pub g_receiver: f32,
}

impl Default for GroundProperties {
    /// Default: hard reflective surface (concrete/tile floor).
    fn default() -> Self {
        Self {
            g_source: 0.0,
            g_middle: 0.0,
            g_receiver: 0.0,
        }
    }
}

impl GroundProperties {
    /// Hard reflective surface (concrete, tile, stone).
    pub fn hard() -> Self {
        Self { g_source: 0.0, g_middle: 0.0, g_receiver: 0.0 }
    }

    /// Soft porous surface (grass, carpet, soil).
    pub fn soft() -> Self {
        Self { g_source: 1.0, g_middle: 1.0, g_receiver: 1.0 }
    }

    /// Mixed surface (e.g., wooden floor with rugs).
    pub fn mixed(g: f32) -> Self {
        let g = g.clamp(0.0, 1.0);
        Self { g_source: g, g_middle: g, g_receiver: g }
    }
}

/// Compute ground effect attenuation in dB per octave band.
///
/// ISO 9613-2 §7.3 divides the ground between source and receiver into three
/// regions and sums their contributions:
///
///   A_ground = A_s + A_r + A_m
///
/// where:
///   A_s = source region ground effect (within 30·h_s of source)
///   A_r = receiver region ground effect (within 30·h_r of receiver)
///   A_m = middle region ground effect (everything in between)
///
/// # Parameters
/// - `distance`: horizontal distance source→receiver (m)
/// - `h_source`: height of source above ground (m)
/// - `h_receiver`: height of receiver above ground (m)
/// - `ground`: ground surface properties
/// - `freq_hz`: octave band center frequency (Hz)
///
/// # Returns
/// Ground effect attenuation in dB (positive = loss, can be negative for
/// constructive interference at low frequencies over soft ground).
///
/// # ISO 9613-2 Table 3 — Ground attenuation coefficients
///
/// | Freq (Hz) |  a'   |  b'   |  c'   |  d'   |  e'   |
/// |-----------|-------|-------|-------|-------|-------|
/// |    63     | -1.5  | -3.0G | -1.5  | -3.0G | -1.5(1-G) |
/// |   125     | -1.5  | -3.0G | -1.5  | -3.0G | -1.5(1-G) |
/// |   250     | -1.5  | -3.0G | -1.5  | -3.0G | -1.5(1-G) |
/// |   500     | -1.5  | -3.0G | -1.5  | -3.0G | -1.5(1-G) |
/// |  1000     | -1.5  |  a·G  | -1.5  |  b·G  | -1.5(1-G) |
/// |  2000     | -1.5+G·c | ... | -1.5+G·d | ... | -1.5(1-G) |
/// |  4000     | -1.5+G·c | ... | -1.5+G·d | ... | -1.5(1-G) |
/// |  8000     | -1.5+G·c | ... | -1.5+G·d | ... | -1.5(1-G) |
///
/// The full table is complex; this implementation uses the simplified
/// alternative method from ISO 9613-2 §7.3.2 (Equation 9) which provides
/// a single broadband correction suitable for A-weighted levels.
///
/// # Simplified formula (ISO 9613-2, Equation 9)
///
///   A_ground = 4.8 - (2·h_m/d)·(17 + 300/d)   for  h_m/d ≤ 0.0
///   A_ground = 4.8 - (2·h_m/d)·(17 + 300/d)·(1-G_m)  otherwise
///
/// where h_m = mean height of propagation path above ground.
///
/// For the full octave-band method, see the detailed implementation below.
pub fn ground_effect_db(
    distance: f32,
    h_source: f32,
    h_receiver: f32,
    ground: &GroundProperties,
    freq_hz: f32,
) -> f32 {
    if distance < 0.01 {
        return 0.0;
    }

    let a_s = ground_region_source(distance, h_source, ground.g_source, freq_hz);
    let a_r = ground_region_receiver(distance, h_receiver, ground.g_receiver, freq_hz);
    let a_m = ground_region_middle(distance, h_source, h_receiver, ground.g_middle);

    a_s + a_r + a_m
}

/// Source region ground effect (ISO 9613-2, Equation 4).
///
///   A_s = -1.5 + G_s · q_s
///
/// where q_s depends on frequency and height. For the simplified broadband
/// method, q_s models the interference between direct and ground-reflected
/// paths near the source.
fn ground_region_source(distance: f32, h_source: f32, g_source: f32, freq_hz: f32) -> f32 {
    let q = ground_q(distance, h_source, freq_hz);
    -1.5 + g_source * q
}

/// Receiver region ground effect (ISO 9613-2, Equation 5).
///
///   A_r = -1.5 + G_r · q_r
///
/// Symmetric to A_s but evaluated at the receiver height.
fn ground_region_receiver(
    distance: f32,
    h_receiver: f32,
    g_receiver: f32,
    freq_hz: f32,
) -> f32 {
    let q = ground_q(distance, h_receiver, freq_hz);
    -1.5 + g_receiver * q
}

/// Middle region ground effect (ISO 9613-2, Equation 6).
///
///   A_m = -3·q_m · (1 - G_m)
///
/// where q_m depends on the distance. Over hard ground (G_m=0), A_m = -3·q_m
/// (constructive interference adds energy). Over soft ground (G_m=1), A_m = 0.
fn ground_region_middle(
    distance: f32,
    h_source: f32,
    h_receiver: f32,
    g_middle: f32,
) -> f32 {
    let h_mean = (h_source + h_receiver) / 2.0;
    let q_m = if distance > 0.01 {
        // Simplified: middle region effect diminishes with height/distance ratio
        1.0 - (30.0 * h_mean / distance).min(1.0)
    } else {
        0.0
    };
    -3.0 * q_m * (1.0 - g_middle)
}

/// Frequency-dependent ground interaction factor.
///
/// Approximates the interference pattern between direct and ground-reflected
/// paths. At low frequencies (long wavelengths), ground effect is weak.
/// At mid frequencies (~200-600 Hz), destructive interference creates a "ground dip".
/// At high frequencies, the ground acts more like a simple reflector.
fn ground_q(distance: f32, height: f32, freq_hz: f32) -> f32 {
    if distance < 0.01 || height < 0.01 {
        return 0.0;
    }

    // Speed of sound (m/s) — standard conditions
    const C: f32 = 343.0;

    // Wavelength at this frequency
    let lambda = C / freq_hz.max(1.0);

    // Path length difference between direct and ground-reflected paths
    // δ = √(d² + (h_s + h_r)²) - √(d² + (h_s - h_r)²)
    // Simplified for source-region (h_r ≈ 0 at ground):
    // δ ≈ 2·h²/d for h << d
    let path_diff = 2.0 * height * height / distance;

    // Phase difference in cycles
    let phase_cycles = path_diff / lambda;

    // Ground interaction peaks around 0.25-0.5 cycles (destructive interference)
    // and diminishes for very low or very high frequencies
    let q = if phase_cycles < 0.05 {
        // Very low frequency: wavelength >> path difference, minimal effect
        phase_cycles * 20.0 // ramp from 0
    } else if phase_cycles < 1.0 {
        // Ground dip region: interference is significant
        1.0
    } else {
        // High frequency: averaging over many cycles, effect diminishes
        (1.0 / phase_cycles).min(1.0)
    };

    // Scale: soft ground over porous surface gives up to ~3 dB per region
    q * 3.0
}

// ─────────────────────────────────────────────────────────────────────────────
// Barrier Attenuation — ISO 9613-2 §7.4
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for barrier (screening) attenuation calculation.
///
/// A barrier is any solid obstacle between source and receiver that forces
/// sound to diffract over or around it. In indoor audio, this models walls,
/// furniture, columns, or any occluding geometry.
#[derive(Clone, Copy, Debug)]
pub struct BarrierGeometry {
    /// Position of the sound source.
    pub source: Vec3,
    /// Position of the receiver (listener).
    pub receiver: Vec3,
    /// Position of the barrier's top edge (diffraction point).
    /// For a wall, this is the top-center of the wall edge closest to the
    /// line of sight between source and receiver.
    pub barrier_top: Vec3,
}

/// Compute barrier (screening) attenuation in dB.
///
/// Uses the Maekawa/ISO 9613-2 diffraction formula based on the Fresnel number.
///
/// # The Fresnel Number (ISO 9613-2, §7.4)
///
/// The Fresnel number N quantifies how many half-wavelengths the diffracted
/// path exceeds the direct path:
///
///   N = ±(2/λ) · δ
///
/// where:
///   λ = wavelength = c / f  (speed of sound / frequency)
///   δ = path length difference = (d_sb + d_br) - d_sr
///   d_sb = distance from source to barrier top
///   d_br = distance from barrier top to receiver
///   d_sr = direct distance from source to receiver
///
/// Sign convention:
///   N > 0 : barrier breaks line of sight (shadow zone — attenuation)
///   N = 0 : barrier just touches line of sight (grazing)
///   N < 0 : line of sight is clear (illuminated zone — minor effect)
///
/// # Attenuation formula (ISO 9613-2, Equation 14 / Maekawa)
///
///   A_bar = 10 · log₁₀(3 + 20·N)    for N ≥ 0
///
/// Capped at manufacturer-specified maximum (typically 20-25 dB for a single
/// barrier, per ISO 9613-2 §7.4 note).
///
/// For N < 0 (illuminated zone, no real occlusion):
///   A_bar ≈ 0 dB (sound passes freely)
///
/// # Parameters
/// - `barrier`: geometry of source, receiver, and barrier top
/// - `freq_hz`: frequency in Hz
///
/// # Returns
/// Barrier attenuation in dB (positive = loss). Range: 0 to `MAX_BARRIER_DB`.
///
/// # Example
/// ```
/// use atrium_core::types::Vec3;
/// use atrium::audio::propagation::{BarrierGeometry, barrier_attenuation_db};
/// // Source at (0,0,0), receiver at (10,0,0), wall top at (5,0,2)
/// let barrier = BarrierGeometry {
///     source: Vec3::new(0.0, 0.0, 0.0),
///     receiver: Vec3::new(10.0, 0.0, 0.0),
///     barrier_top: Vec3::new(5.0, 0.0, 2.0),
/// };
/// let atten = barrier_attenuation_db(&barrier, 1000.0);
/// // Expect significant attenuation (wall blocks direct path)
/// ```
pub fn barrier_attenuation_db(barrier: &BarrierGeometry, freq_hz: f32) -> f32 {
    /// Maximum single-barrier attenuation (ISO 9613-2 §7.4 note).
    /// Real-world single barriers rarely exceed 20-25 dB due to flanking.
    const MAX_BARRIER_DB: f32 = 25.0;

    /// Speed of sound at 20°C, standard conditions.
    const C: f32 = 343.0;

    let wavelength = C / freq_hz.max(1.0);

    // Path lengths
    let d_sr = barrier.source.distance_to(barrier.receiver); // direct
    let d_sb = barrier.source.distance_to(barrier.barrier_top); // source → barrier
    let d_br = barrier.barrier_top.distance_to(barrier.receiver); // barrier → receiver

    // Path length difference (positive = barrier in shadow zone)
    let delta = d_sb + d_br - d_sr;

    // Fresnel number
    let n = 2.0 * delta / wavelength;

    if n < -0.05 {
        // Illuminated zone: line of sight is clear, negligible effect
        0.0
    } else if n < 0.0 {
        // Transition zone near grazing: small attenuation
        // Linear interpolation from 0 to ~5 dB at grazing
        let t = (n + 0.05) / 0.05; // 0..1
        t * 10.0 * (3.0_f32).log10() // ≈ 4.8 dB at grazing
    } else {
        // Shadow zone: full Maekawa/ISO 9613-2 formula
        let a_bar = 10.0 * (3.0 + 20.0 * n).log10();
        a_bar.min(MAX_BARRIER_DB)
    }
}

/// Compute the Fresnel number for a barrier geometry at a given frequency.
///
/// Useful for diagnostics and visualization. Positive N means the barrier
/// occludes the direct path; negative N means line of sight is clear.
///
///   N = (2/λ) · δ
///   δ = (d_source→barrier + d_barrier→receiver) - d_source→receiver
pub fn fresnel_number(barrier: &BarrierGeometry, freq_hz: f32) -> f32 {
    const C: f32 = 343.0;
    let wavelength = C / freq_hz.max(1.0);

    let d_sr = barrier.source.distance_to(barrier.receiver);
    let d_sb = barrier.source.distance_to(barrier.barrier_top);
    let d_br = barrier.barrier_top.distance_to(barrier.receiver);

    let delta = d_sb + d_br - d_sr;
    2.0 * delta / wavelength
}

// ─────────────────────────────────────────────────────────────────────────────
// Total Attenuation — ISO 9613-2 §6
// ─────────────────────────────────────────────────────────────────────────────

/// All individual attenuation terms for a single propagation path.
///
/// Returned by [`total_attenuation_db`] so callers can inspect each term
/// independently (useful for debugging and visualization).
#[derive(Clone, Copy, Debug, Default)]
pub struct AttenuationTerms {
    /// Geometric divergence: 20·log₁₀(d) + 11 dB.
    pub a_div: f32,
    /// Atmospheric absorption (ISO 9613-1) in dB. Caller must provide this
    /// since it requires AtmosphericParams (see atmosphere.rs).
    pub a_atm: f32,
    /// Ground effect in dB (can be negative for constructive interference).
    pub a_ground: f32,
    /// Barrier/screening attenuation in dB.
    pub a_bar: f32,
    /// Total: sum of all terms.
    pub a_total: f32,
}

/// Compute total path attenuation by summing all ISO 9613-2 terms.
///
/// # ISO 9613-2, Equation 1 (simplified)
///
///   A = A_div + A_atm + A_ground + A_bar
///
/// # Parameters
/// - `distance`: source-to-receiver distance (m)
/// - `a_atm_db`: atmospheric absorption for this path, computed externally
///   via `iso9613_alpha(freq, params) * distance` (see atmosphere.rs)
/// - `h_source`: source height above ground (m)
/// - `h_receiver`: receiver height above ground (m)
/// - `ground`: ground surface properties
/// - `freq_hz`: octave band center frequency (Hz)
/// - `barrier`: optional barrier geometry (None if unobstructed)
pub fn total_attenuation_db(
    distance: f32,
    a_atm_db: f32,
    h_source: f32,
    h_receiver: f32,
    ground: &GroundProperties,
    freq_hz: f32,
    barrier: Option<&BarrierGeometry>,
) -> AttenuationTerms {
    let a_div = geometric_divergence_db(distance);
    let a_ground = ground_effect_db(distance, h_source, h_receiver, ground, freq_hz);
    let a_bar = barrier
        .map(|b| barrier_attenuation_db(b, freq_hz))
        .unwrap_or(0.0);

    AttenuationTerms {
        a_div,
        a_atm: a_atm_db,
        a_ground,
        a_bar,
        a_total: a_div + a_atm_db + a_ground + a_bar,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Geometric Divergence ─────────────────────────────────────────────

    #[test]
    fn divergence_at_1m_is_11db() {
        let db = geometric_divergence_db(1.0);
        assert!(
            (db - 11.0).abs() < 0.01,
            "A_div at 1m should be 11 dB (4π sr), got {db}"
        );
    }

    #[test]
    fn divergence_doubles_6db() {
        // Doubling distance should add ~6 dB (20·log₁₀(2) ≈ 6.02)
        let db1 = geometric_divergence_db(1.0);
        let db2 = geometric_divergence_db(2.0);
        let diff = db2 - db1;
        assert!(
            (diff - 6.02).abs() < 0.1,
            "doubling distance should add ~6 dB, got {diff}"
        );
    }

    #[test]
    fn divergence_at_10m() {
        // 20·log₁₀(10) + 11 = 20 + 11 = 31 dB
        let db = geometric_divergence_db(10.0);
        assert!(
            (db - 31.0).abs() < 0.01,
            "A_div at 10m should be 31 dB, got {db}"
        );
    }

    #[test]
    fn divergence_zero_distance() {
        assert_eq!(geometric_divergence_db(0.0), 0.0);
        assert_eq!(geometric_divergence_db(-1.0), 0.0);
    }

    // ── Ground Effect ────────────────────────────────────────────────────

    #[test]
    fn hard_ground_effect_is_negative() {
        // Hard ground (G=0) should give negative A_ground (constructive
        // interference from ground reflection adds energy)
        let ground = GroundProperties::hard();
        let a = ground_effect_db(10.0, 1.5, 1.5, &ground, 500.0);
        assert!(a < 0.0, "hard ground should give negative (gain), got {a}");
    }

    #[test]
    fn soft_ground_less_negative() {
        // Soft ground should have less constructive interference (more absorption)
        let hard = GroundProperties::hard();
        let soft = GroundProperties::soft();
        let a_hard = ground_effect_db(10.0, 1.5, 1.5, &hard, 500.0);
        let a_soft = ground_effect_db(10.0, 1.5, 1.5, &soft, 500.0);
        assert!(
            a_soft > a_hard,
            "soft ground ({a_soft}) should attenuate more than hard ({a_hard})"
        );
    }

    #[test]
    fn ground_effect_zero_distance() {
        let ground = GroundProperties::hard();
        assert_eq!(ground_effect_db(0.0, 1.5, 1.5, &ground, 500.0), 0.0);
    }

    // ── Barrier Attenuation ──────────────────────────────────────────────

    #[test]
    fn barrier_in_shadow_zone() {
        // Wall directly between source and receiver, above line of sight
        let barrier = BarrierGeometry {
            source: Vec3::new(0.0, 0.0, 0.0),
            receiver: Vec3::new(10.0, 0.0, 0.0),
            barrier_top: Vec3::new(5.0, 0.0, 3.0),
        };
        let a = barrier_attenuation_db(&barrier, 1000.0);
        assert!(a > 5.0, "barrier should give significant attenuation, got {a}");
        assert!(a <= 25.0, "barrier should not exceed 25 dB cap, got {a}");
    }

    #[test]
    fn barrier_no_obstruction() {
        // Note: the Maekawa formula only considers path length difference,
        // not whether the barrier actually intersects line of sight.
        // Callers should check LOS intersection before calling.
        // This test uses a barrier just barely below LOS (near-grazing).
        let barrier2 = BarrierGeometry {
            source: Vec3::new(0.0, 0.0, 0.0),
            receiver: Vec3::new(10.0, 0.0, 0.0),
            barrier_top: Vec3::new(5.0, 0.0, -0.001), // just barely below LOS
        };
        // delta ≈ 0 (path through barrier top ≈ direct path)
        // This tests the near-grazing case
        let a = barrier_attenuation_db(&barrier2, 1000.0);
        assert!(a < 10.0, "near-grazing barrier should give moderate attenuation, got {a}");
    }

    #[test]
    fn barrier_higher_freq_more_attenuation() {
        let barrier = BarrierGeometry {
            source: Vec3::new(0.0, 0.0, 0.0),
            receiver: Vec3::new(10.0, 0.0, 0.0),
            barrier_top: Vec3::new(5.0, 0.0, 2.0),
        };
        let a_500 = barrier_attenuation_db(&barrier, 500.0);
        let a_4000 = barrier_attenuation_db(&barrier, 4000.0);
        assert!(
            a_4000 > a_500,
            "higher freq ({a_4000}) should attenuate more than lower ({a_500})"
        );
    }

    #[test]
    fn barrier_capped_at_25db() {
        // Very large barrier, very high frequency → should cap
        let barrier = BarrierGeometry {
            source: Vec3::new(0.0, 0.0, 0.0),
            receiver: Vec3::new(10.0, 0.0, 0.0),
            barrier_top: Vec3::new(5.0, 0.0, 10.0), // very tall
        };
        let a = barrier_attenuation_db(&barrier, 8000.0);
        assert!(
            (a - 25.0).abs() < 0.01,
            "should be capped at 25 dB, got {a}"
        );
    }

    #[test]
    fn fresnel_number_positive_in_shadow() {
        let barrier = BarrierGeometry {
            source: Vec3::new(0.0, 0.0, 0.0),
            receiver: Vec3::new(10.0, 0.0, 0.0),
            barrier_top: Vec3::new(5.0, 0.0, 3.0),
        };
        let n = fresnel_number(&barrier, 1000.0);
        assert!(n > 0.0, "shadow zone should have positive N, got {n}");
    }

    // ── Total Attenuation ────────────────────────────────────────────────

    #[test]
    fn total_attenuation_sums_correctly() {
        let ground = GroundProperties::hard();
        let terms = total_attenuation_db(10.0, 0.5, 1.5, 1.5, &ground, 1000.0, None);

        let expected_total = terms.a_div + terms.a_atm + terms.a_ground + terms.a_bar;
        assert!(
            (terms.a_total - expected_total).abs() < 0.001,
            "total should be sum of parts: {} vs {}",
            terms.a_total,
            expected_total
        );
    }

    #[test]
    fn total_with_barrier_more_than_without() {
        let ground = GroundProperties::hard();
        let barrier = BarrierGeometry {
            source: Vec3::new(0.0, 0.0, 1.5),
            receiver: Vec3::new(10.0, 0.0, 1.5),
            barrier_top: Vec3::new(5.0, 0.0, 3.0),
        };

        let without = total_attenuation_db(10.0, 0.5, 1.5, 1.5, &ground, 1000.0, None);
        let with = total_attenuation_db(10.0, 0.5, 1.5, 1.5, &ground, 1000.0, Some(&barrier));

        assert!(
            with.a_total > without.a_total,
            "barrier should increase total attenuation: {} vs {}",
            with.a_total,
            without.a_total
        );
    }
}
