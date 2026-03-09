//! Room acoustics calculations for physically-grounded reverb parameters.
//!
//! Pure functions derived from established acoustics research:
//!
//! - **Sabine's equation** (Sabine, 1898): RT60 from room geometry and absorption.
//!   Also presented with air absorption term in Wenmaekers (Metis217734, Eq. 1).
//!   `RT60 = 0.161 × V / A`
//!   where V = room volume (m³), A = total absorption area (m²).
//!
//! - **Jot's absorptive delay gain** (Jot & Chaigne, "Digital delay networks
//!   for designing artificial reverberators", AES Convention, 1991):
//!   Per-delay-line attenuation that produces a target RT60.
//!   Also presented in Gardner, "Reverberation Algorithms" (Eq. 3.42–3.43).
//!   `g = 10^(-3 × d / RT60)`
//!   where d = delay time in seconds, RT60 = reverberation time.
//!
//! - **Mean free path** (Kuttruff, "Room Acoustics", 5th ed., 2009):
//!   Average distance a sound ray travels between wall reflections in a diffuse field.
//!   `d̄ = 4V / S`
//!   where V = room volume (m³), S = total surface area (m²).
//!   Divided by speed of sound to get time: `t = d̄ / c`.
//!
//! - **Critical distance** (diffuse field theory):
//!   Distance where direct and reverberant sound energy are equal.
//!   `d_c = 0.057 × √(γ × V / RT60)`
//!   where γ = source directivity factor (1.0 for omni).
//!
//! - **Speed of sound** (ISO 9613-1):
//!   `c = 331.3 + 0.606 × T`
//!   where T = temperature in °C. At 20°C, c ≈ 343.4 m/s.

use crate::audio::atmosphere::{iso9613_alpha, AtmosphericParams};
use crate::pipeline::path::WallMaterial;
use atrium_core::types::Vec3;

/// Compute the volume and total surface area of an axis-aligned room.
///
/// Returns `(volume_m3, surface_area_m2)`.
pub fn room_geometry(room_min: Vec3, room_max: Vec3) -> (f32, f32) {
    let size = room_max - room_min;
    let width = size.x.abs();
    let height = size.y.abs();
    let depth = size.z.abs();

    let volume = width * height * depth;
    let surface_area = 2.0 * (width * height + height * depth + width * depth);

    (volume, surface_area)
}

/// Mean free path time: average time between wall reflections in a diffuse field.
///
/// d̄ = 4V / S (meters), then t = d̄ / c (seconds).
///
/// Reference: Kuttruff, "Room Acoustics" (5th ed., 2009).
///
/// Returns time in seconds.
pub fn mean_free_path_time(volume: f32, surface_area: f32, speed_of_sound: f32) -> f32 {
    if surface_area < 1e-6 {
        return 0.0;
    }
    let mean_free_path_meters = 4.0 * volume / surface_area;
    mean_free_path_meters / speed_of_sound
}

/// Sabine's reverberation time (RT60) in seconds.
///
/// RT60 = 0.161 × V / A
///
/// Where:
/// - `volume`: room volume in cubic meters
/// - `surface_area`: total interior surface area in square meters
/// - `wall_reflectivity`: fraction of energy reflected per bounce (0.0–1.0)
///
/// The absorption area A = surface_area × (1 - wall_reflectivity).
///
/// Reference: Sabine (1898), Wenmaekers (Metis217734, Eq. 1).
///
/// Returns RT60 in seconds, clamped to [0.1, 10.0] for safety.
pub fn sabine_rt60(volume: f32, surface_area: f32, wall_reflectivity: f32) -> f32 {
    let absorption_coefficient = 1.0 - wall_reflectivity.clamp(0.0, 0.99);
    let total_absorption = surface_area * absorption_coefficient;

    if total_absorption < 1e-6 {
        return 10.0;
    }

    let rt60 = 0.161 * volume / total_absorption;
    rt60.clamp(0.1, 10.0)
}

/// Octave-band center frequencies for the 6-band absorption model.
/// Indices: [0]=125Hz, [1]=250Hz, [2]=500Hz, [3]=1kHz, [4]=2kHz, [5]=4kHz.
const OCTAVE_BAND_FREQS: [f32; 6] = [125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0];

/// Per-wall surface areas for an axis-aligned box room.
///
/// Returns `[S_-X, S_+X, S_-Y, S_+Y, S_-Z, S_+Z]` — matching the wall index
/// convention used by `ImageSourceResolver` and `WallMaterial`.
///
/// Each axis pair has the same area (e.g., -X and +X walls are both height × depth).
pub fn wall_surface_areas(room_min: Vec3, room_max: Vec3) -> [f32; 6] {
    let size = room_max - room_min;
    let width = size.x.abs(); // X dimension
    let height = size.y.abs(); // Y dimension
    let depth = size.z.abs(); // Z dimension

    let yz = height * depth; // -X, +X walls
    let xz = width * depth; // -Y, +Y walls
    let xy = width * height; // -Z, +Z walls

    [yz, yz, xz, xz, xy, xy]
}

/// Sabine RT60 at a specific octave band, using per-wall materials and air absorption.
///
/// `RT60(f) = 0.161 × V / A(f)`
///
/// where:
///   `A(f) = Σᵢ Sᵢ × αᵢ(f) + 4 × m(f) × V`
///
/// - `Sᵢ` = surface area of wall i
/// - `αᵢ(f)` = absorption coefficient of wall i's material at band f
/// - `m(f)` = air absorption coefficient in Nepers/m (ISO 9613, converted from dB/m)
/// - The `4mV` term accounts for air absorption within the room volume
///
/// `band_index` selects from [125, 250, 500, 1000, 2000, 4000] Hz.
///
/// Reference: Sabine (1898), extended with air absorption term per Kuttruff (2009).
pub fn sabine_rt60_at_band(
    volume: f32,
    wall_areas: &[f32; 6],
    wall_materials: &[WallMaterial; 6],
    atmosphere: &AtmosphericParams,
    band_index: usize,
) -> f32 {
    let freq = OCTAVE_BAND_FREQS[band_index.min(5)];

    // Surface absorption: Σ Sᵢ × αᵢ(f)
    let mut surface_absorption = 0.0f32;
    for i in 0..6 {
        surface_absorption += wall_areas[i] * wall_materials[i].alpha[band_index.min(5)];
    }

    // Air absorption: 4 × m × V
    // iso9613_alpha returns dB/m; convert to Nepers/m: m = α_dB / (10 × log10(e)) ≈ α_dB / 4.343
    let air_alpha_db_per_m = iso9613_alpha(freq, atmosphere);
    let air_absorption_nepers = air_alpha_db_per_m / 4.343;
    let air_absorption = 4.0 * air_absorption_nepers * volume;

    let total_absorption = surface_absorption + air_absorption;

    if total_absorption < 1e-6 {
        return 10.0;
    }

    let rt60 = 0.161 * volume / total_absorption;
    rt60.clamp(0.1, 10.0)
}

/// Jot's per-delay-line feedback gain for a target RT60.
///
/// g = 10^(-3 × delay_seconds / rt60)
///
/// This ensures the signal decays by exactly 60 dB after `rt60` seconds,
/// regardless of the delay line length. Longer delays get lower gains
/// because each iteration represents more elapsed time.
///
/// Reference: Jot & Chaigne (AES 1991), Gardner Eq. 3.42–3.43.
///
/// Returns gain in [0.0, 1.0].
pub fn jot_feedback_gain(delay_seconds: f32, rt60: f32) -> f32 {
    if rt60 < 1e-6 || delay_seconds < 1e-6 {
        return 0.0;
    }

    let gain = 10.0_f32.powf(-3.0 * delay_seconds / rt60);
    gain.clamp(0.0, 1.0)
}

/// Critical distance: where direct and reverberant sound energy are equal.
///
/// In the diffuse-field approximation (omnidirectional source):
///   d_c = 0.057 × √(γ × V / RT60)
///
/// where γ is the source directivity factor (1.0 for omnidirectional).
/// Directional sources have γ > 1, pushing the critical distance farther out.
///
/// This is the crossover distance for the direct-to-reverberant ratio:
/// - d < d_c: direct sound dominates (drier)
/// - d > d_c: reverberant field dominates (wetter)
///
/// Returns distance in meters, clamped to [0.1, ...] to avoid division issues.
pub fn critical_distance(volume: f32, rt60: f32, directivity_gamma: f32) -> f32 {
    if rt60 < 1e-6 || volume < 1e-6 {
        return 0.1;
    }
    let d_c = 0.057 * (directivity_gamma.max(1.0) * volume / rt60).sqrt();
    d_c.max(0.1)
}

/// Per-source reverb send level from distance relative to critical distance.
///
/// Uses a saturating curve: send = x² / (1 + x²), where x = d / d_c.
///
/// Behavior:
/// - d = 0: send = 0 (fully dry, source at listener)
/// - d = d_c: send = 0.5 (equal direct and reverberant)
/// - d >> d_c: send → 1.0 (fully reverberant)
///
/// This is an artistic saturating control law, not a canonical acoustics formula.
/// The acoustics part is the use of d/d_c; the soft-knee mapping is a design choice.
pub fn reverb_send(distance: f32, critical_distance: f32) -> f32 {
    if critical_distance < 1e-6 {
        return 0.0;
    }
    let x = distance / critical_distance;
    let x2 = x * x;
    x2 / (1.0 + x2)
}

/// Compute per-delay-line feedback gains for the Ambisonics FDN
/// from room geometry and wall reflectivity.
///
/// Combines Sabine (RT60 from room) and Jot (per-line gain from RT60).
/// Each independent delay line gets its own gain: shorter delays → higher gains,
/// longer delays → lower gains. In the FDN topology (independent lines coupled
/// through a unitary Hadamard matrix), Jot's formula gives exact RT60 control:
/// each line decays by exactly 60 dB after RT60 seconds.
///
/// Reference: Jot & Chaigne (AES 1991), Gardner (Eq. 3.42–3.43).
///
/// Returns `(per_line_gains, rt60)` so both can be logged/inspected.
pub fn compute_feedback_gains(
    room_min: Vec3,
    room_max: Vec3,
    wall_reflectivity: f32,
    delay_times_seconds: &[f32],
) -> (Vec<f32>, f32) {
    let (volume, surface_area) = room_geometry(room_min, room_max);
    let rt60 = sabine_rt60(volume, surface_area, wall_reflectivity);
    let gains: Vec<f32> = delay_times_seconds
        .iter()
        .map(|&delay| jot_feedback_gain(delay, rt60))
        .collect();
    (gains, rt60)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Room geometry ────────────────────────────────────────────────

    #[test]
    fn room_geometry_10m_cube() {
        let (volume, surface_area) =
            room_geometry(Vec3::new(-5.0, -5.0, -5.0), Vec3::new(5.0, 5.0, 5.0));
        assert!((volume - 1000.0).abs() < 0.01, "10m cube = 1000 m³");
        assert!(
            (surface_area - 600.0).abs() < 0.01,
            "10m cube = 600 m² surface"
        );
    }

    #[test]
    fn room_geometry_rectangular() {
        // 4m × 3m × 2.5m room (typical small room)
        let (volume, surface_area) =
            room_geometry(Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 3.0, 2.5));
        assert!((volume - 30.0).abs() < 0.01, "4×3×2.5 = 30 m³");
        // 2 × (12 + 7.5 + 10) = 59 m²
        assert!((surface_area - 59.0).abs() < 0.01, "surface = 59 m²");
    }

    // ── Mean free path ──────────────────────────────────────────────

    #[test]
    fn mean_free_path_10m_cube() {
        // V=1000, S=600 → d̄ = 4000/600 = 6.667m → t = 6.667/343.4 = 19.4ms
        let t = mean_free_path_time(1000.0, 600.0, 343.42);
        assert!(
            (t - 0.0194).abs() < 0.001,
            "10m cube MFP should be ~19.4ms, got {:.1}ms",
            t * 1000.0
        );
    }

    #[test]
    fn mean_free_path_daga_2020_rooms() {
        // Small room: 5.5×6.5×3.5 → V=125.125, S=155.75
        // d̄ = 500.5/155.75 = 3.21m → t = 3.21/343.4 = 9.36ms
        let (volume, surface_area) = room_geometry(Vec3::ZERO, Vec3::new(5.5, 6.5, 3.5));
        let t = mean_free_path_time(volume, surface_area, 343.42);
        assert!(
            (t * 1000.0 - 9.36).abs() < 0.5,
            "small room MFP should be ~9.4ms, got {:.1}ms",
            t * 1000.0
        );

        // Concert hall: 30×23.8×20 → V=14280, S=3576
        // d̄ = 57120/3576 = 15.97m → t = 15.97/343.4 = 46.5ms
        let (volume, surface_area) = room_geometry(Vec3::ZERO, Vec3::new(30.0, 23.8, 20.0));
        let t = mean_free_path_time(volume, surface_area, 343.42);
        assert!(
            (t * 1000.0 - 46.5).abs() < 1.0,
            "concert hall MFP should be ~46.5ms, got {:.1}ms",
            t * 1000.0
        );
    }

    // ── Sabine RT60 ──────────────────────────────────────────────────

    #[test]
    fn sabine_rt60_10m_cube_reflectivity_09() {
        // V=1000, S=600, α=0.1 → A=60, RT60=0.161×1000/60 = 2.683s
        let rt60 = sabine_rt60(1000.0, 600.0, 0.9);
        assert!((rt60 - 2.683).abs() < 0.01, "expected ~2.68s, got {rt60}");
    }

    #[test]
    fn sabine_rt60_small_room_reflectivity_07() {
        // V=30, S=59, α=0.3 → A=17.7, RT60=0.161×30/17.7 = 0.273s
        let rt60 = sabine_rt60(30.0, 59.0, 0.7);
        assert!((rt60 - 0.273).abs() < 0.02, "expected ~0.27s, got {rt60}");
    }

    #[test]
    fn sabine_rt60_highly_reflective_clamped() {
        // wall_reflectivity = 1.0 → clamped to 0.99, A = S×0.01
        // V=1000, S=600, A=6 → RT60 = 0.161×1000/6 = 26.8s → clamped to 10.0
        let rt60 = sabine_rt60(1000.0, 600.0, 1.0);
        assert_eq!(rt60, 10.0, "should clamp to 10s maximum");
    }

    #[test]
    fn sabine_rt60_very_absorptive() {
        // reflectivity=0.0 → α=1.0, fully absorptive
        // V=30, S=59, A=59 → RT60=0.161×30/59 = 0.082s → clamped to 0.1
        let rt60 = sabine_rt60(30.0, 59.0, 0.0);
        assert_eq!(rt60, 0.1, "should clamp to 0.1s minimum");
    }

    // ── Jot feedback gain ────────────────────────────────────────────

    #[test]
    fn jot_gain_matches_known_values() {
        // delay=0.3s, RT60=2.683s → g = 10^(-3×0.3/2.683) = 10^(-0.3354) = 0.4624
        let gain = jot_feedback_gain(0.3, 2.683);
        assert!((gain - 0.462).abs() < 0.01, "expected ~0.462, got {gain}");
    }

    #[test]
    fn jot_gain_shorter_delay_higher_gain() {
        // Shorter delay → less time elapsed → less decay → higher gain.
        let gain_100ms = jot_feedback_gain(0.1, 2.0);
        let gain_300ms = jot_feedback_gain(0.3, 2.0);
        assert!(
            gain_100ms > gain_300ms,
            "100ms gain ({gain_100ms}) should exceed 300ms gain ({gain_300ms})"
        );
    }

    #[test]
    fn jot_gain_longer_rt60_higher_gain() {
        // Longer RT60 → slower decay → higher gain for same delay.
        let gain_short_rt60 = jot_feedback_gain(0.3, 0.5);
        let gain_long_rt60 = jot_feedback_gain(0.3, 3.0);
        assert!(
            gain_long_rt60 > gain_short_rt60,
            "long RT60 gain ({gain_long_rt60}) should exceed short RT60 gain ({gain_short_rt60})"
        );
    }

    #[test]
    fn jot_gain_zero_delay_returns_zero() {
        assert_eq!(jot_feedback_gain(0.0, 2.0), 0.0);
    }

    #[test]
    fn jot_gain_zero_rt60_returns_zero() {
        assert_eq!(jot_feedback_gain(0.3, 0.0), 0.0);
    }

    // ── Combined computation ─────────────────────────────────────────

    #[test]
    fn compute_feedback_gains_10m_cube() {
        let room_min = Vec3::new(-5.0, -5.0, -5.0);
        let room_max = Vec3::new(5.0, 5.0, 5.0);
        let delay_times = [0.100, 0.167, 0.233, 0.300];

        let (gains, rt60) = compute_feedback_gains(room_min, room_max, 0.9, &delay_times);

        // RT60 ≈ 2.68s
        assert!(
            (rt60 - 2.68).abs() < 0.02,
            "RT60 should be ~2.68s, got {rt60}"
        );
        // Per-tap gains: shorter delays → higher gains.
        assert!(gains[0] > gains[1], "100ms gain should exceed 167ms gain");
        assert!(gains[1] > gains[2], "167ms gain should exceed 233ms gain");
        assert!(gains[2] > gains[3], "233ms gain should exceed 300ms gain");
        // Longest delay gain ≈ 0.46
        assert!(
            (gains[3] - 0.46).abs() < 0.02,
            "300ms gain should be ~0.46, got {}",
            gains[3]
        );
    }

    #[test]
    fn compute_feedback_gains_small_damped_room() {
        let room_min = Vec3::ZERO;
        let room_max = Vec3::new(4.0, 3.0, 2.5);
        let delay_times = [0.100, 0.167, 0.233, 0.300];

        let (gains, rt60) = compute_feedback_gains(room_min, room_max, 0.7, &delay_times);

        // V=30, S=59, α=0.3, A=17.7, RT60=0.273s
        assert!(
            rt60 < 0.4,
            "small damped room RT60 should be short, got {rt60}"
        );
        // All gains should be very low for a short RT60.
        for (i, &gain) in gains.iter().enumerate() {
            assert!(
                gain < 0.2,
                "tap {i} should have low feedback in small damped room, got {gain}"
            );
        }
    }

    /// Verify that Rudrich's original 0.759 constant corresponds to
    /// a specific room configuration (for documentation, not enforcement).
    #[test]
    fn rudrich_original_value_context() {
        // Rudrich's 2.4 dB ≈ 0.759 was tuned for a 36-line 5th-order system
        // in the Ligeti Hall. We can reverse-engineer what RT60 that implies
        // for a 300ms delay: 0.759 = 10^(-3×0.3/RT60) → RT60 = -0.9/log10(0.759) = 7.5s
        let implied_rt60 = -0.9 / 0.759_f32.log10();
        assert!(
            (implied_rt60 - 7.5).abs() < 0.2,
            "Rudrich's 0.759 implies RT60 ≈ 7.5s (large concert hall), got {implied_rt60}"
        );
    }

    // ── DAGA 2020 virtual rooms (Frank, Rudrich, Brandner) ──────────
    //
    // Table 1 from "Augmented Practice-Room" (DAGA 2020, p.153):
    //
    //   room            RT60   x/m    y/m    z/m   Γ/dB
    //   small room      0.3    5.5    6.5    3.5   -5
    //   chamber music 1 1.0    13     9      6     -0.5
    //   chamber music 2 1.0    15.3   8      5.1   0
    //   concert hall    2.2    30     23.8   20    0
    //   cathedral       5.1    30     23.8   20    -2
    //
    // We use the known RT60 directly with Jot's formula to compute the
    // feedback gain our system would produce for each room. This validates
    // that our calculation produces physically reasonable gains across
    // the full range of room sizes Rudrich actually tested.

    /// Helper: compute Jot feedback gain for a DAGA 2020 room given its
    /// known RT60 and dimensions (for the 300ms longest delay tap).
    fn daga_2020_gain(rt60: f32) -> f32 {
        jot_feedback_gain(0.3, rt60)
    }

    #[test]
    fn daga_2020_small_room() {
        // 5.5×6.5×3.5m, RT60=0.3s, Γ=-5dB
        let gain = daga_2020_gain(0.3);
        // g = 10^(-3×0.3/0.3) = 10^(-3) = 0.001
        // Very low gain — almost no reverb tail. Correct for a dry practice room.
        assert!(
            gain < 0.01,
            "small room (RT60=0.3s) should have near-zero feedback, got {gain:.4}"
        );
    }

    #[test]
    fn daga_2020_chamber_music() {
        // 13×9×6m, RT60=1.0s, Γ=-0.5dB
        let gain = daga_2020_gain(1.0);
        // g = 10^(-3×0.3/1.0) = 10^(-0.9) ≈ 0.126
        assert!(
            (gain - 0.126).abs() < 0.01,
            "chamber music (RT60=1.0s) gain should be ~0.126, got {gain:.4}"
        );
    }

    #[test]
    fn daga_2020_concert_hall() {
        // 30×23.8×20m, RT60=2.2s, Γ=0dB
        let gain = daga_2020_gain(2.2);
        // g = 10^(-3×0.3/2.2) = 10^(-0.409) ≈ 0.390
        assert!(
            (gain - 0.390).abs() < 0.01,
            "concert hall (RT60=2.2s) gain should be ~0.390, got {gain:.4}"
        );
        // Still well below Rudrich's 0.759 — his value was tuned for a
        // 64-channel FDN (DAGA 2020 ref [21]: Jot & Chaigne 1991),
        // not derived from physics for a 4-line FOA system like ours.
        assert!(gain < 0.759);
    }

    #[test]
    fn daga_2020_cathedral() {
        // 30×23.8×20m (same dimensions as concert hall), RT60=5.1s, Γ=-2dB
        let gain = daga_2020_gain(5.1);
        // g = 10^(-3×0.3/5.1) = 10^(-0.176) ≈ 0.666
        assert!(
            (gain - 0.666).abs() < 0.01,
            "cathedral (RT60=5.1s) gain should be ~0.666, got {gain:.4}"
        );
        // Closest to Rudrich's 0.759 — a cathedral with 5s reverb
        // is the kind of space where high feedback gains make sense.
    }

    #[test]
    fn daga_2020_gains_increase_with_rt60() {
        // Across all five rooms, longer RT60 should give higher feedback gain.
        let gains: Vec<f32> = [0.3, 1.0, 1.0, 2.2, 5.1]
            .iter()
            .map(|&rt60| daga_2020_gain(rt60))
            .collect();

        for window in gains.windows(2) {
            assert!(
                window[1] >= window[0],
                "gain should increase with RT60: {:.4} should be >= {:.4}",
                window[1],
                window[0]
            );
        }
    }

    /// Verify that a room with RT60 ≈ 7.5s (the value implied by Rudrich's
    /// 0.759 gain via Jot's formula) does produce gain ≈ 0.759.
    /// This proves our formulas are the mathematical inverse of his constant.
    #[test]
    fn room_with_implied_rt60_reproduces_rudrich_gain() {
        // Reverse-engineer: 0.759 = 10^(-3×0.3/RT60) → RT60 = 7.5s
        let gain = daga_2020_gain(7.5);
        assert!(
            (gain - 0.759).abs() < 0.01,
            "RT60=7.5s should reproduce Rudrich's 0.759, got {gain:.4}"
        );
    }

    // ── RT60 formula comparison against DAGA 2020 Table 1 ────────────
    //
    // For each room we know the dimensions and the target RT60.
    // We try Sabine and Norris-Eyring to see which formula, combined
    // with which wall reflectivity, reproduces the known RT60.
    // This tells us whether our Sabine assumption is adequate or
    // whether a different formula would serve us better.

    /// Norris-Eyring RT60: RT60 = 0.161 × V / (-S × ln(r))
    /// More accurate than Sabine for rooms with higher absorption.
    /// Reference: Norris (1932), Eyring (1930).
    fn eyring_rt60(volume: f32, surface_area: f32, wall_reflectivity: f32) -> f32 {
        let r = wall_reflectivity.clamp(0.01, 0.99);
        let denominator = -surface_area * r.ln();
        if denominator < 1e-6 {
            return 10.0;
        }
        (0.161 * volume / denominator).clamp(0.1, 10.0)
    }

    /// Reverse-solve Sabine for reflectivity: r = 1 - 0.161×V/(S×RT60)
    fn sabine_reflectivity(volume: f32, surface_area: f32, rt60: f32) -> f32 {
        let alpha = 0.161 * volume / (surface_area * rt60);
        (1.0 - alpha).clamp(0.0, 1.0)
    }

    /// Reverse-solve Eyring for reflectivity: r = exp(-0.161×V/(S×RT60))
    fn eyring_reflectivity(volume: f32, surface_area: f32, rt60: f32) -> f32 {
        let exponent = -0.161 * volume / (surface_area * rt60);
        exponent.exp().clamp(0.0, 1.0)
    }

    /// Compare Sabine vs Eyring against all five DAGA 2020 rooms.
    ///
    /// For each room we reverse-solve the reflectivity from the known RT60,
    /// then compute Jot gain with different delay assumptions.
    #[test]
    fn daga_2020_formula_comparison() {
        struct Room {
            name: &'static str,
            x: f32,
            y: f32,
            z: f32,
            rt60: f32,
            gamma_db: f32,
        }

        let rooms = [
            Room {
                name: "small room",
                x: 5.5,
                y: 6.5,
                z: 3.5,
                rt60: 0.3,
                gamma_db: -5.0,
            },
            Room {
                name: "chamber music 1",
                x: 13.0,
                y: 9.0,
                z: 6.0,
                rt60: 1.0,
                gamma_db: -0.5,
            },
            Room {
                name: "chamber music 2",
                x: 15.3,
                y: 8.0,
                z: 5.1,
                rt60: 1.0,
                gamma_db: 0.0,
            },
            Room {
                name: "concert hall",
                x: 30.0,
                y: 23.8,
                z: 20.0,
                rt60: 2.2,
                gamma_db: 0.0,
            },
            Room {
                name: "cathedral",
                x: 30.0,
                y: 23.8,
                z: 20.0,
                rt60: 5.1,
                gamma_db: -2.0,
            },
        ];

        let delay_times = [0.100, 0.167, 0.233, 0.300];

        for room in &rooms {
            let (volume, surface_area) =
                room_geometry(Vec3::ZERO, Vec3::new(room.x, room.y, room.z));

            // Reverse-solve: what reflectivity gives this RT60?
            let r_sabine = sabine_reflectivity(volume, surface_area, room.rt60);
            let r_eyring = eyring_reflectivity(volume, surface_area, room.rt60);

            // Verify round-trip
            let rt60_sabine = sabine_rt60(volume, surface_area, r_sabine);
            let rt60_eyring = eyring_rt60(volume, surface_area, r_eyring);

            // Mean free path time for this room
            let mfp_time = mean_free_path_time(volume, surface_area, 343.42);

            // Per-delay-line Jot gains (the correct Jot approach)
            let per_tap: Vec<f32> = delay_times
                .iter()
                .map(|&d| jot_feedback_gain(d, room.rt60))
                .collect();

            // Verify Sabine round-trips correctly (within tolerance)
            assert!(
                (rt60_sabine - room.rt60).abs() < 0.15,
                "{}: Sabine round-trip failed: r={r_sabine:.3} → RT60={rt60_sabine:.2}s, \
                 expected {:.1}s",
                room.name,
                room.rt60
            );

            // Verify Eyring round-trips correctly
            assert!(
                (rt60_eyring - room.rt60).abs() < 0.15,
                "{}: Eyring round-trip failed: r={r_eyring:.3} → RT60={rt60_eyring:.2}s, \
                 expected {:.1}s",
                room.name,
                room.rt60
            );

            // Both formulas should give similar reflectivities
            assert!(
                (r_sabine - r_eyring).abs() < 0.1,
                "{}: Sabine r={r_sabine:.3} vs Eyring r={r_eyring:.3} differ too much",
                room.name
            );

            // All per-tap gains should be < 1.0 for stability
            for (i, &gain) in per_tap.iter().enumerate() {
                assert!(
                    gain < 1.0,
                    "{}: tap {} gain should be < 1.0 for stability, got {gain:.4}",
                    room.name,
                    i
                );
            }

            // Shorter delays should have higher gains
            for window in per_tap.windows(2) {
                assert!(
                    window[0] >= window[1],
                    "{}: shorter delay should have higher gain: {:.4} >= {:.4}",
                    room.name,
                    window[0],
                    window[1]
                );
            }

            // Log the comparison (visible with --nocapture)
            eprintln!(
                "  {:<18} V={:>8.0}  S={:>7.0}  RT60={:.1}s  Γ={:>3.0}dB  MFP={:>5.1}ms | \
                 r_sab={:.3} r_eyr={:.3} | g(100)={:.4} g(167)={:.4} g(233)={:.4} g(300)={:.4}",
                room.name,
                volume,
                surface_area,
                room.rt60,
                room.gamma_db,
                mfp_time * 1000.0,
                r_sabine,
                r_eyring,
                per_tap[0],
                per_tap[1],
                per_tap[2],
                per_tap[3],
            );
        }
    }

    // ── Critical distance tests ──────────────────────────────────────────

    #[test]
    fn critical_distance_10m_cube() {
        // 10m cube: V=1000, wall_reflectivity=0.9 → RT60≈2.68s
        // d_c = 0.057 × √(1.0 × 1000 / 2.68) ≈ 1.10m
        let volume = 1000.0;
        let rt60 = sabine_rt60(volume, 600.0, 0.9);
        let d_c = critical_distance(volume, rt60, 1.0);
        assert!(
            (d_c - 1.1).abs() < 0.15,
            "10m cube d_c should be ~1.1m, got {d_c}"
        );
    }

    #[test]
    fn critical_distance_small_room_shorter() {
        // Smaller room → smaller d_c (closer crossover point).
        let (vol_small, sa_small) =
            room_geometry(Vec3::new(0.0, 0.0, 0.0), Vec3::new(3.0, 3.0, 3.0));
        let (vol_large, sa_large) =
            room_geometry(Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 10.0, 10.0));
        let rt60_small = sabine_rt60(vol_small, sa_small, 0.8);
        let rt60_large = sabine_rt60(vol_large, sa_large, 0.8);
        let d_c_small = critical_distance(vol_small, rt60_small, 1.0);
        let d_c_large = critical_distance(vol_large, rt60_large, 1.0);
        assert!(
            d_c_small < d_c_large,
            "smaller room should have shorter critical distance: {d_c_small} vs {d_c_large}"
        );
    }

    #[test]
    fn critical_distance_directivity_increases_dc() {
        // Higher directivity factor → larger d_c (directional sources have
        // more direct energy relative to reverberant).
        let d_c_omni = critical_distance(500.0, 2.0, 1.0);
        let d_c_dir = critical_distance(500.0, 2.0, 4.0);
        assert!(
            d_c_dir > d_c_omni,
            "directional source should have larger d_c: {d_c_dir} vs {d_c_omni}"
        );
        // γ=4 should give d_c 2× larger (sqrt(4)=2).
        assert!(
            (d_c_dir / d_c_omni - 2.0).abs() < 0.01,
            "d_c ratio should be 2.0 for γ=4, got {}",
            d_c_dir / d_c_omni
        );
    }

    #[test]
    fn critical_distance_zero_inputs_clamped() {
        // Zero RT60 or volume should return the floor (0.1m), not panic.
        assert!((critical_distance(0.0, 2.0, 1.0) - 0.1).abs() < 1e-6);
        assert!((critical_distance(500.0, 0.0, 1.0) - 0.1).abs() < 1e-6);
    }

    // ── Reverb send tests ────────────────────────────────────────────────

    #[test]
    fn reverb_send_at_critical_distance() {
        // At d = d_c, send should be exactly 0.5.
        let send = reverb_send(2.0, 2.0);
        assert!(
            (send - 0.5).abs() < 1e-6,
            "at d=d_c, send should be 0.5, got {send}"
        );
    }

    #[test]
    fn reverb_send_close_source() {
        // At d = 0.1 × d_c, send ≈ 0.01 (nearly dry).
        let d_c = 2.0;
        let send = reverb_send(0.1 * d_c, d_c);
        assert!(
            send < 0.02,
            "close source (0.1×d_c) should have near-zero send, got {send}"
        );
    }

    #[test]
    fn reverb_send_far_source() {
        // At d = 5 × d_c, send ≈ 0.96 (nearly all reverb).
        let d_c = 2.0;
        let send = reverb_send(5.0 * d_c, d_c);
        assert!(
            send > 0.9,
            "far source (5×d_c) should have send near 1.0, got {send}"
        );
    }

    #[test]
    fn reverb_send_zero_distance() {
        let send = reverb_send(0.0, 2.0);
        assert!(
            send.abs() < 1e-10,
            "zero distance should produce zero send, got {send}"
        );
    }

    #[test]
    fn reverb_send_zero_critical_distance() {
        // Zero d_c should return 0 (not NaN or infinity).
        let send = reverb_send(1.0, 0.0);
        assert!(
            send.abs() < 1e-10,
            "zero d_c should produce zero send, got {send}"
        );
    }

    #[test]
    fn reverb_send_monotonically_increases() {
        let d_c = 1.5;
        let mut prev = 0.0;
        for i in 1..=20 {
            let d = i as f32 * 0.5;
            let send = reverb_send(d, d_c);
            assert!(
                send >= prev,
                "reverb_send should increase with distance: d={d} send={send} < prev={prev}"
            );
            prev = send;
        }
    }
}
