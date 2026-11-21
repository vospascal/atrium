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
use crate::pipeline::environment::Environment;
use crate::pipeline::wall_material::WallMaterial;
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

/// Octave-band center frequencies for the 6-band absorption model.
/// Indices: [0]=125Hz, [1]=250Hz, [2]=500Hz, [3]=1kHz, [4]=2kHz, [5]=4kHz.
const OCTAVE_BAND_FREQS: [f32; 6] = [125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0];

/// Maximum first-order image-source distance for an axis-aligned box room.
///
/// For each axis, the worst-case image distance is when the source is at one wall
/// and the listener is at the opposite corner. The image is mirrored across the wall,
/// so the image-to-listener distance is:
///   X-wall: √((2·Lx)² + Ly² + Lz²)
///   Y-wall: √(Lx² + (2·Ly)² + Lz²)
///   Z-wall: √(Lx² + Ly² + (2·Lz)²)
///
/// Returns the maximum across all three axes.
pub fn max_image_source_distance(room_min: Vec3, room_max: Vec3) -> f32 {
    let size = room_max - room_min;
    let lx = size.x.abs();
    let ly = size.y.abs();
    let lz = size.z.abs();

    let dx = ((2.0 * lx).powi(2) + ly.powi(2) + lz.powi(2)).sqrt();
    let dy = (lx.powi(2) + (2.0 * ly).powi(2) + lz.powi(2)).sqrt();
    let dz = (lx.powi(2) + ly.powi(2) + (2.0 * lz).powi(2)).sqrt();

    dx.max(dy).max(dz)
}

/// Delay buffer capacity (in samples) that holds `longest_delay_seconds` at
/// `sample_rate`.
///
/// Takes the delay in seconds so that a pipeline can be described before the
/// output device is opened and sized once the device's sample rate is known.
/// Returns a power-of-2 value clamped to `[min_capacity, 65536]`; the +2
/// margin accounts for fractional delay interpolation.
pub fn delay_capacity_for_seconds(
    longest_delay_seconds: f32,
    sample_rate: f32,
    min_capacity: usize,
) -> usize {
    let needed = (longest_delay_seconds * sample_rate) as usize + 2;
    let needed = needed.max(min_capacity);
    // Round up to next power of 2.
    needed.next_power_of_two().min(65536)
}

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

/// Octave band (500 Hz) whose decay time stands for "the" RT60 of a room.
pub const MID_BAND_INDEX: usize = 2;

/// Mid-band decay time of the late reverb in `environment`, in seconds.
///
/// An explicit `rt60_override` is a designer's decision and wins; otherwise
/// it is Sabine's RT60 at 500 Hz from the room's size, the material of each
/// face and the air. The reverb tail and the reverb send both use this one
/// number, so how much a source feeds the reverb (critical distance) follows
/// the decay the listener actually hears.
pub fn reverb_decay_seconds(environment: &Environment, rt60_override: Option<f32>) -> f32 {
    if let Some(rt60) = rt60_override.filter(|rt60| *rt60 > 0.0) {
        return rt60;
    }
    let (volume, _surface_area) = room_geometry(environment.min, environment.max);
    let wall_areas = wall_surface_areas(environment.min, environment.max);
    sabine_rt60_at_band(
        volume,
        &wall_areas,
        &environment.wall_materials,
        &environment.atmosphere,
        MID_BAND_INDEX,
    )
}

/// Minimum broadband energy reflectivity for a surface to count as reflective
/// when deciding whether a space can hold a reverberant field.
const REFLECTIVE_SURFACE_THRESHOLD: f32 = 0.05;

/// Can this space sustain a reverberant (diffuse) field at all?
///
/// Sabine's equation assumes sound is trapped and bounces many times. That
/// needs at least one pair of FACING reflective surfaces. An open field has
/// only the ground: sound reflects once and leaves, so there is no late
/// reverberation however large the bounding box is. Sabine cannot express
/// this — with every wall fully absorbing (α = 1) it still returns
/// RT60 = 0.161·V/S, which is 1.2 s for a 60×60×30 m meadow.
///
/// `wall_materials` uses the `[−X, +X, −Y, +Y, −Z, +Z]` convention, so the
/// facing pairs are (0,1), (2,3) and (4,5).
pub fn has_facing_reflective_surfaces(wall_materials: &[WallMaterial; 6]) -> bool {
    let reflective = |index: usize| {
        wall_materials[index].broadband_reflectivity() > REFLECTIVE_SURFACE_THRESHOLD
    };
    (reflective(0) && reflective(1))
        || (reflective(2) && reflective(3))
        || (reflective(4) && reflective(5))
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

/// Reverb-send scale for a source of the given MDAP `spread`.
///
/// A point source (spread 0) excites the room fully — you hear it reflect and
/// reverberate. A distributed/diffuse source *is* the ambient field itself
/// (wind, rain, waves are all around you, not a point that echoes off a wall),
/// so it should get NO reverb tail — the scale reaches 0 by spread 0.75. This
/// keeps a localized bird reverberant while the diffuse weather beds are fully
/// dry (their field recordings also already carry their own ambience, so engine
/// reverb would double it and read as echo).
pub fn diffuse_reverb_scale(spread: f32) -> f32 {
    (1.0 - spread / 0.75).clamp(0.0, 1.0)
}

/// Sabine's RT60 for a room whose every face has the same reflectivity:
/// the textbook single-number form, used here to derive expected values.
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
#[cfg(test)]
pub(crate) fn sabine_rt60(volume: f32, surface_area: f32, wall_reflectivity: f32) -> f32 {
    let absorption_coefficient = 1.0 - wall_reflectivity.clamp(0.0, 0.99);
    let total_absorption = surface_area * absorption_coefficient;

    if total_absorption < 1e-6 {
        return 10.0;
    }

    let rt60 = 0.161 * volume / total_absorption;
    rt60.clamp(0.1, 10.0)
}

#[cfg(test)]
mod tests;
