/// ISO 9613-1:1993 — Atmospheric sound absorption.
///
/// Computes frequency-dependent absorption coefficients (dB/m) from temperature,
/// humidity, and pressure. Used to derive a per-source low-pass filter cutoff
/// that models how air absorbs high frequencies more than lows.
///
/// Reference: <https://www.w3.org/TR/webaudio/#distance-attenuation>
/// Python reference: python-acoustics ISO 9613-1 implementation
/// Speed of sound at 20°C, 1 atm (m/s). ISO 9613-1 reference conditions.
pub const SPEED_OF_SOUND: f32 = 343.0;

/// Temperature-dependent speed of sound (m/s).
/// Approximation from ISO 9613-1: c ≈ 331.3 + 0.606 × T_celsius.
#[inline]
pub fn speed_of_sound(temp_c: f32) -> f32 {
    331.3 + 0.606 * temp_c
}

/// ISO 9613-1 constants.
const T_REF: f32 = 293.15; // Reference temperature (20°C) in Kelvin
const T_TRIPLE: f32 = 273.16; // Triple point of water in Kelvin
const P_REF: f32 = 101.325; // International Standard Atmosphere in kPa

/// Atmospheric conditions for ISO 9613-1 absorption calculation.
///
/// All fields are `Copy` so this struct can be sent via the rtrb Command queue.
#[derive(Clone, Copy, Debug)]
pub struct AtmosphericParams {
    /// Temperature in degrees Celsius.
    pub temperature_c: f32,
    /// Relative humidity as percentage (0–100).
    pub humidity_pct: f32,
    /// Ambient air pressure in kPa.
    pub pressure_kpa: f32,
}

impl Default for AtmosphericParams {
    fn default() -> Self {
        Self {
            temperature_c: 20.0,
            humidity_pct: 50.0,
            pressure_kpa: P_REF,
        }
    }
}

/// Compute the atmospheric absorption coefficient α in dB/m at a given frequency,
/// per ISO 9613-1:1993.
///
/// The formula has three additive terms:
/// 1. Classical absorption (viscosity + thermal conduction) — proportional to f²
/// 2. Oxygen vibrational relaxation — peaks around fr_O
/// 3. Nitrogen vibrational relaxation — peaks around fr_N
pub fn iso9613_alpha(freq: f32, params: &AtmosphericParams) -> f32 {
    let freq = freq.max(1.0); // Guard against zero/negative frequency
    let t_k = params.temperature_c + 273.15; // Temperature in Kelvin
    let t_rel = t_k / T_REF; // T / T_ref
    let p_rel = params.pressure_kpa / P_REF; // p / p_ref

    // 1. Saturation vapour pressure: p_sat / p_ref = 10^C
    //    C = -6.8346 * (T_triple / T)^1.261 + 4.6151
    let p_sat_ratio = 10.0_f32.powf(-6.8346 * (T_TRIPLE / t_k).powf(1.261) + 4.6151);

    // 2. Molar concentration of water vapour: h = humidity * p_sat / p_ambient
    let h = params.humidity_pct * p_sat_ratio * (P_REF / params.pressure_kpa);

    // 3. Oxygen relaxation frequency
    let fr_o = p_rel * (24.0 + 4.04e4 * h * (0.02 + h) / (0.391 + h));

    // 4. Nitrogen relaxation frequency
    let fr_n = p_rel
        * t_rel.powf(-0.5)
        * (9.0 + 280.0 * h * (-4.170 * (t_rel.powf(-1.0 / 3.0) - 1.0)).exp());

    // 5. Absorption coefficient (dB/m)
    let f2 = freq * freq;

    // Classical + rotational absorption
    let classical = 1.84e-11 * p_rel.recip() * t_rel.sqrt();

    // Vibrational absorption (O2)
    let vib_o2 = t_rel.powf(-2.5) * 0.01275 * (-2239.1 / t_k).exp() * (fr_o + f2 / fr_o).recip();

    // Vibrational absorption (N2)
    let vib_n2 = t_rel.powf(-2.5) * 0.1068 * (-3352.0 / t_k).exp() * (fr_n + f2 / fr_n).recip();

    let alpha = 8.686 * f2 * (classical + vib_o2 + vib_n2);

    if alpha.is_finite() {
        alpha
    } else {
        0.0
    }
}

/// Derive a low-pass filter cutoff frequency from ISO 9613-1 absorption.
///
/// Computes absorption at a reference frequency (4 kHz), scales by distance,
/// and converts the dB loss to an equivalent cutoff frequency:
///   cutoff = 20kHz × 10^(-absorption_dB / 20)
///
/// Each ~6 dB of absorption halves the cutoff.
pub fn iso9613_cutoff(distance: f32, params: &AtmosphericParams) -> f32 {
    const REFERENCE_FREQ: f32 = 4000.0;
    const MAX_CUTOFF: f32 = 20000.0;
    const MIN_CUTOFF: f32 = 200.0;

    let alpha = iso9613_alpha(REFERENCE_FREQ, params);
    let total_db = alpha * distance;

    let cutoff = MAX_CUTOFF * 10.0_f32.powf(-total_db / 20.0);
    cutoff.clamp(MIN_CUTOFF, MAX_CUTOFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standard_conditions() -> AtmosphericParams {
        AtmosphericParams::default() // 20°C, 50%, 101.325 kPa
    }

    #[test]
    fn iso9613_standard_conditions_order_of_magnitude() {
        let p = standard_conditions();
        let a1k = iso9613_alpha(1000.0, &p);
        let a4k = iso9613_alpha(4000.0, &p);
        let a8k = iso9613_alpha(8000.0, &p);

        // ISO tables for 20°C, 50% RH, 101.325 kPa (approximate ranges)
        assert!(a1k > 0.001 && a1k < 0.02, "1kHz alpha={a1k}");
        assert!(a4k > 0.005 && a4k < 0.05, "4kHz alpha={a4k}");
        assert!(a8k > 0.02 && a8k < 0.15, "8kHz alpha={a8k}");
    }

    #[test]
    fn absorption_increases_with_frequency() {
        let p = standard_conditions();
        let a1k = iso9613_alpha(1000.0, &p);
        let a4k = iso9613_alpha(4000.0, &p);
        let a8k = iso9613_alpha(8000.0, &p);
        assert!(a4k > a1k, "4kHz ({a4k}) should > 1kHz ({a1k})");
        assert!(a8k > a4k, "8kHz ({a8k}) should > 4kHz ({a4k})");
    }

    #[test]
    fn cutoff_decreases_with_distance() {
        let p = standard_conditions();
        let c1 = iso9613_cutoff(1.0, &p);
        let c5 = iso9613_cutoff(5.0, &p);
        let c20 = iso9613_cutoff(20.0, &p);
        assert!(c1 > c5, "1m cutoff ({c1}) should > 5m ({c5})");
        assert!(c5 > c20, "5m cutoff ({c5}) should > 20m ({c20})");
    }

    #[test]
    fn zero_distance_is_transparent() {
        let p = standard_conditions();
        let cutoff = iso9613_cutoff(0.0, &p);
        assert!(
            (cutoff - 20000.0).abs() < 1.0,
            "cutoff at 0m should be ~20kHz, got {cutoff}"
        );
    }

    #[test]
    fn humidity_affects_absorption() {
        let dry = AtmosphericParams {
            humidity_pct: 10.0,
            ..Default::default()
        };
        let humid = AtmosphericParams {
            humidity_pct: 80.0,
            ..Default::default()
        };
        let a_dry = iso9613_alpha(8000.0, &dry);
        let a_humid = iso9613_alpha(8000.0, &humid);
        // Both should be positive and different
        assert!(a_dry > 0.0 && a_humid > 0.0);
        assert!(
            (a_dry - a_humid).abs() > 0.001,
            "humidity should affect absorption: dry={a_dry}, humid={a_humid}"
        );
    }

    #[test]
    fn temperature_affects_absorption() {
        let cold = AtmosphericParams {
            temperature_c: 0.0,
            ..Default::default()
        };
        let warm = AtmosphericParams {
            temperature_c: 35.0,
            ..Default::default()
        };
        let a_cold = iso9613_alpha(4000.0, &cold);
        let a_warm = iso9613_alpha(4000.0, &warm);
        assert!(a_cold > 0.0 && a_warm > 0.0);
        assert!(
            (a_cold - a_warm).abs() > 0.001,
            "temperature should affect absorption: cold={a_cold}, warm={a_warm}"
        );
    }
}
