use super::*;
use crate::audio::atmosphere::iso9613_alpha;
use realfft::num_complex::Complex;

const SAMPLE_RATES: [f32; 3] = [44_100.0, 48_000.0, 96_000.0];

// ── Independent references ──────────────────────────────────────────────
//
// Every reference below is derived from the *analog* prototype in the RBJ
// Audio EQ Cookbook mapped through the bilinear transform with frequency
// prewarping at the design frequency: s = j · tan(ω/2) / tan(ω0/2).
// None of them read the coefficients that the code under test produced.

/// Normalized analog frequency for the prewarped bilinear map.
fn prewarped_ratio(frequency_hz: f64, design_hz: f64, sample_rate: f64) -> f64 {
    (std::f64::consts::PI * frequency_hz / sample_rate).tan()
        / (std::f64::consts::PI * design_hz / sample_rate).tan()
}

fn to_db(magnitude: f64) -> f64 {
    20.0 * magnitude.log10()
}

/// Butterworth (Q = 1/√2) lowpass magnitude in dB: -10·log10(1 + r⁴).
fn reference_lowpass_db(frequency_hz: f64, cutoff_hz: f64, sample_rate: f64) -> f64 {
    let ratio = prewarped_ratio(frequency_hz, cutoff_hz, sample_rate);
    -10.0 * (1.0 + ratio.powi(4)).log10()
}

/// Butterworth (Q = 1/√2) highpass magnitude in dB: -10·log10(1 + r⁻⁴).
fn reference_highpass_db(frequency_hz: f64, cutoff_hz: f64, sample_rate: f64) -> f64 {
    let ratio = prewarped_ratio(frequency_hz, cutoff_hz, sample_rate);
    -10.0 * (1.0 + ratio.powi(-4)).log10()
}

/// RBJ low-shelf analog prototype with Q = 1/√2:
/// H(s) = A·(s² + (√A/Q)·s + A) / (A·s² + (√A/Q)·s + 1).
fn reference_low_shelf_db(frequency_hz: f64, shelf_hz: f64, gain_db: f64, sample_rate: f64) -> f64 {
    let a = 10f64.powf(gain_db / 40.0);
    let sqrt_a_over_q = a.sqrt() * std::f64::consts::SQRT_2;
    let s = Complex::new(0.0, prewarped_ratio(frequency_hz, shelf_hz, sample_rate));
    let numerator = (s * s + s * sqrt_a_over_q + a) * a;
    let denominator = s * s * a + s * sqrt_a_over_q + 1.0;
    to_db((numerator / denominator).norm())
}

/// RBJ high-shelf analog prototype with Q = 1/√2:
/// H(s) = A·(A·s² + (√A/Q)·s + 1) / (s² + (√A/Q)·s + A).
fn reference_high_shelf_db(
    frequency_hz: f64,
    shelf_hz: f64,
    gain_db: f64,
    sample_rate: f64,
) -> f64 {
    let a = 10f64.powf(gain_db / 40.0);
    let sqrt_a_over_q = a.sqrt() * std::f64::consts::SQRT_2;
    let s = Complex::new(0.0, prewarped_ratio(frequency_hz, shelf_hz, sample_rate));
    let numerator = (s * s * a + s * sqrt_a_over_q + 1.0) * a;
    let denominator = s * s + s * sqrt_a_over_q + a;
    to_db((numerator / denominator).norm())
}

/// RBJ peaking-EQ analog prototype:
/// H(s) = (s² + s·A/Q + 1) / (s² + s/(A·Q) + 1).
fn reference_peak_db(
    frequency_hz: f64,
    center_hz: f64,
    gain_db: f64,
    q: f64,
    sample_rate: f64,
) -> f64 {
    let a = 10f64.powf(gain_db / 40.0);
    let s = Complex::new(0.0, prewarped_ratio(frequency_hz, center_hz, sample_rate));
    let numerator = s * s + s * (a / q) + 1.0;
    let denominator = s * s + s * (1.0 / (a * q)) + 1.0;
    to_db((numerator / denominator).norm())
}

/// |H(e^jω)| in dB evaluated in f64 from the biquad's stored (f32) coefficients.
fn coefficient_magnitude_db(biquad: &Biquad, frequency_hz: f64, sample_rate: f64) -> f64 {
    let omega = 2.0 * std::f64::consts::PI * frequency_hz / sample_rate;
    let z_inverse = Complex::from_polar(1.0, -omega);
    let z_inverse_squared = z_inverse * z_inverse;
    let numerator = Complex::new(biquad.b0 as f64, 0.0)
        + z_inverse * biquad.b1 as f64
        + z_inverse_squared * biquad.b2 as f64;
    let denominator = Complex::new(1.0, 0.0)
        + z_inverse * biquad.a1 as f64
        + z_inverse_squared * biquad.a2 as f64;
    to_db((numerator / denominator).norm())
}

/// Measure the steady-state gain in dB by running a sine through `process`
/// and demodulating the last second with a Hann window (negligible leakage).
fn measured_gain_db(biquad: &Biquad, frequency_hz: f64, sample_rate: f64) -> f64 {
    let mut filter = biquad.clone();
    filter.reset();
    let warmup_samples = (sample_rate * 1.0) as usize;
    let window_samples = (sample_rate * 1.0) as usize;
    let mut in_phase = 0.0f64;
    let mut quadrature = 0.0f64;
    let mut window_sum = 0.0f64;
    for index in 0..warmup_samples + window_samples {
        let phase = 2.0 * std::f64::consts::PI * frequency_hz * index as f64 / sample_rate;
        let output = filter.process(phase.sin() as f32) as f64;
        if index >= warmup_samples {
            let position = (index - warmup_samples) as f64 / window_samples as f64;
            let weight = 0.5 * (1.0 - (2.0 * std::f64::consts::PI * position).cos());
            in_phase += weight * output * phase.sin();
            quadrature += weight * output * phase.cos();
            window_sum += weight;
        }
    }
    let amplitude = 2.0 * (in_phase * in_phase + quadrature * quadrature).sqrt() / window_sum;
    to_db(amplitude)
}

/// Pole-radius check on the stored coefficients: both poles strictly inside
/// the unit circle iff |a2| < 1 and |a1| < 1 + a2 (Jury criterion).
fn assert_stable(biquad: &Biquad, label: &str) {
    let a1 = biquad.a1 as f64;
    let a2 = biquad.a2 as f64;
    assert!(
        a2.abs() < 1.0 && a1.abs() < 1.0 + a2,
        "{label}: poles outside the unit circle (a1={a1}, a2={a2})"
    );
}

// ── Butterworth lowpass / highpass ──────────────────────────────────────

/// Butterworth Q = 1/√2: exactly -3.0103 dB at the cutoff, 0 dB at DC and
/// -∞ dB (here: ≥ 60 dB down) at Nyquist, at every supported sample rate.
#[test]
fn butterworth_is_minus_3_db_at_cutoff() {
    let minus_3_db = -10.0 * 2.0f64.log10();
    for sample_rate in SAMPLE_RATES {
        for cutoff_hz in [60.0f32, 120.0, 1000.0, 8000.0, 16000.0] {
            let sample_rate = sample_rate as f64;
            let lowpass = Biquad::lowpass(cutoff_hz, sample_rate as f32);
            let highpass = Biquad::highpass(cutoff_hz, sample_rate as f32);
            // f32 coefficients at a very low cutoff/rate ratio (60 Hz @ 96 kHz)
            // quantize to ~0.025 dB; everywhere else the error is < 0.01 dB.
            let lowpass_db = coefficient_magnitude_db(&lowpass, cutoff_hz as f64, sample_rate);
            let highpass_db = coefficient_magnitude_db(&highpass, cutoff_hz as f64, sample_rate);
            assert!(
                (lowpass_db - minus_3_db).abs() < 0.04,
                "LP {cutoff_hz} Hz @ {sample_rate}: {lowpass_db:.4} dB"
            );
            assert!(
                (highpass_db - minus_3_db).abs() < 0.04,
                "HP {cutoff_hz} Hz @ {sample_rate}: {highpass_db:.4} dB"
            );
        }
    }
}

/// The full magnitude curve matches the prewarped Butterworth reference
/// (flat passband, -12 dB/octave stopband, mirrored for the highpass).
#[test]
fn butterworth_curve_matches_analytic_reference() {
    for sample_rate in SAMPLE_RATES {
        let sample_rate_f64 = sample_rate as f64;
        for cutoff_hz in [120.0f32, 1000.0, 5000.0] {
            let lowpass = Biquad::lowpass(cutoff_hz, sample_rate);
            let highpass = Biquad::highpass(cutoff_hz, sample_rate);
            for frequency_hz in [30.0f64, 60.0, 120.0, 500.0, 1000.0, 3000.0, 9000.0, 18000.0] {
                if frequency_hz > 0.45 * sample_rate_f64 {
                    continue;
                }
                let expected_lowpass =
                    reference_lowpass_db(frequency_hz, cutoff_hz as f64, sample_rate_f64);
                let expected_highpass =
                    reference_highpass_db(frequency_hz, cutoff_hz as f64, sample_rate_f64);
                let actual_lowpass =
                    coefficient_magnitude_db(&lowpass, frequency_hz, sample_rate_f64);
                let actual_highpass =
                    coefficient_magnitude_db(&highpass, frequency_hz, sample_rate_f64);
                // Compare only above -80 dB: f32 coefficient rounding limits
                // how deep a stopband the stored coefficients can resolve.
                if expected_lowpass > -80.0 {
                    assert!(
                        (actual_lowpass - expected_lowpass).abs() < 0.02,
                        "LP fc={cutoff_hz} f={frequency_hz} @ {sample_rate}: \
                         {actual_lowpass:.3} vs {expected_lowpass:.3} dB"
                    );
                }
                if expected_highpass > -80.0 {
                    assert!(
                        (actual_highpass - expected_highpass).abs() < 0.02,
                        "HP fc={cutoff_hz} f={frequency_hz} @ {sample_rate}: \
                         {actual_highpass:.3} vs {expected_highpass:.3} dB"
                    );
                }
            }
        }
    }
}

/// Stopband slope of a 2nd-order filter: -12 dB per octave far from cutoff.
#[test]
fn butterworth_rolloff_is_12_db_per_octave() {
    let sample_rate = 48_000.0f64;
    let lowpass = Biquad::lowpass(1000.0, sample_rate as f32);
    let highpass = Biquad::highpass(1000.0, sample_rate as f32);
    // Highpass slope below cutoff (analytic regime, low frequencies).
    let highpass_octave = coefficient_magnitude_db(&highpass, 50.0, sample_rate)
        - coefficient_magnitude_db(&highpass, 25.0, sample_rate);
    assert!(
        (highpass_octave - 12.04).abs() < 0.1,
        "HP slope {highpass_octave:.2} dB/oct"
    );
    // Lowpass slope above cutoff, low enough that bilinear warping is small.
    let lowpass_octave = coefficient_magnitude_db(&lowpass, 4000.0, sample_rate)
        - coefficient_magnitude_db(&lowpass, 8000.0, sample_rate);
    assert!(
        lowpass_octave > 11.0 && lowpass_octave < 14.0,
        "LP slope {lowpass_octave:.2} dB/oct"
    );
}

/// `process` (the real f32 recursion) agrees with the analytic magnitude,
/// so coefficient storage order and the DF1 recursion signs are right.
#[test]
fn butterworth_process_matches_analytic_magnitude() {
    for sample_rate in SAMPLE_RATES {
        let sample_rate_f64 = sample_rate as f64;
        let lowpass = Biquad::lowpass(1000.0, sample_rate);
        let highpass = Biquad::highpass(1000.0, sample_rate);
        for frequency_hz in [100.0f64, 500.0, 1000.0, 2000.0, 6000.0] {
            let measured_lowpass = measured_gain_db(&lowpass, frequency_hz, sample_rate_f64);
            let expected_lowpass = reference_lowpass_db(frequency_hz, 1000.0, sample_rate_f64);
            assert!(
                (measured_lowpass - expected_lowpass).abs() < 0.05,
                "LP {frequency_hz} Hz @ {sample_rate}: {measured_lowpass:.3} vs {expected_lowpass:.3}"
            );
            let measured_highpass = measured_gain_db(&highpass, frequency_hz, sample_rate_f64);
            let expected_highpass = reference_highpass_db(frequency_hz, 1000.0, sample_rate_f64);
            assert!(
                (measured_highpass - expected_highpass).abs() < 0.05,
                "HP {frequency_hz} Hz @ {sample_rate}: {measured_highpass:.3} vs {expected_highpass:.3}"
            );
        }
    }
}

/// First impulse-response samples of the lowpass: y[0] = b0, and unity DC
/// gain means the impulse response sums to 1.
#[test]
fn lowpass_impulse_response_sums_to_dc_gain_of_one() {
    let mut lowpass = Biquad::lowpass(1000.0, 48_000.0);
    let first = lowpass.process(1.0);
    assert!((first - lowpass.b0).abs() < 1e-7, "y[0] must equal b0");
    let mut sum = first as f64;
    for _ in 0..20_000 {
        sum += lowpass.process(0.0) as f64;
    }
    assert!((sum - 1.0).abs() < 1e-4, "IR sum {sum}");

    // The highpass blocks DC: its impulse response sums to 0.
    let mut highpass = Biquad::highpass(1000.0, 48_000.0);
    let mut sum = highpass.process(1.0) as f64;
    for _ in 0..20_000 {
        sum += highpass.process(0.0) as f64;
    }
    assert!(sum.abs() < 1e-4, "HP IR sum {sum}");
}

/// Poles stay strictly inside the unit circle from 20 Hz up to just below
/// Nyquist, for every filter type and sample rate. Also verifies the
/// recursion actually decays (no f32 blow-up) at the extreme corners.
#[test]
fn all_filter_types_are_stable_across_rates_and_cutoffs() {
    for sample_rate in SAMPLE_RATES {
        let mut cutoff_hz = 20.0f32;
        while cutoff_hz < 0.49 * sample_rate {
            assert_stable(
                &Biquad::lowpass(cutoff_hz, sample_rate),
                &format!("LP {cutoff_hz} @ {sample_rate}"),
            );
            assert_stable(
                &Biquad::highpass(cutoff_hz, sample_rate),
                &format!("HP {cutoff_hz} @ {sample_rate}"),
            );
            for gain_db in [-24.0f32, -6.0, 6.0, 24.0] {
                let mut shelf = Biquad::unity();
                shelf.set_low_shelf(cutoff_hz, gain_db, sample_rate);
                assert_stable(&shelf, &format!("LS {cutoff_hz} {gain_db} @ {sample_rate}"));
                shelf.set_high_shelf(cutoff_hz, gain_db, sample_rate);
                assert_stable(&shelf, &format!("HS {cutoff_hz} {gain_db} @ {sample_rate}"));
                for q in [0.3f32, std::f32::consts::FRAC_1_SQRT_2, 4.0, 10.0] {
                    shelf.set_peak(cutoff_hz, gain_db, q, sample_rate);
                    assert_stable(
                        &shelf,
                        &format!("PK {cutoff_hz} {gain_db} q={q} @ {sample_rate}"),
                    );
                }
            }
            cutoff_hz *= 1.15;
        }
    }

    // Corner cases in the actual f32 recursion: lowest cutoff at the highest
    // rate, and a cutoff within 2% of Nyquist. Both must decay to zero.
    for (cutoff_hz, sample_rate) in [(20.0f32, 96_000.0f32), (23_500.0, 48_000.0)] {
        for mut filter in [
            Biquad::lowpass(cutoff_hz, sample_rate),
            Biquad::highpass(cutoff_hz, sample_rate),
        ] {
            let mut last = filter.process(1.0);
            for _ in 0..600_000 {
                last = filter.process(0.0);
                assert!(last.is_finite());
            }
            assert!(
                last.abs() < 1e-3,
                "impulse response did not decay at {cutoff_hz} Hz @ {sample_rate}: {last}"
            );
        }
    }
}

/// The f32 recursion keeps unity DC gain for a low-ratio cutoff
/// (120 Hz @ 96 kHz is the worst case used by the LFE crossover).
#[test]
fn lowpass_dc_gain_is_accurate_at_low_cutoff_ratio() {
    for sample_rate in SAMPLE_RATES {
        let lowpass = Biquad::lowpass(120.0, sample_rate);
        let denominator = 1.0 + lowpass.a1 as f64 + lowpass.a2 as f64;
        let numerator = lowpass.b0 as f64 + lowpass.b1 as f64 + lowpass.b2 as f64;
        let dc_gain_db = to_db(numerator / denominator);
        assert!(
            dc_gain_db.abs() < 0.05,
            "DC gain at 120 Hz @ {sample_rate}: {dc_gain_db:.4} dB"
        );
    }
}

// ── Shelves ─────────────────────────────────────────────────────────────

/// Low shelf: full gain at DC, 0 dB at Nyquist, exactly half the gain (in
/// dB) at the shelf frequency, and the whole curve matches the RBJ analog
/// prototype.
#[test]
fn low_shelf_gain_at_dc_nyquist_and_corner() {
    for sample_rate in SAMPLE_RATES {
        let sample_rate_f64 = sample_rate as f64;
        for gain_db in [-18.0f32, -6.0, 3.0, 12.0] {
            let mut shelf = Biquad::unity();
            shelf.set_low_shelf(500.0, gain_db, sample_rate);
            let dc = coefficient_magnitude_db(&shelf, 1.0, sample_rate_f64);
            let nyquist = coefficient_magnitude_db(&shelf, 0.5 * sample_rate_f64, sample_rate_f64);
            let corner = coefficient_magnitude_db(&shelf, 500.0, sample_rate_f64);
            assert!((dc - gain_db as f64).abs() < 0.02, "DC {dc} vs {gain_db}");
            assert!(nyquist.abs() < 0.02, "Nyquist {nyquist}");
            assert!(
                (corner - gain_db as f64 / 2.0).abs() < 0.02,
                "corner {corner} vs {}",
                gain_db / 2.0
            );
            for frequency_hz in [50.0f64, 250.0, 500.0, 1000.0, 4000.0] {
                let expected =
                    reference_low_shelf_db(frequency_hz, 500.0, gain_db as f64, sample_rate_f64);
                let actual = coefficient_magnitude_db(&shelf, frequency_hz, sample_rate_f64);
                assert!(
                    (actual - expected).abs() < 0.02,
                    "LS {gain_db} dB f={frequency_hz} @ {sample_rate}: {actual:.3} vs {expected:.3}"
                );
            }
        }
    }
}

/// High shelf: 0 dB at DC, full gain at Nyquist, half the gain at the shelf
/// frequency, curve matches the RBJ analog prototype.
#[test]
fn high_shelf_gain_at_dc_nyquist_and_corner() {
    for sample_rate in SAMPLE_RATES {
        let sample_rate_f64 = sample_rate as f64;
        for gain_db in [-18.0f32, -6.0, 3.0, 12.0] {
            let mut shelf = Biquad::unity();
            shelf.set_high_shelf(4000.0, gain_db, sample_rate);
            let dc = coefficient_magnitude_db(&shelf, 1.0, sample_rate_f64);
            let nyquist = coefficient_magnitude_db(&shelf, 0.5 * sample_rate_f64, sample_rate_f64);
            let corner = coefficient_magnitude_db(&shelf, 4000.0, sample_rate_f64);
            assert!(dc.abs() < 0.02, "DC {dc}");
            assert!(
                (nyquist - gain_db as f64).abs() < 0.05,
                "Nyquist {nyquist} vs {gain_db}"
            );
            assert!(
                (corner - gain_db as f64 / 2.0).abs() < 0.02,
                "corner {corner} vs {}",
                gain_db / 2.0
            );
            for frequency_hz in [200.0f64, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0] {
                if frequency_hz > 0.45 * sample_rate_f64 {
                    continue;
                }
                let expected =
                    reference_high_shelf_db(frequency_hz, 4000.0, gain_db as f64, sample_rate_f64);
                let actual = coefficient_magnitude_db(&shelf, frequency_hz, sample_rate_f64);
                assert!(
                    (actual - expected).abs() < 0.02,
                    "HS {gain_db} dB f={frequency_hz} @ {sample_rate}: {actual:.3} vs {expected:.3}"
                );
            }
        }
    }
}

/// Peaking EQ: full gain at the center, 0 dB at DC and Nyquist, and the
/// curve matches the RBJ analog prototype for several Q values.
#[test]
fn peak_gain_at_center_dc_and_nyquist() {
    for sample_rate in SAMPLE_RATES {
        let sample_rate_f64 = sample_rate as f64;
        for (gain_db, q) in [(-12.0f32, 0.75f32), (6.0, 1.0), (12.0, 4.0)] {
            let mut peak = Biquad::unity();
            peak.set_peak(1000.0, gain_db, q, sample_rate);
            let center = coefficient_magnitude_db(&peak, 1000.0, sample_rate_f64);
            let dc = coefficient_magnitude_db(&peak, 1.0, sample_rate_f64);
            let nyquist = coefficient_magnitude_db(&peak, 0.5 * sample_rate_f64, sample_rate_f64);
            assert!((center - gain_db as f64).abs() < 0.02, "center {center}");
            assert!(dc.abs() < 0.02, "DC {dc}");
            assert!(nyquist.abs() < 0.1, "Nyquist {nyquist}");
            for frequency_hz in [100.0f64, 500.0, 800.0, 1000.0, 1250.0, 2000.0, 10000.0] {
                let expected = reference_peak_db(
                    frequency_hz,
                    1000.0,
                    gain_db as f64,
                    q as f64,
                    sample_rate_f64,
                );
                let actual = coefficient_magnitude_db(&peak, frequency_hz, sample_rate_f64);
                assert!(
                    (actual - expected).abs() < 0.02,
                    "PK {gain_db} dB q={q} f={frequency_hz} @ {sample_rate}: {actual:.3} vs {expected:.3}"
                );
            }
        }
    }
}

/// Measured (f32 `process`) steady-state response of every `set_*` type
/// agrees with the analytic reference.
#[test]
fn shelf_and_peak_process_match_analytic_magnitude() {
    let sample_rate = 48_000.0f32;
    let sample_rate_f64 = sample_rate as f64;
    let mut low_shelf = Biquad::unity();
    low_shelf.set_low_shelf(500.0, -9.0, sample_rate);
    let mut high_shelf = Biquad::unity();
    high_shelf.set_high_shelf(4000.0, -9.0, sample_rate);
    let mut peak = Biquad::unity();
    peak.set_peak(1000.0, 9.0, 2.0, sample_rate);
    for frequency_hz in [100.0f64, 500.0, 1000.0, 4000.0, 10000.0] {
        let cases = [
            (
                "low shelf",
                measured_gain_db(&low_shelf, frequency_hz, sample_rate_f64),
                reference_low_shelf_db(frequency_hz, 500.0, -9.0, sample_rate_f64),
            ),
            (
                "high shelf",
                measured_gain_db(&high_shelf, frequency_hz, sample_rate_f64),
                reference_high_shelf_db(frequency_hz, 4000.0, -9.0, sample_rate_f64),
            ),
            (
                "peak",
                measured_gain_db(&peak, frequency_hz, sample_rate_f64),
                reference_peak_db(frequency_hz, 1000.0, 9.0, 2.0, sample_rate_f64),
            ),
        ];
        for (name, measured, expected) in cases {
            assert!(
                (measured - expected).abs() < 0.05,
                "{name} @ {frequency_hz} Hz: measured {measured:.3} vs {expected:.3} dB"
            );
        }
    }
}

/// Gains below the 0.01 dB dead-band collapse to an exact passthrough.
#[test]
fn near_zero_gain_is_exact_unity() {
    let mut biquad = Biquad::lowpass(1000.0, 48_000.0);
    biquad.set_low_shelf(500.0, 0.005, 48_000.0);
    assert_eq!((biquad.b0, biquad.b1, biquad.b2), (1.0, 0.0, 0.0));
    assert_eq!((biquad.a1, biquad.a2), (0.0, 0.0));
    biquad.set_high_shelf(4000.0, -0.005, 48_000.0);
    assert_eq!((biquad.b0, biquad.a1), (1.0, 0.0));
    biquad.set_peak(1000.0, 0.0, 1.0, 48_000.0);
    assert_eq!((biquad.b0, biquad.a1), (1.0, 0.0));
    assert_eq!(biquad.process(0.25), 0.25);
}

/// Peak Q below 0.1 is clamped, not used raw (guards against alpha blow-up).
#[test]
fn peak_q_is_floored_at_point_one() {
    let mut floored = Biquad::unity();
    floored.set_peak(1000.0, 6.0, 0.0, 48_000.0);
    let mut explicit = Biquad::unity();
    explicit.set_peak(1000.0, 6.0, 0.1, 48_000.0);
    assert_eq!(floored.b0, explicit.b0);
    assert_eq!(floored.a2, explicit.a2);
    assert_stable(&floored, "q floor");
}

/// `set_*` keeps the delay-line state (click-free updates); `reset` clears it.
#[test]
fn coefficient_updates_preserve_state_and_reset_clears_it() {
    let mut biquad = Biquad::lowpass(1000.0, 48_000.0);
    for _ in 0..100 {
        biquad.process(1.0);
    }
    let (x1, y1) = (biquad.x1, biquad.y1);
    assert!(y1 != 0.0);
    biquad.set_low_shelf(500.0, 6.0, 48_000.0);
    assert_eq!((biquad.x1, biquad.y1), (x1, y1));
    biquad.set_unity();
    assert_eq!((biquad.x1, biquad.y1), (x1, y1));
    biquad.reset();
    assert_eq!(
        (biquad.x1, biquad.x2, biquad.y1, biquad.y2),
        (0.0, 0.0, 0.0, 0.0)
    );
}

/// The constructors keep the cutoff below Nyquist. Unclamped, a cutoff at
/// or above Nyquist yields an unstable filter (a2 >= 1).
#[test]
fn cutoff_at_or_above_nyquist_must_not_produce_unstable_filter() {
    for cutoff_hz in [24_000.0f32, 25_000.0, 30_000.0] {
        assert_stable(&Biquad::lowpass(cutoff_hz, 48_000.0), "LP above Nyquist");
        assert_stable(&Biquad::highpass(cutoff_hz, 48_000.0), "HP above Nyquist");
    }
}

// ── AirAbsorptionFilter ─────────────────────────────────────────────────

/// Combined two-shelf response of an `AirAbsorptionFilter` driven to
/// `distance`, in dB at `frequency_hz` (read from the real filter state).
fn air_filter_response_db(distance: f32, frequency_hz: f64) -> f64 {
    let atmosphere = AtmosphericParams::default();
    let mut filter = AirAbsorptionFilter::new(48_000.0);
    filter.update(distance, &atmosphere);
    20.0 * (filter.broadband_gain as f64).log10()
        + coefficient_magnitude_db(&filter.low_shelf, frequency_hz, 48_000.0)
        + coefficient_magnitude_db(&filter.high_shelf, frequency_hz, 48_000.0)
}

/// The two-shelf fit reproduces the ISO 9613-1 curve where the doc comment
/// on `air_absorption_gains` says it does: within 1.5 dB at 4 kHz and
/// 0.5 dB at 8 kHz for indoor distances (hysteresis adds up to 0.5 dB).
#[test]
fn air_absorption_filter_tracks_iso9613_at_indoor_distances() {
    let atmosphere = AtmosphericParams::default();
    for distance in [10.0f32, 25.0, 50.0] {
        for (frequency_hz, tolerance_db) in [(4000.0f64, 1.5f64), (8000.0, 0.5)] {
            let expected = -(iso9613_alpha(frequency_hz as f32, &atmosphere) * distance) as f64;
            let actual = air_filter_response_db(distance, frequency_hz);
            assert!(
                (actual - expected).abs() < tolerance_db + 0.5,
                "{distance} m @ {frequency_hz} Hz: filter {actual:.2} dB vs ISO {expected:.2} dB"
            );
        }
    }
}

/// Zero distance leaves the filter at exact unity and `reset` restores it.
#[test]
fn air_absorption_filter_is_transparent_at_zero_distance_and_after_reset() {
    let atmosphere = AtmosphericParams::default();
    let mut filter = AirAbsorptionFilter::new(48_000.0);
    filter.update(0.0, &atmosphere);
    assert_eq!(filter.process(0.7), 0.7);
    filter.update(100.0, &atmosphere);
    assert!(filter.current_high_db < -1.0);
    filter.reset();
    assert_eq!(filter.process(0.7), 0.7);
    assert_eq!(filter.current_low_db, 0.0);
    assert_eq!(filter.current_high_db, 0.0);
}

/// Hysteresis: gain changes below 0.5 dB must not recompute coefficients.
#[test]
fn air_absorption_filter_hysteresis_holds_coefficients() {
    let atmosphere = AtmosphericParams::default();
    let mut filter = AirAbsorptionFilter::new(48_000.0);
    filter.update(200.0, &atmosphere);
    let high_before = filter.current_high_db;
    let b0_before = filter.high_shelf.b0;
    // ~0.05 dB of extra high-shelf gain change.
    filter.update(200.2, &atmosphere);
    assert_eq!(filter.current_high_db, high_before);
    assert_eq!(filter.high_shelf.b0, b0_before);
}
