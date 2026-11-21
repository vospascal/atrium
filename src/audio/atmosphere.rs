/// ISO 9613-1:1993 — Atmospheric sound absorption.
///
/// Computes frequency-dependent absorption coefficients (dB/m) from temperature,
/// humidity, and pressure. Used to derive a per-source low-pass filter cutoff
/// that models how air absorbs high frequencies more than lows.
///
/// Reference: <https://www.w3.org/TR/webaudio/#distance-attenuation>
/// Python reference: python-acoustics ISO 9613-1 implementation
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

impl AtmosphericParams {
    /// Temperature-corrected speed of sound (m/s).
    /// Delegates to `speed_of_sound(self.temperature_c)`.
    #[inline]
    pub fn speed_of_sound(&self) -> f32 {
        speed_of_sound(self.temperature_c)
    }
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

/// Gains (in dB) of the three-part air absorption fit at one distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AirAbsorptionGains {
    /// Applied to every frequency: −α(500 Hz) × distance.
    pub broadband_db: f32,
    /// Low shelf that gives the bottom octaves back what the broadband gain
    /// took and ISO does not: +(α(500 Hz) − α(63 Hz)) × distance.
    pub low_shelf_db: f32,
    /// High shelf for the steep rise above the mid band:
    /// −(α(8 kHz) − α(500 Hz)) × distance.
    pub high_shelf_db: f32,
}

/// Fit ISO 9613-1 air absorption at `distance` with a broadband gain and two
/// shelves, exact at 63 Hz, 500 Hz and 8 kHz.
///
/// - **Broadband gain**: the 500 Hz absorption, the level of the mid band.
/// - **Low shelf (177 Hz)**: absorption falls roughly as f² toward low
///   frequencies, so the bottom octaves lose almost nothing; this shelf
///   restores them. Without it the lows would be cut as much as 500 Hz is
///   (1.7 dB too much at 300 m in −20 °C air).
/// - **High shelf (4 kHz)**: the additional loss from N₂ relaxation and
///   classical absorption.
///
/// The high shelf gain is derived from the 8 kHz absorption (not 4 kHz) because
/// a 2nd-order shelf provides roughly half its gain at the center frequency and
/// full gain one octave above. By targeting 8 kHz, the shelf naturally gives:
/// - ~half the gain at 4 kHz (close to the actual α(4 kHz) value)
/// - full gain at 8 kHz (matching the steep HF rolloff)
///
/// Accuracy: within 1.5 dB at 4 kHz and within 0.5 dB at 8 kHz across typical
/// indoor distances. Above 8 kHz the fit saturates while ISO keeps rising.
///
/// Every gain is clamped to 40 dB to prevent extreme filter values at very
/// large distances.
pub fn air_absorption_gains(distance: f32, params: &AtmosphericParams) -> AirAbsorptionGains {
    let alpha_bottom = iso9613_alpha(63.0, params); // dB/m at 63 Hz
    let alpha_mid = iso9613_alpha(500.0, params); // dB/m at 500 Hz
    let alpha_high = iso9613_alpha(8000.0, params); // dB/m at 8 kHz

    AirAbsorptionGains {
        broadband_db: (-alpha_mid * distance).clamp(-40.0, 0.0),
        low_shelf_db: ((alpha_mid - alpha_bottom) * distance).clamp(0.0, 40.0),
        high_shelf_db: (-(alpha_high - alpha_mid) * distance).clamp(-40.0, 0.0),
    }
}

#[cfg(test)]
mod tests;
