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
        Self {
            g_source: 0.0,
            g_middle: 0.0,
            g_receiver: 0.0,
        }
    }

    /// Soft porous surface (grass, carpet, soil).
    pub fn soft() -> Self {
        Self {
            g_source: 1.0,
            g_middle: 1.0,
            g_receiver: 1.0,
        }
    }

    /// Mixed surface (e.g., wooden floor with rugs).
    pub fn mixed(g: f32) -> Self {
        let g = g.clamp(0.0, 1.0);
        Self {
            g_source: g,
            g_middle: g,
            g_receiver: g,
        }
    }
}

/// Compute ground effect attenuation in dB per octave band.
///
/// Implements ISO 9613-2 §7.3 (detailed method) using the three-region
/// decomposition with Table 3 anchor values:
///
///   A_ground = A_s + A_r + A_m
///
/// where:
///   A_s = source region ground effect (within 30·h_s of source)
///   A_r = receiver region ground effect (within 30·h_r of receiver)
///   A_m = middle region ground effect (everything in between)
///
/// Source/receiver regions use the octave-band structure from Table 3:
///   - f ≤ 125 Hz:  A = -1.5 - 3.0·G  (porous ground boosts low-freq reflections)
///   - f ≥ 2000 Hz: A = -1.5·(1-G)     (porous ground absorbs high-freq reflections)
///   - Transition zone (250–1000 Hz) includes a height-dependent ground dip
///     from destructive interference at the frequency where path_diff ≈ λ/2.
///
/// Middle region uses the ISO threshold at d = 30·(h_s + h_r), smoothed
/// to avoid discontinuities in real-time rendering.
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
/// constructive interference over reflective ground).
pub fn ground_effect_db(
    distance: f32,
    h_source: f32,
    h_receiver: f32,
    ground: &GroundProperties,
    freq_hz: f32,
    speed_of_sound: f32,
) -> f32 {
    if distance < 0.01 {
        return 0.0;
    }

    let a_s = ground_region_source(distance, h_source, ground.g_source, freq_hz, speed_of_sound);
    let a_r = ground_region_receiver(
        distance,
        h_receiver,
        ground.g_receiver,
        freq_hz,
        speed_of_sound,
    );
    let a_m = ground_region_middle(distance, h_source, h_receiver, ground.g_middle);

    a_s + a_r + a_m
}

/// Source region ground effect (ISO 9613-2, Table 3).
///
/// Uses the octave-band anchor values from ISO 9613-2 Table 3:
///   - f ≤ 125 Hz: A_s = -1.5 - 3.0·G_s  (porous ground boosts low-freq reflections)
///   - f ≥ 2000 Hz: A_s = -1.5·(1 - G_s)  (porous ground absorbs high-freq reflections)
///   - 125 < f < 2000 Hz: interpolation with height-dependent ground dip
///
/// The -1.5 dB base term is the coherent ground reflection gain (present for
/// all surface types). The G-dependent correction captures how porous surfaces
/// behave differently at low vs. high frequencies.
fn ground_region_source(
    distance: f32,
    h_source: f32,
    g_source: f32,
    freq_hz: f32,
    speed_of_sound: f32,
) -> f32 {
    iso_ground_region(g_source, freq_hz, h_source, distance, speed_of_sound)
}

/// Receiver region ground effect (ISO 9613-2, Table 3).
///
/// Symmetric to A_s but evaluated at the receiver height.
fn ground_region_receiver(
    distance: f32,
    h_receiver: f32,
    g_receiver: f32,
    freq_hz: f32,
    speed_of_sound: f32,
) -> f32 {
    iso_ground_region(g_receiver, freq_hz, h_receiver, distance, speed_of_sound)
}

/// Compute source or receiver region ground effect per ISO 9613-2 Table 3.
///
/// The attenuation has two components:
///   A = -1.5 + G × C(f, h, d)
///
/// where C is a frequency-dependent correction factor:
///   - C = -3.0 at f ≤ 125 Hz (Table 3 rows 63–125 Hz)
///   - C = +1.5 at f ≥ 2000 Hz (Table 3 rows 2000–8000 Hz)
///   - C interpolates through the transition zone (250–1000 Hz) where the
///     "ground dip" occurs — destructive interference when the path difference
///     between direct and ground-reflected waves ≈ λ/2.
fn iso_ground_region(g: f32, freq_hz: f32, height: f32, distance: f32, speed_of_sound: f32) -> f32 {
    // Table 3 anchor values for the G-dependent correction term
    const C_LOW: f32 = -3.0; // f ≤ 125 Hz
    const C_HIGH: f32 = 1.5; // f ≥ 2000 Hz

    let correction = if freq_hz <= 125.0 {
        C_LOW
    } else if freq_hz >= 2000.0 {
        C_HIGH
    } else {
        // Log-frequency interpolation between Table 3 anchor points
        // ln(2000/125) = ln(16) ≈ 2.773
        let t = (freq_hz / 125.0).ln() / (2000.0_f32 / 125.0).ln();
        let base = C_LOW + (C_HIGH - C_LOW) * t;

        // Ground dip: additional attenuation near the destructive interference
        // frequency, where the path difference ≈ λ/2.
        // This models the height-dependent terms in Table 3 rows 250–1000 Hz.
        if distance > 0.1 && height > 0.01 {
            // Path difference for ground reflection: δ ≈ 2h²/d (far-field approx)
            let path_diff = 2.0 * height * height / distance;
            let lambda = speed_of_sound / freq_hz;
            let phase_ratio = path_diff / lambda;

            // Gaussian peak centered at phase_ratio = 0.5 (half-wavelength)
            // Width tuned so dip spans roughly one octave around the dip frequency
            let dip = (-((phase_ratio - 0.5) * 4.0).powi(2)).exp() * 3.0;
            base + dip
        } else {
            base
        }
    };

    -1.5 + g * correction
}

/// Middle region ground effect (ISO 9613-2, §7.3, Equation 6).
///
///   A_m = -3 · q · (1 - G_m)
///
/// The middle region exists only when the propagation distance exceeds
/// 30·(h_s + h_r) — i.e., source and receiver ground-interaction zones
/// don't overlap. Over hard ground (G_m=0), A_m = -3·q (constructive
/// interference adds energy). Over soft ground (G_m=1), A_m = 0.
///
/// ISO specifies a hard threshold at d = 30·(h_s + h_r). This implementation
/// uses a smooth ramp (±20%) to avoid discontinuities in real-time rendering.
fn ground_region_middle(distance: f32, h_source: f32, h_receiver: f32, g_middle: f32) -> f32 {
    let h_sum = h_source + h_receiver;

    // ISO 9613-2: middle region exists when d > 30·(h_s + h_r)
    // Smoothed: linear ramp from 80% to 120% of threshold
    let q = if h_sum > 0.001 {
        let threshold = 30.0 * h_sum;
        let ratio = distance / threshold;
        ((ratio - 0.8) / 0.4).clamp(0.0, 1.0)
    } else {
        // Heights near zero → no ground interaction zones, full middle region
        if distance > 0.01 {
            1.0
        } else {
            0.0
        }
    };

    -3.0 * q * (1.0 - g_middle)
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

/// A barrier obstacle defined in the room (wall, column, furniture).
///
/// The diffraction edge runs from `base` to `top`. For a simple wall,
/// `base` is the floor-level edge point and `top` is the top-of-wall
/// edge point nearest the source-receiver line of sight.
#[derive(Clone, Copy, Debug)]
pub struct Barrier {
    /// Base of the diffraction edge (e.g., floor level).
    pub base: Vec3,
    /// Top of the diffraction edge (diffraction point).
    pub top: Vec3,
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
/// let atten = barrier_attenuation_db(&barrier, 1000.0, 343.42);
/// // Expect significant attenuation (wall blocks direct path)
/// ```
/// Determine the sign of the Fresnel number: +1.0 if the barrier top is above
/// (or on) the source-receiver line of sight, -1.0 if below.
///
/// Projects the barrier top onto the source-receiver line and compares heights.
/// This implements the ISO 9613-2 §7.4 sign convention where N > 0 means
/// shadow zone and N < 0 means illuminated zone.
fn barrier_sign(source: &Vec3, receiver: &Vec3, barrier_top: &Vec3) -> f32 {
    // Parameter t: fraction along source→receiver line closest to barrier_top (in XY).
    let sr = *receiver - *source;
    let sb = *barrier_top - *source;
    let sr_len_sq = sr.x * sr.x + sr.y * sr.y + sr.z * sr.z;
    if sr_len_sq < 1e-10 {
        return 1.0; // degenerate: source ≈ receiver
    }
    let t = (sb.x * sr.x + sb.y * sr.y + sb.z * sr.z) / sr_len_sq;
    let t = t.clamp(0.0, 1.0);

    // Height of LOS at parameter t.
    let los_z = source.z + t * sr.z;

    if barrier_top.z >= los_z {
        1.0 // shadow zone (barrier above LOS)
    } else {
        -1.0 // illuminated zone (barrier below LOS)
    }
}

pub fn barrier_attenuation_db(barrier: &BarrierGeometry, freq_hz: f32, speed_of_sound: f32) -> f32 {
    /// Maximum single-barrier attenuation (ISO 9613-2 §7.4 note).
    /// Real-world single barriers rarely exceed 20-25 dB due to flanking.
    const MAX_BARRIER_DB: f32 = 25.0;

    let wavelength = speed_of_sound / freq_hz.max(1.0);

    // Path lengths
    let d_sr = barrier.source.distance_to(barrier.receiver); // direct
    let d_sb = barrier.source.distance_to(barrier.barrier_top); // source → barrier
    let d_br = barrier.barrier_top.distance_to(barrier.receiver); // barrier → receiver

    // Path length difference (always ≥ 0 by triangle inequality).
    let delta = d_sb + d_br - d_sr;

    // ISO 9613-2 sign convention: N is negative when the barrier top is
    // below the line of sight (illuminated zone). Determine sign by checking
    // whether the barrier top is above or below the source-receiver line.
    let sign = barrier_sign(&barrier.source, &barrier.receiver, &barrier.barrier_top);

    // Fresnel number (signed)
    let n = sign * 2.0 * delta / wavelength;

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
pub fn fresnel_number(barrier: &BarrierGeometry, freq_hz: f32, speed_of_sound: f32) -> f32 {
    let wavelength = speed_of_sound / freq_hz.max(1.0);

    let d_sr = barrier.source.distance_to(barrier.receiver);
    let d_sb = barrier.source.distance_to(barrier.barrier_top);
    let d_br = barrier.barrier_top.distance_to(barrier.receiver);

    let delta = d_sb + d_br - d_sr;
    let sign = barrier_sign(&barrier.source, &barrier.receiver, &barrier.barrier_top);
    sign * 2.0 * delta / wavelength
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
    speed_of_sound: f32,
) -> AttenuationTerms {
    let a_div = geometric_divergence_db(distance);
    let a_ground = ground_effect_db(
        distance,
        h_source,
        h_receiver,
        ground,
        freq_hz,
        speed_of_sound,
    );
    let a_bar = barrier
        .map(|b| barrier_attenuation_db(b, freq_hz, speed_of_sound))
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
// Ground Effect — Real-time linear gain
// ─────────────────────────────────────────────────────────────────────────────

/// Compute a broadband ground effect as a linear gain multiplier for real-time use.
///
/// Averages the ISO 9613-2 ground effect across octave bands (125 Hz – 4 kHz)
/// and converts from dB to a linear amplitude factor. This is an approximation
/// suitable for the per-source mixing loop where we don't have per-band processing.
///
/// Returns a gain in (0, ~1.5]. Values > 1.0 indicate constructive interference
/// over hard ground. Values < 1.0 indicate absorption by soft ground.
pub fn ground_effect_gain(
    distance: f32,
    h_source: f32,
    h_receiver: f32,
    ground: &GroundProperties,
    speed_of_sound: f32,
) -> f32 {
    if distance < 0.01 {
        return 1.0;
    }

    // Average ground effect across perceptually important octave bands
    const BANDS: &[f32] = &[125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0];
    let sum_db: f32 = BANDS
        .iter()
        .map(|&f| ground_effect_db(distance, h_source, h_receiver, ground, f, speed_of_sound))
        .sum();
    let avg_db = sum_db / BANDS.len() as f32;

    // Convert dB attenuation to linear gain.
    // ground_effect_db returns positive = loss, negative = gain,
    // so gain = 10^(-avg_db / 20)
    10.0_f32.powf(-avg_db / 20.0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Barrier — Real-time linear gain
// ─────────────────────────────────────────────────────────────────────────────

/// Compute a broadband barrier attenuation as a linear gain multiplier.
///
/// Averages the ISO 9613-2 §7.4 barrier attenuation across octave bands
/// (125 Hz – 4 kHz) and converts from dB to a linear amplitude factor.
/// Same averaging approach as [`ground_effect_gain`].
///
/// Returns a gain in (0, 1.0]. 1.0 means no attenuation (barrier not in path
/// or below line of sight).
pub fn barrier_attenuation_gain(barrier: &BarrierGeometry, speed_of_sound: f32) -> f32 {
    const BANDS: &[f32] = &[125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0];
    let sum_db: f32 = BANDS
        .iter()
        .map(|&f| barrier_attenuation_db(barrier, f, speed_of_sound))
        .sum();
    let avg_db = sum_db / BANDS.len() as f32;

    // barrier_attenuation_db returns positive = loss,
    // so gain = 10^(-avg_db / 20)
    10.0_f32.powf(-avg_db / 20.0)
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

    const TEST_SPEED: f32 = 343.42;

    #[test]
    fn hard_ground_effect_is_negative() {
        // Hard ground (G=0) should give negative A_ground (constructive
        // interference from ground reflection adds energy)
        let ground = GroundProperties::hard();
        let a = ground_effect_db(10.0, 1.5, 1.5, &ground, 500.0, TEST_SPEED);
        assert!(a < 0.0, "hard ground should give negative (gain), got {a}");
    }

    #[test]
    fn soft_ground_less_negative() {
        // Soft ground should have less constructive interference (more absorption)
        let hard = GroundProperties::hard();
        let soft = GroundProperties::soft();
        let a_hard = ground_effect_db(10.0, 1.5, 1.5, &hard, 500.0, TEST_SPEED);
        let a_soft = ground_effect_db(10.0, 1.5, 1.5, &soft, 500.0, TEST_SPEED);
        assert!(
            a_soft > a_hard,
            "soft ground ({a_soft}) should attenuate more than hard ({a_hard})"
        );
    }

    #[test]
    fn ground_effect_zero_distance() {
        let ground = GroundProperties::hard();
        assert_eq!(
            ground_effect_db(0.0, 1.5, 1.5, &ground, 500.0, TEST_SPEED),
            0.0
        );
    }

    #[test]
    fn ground_effect_gain_hard_boosts() {
        // Hard ground: constructive interference → gain > 1.0
        let ground = GroundProperties::hard();
        let g = ground_effect_gain(10.0, 1.5, 1.5, &ground, TEST_SPEED);
        assert!(g > 1.0, "hard ground should boost: got {g}");
    }

    #[test]
    fn ground_effect_gain_soft_attenuates_relative() {
        let hard = GroundProperties::hard();
        let soft = GroundProperties::soft();
        let g_hard = ground_effect_gain(10.0, 1.5, 1.5, &hard, TEST_SPEED);
        let g_soft = ground_effect_gain(10.0, 1.5, 1.5, &soft, TEST_SPEED);
        assert!(
            g_soft < g_hard,
            "soft ({g_soft}) should be less than hard ({g_hard})"
        );
    }

    #[test]
    fn ground_effect_gain_zero_distance_is_unity() {
        let ground = GroundProperties::soft();
        assert_eq!(ground_effect_gain(0.0, 1.5, 1.5, &ground, TEST_SPEED), 1.0);
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
        let a = barrier_attenuation_db(&barrier, 1000.0, TEST_SPEED);
        assert!(
            a > 5.0,
            "barrier should give significant attenuation, got {a}"
        );
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
        let a = barrier_attenuation_db(&barrier2, 1000.0, TEST_SPEED);
        assert!(
            a < 10.0,
            "near-grazing barrier should give moderate attenuation, got {a}"
        );
    }

    #[test]
    fn barrier_higher_freq_more_attenuation() {
        let barrier = BarrierGeometry {
            source: Vec3::new(0.0, 0.0, 0.0),
            receiver: Vec3::new(10.0, 0.0, 0.0),
            barrier_top: Vec3::new(5.0, 0.0, 2.0),
        };
        let a_500 = barrier_attenuation_db(&barrier, 500.0, TEST_SPEED);
        let a_4000 = barrier_attenuation_db(&barrier, 4000.0, TEST_SPEED);
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
        let a = barrier_attenuation_db(&barrier, 8000.0, TEST_SPEED);
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
        let n = fresnel_number(&barrier, 1000.0, TEST_SPEED);
        assert!(n > 0.0, "shadow zone should have positive N, got {n}");
    }

    // ── Barrier Attenuation Gain (broadband) ────────────────────────────

    #[test]
    fn barrier_gain_unobstructed_near_unity() {
        // Barrier below line of sight → N < 0 → ~0 dB → gain ≈ 1.0
        let barrier = BarrierGeometry {
            source: Vec3::new(0.0, 0.0, 2.0),
            receiver: Vec3::new(10.0, 0.0, 2.0),
            barrier_top: Vec3::new(5.0, 0.0, 0.5), // well below LOS
        };
        let g = barrier_attenuation_gain(&barrier, TEST_SPEED);
        assert!(
            (g - 1.0).abs() < 0.05,
            "unobstructed barrier should give gain ≈ 1.0, got {g}"
        );
    }

    #[test]
    fn barrier_gain_shadow_zone_attenuates() {
        let barrier = BarrierGeometry {
            source: Vec3::new(0.0, 0.0, 0.0),
            receiver: Vec3::new(10.0, 0.0, 0.0),
            barrier_top: Vec3::new(5.0, 0.0, 3.0),
        };
        let g = barrier_attenuation_gain(&barrier, TEST_SPEED);
        assert!(
            g < 0.5,
            "shadow zone should attenuate significantly, got {g}"
        );
        assert!(g > 0.0, "gain should be positive, got {g}");
    }

    #[test]
    fn barrier_gain_tall_barrier_near_minimum() {
        // Very tall barrier → 25 dB cap → gain ≈ 10^(-25/20) ≈ 0.056
        let barrier = BarrierGeometry {
            source: Vec3::new(0.0, 0.0, 0.0),
            receiver: Vec3::new(10.0, 0.0, 0.0),
            barrier_top: Vec3::new(5.0, 0.0, 10.0),
        };
        let g = barrier_attenuation_gain(&barrier, TEST_SPEED);
        assert!(
            g < 0.1,
            "very tall barrier should give gain near minimum, got {g}"
        );
        assert!(g > 0.03, "gain should not be unreasonably small, got {g}");
    }

    // ── Total Attenuation ────────────────────────────────────────────────

    #[test]
    fn total_attenuation_sums_correctly() {
        let ground = GroundProperties::hard();
        let terms = total_attenuation_db(10.0, 0.5, 1.5, 1.5, &ground, 1000.0, None, TEST_SPEED);

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

        let without = total_attenuation_db(10.0, 0.5, 1.5, 1.5, &ground, 1000.0, None, TEST_SPEED);
        let with = total_attenuation_db(
            10.0,
            0.5,
            1.5,
            1.5,
            &ground,
            1000.0,
            Some(&barrier),
            TEST_SPEED,
        );

        assert!(
            with.a_total > without.a_total,
            "barrier should increase total attenuation: {} vs {}",
            with.a_total,
            without.a_total
        );
    }
}
