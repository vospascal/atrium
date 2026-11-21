use super::*;
use crate::pipeline::stages::fdn_reverb::damping::MAXIMUM_LOOP_GAIN;

// ── End-to-end physics: mixing matrix, damping, measured decay ──────────

const SAMPLE_RATE: f32 = 48_000.0;

/// Deterministic white noise in [-1, 1) (64-bit LCG, top 24 bits).
fn white_noise(length: usize, seed: u64) -> Vec<f32> {
    let mut state = seed;
    (0..length)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
        })
        .collect()
}

/// A stereo stage initialised in the 10 m hard-walled test cube, then
/// optionally given an explicit (low, high) RT60 pair directly.
fn stereo_stage(config: FdnConfig, rt60_low_high: Option<(f32, f32)>) -> FdnReverbStage {
    let (layout, listener) = make_ctx(2, 2);
    let ctx = mix_ctx(&layout, &listener, 2, 2);
    let mut fdn = FdnReverbStage::with_config(config);
    fdn.init(&ctx);
    if let Some((rt60_low, rt60_high)) = rt60_low_high {
        fdn.compute_damping(SAMPLE_RATE, rt60_low, rt60_high);
    }
    fdn
}

/// Run `send` through the stage as its mono reverb bus and return the
/// per-frame wet output of both stereo channels.
fn stereo_wet_output(fdn: &mut FdnReverbStage, send: &[f32]) -> Vec<[f32; 2]> {
    let channels = 2;
    let (layout, listener) = make_ctx(channels, channels);
    let mut reverb_input = vec![0.0f32; send.len() * channels];
    for (frame, &sample) in send.iter().enumerate() {
        reverb_input[frame * channels] = sample;
    }
    let ctx = MixContext {
        reverb_input: Some(&reverb_input),
        ..mix_ctx(&layout, &listener, channels, channels)
    };
    let mut buffer = vec![0.0f32; send.len() * channels];
    fdn.process(&mut buffer, &ctx);
    buffer
        .chunks_exact(channels)
        .map(|frame| [frame[0], frame[1]])
        .collect()
}

/// Mono (L+R) impulse response of the tail for a unit impulse on the send.
fn impulse_response(fdn: &mut FdnReverbStage, frames: usize) -> Vec<f32> {
    let mut send = vec![0.0f32; frames];
    send[0] = 1.0;
    stereo_wet_output(fdn, &send)
        .iter()
        .map(|frame| frame[0] + frame[1])
        .collect()
}

/// Octave band around `center_hz`: two cascaded Butterworth high-passes at
/// center/√2 and two low-passes at center·√2 (24 dB/oct skirts).
fn octave_band(signal: &[f32], center_hz: f32) -> Vec<f32> {
    let lower_edge = center_hz / std::f32::consts::SQRT_2;
    let upper_edge = center_hz * std::f32::consts::SQRT_2;
    let mut filters = [
        Biquad::highpass(lower_edge, SAMPLE_RATE),
        Biquad::highpass(lower_edge, SAMPLE_RATE),
        Biquad::lowpass(upper_edge, SAMPLE_RATE),
        Biquad::lowpass(upper_edge, SAMPLE_RATE),
    ];
    signal
        .iter()
        .map(|&sample| {
            filters
                .iter_mut()
                .fold(sample, |x, filter| filter.process(x))
        })
        .collect()
}

/// Schroeder backward integration: energy remaining after each sample, in
/// dB relative to the total energy (ISO 3382-1 energy decay curve).
fn schroeder_decay_db(impulse_response: &[f32]) -> Vec<f64> {
    let mut remaining_energy = vec![0.0f64; impulse_response.len()];
    let mut running_sum = 0.0f64;
    for (index, &sample) in impulse_response.iter().enumerate().rev() {
        running_sum += f64::from(sample) * f64::from(sample);
        remaining_energy[index] = running_sum;
    }
    let total_energy = running_sum.max(f64::MIN_POSITIVE);
    remaining_energy
        .iter()
        .map(|&energy| 10.0 * (energy / total_energy).max(1e-30).log10())
        .collect()
}

/// RT60 extrapolated from a least-squares line through the decay curve
/// between -5 dB and -25 dB (the ISO 3382 T20 evaluation range).
fn rt60_from_decay(decay_db: &[f64]) -> f64 {
    let points: Vec<(f64, f64)> = decay_db
        .iter()
        .enumerate()
        .filter(|(_, &level)| (-25.0..=-5.0).contains(&level))
        .map(|(index, &level)| (index as f64 / f64::from(SAMPLE_RATE), level))
        .collect();
    assert!(points.len() > 100, "decay curve never spans -5..-25 dB");
    let count = points.len() as f64;
    let mean_time = points.iter().map(|point| point.0).sum::<f64>() / count;
    let mean_level = points.iter().map(|point| point.1).sum::<f64>() / count;
    let covariance: f64 = points
        .iter()
        .map(|(time, level)| (time - mean_time) * (level - mean_level))
        .sum();
    let variance: f64 = points
        .iter()
        .map(|(time, _)| (time - mean_time).powi(2))
        .sum();
    let slope_db_per_second = covariance / variance;
    -60.0 / slope_db_per_second
}

/// The Walsh–Hadamard mixing matrix must be orthonormal (lossless): every
/// entry ±1/√16 = ±0.25, H·Hᵀ = I, energy preserved for any input, and —
/// since the Sylvester Hadamard matrix is symmetric — its own inverse.
#[test]
fn hadamard_is_orthonormal_energy_preserving_and_self_inverse() {
    let mut matrix = [[0.0f32; NUM_LINES]; NUM_LINES];
    for (column, column_values) in matrix.iter_mut().enumerate() {
        let mut basis = [0.0f32; NUM_LINES];
        basis[column] = 1.0;
        hadamard(&mut basis);
        *column_values = basis;
    }
    let entry_magnitude = 1.0 / (NUM_LINES as f32).sqrt();
    for row in 0..NUM_LINES {
        for column in 0..NUM_LINES {
            assert!(
                (matrix[column][row].abs() - entry_magnitude).abs() < 1e-6,
                "entry ({row},{column}) = {} should be ±{entry_magnitude}",
                matrix[column][row]
            );
            let dot: f32 = (0..NUM_LINES)
                .map(|k| matrix[row][k] * matrix[column][k])
                .sum();
            let identity = if row == column { 1.0 } else { 0.0 };
            assert!(
                (dot - identity).abs() < 1e-6,
                "H·Hᵀ[{row}][{column}] = {dot}, expected {identity}"
            );
        }
    }

    let noise = white_noise(NUM_LINES * 64, 7);
    for chunk in noise.chunks_exact(NUM_LINES) {
        let original: [f32; NUM_LINES] = chunk.try_into().unwrap();
        let mut transformed = original;
        hadamard(&mut transformed);
        let energy_in: f32 = original.iter().map(|x| x * x).sum();
        let energy_out: f32 = transformed.iter().map(|x| x * x).sum();
        assert!(
            (energy_out / energy_in - 1.0).abs() < 1e-5,
            "Hadamard must preserve energy: in={energy_in}, out={energy_out}"
        );
        hadamard(&mut transformed);
        for (back, forward) in transformed.iter().zip(&original) {
            assert!(
                (back - forward).abs() < 1e-5,
                "H(H(v)) must equal v: {back} vs {forward}"
            );
        }
    }
}

/// Analytic one-pole gain |H(e^{jω})| = k / √(1 + p² − 2p·cos ω).
fn damping_gain(filter: &DampingFilter, frequency_hz: f64, sample_rate: f32) -> f64 {
    let angle = std::f64::consts::TAU * frequency_hz / f64::from(sample_rate);
    let pole = f64::from(filter.p);
    f64::from(filter.k) / (1.0 + pole * pole - 2.0 * pole * angle.cos()).sqrt()
}

/// Steady-state amplitude of the filter's response to a unit sine at
/// `frequency_hz` (48 kHz), measured by quadrature correlation over the
/// last 960 samples — a whole number of periods at both 500 Hz and 4 kHz.
fn measured_sine_gain(filter: DampingFilter, frequency_hz: f64) -> f64 {
    let mut filter = filter;
    let angle_per_sample = std::f64::consts::TAU * frequency_hz / f64::from(SAMPLE_RATE);
    let (settle, window) = (4800, 960);
    let (mut in_phase, mut quadrature) = (0.0f64, 0.0f64);
    for sample in 0..settle + window {
        let angle = angle_per_sample * sample as f64;
        let output = f64::from(filter.process(angle.sin() as f32));
        if sample >= settle {
            in_phase += output * angle.sin();
            quadrature += output * angle.cos();
        }
    }
    2.0 * in_phase.hypot(quadrature) / window as f64
}

/// The damping filter must hit the Jot per-pass gain at the frequencies the
/// two RT60s are defined at: 10^(−3m/(RT60_low·fs)) at 500 Hz and
/// 10^(−3m/(RT60_high·fs)) at 4 kHz.
///
/// Hand derivation, 2237-sample line at 48 kHz, RT60 2.0 s / 1.2 s:
///   g_low  = 10^(−3·2237/96000) = 10^(−0.069906) = 0.85132
///   g_high = 10^(−3·2237/57600) = 10^(−0.116510) = 0.76470
///   v_low  = 1 − cos(2π·500/48000) = 0.0021411, v_high = 1 − cos(π/6) = 0.13397
///   r = (g_low/g_high)² = 1.23939
///   s = (v_high − r·v_low)/(r − 1) = (0.13397 − 0.00265)/0.23939 = 0.54856
///   p = 1 + s − √(s·(2 + s)) = 1.54856 − 1.18240 = 0.36617
///   k = g_low·√((1 − p)² + 2p·v_low) = 0.85132·√(0.40174 + 0.00157) = 0.54064
///   DC gain k/(1 − p) = 0.85298 (the peak, < 1); Nyquist k/(1 + p) = 0.39574.
/// The gains are measured by driving the filter with 500 Hz and 4 kHz sines.
/// Every line at 44.1, 48 and 96 kHz must meet both targets analytically,
/// with its peak (DC) gain below 1.
#[test]
fn damping_filter_hits_jot_gain_at_500_hz_and_4_khz() {
    let mut fdn = FdnReverbStage::new();
    fdn.compute_delays(SAMPLE_RATE);
    fdn.compute_damping(SAMPLE_RATE, 2.0, 1.2);

    let line = BASE_DELAYS.iter().position(|&delay| delay == 2237).unwrap();
    let filter = fdn.damping[line];
    assert!((filter.p - 0.36617).abs() < 1e-4, "pole {}", filter.p);
    assert!((filter.k - 0.54064).abs() < 1e-4, "gain {}", filter.k);
    let dc_gain = filter.k / (1.0 - filter.p);
    let nyquist_gain = filter.k / (1.0 + filter.p);
    assert!((dc_gain - 0.85298).abs() < 1e-4, "DC gain {dc_gain}");
    assert!(
        (nyquist_gain - 0.39574).abs() < 1e-4,
        "Nyquist gain {nyquist_gain}"
    );
    let low_output = measured_sine_gain(filter, 500.0);
    let high_output = measured_sine_gain(filter, 4000.0);
    assert!(
        (low_output - 0.85132).abs() < 2e-4,
        "500 Hz loop gain should be 0.85132, got {low_output}"
    );
    assert!(
        (high_output - 0.76470).abs() < 2e-4,
        "4 kHz loop gain should be 0.76470, got {high_output}"
    );

    // Every line and sample rate: the design points equal
    // room_acoustics::jot_feedback_gain, the documented home of the formula.
    for sample_rate in [44_100.0f32, 48_000.0, 96_000.0] {
        let mut fdn = FdnReverbStage::new();
        fdn.compute_delays(sample_rate);
        fdn.compute_damping(sample_rate, 2.0, 1.2);
        for (index, filter) in fdn.damping.iter().enumerate() {
            let delay_seconds = fdn.delays[index] as f32 / sample_rate;
            let low_gain = damping_gain(filter, 500.0, sample_rate);
            let high_gain = damping_gain(filter, 4000.0, sample_rate);
            let expected_low = f64::from(room_acoustics::jot_feedback_gain(delay_seconds, 2.0));
            let expected_high = f64::from(room_acoustics::jot_feedback_gain(delay_seconds, 1.2));
            assert!(
                (low_gain - expected_low).abs() < 1e-5,
                "{sample_rate} Hz line {index}: 500 Hz gain {low_gain} vs {expected_low}"
            );
            assert!(
                (high_gain - expected_high).abs() < 1e-5,
                "{sample_rate} Hz line {index}: 4 kHz gain {high_gain} vs {expected_high}"
            );
            let peak_gain = filter.k / (1.0 - filter.p);
            assert!(
                (0.0..1.0).contains(&filter.p) && peak_gain < 1.0,
                "{sample_rate} Hz line {index}: pole {} peak gain {peak_gain}",
                filter.p
            );
        }
    }
}

/// Targets a one-pole cannot meet, and the no-damping and lossless limits,
/// must still give a passive loop with the 500 Hz gain exact.
/// - 10 s / 0.1 s asks for up to 48 dB per pass between 500 Hz and 4 kHz;
///   a one-pole spans at most ≈ 18 dB. The pole stops where DC decays at
///   2 × the 500 Hz RT60 (g_dc = √g_low), so 4 kHz is under-damped, never over.
/// - RT60_high ≥ RT60_low: no damping, p = 0 and k = g_low (flat).
/// - Infinite or near-infinite RT60: loop gain capped at 0.9999, not 1.
#[test]
fn damping_filter_stays_passive_when_targets_are_unreachable() {
    for sample_rate in [44_100.0f32, 48_000.0, 96_000.0] {
        let mut fdn = FdnReverbStage::new();
        fdn.compute_delays(sample_rate);

        fdn.compute_damping(sample_rate, 10.0, 0.1);
        for (index, filter) in fdn.damping.iter().enumerate() {
            let delay_seconds = fdn.delays[index] as f32 / sample_rate;
            let expected_low = f64::from(room_acoustics::jot_feedback_gain(delay_seconds, 10.0));
            let target_high = f64::from(room_acoustics::jot_feedback_gain(delay_seconds, 0.1));
            let low_gain = damping_gain(filter, 500.0, sample_rate);
            let high_gain = damping_gain(filter, 4000.0, sample_rate);
            let peak_gain = f64::from(filter.k) / (1.0 - f64::from(filter.p));
            assert!(
                (low_gain - expected_low).abs() < 1e-5,
                "{sample_rate} Hz line {index}: 500 Hz gain {low_gain} vs {expected_low}"
            );
            assert!(
                (peak_gain - expected_low.sqrt()).abs() < 1e-4,
                "{sample_rate} Hz line {index}: DC gain {peak_gain} should sit at the cap {}",
                expected_low.sqrt()
            );
            assert!(
                high_gain >= target_high && high_gain < low_gain,
                "{sample_rate} Hz line {index}: 4 kHz gain {high_gain} (target {target_high})"
            );
            assert!(filter.p < 1.0 && peak_gain < 1.0);
        }

        fdn.compute_damping(sample_rate, 1.0, 2.0);
        for (index, filter) in fdn.damping.iter().enumerate() {
            let delay_seconds = fdn.delays[index] as f32 / sample_rate;
            assert_eq!(filter.p, 0.0, "line {index}: no HF damping requested");
            assert!(
                (filter.k - room_acoustics::jot_feedback_gain(delay_seconds, 1.0)).abs() < 1e-6
            );
        }

        for (rt60_low, rt60_high) in [(f32::INFINITY, f32::INFINITY), (1.0e6, 0.6e6)] {
            fdn.compute_damping(sample_rate, rt60_low, rt60_high);
            for filter in &fdn.damping {
                let peak_gain = filter.k / (1.0 - filter.p);
                assert!(
                    peak_gain < 1.0 && peak_gain > 0.9998,
                    "RT60 {rt60_low}: peak gain {peak_gain}"
                );
            }
            assert!(fdn.output_normalization.is_finite() && fdn.output_normalization > 0.0);
        }
    }
}

/// Freeze the delay-modulation LFOs so every read is an integer delay.
fn without_modulation(mut fdn: FdnReverbStage) -> FdnReverbStage {
    for modulator in &mut fdn.modulators {
        modulator.depth = 0.0;
    }
    fdn
}

/// Measured RT60 of the octave band at `center_hz` (or broadband if `None`).
fn measured_rt60(response: &[f32], center_hz: Option<f32>) -> f64 {
    match center_hz {
        Some(center) => rt60_from_decay(&schroeder_decay_db(&octave_band(response, center))),
        None => rt60_from_decay(&schroeder_decay_db(response)),
    }
}

/// The Jot + Hadamard core is exact: with a frequency-independent RT60 of
/// 1.5 s and the LFOs frozen, the Schroeder decay of the real impulse
/// response (16 lines, Hadamard feedback, damping filters, output combiner)
/// extrapolates to 1.5 s ± 10 % broadband and in every octave 500 Hz–8 kHz.
/// (Measured: 1.54 broadband; 1.52 / 1.57 / 1.51 / 1.57 / 1.55 s per band.)
#[test]
fn measured_rt60_matches_configured_decay_in_every_band() {
    let mut fdn = without_modulation(stereo_stage(FdnConfig::default(), Some((1.5, 1.5))));
    let response = impulse_response(&mut fdn, (1.6 * SAMPLE_RATE) as usize);
    for band in [
        None,
        Some(500.0),
        Some(1000.0),
        Some(2000.0),
        Some(4000.0),
        Some(8000.0),
    ] {
        let measured = measured_rt60(&response, band);
        assert!(
            (measured - 1.5).abs() < 0.15,
            "flat 1.5 s FDN, band {band:?}: measured RT60 {measured:.3} s"
        );
    }
}

/// End-to-end through `init` with the production (modulated) network: an
/// RT60 override of 2.0 s sets the 500 Hz decay. The measured 500 Hz
/// octave-band RT60 must be 2.0 s ± 10 % (measured 1.97 s).
#[test]
fn measured_mid_band_rt60_matches_override() {
    let mut fdn = stereo_stage(
        FdnConfig {
            rt60_seconds: Some(2.0),
            ..FdnConfig::default()
        },
        None,
    );
    let response = impulse_response(&mut fdn, (1.8 * SAMPLE_RATE) as usize);
    let measured = measured_rt60(&response, Some(500.0));
    assert!(
        (measured - 2.0).abs() < 0.2,
        "500 Hz band should decay with the 2.0 s override, got {measured:.3} s"
    );
}

/// `FdnConfig::rt60_seconds` documents that "the 4 kHz decay is derived via
/// HF_DECAY_RATIO" (0.6 → 1.2 s for a 2.0 s override). The damping filters
/// meet exactly that per-pass gain at 4 kHz, so the 4 kHz octave band must
/// measure 1.2 s ± 15 %. LFOs are frozen to isolate the filter mapping.
///
/// Measured 1.37 s, close to the +15 % edge, and not a filter error (the
/// gain at 4 kHz is pinned by `damping_filter_hits_jot_gain_at_500_hz_and_4_khz`):
/// a one-pole's RT60 falls across the octave, 1.48 s at 2.83 kHz to 0.91 s
/// at 5.66 kHz, and a T20 fit follows the slow edge (1.25 s for an ideal
/// band); the 24 dB/oct band filter's skirts admit slower 2 kHz energy, and
/// the method itself reads a flat 1.5 s as 1.57 s here. Before the fix,
/// with the 1.2 s gain at Nyquist, this measured 1.97 s. The production
/// modulated network measures 1.31 s (1.25 s with the old linear
/// interpolation; see `modulated_interpolation_preserves_high_band_decay`).
#[test]
fn measured_high_band_rt60_matches_hf_decay_ratio() {
    let mut fdn = without_modulation(stereo_stage(
        FdnConfig {
            rt60_seconds: Some(2.0),
            ..FdnConfig::default()
        },
        None,
    ));
    let response = impulse_response(&mut fdn, (1.8 * SAMPLE_RATE) as usize);
    let measured = measured_rt60(&response, Some(4000.0));
    let expected = f64::from(2.0 * HF_DECAY_RATIO);
    assert!(
        (measured - expected).abs() < 0.15 * expected,
        "4 kHz band should decay in {expected:.2} s (HF_DECAY_RATIO), got {measured:.3} s"
    );
}

/// The modulated read's interpolation loss is not in the Jot gains, so it
/// shortens the decay wherever it bites. Linear interpolation averaged
/// (over the swept fraction f) |H|² = 2/3 + cos(ω)/3 per pass, −0.79 dB at
/// 8 kHz on every ~48 ms trip round the loop: a flat 1.5 s configuration
/// measured 1.14 s at 8 kHz and 0.92 s at 12 kHz (frozen LFOs: 1.55 s).
/// Catmull-Rom loses −0.12 dB at 8 kHz (see `hermite_coefficients`), and
/// the 8 kHz octave must decay in 1.5 s ± 10 %. Measured 1.41 s (12 kHz
/// 1.25 s, broadband 1.40 s; white-noise wet/send −15.4 dB, was −16.7 dB,
/// −12.7 dB frozen).
#[test]
fn modulated_interpolation_preserves_high_band_decay() {
    let mut fdn = stereo_stage(FdnConfig::default(), Some((1.5, 1.5)));
    let response = impulse_response(&mut fdn, (1.6 * SAMPLE_RATE) as usize);
    let measured = measured_rt60(&response, Some(8000.0));
    assert!(
        (measured - 1.5).abs() < 0.15,
        "flat 1.5 s FDN should decay in 1.5 s at 8 kHz, got {measured:.3} s"
    );
}

/// The modulated read must never add gain, or a lossless-limit loop
/// (damping capped at `MAXIMUM_LOOP_GAIN` = 0.9999) could reach 1.
/// |H(e^{jω})| = |Σ c_k e^{−jωk}| over 1025 fractions × 4097 frequencies
/// in [0, π] must stay ≤ 1 up to f32 coefficient rounding (measured worst
/// case 1 + 6e-8, at DC, where the coefficients sum to 1), and its product
/// with the loop-gain cap must stay below 1. A frozen LFO (fraction 0) must
/// read the integer sample alone. The fraction-averaged power at 8 kHz and
/// 12 kHz pins the flatness: −0.118 dB and −0.527 dB per pass (hand-derived
/// by integrating |H|² over the fraction; linear interpolation gives −0.79
/// and −1.76 dB).
#[test]
fn hermite_interpolator_is_passive_at_every_fraction() {
    assert_eq!(hermite_coefficients(0.0), [0.0, 1.0, 0.0, 0.0]);

    let magnitude_squared = |coefficients: &[f32; 4], angle: f64| -> f64 {
        let (real, imaginary) = coefficients.iter().enumerate().fold(
            (0.0f64, 0.0f64),
            |(real, imaginary), (tap, &coefficient)| {
                let phase = angle * tap as f64;
                (
                    real + f64::from(coefficient) * phase.cos(),
                    imaginary - f64::from(coefficient) * phase.sin(),
                )
            },
        );
        real * real + imaginary * imaginary
    };

    let fraction_steps = 1024;
    let frequency_steps = 4096;
    let mut worst_magnitude = 0.0f64;
    let mut worst_at = (0.0f32, 0.0f64);
    for fraction_step in 0..=fraction_steps {
        let fraction = fraction_step as f32 / fraction_steps as f32;
        let coefficients = hermite_coefficients(fraction);
        for frequency_step in 0..=frequency_steps {
            let angle = std::f64::consts::PI * frequency_step as f64 / frequency_steps as f64;
            let magnitude = magnitude_squared(&coefficients, angle).sqrt();
            if magnitude > worst_magnitude {
                worst_magnitude = magnitude;
                worst_at = (
                    fraction,
                    angle * f64::from(SAMPLE_RATE) / std::f64::consts::TAU,
                );
            }
        }
    }
    eprintln!(
        "Hermite worst |H| = 1 + {:.2e} at fraction {}, {:.0} Hz",
        worst_magnitude - 1.0,
        worst_at.0,
        worst_at.1
    );
    assert!(
        worst_magnitude <= 1.0 + 1e-6,
        "interpolator gain {worst_magnitude} at fraction {}, {:.0} Hz",
        worst_at.0,
        worst_at.1
    );
    assert!(worst_magnitude * MAXIMUM_LOOP_GAIN < 1.0);

    for (frequency_hz, expected_db) in [(8000.0, -0.118), (12000.0, -0.527)] {
        let angle = std::f64::consts::TAU * frequency_hz / f64::from(SAMPLE_RATE);
        let mean_power = (0..fraction_steps)
            .map(|fraction_step| {
                let fraction = (fraction_step as f32 + 0.5) / fraction_steps as f32;
                magnitude_squared(&hermite_coefficients(fraction), angle)
            })
            .sum::<f64>()
            / fraction_steps as f64;
        let mean_db = 10.0 * mean_power.log10();
        assert!(
            (mean_db - expected_db).abs() < 0.005,
            "{frequency_hz} Hz: fraction-averaged loss {mean_db:.4} dB, expected {expected_db}"
        );
    }
}

/// Steady-state wet/send power ratio (total over both channels) for white
/// noise on the send, through the production (modulated) network.
fn steady_state_wet_to_send_db(rt60: f32) -> f64 {
    let mut fdn = stereo_stage(FdnConfig::default(), Some((rt60, rt60)));
    let frames = (2.5 * SAMPLE_RATE) as usize;
    let send = white_noise(frames, 11);
    let wet = stereo_wet_output(&mut fdn, &send);
    // Skip 1.5 s: the stored energy is within 0.03 dB of steady state even
    // for RT60 = 4 s (1 − 10^(−6·1.5/4) = 0.994).
    let start = (1.5 * SAMPLE_RATE) as usize;
    let send_power: f64 = send[start..]
        .iter()
        .map(|&x| f64::from(x) * f64::from(x))
        .sum();
    let wet_power: f64 = wet[start..]
        .iter()
        .map(|frame| f64::from(frame[0]).powi(2) + f64::from(frame[1]).powi(2))
        .sum();
    10.0 * (wet_power / send_power).log10()
}

/// Gain staging of the send → wet path, pinned so it cannot drift silently.
/// `output_normalization` divides out the network's energy build-up
/// (1/(1−g²)) so the wet level should not depend on RT60. Measured
/// wet/send power: −18.5 / −16.1 / −15.2 / −15.1 dB at RT60 0.5 / 1 / 2 / 4 s
/// — held within 3.5 dB over an 8× RT60 range (−19.4 / −17.2 / −16.6 /
/// −16.8 dB with the old linear interpolation). The absolute level sits
/// ~16 dB below the send: −12 dB is the output combiner extracting the
/// per-line average (1/N, N = 16), the rest is loop loss g² and the
/// modulated read (interpolation loss above ~10 kHz plus a ~2 dB deficit
/// against frozen LFOs that better interpolation does not remove). Hence
/// at d = d_c (send 0.5, −6 dB) the tail is ~22 dB under the direct path
/// before `wet`, not the 0 dB D/R that the critical distance defines.
#[test]
fn steady_state_wet_level_is_nearly_independent_of_rt60() {
    let rt60_values = [0.5f32, 1.0, 2.0, 4.0];
    let levels: Vec<f64> = rt60_values
        .iter()
        .map(|&rt60| steady_state_wet_to_send_db(rt60))
        .collect();
    for (rt60, level) in rt60_values.iter().zip(&levels) {
        assert!(
            (level - -17.5).abs() < 2.5,
            "RT60 {rt60} s: wet/send power should be ≈ −17.5 dB, got {level:.1} dB"
        );
    }
    let loudest = levels.iter().copied().fold(f64::MIN, f64::max);
    let quietest = levels.iter().copied().fold(f64::MAX, f64::min);
    assert!(
        loudest - quietest < 4.0,
        "normalisation should hold the wet level within 4 dB over RT60 0.5–4 s: {levels:?}"
    );
}

/// Stability at the extremes. A near-infinite RT60 (loop gain → 1) driven by
/// full-scale noise must stay bounded by the ±16 soft clamp inside the loop
/// and the ±100 output ceiling, and remain finite; a near-zero RT60 must die
/// out immediately. Covers both the normal and measurement-mode clamps.
#[test]
fn impulse_response_bounded_for_extreme_rt60() {
    for measurement_mode in [false, true] {
        for rt60 in [1.0e6_f32, f32::INFINITY] {
            let (layout, listener) = make_ctx(2, 2);
            let ctx = MixContext {
                measurement_mode,
                ..mix_ctx(&layout, &listener, 2, 2)
            };
            let mut fdn = FdnReverbStage::with_config(FdnConfig {
                rt60_seconds: Some(rt60),
                ..FdnConfig::default()
            });
            fdn.init(&ctx);
            let mut send = white_noise((0.5 * SAMPLE_RATE) as usize, 3);
            send.extend(std::iter::repeat_n(0.0, (0.5 * SAMPLE_RATE) as usize));
            let wet = stereo_wet_output(&mut fdn, &send);
            let peak = wet
                .iter()
                .flat_map(|frame| frame.iter())
                .fold(0.0f32, |peak, &x| {
                    assert!(x.is_finite(), "non-finite output at RT60={rt60}");
                    peak.max(x.abs())
                });
            assert!(peak <= 100.0, "output exceeded ±100 ceiling: {peak}");
            let line_ceiling = if measurement_mode { 100.0 } else { 16.0 };
            let stored_peak = fdn
                .delay_buffers
                .iter()
                .flat_map(|line| line.iter())
                .fold(0.0f32, |peak, &x| peak.max(x.abs()));
            assert!(
                stored_peak <= line_ceiling,
                "delay-line content {stored_peak} exceeded ±{line_ceiling} \
                 (RT60={rt60}, measurement_mode={measurement_mode})"
            );
        }
    }

    // Near-zero RT60: per-pass gain 10^(-3·1249/(1e-3·48000)) underflows to 0,
    // so nothing survives the first pass through the loop.
    let mut fdn = stereo_stage(FdnConfig::default(), Some((1.0e-3, 1.0e-3)));
    let response = impulse_response(&mut fdn, (0.2 * SAMPLE_RATE) as usize);
    let late_peak = response[(0.1 * SAMPLE_RATE) as usize..]
        .iter()
        .fold(0.0f32, |peak, &x| peak.max(x.abs()));
    assert!(
        late_peak < 1e-6,
        "1 ms RT60 should leave no tail, got {late_peak}"
    );
}

/// The in-loop soft clamp may only ever remove energy: identity below the
/// knee, odd-symmetric, monotonic, |f(x)| ≤ |x|, C¹ at the knee (slope 1),
/// and asymptotic to ±16. Together with |H_damping| < 1 this keeps the loop
/// gain below 1 at every frequency and amplitude.
#[test]
fn feedback_soft_clamp_is_passive_monotonic_and_smooth() {
    let mut previous = feedback_soft_clamp(-1000.0);
    let mut value = -1000.0f32;
    while value <= 1000.0 {
        let clamped = feedback_soft_clamp(value);
        assert!(
            clamped.abs() <= value.abs() + 1e-6,
            "f({value}) gained energy"
        );
        assert!(
            clamped.abs() < 16.0,
            "f({value}) = {clamped} reached the ceiling"
        );
        assert!(clamped >= previous, "f not monotonic at {value}");
        assert_eq!(
            feedback_soft_clamp(-value),
            -clamped,
            "f not odd at {value}"
        );
        previous = clamped;
        value += 0.25;
    }
    let step = 1e-3f32;
    let slope_above_knee = (feedback_soft_clamp(14.0 + step) - 14.0) / step;
    assert!(
        (slope_above_knee - 1.0).abs() < 2e-3,
        "slope just above the knee should be 1 (C¹), got {slope_above_knee}"
    );
}

/// The default pre-delay is the mean-free-path time 4V/(S·c). For the
/// shipped cathedral (40×25×20 m, V = 20000 m³, S = 4600 m²) that is
/// 17.39 m / 343.42 m/s = 50.6 ms. A fixed 2048-sample pre-delay buffer
/// used to cap it at 2047/48000 = 42.6 ms (21.3 ms at 96 kHz); the ring is
/// now sized from the pre-delay and the sample rate.
#[test]
fn pre_delay_cathedral_not_truncated_by_buffer() {
    let (volume, surface_area) =
        room_acoustics::room_geometry(Vec3::ZERO, Vec3::new(40.0, 25.0, 20.0));
    for sample_rate in [48_000.0f32, 96_000.0] {
        let mut fdn = FdnReverbStage::new();
        fdn.compute_pre_delay(sample_rate, volume, surface_area, 343.42);
        let actual_ms = fdn.pre_delay_samples as f32 / sample_rate * 1000.0;
        assert!(
            (actual_ms - 50.6).abs() < 0.5,
            "cathedral pre-delay at {sample_rate} Hz should be 50.6 ms, got {actual_ms:.1} ms"
        );
    }
}

/// An explicit pre-delay beyond the old 2048-sample cap is honoured end to
/// end: 100 ms at 96 kHz = 9600 samples, so a unit impulse on the send
/// cannot reach the output before frame 9600 + 2498 − 22 (the shortest
/// line, 1249 × 2, shortened by at most the LFO's 20-sample depth at
/// 96 kHz, the interpolator's newest tap and the integer rounding — any
/// earlier wet means the read wrapped round an undersized ring). Overrides
/// above 250 ms are bounded to 250 ms (24000 samples at 96 kHz).
#[test]
fn pre_delay_override_beyond_old_cap_reaches_the_output() {
    let (layout, listener) = make_ctx(2, 2);
    let sample_rate = 96_000.0;
    let frames = 16_384;
    let mut reverb_input = vec![0.0f32; frames * 2];
    reverb_input[0] = 1.0;
    let ctx = MixContext {
        sample_rate,
        reverb_input: Some(&reverb_input),
        ..mix_ctx(&layout, &listener, 2, 2)
    };
    let mut fdn = FdnReverbStage::with_config(FdnConfig {
        pre_delay_ms: Some(100.0),
        ..FdnConfig::default()
    });
    fdn.init(&ctx);
    assert_eq!(fdn.pre_delay_samples, 9600);
    assert!(fdn.pre_delay_buffer.len() > 9600);

    let mut buffer = vec![0.0f32; frames * 2];
    fdn.process(&mut buffer, &ctx);
    let first_wet_frame = buffer
        .chunks_exact(2)
        .position(|frame| frame[0] != 0.0 || frame[1] != 0.0)
        .expect("the tail must reach the output within the window");
    assert!(
        first_wet_frame >= 9600 + 2498 - 22,
        "wet arrived at frame {first_wet_frame}, before pre-delay + shortest line"
    );

    let mut fdn = FdnReverbStage::with_config(FdnConfig {
        pre_delay_ms: Some(1000.0),
        ..FdnConfig::default()
    });
    fdn.init(&ctx);
    assert_eq!(fdn.pre_delay_samples, 24_000);
}
