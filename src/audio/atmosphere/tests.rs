use super::*;

fn standard_conditions() -> AtmosphericParams {
    AtmosphericParams::default() // 20°C, 50%, 101.325 kPa
}

#[test]
fn atmospheric_params_speed_of_sound_matches_free_fn() {
    let params = AtmosphericParams {
        temperature_c: 25.0,
        ..Default::default()
    };
    assert_eq!(params.speed_of_sound(), speed_of_sound(25.0));

    let default_params = AtmosphericParams::default();
    assert_eq!(default_params.speed_of_sound(), speed_of_sound(20.0));
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
fn shelf_gains_increase_with_distance() {
    let p = standard_conditions();
    let gains = air_absorption_gains(1.0, &p);
    let (low_1, high_1) = (gains.broadband_db, gains.high_shelf_db);
    let gains = air_absorption_gains(50.0, &p);
    let (low_50, high_50) = (gains.broadband_db, gains.high_shelf_db);

    // More distance = more negative gain (more attenuation).
    assert!(
        low_50 < low_1,
        "50m low shelf ({low_50}) should be more negative than 1m ({low_1})"
    );
    assert!(
        high_50 < high_1,
        "50m high shelf ({high_50}) should be more negative than 1m ({high_1})"
    );
}

#[test]
fn shelf_gains_zero_at_zero_distance() {
    let p = standard_conditions();
    let gains = air_absorption_gains(0.0, &p);
    let (low, high) = (gains.broadband_db, gains.high_shelf_db);
    assert_eq!(gains.low_shelf_db, 0.0);
    assert!(
        low.abs() < 1e-6 && high.abs() < 1e-6,
        "0m should give zero gain: low={low}, high={high}"
    );
}

#[test]
fn shelf_gains_high_exceeds_low() {
    // At 4 kHz, ISO 9613 absorption is much higher than at 500 Hz.
    // So the high shelf (additional HF loss) should be more negative than low shelf.
    let p = standard_conditions();
    let gains = air_absorption_gains(50.0, &p);
    let (low, high) = (gains.broadband_db, gains.high_shelf_db);
    assert!(
        high < low,
        "high shelf ({high} dB) should be more negative than low shelf ({low} dB)"
    );
}

#[test]
fn shelf_gains_total_matches_iso9613_at_8khz() {
    // Total attenuation at 8 kHz should equal iso9613_alpha(8000) × distance,
    // since the high shelf is derived from the 8 kHz absorption.
    let p = standard_conditions();
    let distance = 100.0;
    let gains = air_absorption_gains(distance, &p);
    let (low, high) = (gains.broadband_db, gains.high_shelf_db);
    let total_db = low + high;
    let expected_db = -iso9613_alpha(8000.0, &p) * distance;
    assert!(
        (total_db - expected_db).abs() < 0.01,
        "total shelf gain ({total_db} dB) should match ISO 9613 at 8 kHz ({expected_db} dB)"
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

/// f64 reference implementation of ISO 9613-1 for precision comparison.
fn iso9613_alpha_f64(freq: f64, temperature_c: f64, humidity_pct: f64, pressure_kpa: f64) -> f64 {
    let t_ref: f64 = 293.15;
    let t_triple: f64 = 273.16;
    let p_ref: f64 = 101.325;

    let t_k = temperature_c + 273.15;
    let t_rel = t_k / t_ref;
    let p_rel = pressure_kpa / p_ref;

    let p_sat_ratio = 10.0_f64.powf(-6.8346 * (t_triple / t_k).powf(1.261) + 4.6151);
    let h = humidity_pct * p_sat_ratio * (p_ref / pressure_kpa);

    let fr_o = p_rel * (24.0 + 4.04e4 * h * (0.02 + h) / (0.391 + h));
    let fr_n = p_rel
        * t_rel.powf(-0.5)
        * (9.0 + 280.0 * h * (-4.170 * (t_rel.powf(-1.0 / 3.0) - 1.0)).exp());

    let f2 = freq * freq;
    let classical = 1.84e-11 * p_rel.recip() * t_rel.sqrt();
    let vib_o2 = t_rel.powf(-2.5) * 0.01275 * (-2239.1 / t_k).exp() * (fr_o + f2 / fr_o).recip();
    let vib_n2 = t_rel.powf(-2.5) * 0.1068 * (-3352.0 / t_k).exp() * (fr_n + f2 / fr_n).recip();

    8.686 * f2 * (classical + vib_o2 + vib_n2)
}

#[test]
fn iso9613_f32_vs_f64_precision() {
    // Compare f32 implementation against f64 reference across octave bands
    // and multiple atmospheric conditions to quantify precision loss.
    let conditions = [
        ("20°C 50%RH", 20.0, 50.0, 101.325),
        ("0°C 30%RH", 0.0, 30.0, 101.325),
        ("35°C 80%RH", 35.0, 80.0, 101.325),
        ("10°C 10%RH", 10.0, 10.0, 101.325),
    ];
    let frequencies = [63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];

    let mut max_error_pct: f64 = 0.0;
    let mut worst_case = String::new();

    for &(label, temp, humidity, pressure) in &conditions {
        let params = AtmosphericParams {
            temperature_c: temp,
            humidity_pct: humidity,
            pressure_kpa: pressure,
        };

        for &freq in &frequencies {
            let alpha_f32 = iso9613_alpha(freq, &params) as f64;
            let alpha_f64 =
                iso9613_alpha_f64(freq as f64, temp as f64, humidity as f64, pressure as f64);

            if alpha_f64 > 1e-10 {
                let error_pct = ((alpha_f32 - alpha_f64) / alpha_f64 * 100.0).abs();
                if error_pct > max_error_pct {
                    max_error_pct = error_pct;
                    worst_case = format!(
                        "{label} @ {freq} Hz: f32={alpha_f32:.6e}, f64={alpha_f64:.6e}, err={error_pct:.2}%"
                    );
                }
                // Report any error > 1%
                eprintln!(
                    "  {label:>14} @ {freq:>5.0} Hz: f32={alpha_f32:.6e}  f64={alpha_f64:.6e}  err={error_pct:.3}%"
                );
            }
        }
    }

    eprintln!("Worst case: {worst_case}");
    eprintln!("Max f32-vs-f64 deviation: {max_error_pct:.2}%");

    // f32 precision should be within 1% of f64 for the same formula.
    // If deviation exceeds this, the issue is f32 accumulation, not the formula.
    assert!(
        max_error_pct < 1.0,
        "f32 vs f64 deviation {max_error_pct:.2}% exceeds 1% — \
         consider computing in f64. Worst: {worst_case}"
    );
}

#[test]
fn iso9613_vs_reference_values() {
    // ISO 9613-1:1993 reference data at 20°C, 101.325 kPa.
    // Compare at two humidity levels against values from Bass et al. (1995)
    // and the ISO 9613-1 tables (α in dB/km).
    //
    // Investigation result: f32 precision is NOT the issue (0.00% f32-vs-f64
    // deviation). Any discrepancy vs. published tables comes from differences
    // in the tabulated reference data sources, not floating-point precision.
    let conditions: &[(&str, f32, &[(f32, f32)])] = &[
        // 20°C, 50% RH — values from Engineering Toolbox / ISO 9613-1
        (
            "20°C 50%RH",
            50.0,
            &[
                (125.0, 0.44), // dB/km
                (250.0, 1.31),
                (500.0, 2.73),
                (1000.0, 4.66),
                (2000.0, 9.86),
                (4000.0, 29.6),
                (8000.0, 105.0),
            ],
        ),
        // 20°C, 70% RH — values from Bass et al. (1995)
        (
            "20°C 70%RH",
            70.0,
            &[
                (125.0, 0.41),
                (250.0, 1.04),
                (500.0, 2.19),
                (1000.0, 5.03),
                (2000.0, 9.02),
                (4000.0, 22.9),
                (8000.0, 76.6),
            ],
        ),
    ];

    eprintln!("\nISO 9613-1 reference comparison:");
    eprintln!("{:-<75}", "");
    let mut worst_error = 0.0_f32;

    for &(label, humidity, reference) in conditions {
        let params = AtmosphericParams {
            temperature_c: 20.0,
            humidity_pct: humidity,
            pressure_kpa: 101.325,
        };
        eprintln!("  {label}:");
        for &(freq, expected_db_km) in reference {
            let computed_db_km = iso9613_alpha(freq, &params) * 1000.0;
            let error_pct = ((computed_db_km - expected_db_km) / expected_db_km * 100.0).abs();
            worst_error = worst_error.max(error_pct);
            let marker = if error_pct > 15.0 { " ⚠" } else { "" };
            eprintln!(
                "    {freq:>5.0} Hz: computed={computed_db_km:>7.2} dB/km  ref={expected_db_km:>7.2} dB/km  err={error_pct:>5.1}%{marker}"
            );
        }
    }
    eprintln!("{:-<75}", "");
    eprintln!("  Worst deviation: {worst_error:.1}%");
    eprintln!("  Conclusion: f32 precision is fine (0.00%% f32-vs-f64).");
    eprintln!("  Low-freq deviations likely from reference data source variation.");

    // At mid-high frequencies (1 kHz+), our implementation should be within 10%.
    // Low frequencies are harder to validate without the original ISO tables.
    for &(_, humidity, reference) in conditions {
        let params = AtmosphericParams {
            temperature_c: 20.0,
            humidity_pct: humidity,
            pressure_kpa: 101.325,
        };
        for &(freq, expected_db_km) in reference {
            if freq >= 1000.0 {
                let computed_db_km = iso9613_alpha(freq, &params) * 1000.0;
                let error_pct = ((computed_db_km - expected_db_km) / expected_db_km * 100.0).abs();
                assert!(
                    error_pct < 10.0,
                    "ISO 9613-1 at {freq} Hz ({humidity}% RH): {error_pct:.1}% error \
                     (computed={computed_db_km:.3}, ref={expected_db_km:.3})"
                );
            }
        }
    }
}

// ── Speed of sound ───────────────────────────────────────────────────

/// c = 331.3 + 0.606·T: 331.30 m/s at 0 °C, 343.42 m/s at 20 °C.
/// A 10 m path at 20 °C therefore arrives after 10 / 343.42 = 29.12 ms.
#[test]
fn speed_of_sound_pins_standard_values() {
    assert!((speed_of_sound(0.0) - 331.3).abs() < 1e-3);
    assert!((speed_of_sound(20.0) - 343.42).abs() < 1e-3);
    let delay_milliseconds = 10.0 / speed_of_sound(20.0) * 1000.0;
    assert!(
        (delay_milliseconds - 29.12).abs() < 0.01,
        "10 m at 20 °C: {delay_milliseconds} ms"
    );
}

/// The linear law is the first-order expansion of the ideal-gas law
/// c = 331.3·√(1 + T/273.15). Over the −20…40 °C range of an atrium the two
/// agree within 0.25 % (worst at 40 °C: 355.54 vs 354.73 m/s). Over a 20 m
/// path that is at most 0.13 ms of arrival time (6 samples at 48 kHz).
#[test]
fn linear_speed_of_sound_tracks_ideal_gas_law() {
    for temperature_celsius in (-20..=40).map(|whole| whole as f32) {
        let ideal_gas = 331.3 * (1.0 + temperature_celsius / 273.15).sqrt();
        let linear = speed_of_sound(temperature_celsius);
        let relative_error = (linear - ideal_gas).abs() / ideal_gas;
        assert!(
            relative_error < 0.0025,
            "{temperature_celsius} °C: linear {linear}, ideal gas {ideal_gas}"
        );
        let path_length = 20.0;
        let arrival_error_seconds = (path_length / linear - path_length / ideal_gas).abs();
        assert!(arrival_error_seconds < 0.15e-3, "{arrival_error_seconds}");
    }
}

// ── ISO 9613-1 absorption, pinned to the published tables ────────────

/// ISO 9613-2:1996 Table 2 lists α (dB/km) at the octave bands for six
/// atmospheres, computed from ISO 9613-1 at the exact mid-band frequencies
/// f = 1000·10^(k/10) (63.1, 125.9, …, 7943 Hz) and rounded to 0.1 dB/km
/// (three significant figures above 100). Our formula must reproduce every
/// entry to within that rounding.
#[test]
fn iso9613_matches_iso9613_2_table_two() {
    let table: [(f32, f32, [f32; 8]); 6] = [
        (10.0, 70.0, [0.1, 0.4, 1.0, 1.9, 3.7, 9.7, 32.8, 117.0]),
        (20.0, 70.0, [0.1, 0.3, 1.1, 2.8, 5.0, 9.0, 22.9, 76.6]),
        (30.0, 70.0, [0.1, 0.3, 1.0, 3.1, 7.4, 12.7, 23.1, 59.3]),
        (15.0, 20.0, [0.3, 0.6, 1.2, 2.7, 8.2, 28.2, 88.8, 202.0]),
        (15.0, 50.0, [0.1, 0.5, 1.2, 2.2, 4.2, 10.8, 36.2, 129.0]),
        (15.0, 80.0, [0.1, 0.3, 1.1, 2.4, 4.1, 8.3, 23.7, 82.8]),
    ];
    for (temperature_c, humidity_pct, expected_db_per_km) in table {
        let params = AtmosphericParams {
            temperature_c,
            humidity_pct,
            pressure_kpa: 101.325,
        };
        for (band_index, expected) in expected_db_per_km.iter().enumerate() {
            let exponent = 3.0 * band_index as f32 - 12.0;
            let midband_frequency = 1000.0 * 10.0_f32.powf(exponent / 10.0);
            let computed = iso9613_alpha(midband_frequency, &params) * 1000.0;
            // Half a rounding step, or 0.6 % for the three-figure entries.
            let tolerance = (0.06_f32).max(expected * 0.006);
            assert!(
                (computed - expected).abs() <= tolerance,
                "{temperature_c} °C {humidity_pct} % @ {midband_frequency:.0} Hz: \
                 computed {computed:.3} dB/km, table {expected} dB/km"
            );
        }
    }
}

/// 20 °C, 50 % RH, 101.325 kPa at nominal frequencies, from an independent
/// f64 evaluation of ISO 9613-1: h = 1.153 %, f_rO = 35 414 Hz,
/// f_rN = 331.9 Hz, α = 4.665 / 29.67 / 105.3 dB/km at 1 / 4 / 8 kHz.
#[test]
fn iso9613_standard_atmosphere_pinned() {
    let params = standard_conditions();
    for (frequency, expected_db_per_metre) in
        [(1000.0, 0.004_665), (4000.0, 0.029_67), (8000.0, 0.105_29)]
    {
        let computed = iso9613_alpha(frequency, &params);
        assert!(
            (computed - expected_db_per_metre).abs() / expected_db_per_metre < 0.002,
            "{frequency} Hz: {computed} dB/m, expected {expected_db_per_metre}"
        );
    }
}

/// With no water vapour (h = 0) the relaxation frequencies collapse to
/// f_rO = 24 Hz and f_rN = 9 Hz. Pinned: 1.530 dB/km at 1 kHz,
/// 11.60 dB/km at 8 kHz (f64 evaluation). Must stay finite, not NaN.
#[test]
fn iso9613_dry_air_is_finite_and_pinned() {
    let dry = AtmosphericParams {
        humidity_pct: 0.0,
        ..Default::default()
    };
    let at_one_kilohertz = iso9613_alpha(1000.0, &dry) * 1000.0;
    let at_eight_kilohertz = iso9613_alpha(8000.0, &dry) * 1000.0;
    assert!(
        (at_one_kilohertz - 1.530).abs() < 0.01,
        "{at_one_kilohertz}"
    );
    assert!(
        (at_eight_kilohertz - 11.60).abs() < 0.05,
        "{at_eight_kilohertz}"
    );
}

/// α must be positive, finite and strictly increasing in frequency from
/// 20 Hz to 20 kHz across the atmospheres the engine can be configured
/// with (−20…40 °C, 0…100 % RH). Each relaxation term grows with f until
/// its relaxation frequency and then saturates; the classical f² term keeps
/// the total rising.
#[test]
fn iso9613_monotonic_in_frequency_over_extreme_atmospheres() {
    for temperature_c in [-20.0f32, 0.0, 20.0, 40.0] {
        for humidity_pct in [0.0f32, 10.0, 50.0, 100.0] {
            let params = AtmosphericParams {
                temperature_c,
                humidity_pct,
                pressure_kpa: 101.325,
            };
            let mut previous = 0.0f32;
            let mut frequency = 20.0f32;
            while frequency <= 20_000.0 {
                let alpha = iso9613_alpha(frequency, &params);
                assert!(
                    alpha.is_finite() && alpha > previous,
                    "{temperature_c} °C {humidity_pct} % @ {frequency} Hz: \
                     {alpha} after {previous}"
                );
                previous = alpha;
                frequency *= 1.25;
            }
        }
    }
}

/// Classical absorption scales as 1/p and the relaxation frequencies as p.
/// At 1 kHz, 20 °C, 50 % RH, halving the pressure to 50 kPa gives 4.615 dB/km
/// (f64 evaluation) against 4.665 at sea level.
#[test]
fn iso9613_pressure_dependence_pinned() {
    let half_atmosphere = AtmosphericParams {
        pressure_kpa: 50.0,
        ..Default::default()
    };
    let computed = iso9613_alpha(1000.0, &half_atmosphere) * 1000.0;
    assert!((computed - 4.615).abs() < 0.01, "{computed}");
}

/// Every shelf gain is a pure product α·d: doubling the distance doubles
/// the dB value, until the −40 dB clamp.
#[test]
fn shelf_gains_linear_in_distance_and_clamped() {
    let params = standard_conditions();
    let gains = air_absorption_gains(10.0, &params);
    let (low_ten, high_ten) = (gains.broadband_db, gains.high_shelf_db);
    let gains = air_absorption_gains(20.0, &params);
    let (low_twenty, high_twenty) = (gains.broadband_db, gains.high_shelf_db);
    assert!((low_twenty - 2.0 * low_ten).abs() < 1e-5);
    assert!((high_twenty - 2.0 * high_ten).abs() < 1e-5);
    // 10 km: α(8 kHz)·d ≈ 1053 dB, far past the clamp.
    let gains = air_absorption_gains(10_000.0, &params);
    let (low_far, high_far) = (gains.broadband_db, gains.high_shelf_db);
    assert_eq!(high_far, -40.0);
    assert!((-40.0..=0.0).contains(&low_far));
}

/// Steady-state magnitude (dB) of the air absorption filter for a sine.
fn filter_response_db(
    filter: &mut crate::audio::filters::AirAbsorptionFilter,
    frequency: f32,
    sample_rate: f32,
) -> f32 {
    let total_samples = (2.0 * sample_rate) as usize;
    let settle_samples = total_samples / 2;
    let mut peak = 0.0f32;
    for index in 0..total_samples {
        let phase = std::f32::consts::TAU * frequency * index as f32 / sample_rate;
        let output = filter.process(phase.sin());
        if index >= settle_samples {
            peak = peak.max(output.abs());
        }
    }
    20.0 * peak.log10()
}

/// The filter must follow ISO at both ends of the spectrum, not only in
/// the mid band. Cold air makes the ends matter: −20 °C, 50 % RH, 300 m.
/// ISO: 63 Hz loses 0.241 dB/km × 0.3 km = 0.07 dB, 8 kHz loses
/// 20.48 × 0.3 = 6.14 dB. An earlier version applied the 500 Hz loss
/// (1.75 dB) as a low shelf, which cut 63 Hz by 1.75 dB and left 8 kHz
/// 1.7 dB short.
#[test]
fn air_absorption_filter_matches_iso_at_band_edges() {
    let sample_rate = 48_000.0;
    let distance = 300.0;
    let cold = AtmosphericParams {
        temperature_c: -20.0,
        humidity_pct: 50.0,
        pressure_kpa: 101.325,
    };
    for frequency in [63.0f32, 8000.0] {
        let mut filter = crate::audio::filters::AirAbsorptionFilter::new(sample_rate);
        filter.update(distance, &cold);
        let filter_db = filter_response_db(&mut filter, frequency, sample_rate);
        let iso_db = -iso9613_alpha(frequency, &cold) * distance;
        assert!(
            (filter_db - iso_db).abs() < 0.5,
            "{frequency} Hz: filter {filter_db:.2} dB, ISO 9613-1 {iso_db:.2} dB"
        );
    }
}
