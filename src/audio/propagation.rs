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

/// Octave-band centre frequencies of ISO 9613-2 Table 3 (Hz).
pub const GROUND_BAND_FREQUENCIES: [f32; 8] =
    [63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];

/// Ground attenuation per octave band, ISO 9613-2 §7.3.1 (Table 3).
///
///   A_ground = A_s + A_r + A_m      (dB, positive = loss)
///
/// Source and receiver regions (same expressions, own height and G):
///
/// | Band        | A_s or A_r            |
/// |-------------|-----------------------|
/// | 63 Hz       | −1.5                  |
/// | 125 Hz      | −1.5 + G · a′(h)      |
/// | 250 Hz      | −1.5 + G · b′(h)      |
/// | 500 Hz      | −1.5 + G · c′(h)      |
/// | 1 kHz       | −1.5 + G · d′(h)      |
/// | 2, 4, 8 kHz | −1.5 · (1 − G)        |
///
/// Every height function is ≥ 1.5, so porous ground (G = 1) never amplifies:
/// the −1.5 dB reflection gain is cancelled or exceeded in every band. Hard
/// ground (G = 0) is −1.5 dB per region in every band: a flat +3 dB boost.
///
/// Middle region: A_m = −3q at 63 Hz and −3q·(1 − G_m) above, with
/// q = 0 while d_p ≤ 30·(h_s + h_r), else q = 1 − 30·(h_s + h_r)/d_p.
/// The standard's own q is continuous, so no extra smoothing is needed.
///
/// `distance` is the source→receiver distance projected on the ground (m).
/// Returned values are indexed like [`GROUND_BAND_FREQUENCIES`].
pub fn ground_effect_bands(
    distance: f32,
    h_source: f32,
    h_receiver: f32,
    ground: &GroundProperties,
) -> [f32; 8] {
    if distance < 0.01 {
        return [0.0; 8];
    }
    let h_source = h_source.max(0.0);
    let h_receiver = h_receiver.max(0.0);

    let h_sum = h_source + h_receiver;
    let q = if distance <= 30.0 * h_sum {
        0.0
    } else {
        1.0 - 30.0 * h_sum / distance
    };

    std::array::from_fn(|band| {
        let middle = if band == 0 {
            -3.0 * q
        } else {
            -3.0 * q * (1.0 - ground.g_middle)
        };
        ground_region_band(ground.g_source, h_source, distance, band)
            + ground_region_band(ground.g_receiver, h_receiver, distance, band)
            + middle
    })
}

/// Source- or receiver-region term of ISO 9613-2 Table 3 for one octave band.
fn ground_region_band(g: f32, height: f32, distance: f32, band: usize) -> f32 {
    // (1 − e^(−d_p/50)): the height functions grow in over the first ~100 m.
    let range_term = 1.0 - (-distance / 50.0).exp();
    let height_squared = height * height;
    match band {
        0 => -1.5,
        1 => {
            let a_prime = 1.5
                + 3.0 * (-0.12 * (height - 5.0) * (height - 5.0)).exp() * range_term
                + 5.7
                    * (-0.09 * height_squared).exp()
                    * (1.0 - (-2.8e-6 * distance * distance).exp());
            -1.5 + g * a_prime
        }
        2 => -1.5 + g * (1.5 + 8.6 * (-0.09 * height_squared).exp() * range_term),
        3 => -1.5 + g * (1.5 + 14.0 * (-0.46 * height_squared).exp() * range_term),
        4 => -1.5 + g * (1.5 + 5.0 * (-0.9 * height_squared).exp() * range_term),
        _ => -1.5 * (1.0 - g),
    }
}

/// Ground attenuation in dB at an arbitrary frequency (positive = loss).
///
/// Interpolates the ISO 9613-2 octave-band values of [`ground_effect_bands`]
/// linearly in log-frequency; frequencies outside 63 Hz–8 kHz take the nearest
/// band's value.
pub fn ground_effect_db(
    distance: f32,
    h_source: f32,
    h_receiver: f32,
    ground: &GroundProperties,
    freq_hz: f32,
) -> f32 {
    let bands = ground_effect_bands(distance, h_source, h_receiver, ground);
    let last = GROUND_BAND_FREQUENCIES.len() - 1;
    if freq_hz <= GROUND_BAND_FREQUENCIES[0] {
        return bands[0];
    }
    if freq_hz >= GROUND_BAND_FREQUENCIES[last] {
        return bands[last];
    }
    let upper = GROUND_BAND_FREQUENCIES
        .iter()
        .position(|&centre| centre >= freq_hz)
        .unwrap_or(last);
    let lower = upper - 1;
    let fraction = (freq_hz / GROUND_BAND_FREQUENCIES[lower]).ln()
        / (GROUND_BAND_FREQUENCIES[upper] / GROUND_BAND_FREQUENCIES[lower]).ln();
    bands[lower] + (bands[upper] - bands[lower]) * fraction
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
    let a_ground = ground_effect_db(distance, h_source, h_receiver, ground, freq_hz);
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

/// Broadband ground effect as a linear gain multiplier for real-time use.
///
/// Averages the ISO 9613-2 ground attenuation (in dB) over the 125 Hz–4 kHz
/// octave bands and converts to an amplitude factor. Used where a path has no
/// per-band processing (the per-speaker WorldLocked renderer).
///
/// Hard ground gives +3 dB (×1.41) at short range and up to +6 dB once the
/// middle region opens; porous ground gives ≤ 0 dB (never a boost).
pub fn ground_effect_gain(
    distance: f32,
    h_source: f32,
    h_receiver: f32,
    ground: &GroundProperties,
) -> f32 {
    if distance < 0.01 {
        return 1.0;
    }
    let bands = ground_effect_bands(distance, h_source, h_receiver, ground);
    // Bands 1..=6 are 125 Hz–4 kHz.
    let average_db = bands[1..=6].iter().sum::<f32>() / 6.0;
    // Positive dB = loss, so gain = 10^(−dB/20).
    10.0_f32.powf(-average_db / 20.0)
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

    // The pinned values below are worked by hand from ISO 9613-2 Table 3.

    /// Hard ground (G = 0) is −1.5 dB per region in every band, and the middle
    /// region is empty while d_p ≤ 30·(h_s + h_r): a flat −3 dB (a +3 dB boost).
    #[test]
    fn hard_ground_is_flat_minus_three_db_at_short_range() {
        let bands = ground_effect_bands(10.0, 1.5, 1.5, &GroundProperties::hard());
        for (band, value) in bands.iter().enumerate() {
            assert!(
                (value + 3.0).abs() < 1e-4,
                "band {band}: expected -3.0 dB, got {value}"
            );
        }
        let gain = ground_effect_gain(10.0, 1.5, 1.5, &GroundProperties::hard());
        assert!((20.0 * gain.log10() - 3.0).abs() < 1e-3, "gain {gain}");
    }

    /// Hard ground with source and receiver on the ground: the middle region
    /// covers the whole path (q = 1), adding −3 dB: −6 dB in total.
    #[test]
    fn hard_ground_at_zero_height_is_minus_six_db() {
        let bands = ground_effect_bands(20.0, 0.0, 0.0, &GroundProperties::hard());
        for value in bands {
            assert!((value + 6.0).abs() < 1e-4, "expected -6.0 dB, got {value}");
        }
    }

    /// Middle region factor q = 1 − 30·(h_s + h_r)/d_p. Heights 1 m + 1 m at
    /// 120 m: q = 0.5, so hard ground gives −3 − 1.5 = −4.5 dB.
    #[test]
    fn middle_region_uses_iso_q() {
        let bands = ground_effect_bands(120.0, 1.0, 1.0, &GroundProperties::hard());
        assert!((bands[3] + 4.5).abs() < 1e-4, "got {}", bands[3]);
        // At exactly the threshold (60 m) the middle region is still empty.
        let at_threshold = ground_effect_bands(60.0, 1.0, 1.0, &GroundProperties::hard());
        assert!(
            (at_threshold[3] + 3.0).abs() < 1e-4,
            "got {}",
            at_threshold[3]
        );
    }

    /// Porous ground, source and receiver on the ground, 1000 m (so the range
    /// term 1 − e^(−d/50) ≈ 1). Per region:
    ///   125 Hz: a′(0) − 1.5 = 3.0·e^(−3) + 5.7·(1 − e^(−2.8)) = 0.149 + 5.353 = 5.50
    ///   250 Hz: b′(0) − 1.5 = 8.6
    ///   500 Hz: c′(0) − 1.5 = 14.0
    ///   1 kHz:  d′(0) − 1.5 = 5.0
    ///   2 kHz+: 0
    /// Two regions double each; the middle region is 0 for G = 1 above 63 Hz,
    /// and −3 dB at 63 Hz on top of 2 × −1.5.
    #[test]
    fn porous_ground_matches_table_three_height_functions() {
        let bands = ground_effect_bands(1000.0, 0.0, 0.0, &GroundProperties::soft());
        let expected = [-6.0, 11.0, 17.2, 28.0, 10.0, 0.0, 0.0, 0.0];
        for (band, (value, want)) in bands.iter().zip(expected).enumerate() {
            assert!(
                (value - want).abs() < 0.02,
                "band {band} ({} Hz): expected {want} dB, got {value}",
                GROUND_BAND_FREQUENCIES[band]
            );
        }
    }

    /// The bug this replaced: grass came out +3.8 dB louder than free field,
    /// louder than concrete. Porous ground must never amplify from 125 Hz up,
    /// at any geometry.
    #[test]
    fn porous_ground_never_amplifies() {
        let soft = GroundProperties::soft();
        let hard = GroundProperties::hard();
        for distance in [0.5f32, 2.0, 10.0, 20.0, 60.0, 300.0] {
            for h_source in [0.0f32, 0.3, 1.2, 6.0, 20.0] {
                for h_receiver in [0.0f32, 1.2, 1.7] {
                    let bands = ground_effect_bands(distance, h_source, h_receiver, &soft);
                    for (band, value) in bands.iter().enumerate().skip(1) {
                        assert!(
                            *value >= -1e-4,
                            "d={distance} hs={h_source} hr={h_receiver} band {band}: {value} dB"
                        );
                    }
                    let soft_gain = ground_effect_gain(distance, h_source, h_receiver, &soft);
                    let hard_gain = ground_effect_gain(distance, h_source, h_receiver, &hard);
                    assert!(soft_gain <= 1.0 + 1e-5, "soft gain {soft_gain}");
                    assert!(
                        hard_gain > soft_gain,
                        "hard {hard_gain} vs soft {soft_gain}"
                    );
                }
            }
        }
    }

    /// Between band centres the value is interpolated in log-frequency.
    #[test]
    fn ground_effect_db_interpolates_between_bands() {
        let soft = GroundProperties::soft();
        let bands = ground_effect_bands(1000.0, 0.0, 0.0, &soft);
        let between = ground_effect_db(1000.0, 0.0, 0.0, &soft, (250.0f32 * 500.0).sqrt());
        assert!(
            (between - (bands[2] + bands[3]) / 2.0).abs() < 0.01,
            "{between}"
        );
        assert_eq!(ground_effect_db(1000.0, 0.0, 0.0, &soft, 20.0), bands[0]);
        assert_eq!(ground_effect_db(1000.0, 0.0, 0.0, &soft, 16000.0), bands[7]);
    }

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

    #[test]
    fn ground_effect_gain_hard_boosts() {
        // Hard ground: constructive interference → gain > 1.0
        let ground = GroundProperties::hard();
        let g = ground_effect_gain(10.0, 1.5, 1.5, &ground);
        assert!(g > 1.0, "hard ground should boost: got {g}");
    }

    #[test]
    fn ground_effect_gain_soft_attenuates_relative() {
        let hard = GroundProperties::hard();
        let soft = GroundProperties::soft();
        let g_hard = ground_effect_gain(10.0, 1.5, 1.5, &hard);
        let g_soft = ground_effect_gain(10.0, 1.5, 1.5, &soft);
        assert!(
            g_soft < g_hard,
            "soft ({g_soft}) should be less than hard ({g_hard})"
        );
    }

    #[test]
    fn ground_effect_gain_zero_distance_is_unity() {
        let ground = GroundProperties::soft();
        assert_eq!(ground_effect_gain(0.0, 1.5, 1.5, &ground), 1.0);
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
