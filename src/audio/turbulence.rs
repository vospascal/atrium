//! How moving air makes a distant sound waver.
//!
//! Outdoors the air between a sound and a listener is never still. Wind
//! shear and the sun on the ground fill it with eddies of slightly different
//! wind and temperature, each bending the sound a little. Close by nothing
//! is heard of it; over tens of metres the level starts to come and go, the
//! treble most, the way a far blackbird fades and returns on a breezy day.
//!
//! This is the line-of-sight theory of Ostashev and Wilson (*Acoustics in
//! Moving Inhomogeneous Media*, 2nd ed., 2015) for a spherical wave in
//! turbulence with a von Kármán spectrum, in the form of Kamrath et al.,
//! "Auralization of atmospheric turbulence-induced amplitude fluctuations in
//! aircraft flyover sound based on a semi-empirical model", Acta Acustica 8
//! (2024), equations 11, 12, 20–28 and 42. The variance of the log-amplitude
//! χ = ln(A / A₀) is
//!
//! ```text
//! ⟨χ²⟩ = φ_T² / 2 · G_T(D_T)  +  φ_v² / 2 · G_v(D_v)
//!
//! φ_T² = k² L σ_T² ℒ_T / (2 T₀²)        strength, temperature eddies
//! φ_v² = 2 k² L σ_v² ℒ_v / c₀²          strength, wind eddies
//! D    = L / (k L_x²)                    wave parameter
//! ℒ    = √π Γ(5/6) / Γ(1/3) · L_x        integral length, 0.7468 L_x
//! ```
//!
//! with k the acoustic wavenumber, L the path length, and G the share of the
//! strength that diffraction turns into level changes (the table below).
//! The eddies come from the atmosphere's surface layer (Monin–Obukhov
//! similarity): σ_v² = 3 u*², L_v = 1.8 z for wind shear, and σ_T², L_T from
//! the heat the sun puts into the ground.
//!
//! Everything here is a pure function of the air, the path and the
//! frequency, so it is tested without a device.

use std::f32::consts::PI;

/// Wind speed at 10 m for the wind condition at 1 (m/s): a strong gale. The
/// field wind's own follow rule reaches 9 m/s at 0.7, which this matches.
pub const FULL_WIND_METRES_PER_SECOND: f32 = 13.0;
/// Heat the ground gives the air under a clear midday sun (W/m²), a typical
/// value over grass in summer.
pub const CLEAR_NOON_HEAT_FLUX: f32 = 200.0;

/// Von Kármán's constant.
const VON_KARMAN: f32 = 0.4;
/// Roughness length of grass (m): where the logarithmic wind profile
/// reaches zero.
const ROUGHNESS_METRES: f32 = 0.03;
/// Height the wind condition's speed is given at (m), as weather stations do.
const WIND_HEIGHT_METRES: f32 = 10.0;
/// Heat capacity of a cubic metre of air (J/(m³·K)): 1.2 kg/m³ × 1005 J/(kg·K).
const AIR_HEAT_CAPACITY: f32 = 1206.0;
const GRAVITY: f32 = 9.81;
/// Depth of the daytime boundary layer (m), for the size of thermals.
const BOUNDARY_LAYER_METRES: f32 = 1000.0;
/// The friction velocity never taken lower than this (m/s) where it divides:
/// on a still sunny day the heat stirs the air by itself (free convection),
/// and the temperature formulas need a finite value to reach that limit.
const LEAST_FRICTION_VELOCITY: f32 = 0.05;
/// The surface-layer formulas are for heights well above the roughness; a
/// path that hugs the ground is taken at this height (m).
const LOWEST_PATH_HEIGHT: f32 = 0.5;
/// √π Γ(5/6) / Γ(1/3): the integral length of a von Kármán scale.
const INTEGRAL_LENGTH: f32 = 0.746_834;
/// The empirical boundary between weak and strong scattering, b² in the
/// paper's equations 29 and 43.
const WEAK_SCATTERING_LIMIT: f32 = 1.0 / 6.0;
/// Where the log-amplitude variance saturates however strong the
/// turbulence: π²/24, a level deviation of 5.6 dB.
pub const SATURATED_VARIANCE: f32 = PI * PI / 24.0;
/// 20 lg e: nepers of log-amplitude to dB.
pub const DECIBELS_PER_NEPER: f32 = 8.685_89;
/// The frequency whose Fresnel zone times the wavering (Hz): the treble,
/// which is what is heard to waver.
const FLUTTER_FREQUENCY_HZ: f32 = 4000.0;

/// The moving air of a place: its wind, and the heat rising from the ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AirMotion {
    /// Mean wind speed 10 m above the ground (m/s).
    pub wind_speed: f32,
    /// Unit vector over the ground the wind blows toward (x east, y north).
    pub wind_toward: [f32; 2],
    /// Heat the ground gives the air (W/m²); 0 at night and under cloud.
    pub heat_flux: f32,
}

impl AirMotion {
    /// Still air under no sun: nothing wavers.
    pub const STILL: Self = Self {
        wind_speed: 0.0,
        wind_toward: [1.0, 0.0],
        heat_flux: 0.0,
    };

    /// The air under the place's conditions: `wind` and `daylight` from 0 to
    /// 1, and the compass bearing the wind comes from in degrees (0 north,
    /// 90 east).
    pub fn from_weather(wind: f32, daylight: f32, wind_from_degrees: f32) -> Self {
        let bearing = wind_from_degrees.to_radians();
        Self {
            wind_speed: wind.clamp(0.0, 1.0) * FULL_WIND_METRES_PER_SECOND,
            // It comes from the bearing, so it blows the opposite way.
            wind_toward: [-bearing.sin(), -bearing.cos()],
            heat_flux: daylight.clamp(0.0, 1.0) * CLEAR_NOON_HEAT_FLUX,
        }
    }

    /// Whether anything stirs the air at all.
    pub fn is_still(&self) -> bool {
        self.wind_speed <= 0.0 && self.heat_flux <= 0.0
    }
}

/// The eddies at one height above the ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceLayer {
    /// Variance of the wind's fluctuations from shear (m²/s²).
    pub velocity_variance: f32,
    /// Their length scale (m).
    pub velocity_length: f32,
    /// Variance of the temperature's fluctuations (K²).
    pub temperature_variance: f32,
    /// Their length scale (m).
    pub temperature_length: f32,
    /// Mean wind speed at this height (m/s).
    pub mean_wind: f32,
    /// How fast the eddies themselves churn (m/s): what moves them across a
    /// path when the mean wind does not.
    pub eddy_speed: f32,
}

/// The eddies `height` metres above the ground in `air` at `temperature`
/// (K), by Monin–Obukhov similarity for a neutral or sunlit surface layer.
pub fn surface_layer(air: &AirMotion, height: f32, temperature: f32) -> SurfaceLayer {
    let height = height.max(LOWEST_PATH_HEIGHT);
    let profile = (WIND_HEIGHT_METRES / ROUGHNESS_METRES).ln();
    let friction_velocity = VON_KARMAN * air.wind_speed.max(0.0) / profile;
    let velocity_variance = 3.0 * friction_velocity * friction_velocity;
    let mean_wind = friction_velocity / VON_KARMAN * (height / ROUGHNESS_METRES).ln();

    let heat_flux = air.heat_flux.max(0.0);
    let (temperature_variance, temperature_length, convection_variance) = if heat_flux > 0.0 {
        let stirring = friction_velocity.max(LEAST_FRICTION_VELOCITY);
        let temperature_scale = heat_flux / (AIR_HEAT_CAPACITY * stirring);
        // −z / L_o, positive under a heated ground.
        let instability = height * GRAVITY * VON_KARMAN * heat_flux
            / (stirring.powi(3) * temperature * AIR_HEAT_CAPACITY);
        let convection_velocity = (BOUNDARY_LAYER_METRES * GRAVITY * heat_flux
            / (AIR_HEAT_CAPACITY * temperature))
            .cbrt();
        (
            4.0 * temperature_scale * temperature_scale
                / (1.0 + 10.0 * instability).powf(2.0 / 3.0),
            2.0 * height * (1.0 + 7.0 * instability) / (1.0 + 10.0 * instability),
            0.35 * convection_velocity * convection_velocity,
        )
    } else {
        (0.0, 2.0 * height, 0.0)
    };

    SurfaceLayer {
        velocity_variance,
        velocity_length: 1.8 * height,
        temperature_variance,
        temperature_length,
        mean_wind,
        eddy_speed: (velocity_variance + convection_variance).sqrt(),
    }
}

/// Variance of the log-amplitude of a tone at `frequency_hz` after `distance`
/// metres through `layer`, with the speed of sound and the temperature (K)
/// of the air: weak-scattering theory, eased toward saturation.
pub fn log_amplitude_variance(
    layer: &SurfaceLayer,
    frequency_hz: f32,
    distance: f32,
    speed_of_sound: f32,
    temperature: f32,
) -> f32 {
    saturating(weak_variance(
        layer,
        frequency_hz,
        distance,
        speed_of_sound,
        temperature,
    ))
}

/// The variance weak-scattering theory gives, before saturation.
fn weak_variance(
    layer: &SurfaceLayer,
    frequency_hz: f32,
    distance: f32,
    speed_of_sound: f32,
    temperature: f32,
) -> f32 {
    if distance <= 0.0 || frequency_hz <= 0.0 {
        return 0.0;
    }
    let wavenumber = 2.0 * PI * frequency_hz / speed_of_sound;
    let reach = wavenumber * wavenumber * distance;
    let temperature_strength =
        reach * layer.temperature_variance * INTEGRAL_LENGTH * layer.temperature_length
            / (2.0 * temperature * temperature);
    let velocity_strength =
        2.0 * reach * layer.velocity_variance * INTEGRAL_LENGTH * layer.velocity_length
            / (speed_of_sound * speed_of_sound);
    let wave_parameter = |length: f32| distance / (wavenumber * length * length);
    temperature_strength / 2.0
        * diffraction_share(&TEMPERATURE_SHARE, wave_parameter(layer.temperature_length))
        + velocity_strength / 2.0
            * diffraction_share(&VELOCITY_SHARE, wave_parameter(layer.velocity_length))
}

/// The weak-scattering variance eased toward saturation: the paper's
/// equation 42, ⟨χ²⟩ / (1 + φ / φ_max), which for one kind of eddy is
/// ⟨χ²⟩ / (1 + √(⟨χ²⟩ / b²)), and never above [`SATURATED_VARIANCE`].
pub fn saturating(weak_variance: f32) -> f32 {
    if weak_variance <= 0.0 {
        return 0.0;
    }
    let eased = weak_variance / (1.0 + (weak_variance / WEAK_SCATTERING_LIMIT).sqrt());
    eased.min(SATURATED_VARIANCE)
}

/// How long the level holds before it has wandered off (s): the time the air
/// takes to carry the eddies that matter, a Fresnel zone wide, across the
/// path. `crosswind` is the share of the mean wind that blows across the
/// path, 0 to 1; along the path only the eddies' own churning moves them.
pub fn wavering_seconds(
    layer: &SurfaceLayer,
    distance: f32,
    speed_of_sound: f32,
    crosswind: f32,
) -> f32 {
    let wavelength = speed_of_sound / FLUTTER_FREQUENCY_HZ;
    let fresnel_zone = (wavelength * distance.max(0.0)).sqrt();
    let across = layer.mean_wind * crosswind.clamp(0.0, 1.0);
    let speed = (across * across + layer.eddy_speed * layer.eddy_speed).sqrt();
    fresnel_zone.max(MINIMUM_EDDY_METRES) / speed.max(SLOWEST_AIR)
}

/// Eddies smaller than this are smoothed away by viscosity (m).
const MINIMUM_EDDY_METRES: f32 = 0.05;
/// Air slower than this is treated as this (m/s), so the time stays finite.
const SLOWEST_AIR: f32 = 0.05;

/// Decades of the wave parameter the tables cover, from 10⁻³ to 10².
const TABLE_FIRST_DECADE: f32 = -3.0;
const TABLE_STEPS_PER_DECADE: f32 = 4.0;
const TABLE_LENGTH: usize = 21;
/// Below the tables the share falls as D^(5/6), the inertial range: it gives
/// the classic ⟨χ²⟩ ∝ k^(7/6) L^(11/6).
const INERTIAL_SLOPE: f32 = 5.0 / 6.0;

/// G_v(D) = ∫₀¹ [1 − H_v(D, η)] dη for wind eddies, at D = 10^(−3 + i/4):
/// H_v = ∫₀^∞ α(α−1) x (1+x)^(−α−1) cos(η(1−η) x D) dx with α = 11/6
/// (the paper's equations 24–26, integrated numerically).
const VELOCITY_SHARE: [f32; TABLE_LENGTH] = [
    0.00184, 0.00297, 0.00480, 0.00773, 0.01246, 0.02003, 0.03209, 0.05117, 0.08092, 0.12638,
    0.19371, 0.28888, 0.41445, 0.56448, 0.72038, 0.85411, 0.94305, 0.98573, 0.99920, 1.0, 1.0,
];
/// G_T(D) for temperature eddies, at the same D:
/// H_T = ∫₀^∞ (α−1) (1+x)^(−α) cos(η(1−η) x D) dx (equations 21–22).
const TEMPERATURE_SHARE: [f32; TABLE_LENGTH] = [
    0.00100, 0.00162, 0.00262, 0.00423, 0.00681, 0.01097, 0.01762, 0.02822, 0.04492, 0.07089,
    0.11041, 0.16858, 0.25026, 0.35745, 0.48571, 0.62180, 0.74661, 0.84425, 0.91008, 0.94978,
    0.97226,
];

/// The share of the strength that becomes level changes at wave parameter
/// `wave_parameter`: none in the limit of straight rays, all of it where
/// diffraction rules.
fn diffraction_share(table: &[f32; TABLE_LENGTH], wave_parameter: f32) -> f32 {
    if wave_parameter <= 0.0 {
        return 0.0;
    }
    let position = (wave_parameter.log10() - TABLE_FIRST_DECADE) * TABLE_STEPS_PER_DECADE;
    if position <= 0.0 {
        let first = 10f32.powf(TABLE_FIRST_DECADE);
        return table[0] * (wave_parameter / first).powf(INERTIAL_SLOPE);
    }
    let last = (TABLE_LENGTH - 1) as f32;
    if position >= last {
        return table[TABLE_LENGTH - 1];
    }
    let index = position as usize;
    let along = position - index as f32;
    // Straight between neighbours on logarithmic axes, as the curve nearly is.
    (table[index].ln() + (table[index + 1].ln() - table[index].ln()) * along).exp()
}

mod waver;

pub use waver::{AirPath, AirWaver, Wavering};

#[cfg(test)]
mod tests;
