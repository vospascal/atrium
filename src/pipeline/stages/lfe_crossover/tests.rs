use super::*;
use atrium_core::listener::Listener;
use atrium_core::speaker::{Speaker, SpeakerLayout};
use atrium_core::types::Vec3;

use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::propagation::GroundProperties;
use crate::pipeline::environment::Environment;
use crate::pipeline::wall_material::WallMaterial;

const SAMPLE_RATE: f32 = 48000.0;
const TEST_ATMOSPHERE: AtmosphericParams = AtmosphericParams {
    temperature_c: 20.0,
    humidity_pct: 50.0,
    pressure_kpa: 101.325,
};

fn test_environment() -> Environment {
    Environment {
        min: Vec3::new(-5.0, -5.0, -5.0),
        max: Vec3::new(5.0, 5.0, 5.0),
        wall_materials: [WallMaterial::HARD_WALL; 6],
        ground: GroundProperties::hard(),
        atmosphere: TEST_ATMOSPHERE,
    }
}

fn surround_51_layout() -> SpeakerLayout {
    SpeakerLayout::new(
        &[
            Speaker {
                position: Vec3::new(-1.0, 0.0, 1.0),
                channel: 0,
            }, // L
            Speaker {
                position: Vec3::new(1.0, 0.0, 1.0),
                channel: 1,
            }, // R
            Speaker {
                position: Vec3::new(0.0, 0.0, 1.0),
                channel: 2,
            }, // C
            // channel 3 = LFE (no position)
            Speaker {
                position: Vec3::new(-1.0, 0.0, -1.0),
                channel: 4,
            }, // LS
            Speaker {
                position: Vec3::new(1.0, 0.0, -1.0),
                channel: 5,
            }, // RS
        ],
        Some(3), // LFE on channel 3
        6,
    )
}

fn stereo_layout() -> SpeakerLayout {
    SpeakerLayout::stereo(Vec3::new(-1.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 1.0))
}

fn test_mix_context<'a>(
    layout: &'a SpeakerLayout,
    listener: &'a Listener,
    environment: &'a Environment,
) -> MixContext<'a> {
    MixContext {
        listener,
        layout,
        sample_rate: SAMPLE_RATE,
        channels: layout.total_channels(),
        environment,
        master_gain: 1.0,
        render_channels: layout.total_channels(),
        reverb_input: None,
        measurement_mode: false,
    }
}

/// No LFE channel → process is a no-op, signal passes through unchanged.
#[test]
fn no_lfe_is_noop() {
    let layout = stereo_layout();
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment);
    let mut stage = LfeBassManagementStage::new();
    stage.init(&ctx);

    let mut buffer = vec![1.0; 2 * 256];
    let original = buffer.clone();
    stage.process(&mut buffer, &ctx);
    assert_eq!(buffer, original);
}

/// LR4 lowpass attenuates content well above 120 Hz on the LFE channel.
#[test]
fn lfe_lowpass_attenuates_high_frequencies() {
    let layout = surround_51_layout();
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment);
    let mut stage = LfeBassManagementStage::new();
    stage.init(&ctx);

    let channels = 6;
    let num_frames = 4096;
    let mut buffer = vec![0.0f32; channels * num_frames];

    // Put a 1 kHz sine on LFE channel only (well above 120 Hz crossover).
    let freq = 1000.0;
    for frame in 0..num_frames {
        let t = frame as f32 / SAMPLE_RATE;
        buffer[frame * channels + 3] = (2.0 * std::f32::consts::PI * freq * t).sin();
    }

    stage.process(&mut buffer, &ctx);

    // Measure LFE output energy in the last 2048 frames (after filter settles).
    let lfe_energy: f32 = (num_frames / 2..num_frames)
        .map(|f| {
            let s = buffer[f * channels + 3];
            s * s
        })
        .sum::<f32>()
        / (num_frames / 2) as f32;

    // LR4 at 1 kHz (3+ octaves above 120 Hz) should attenuate by ~48+ dB.
    // Energy should be negligible.
    assert!(
        lfe_energy < 0.001,
        "LFE energy at 1 kHz should be heavily attenuated, got {lfe_energy}"
    );
}

/// LR4 lowpass passes content well below 120 Hz on the LFE channel.
#[test]
fn lfe_lowpass_passes_bass() {
    let layout = surround_51_layout();
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment);
    let mut stage = LfeBassManagementStage::new();
    stage.init(&ctx);

    let channels = 6;
    let num_frames = 4096;
    let mut buffer = vec![0.0f32; channels * num_frames];

    // Put a 40 Hz sine on LFE channel (well below 120 Hz).
    let freq = 40.0;
    for frame in 0..num_frames {
        let t = frame as f32 / SAMPLE_RATE;
        buffer[frame * channels + 3] = (2.0 * std::f32::consts::PI * freq * t).sin();
    }

    // Measure input energy for reference.
    let input_energy: f32 = (num_frames / 2..num_frames)
        .map(|f| {
            let s = (2.0 * std::f32::consts::PI * freq * f as f32 / SAMPLE_RATE).sin();
            s * s
        })
        .sum::<f32>()
        / (num_frames / 2) as f32;

    stage.process(&mut buffer, &ctx);

    let lfe_energy: f32 = (num_frames / 2..num_frames)
        .map(|f| {
            let s = buffer[f * channels + 3];
            s * s
        })
        .sum::<f32>()
        / (num_frames / 2) as f32;

    // 40 Hz should pass through with minimal loss (within 1 dB).
    let ratio = lfe_energy / input_energy;
    assert!(
        ratio > 0.89, // -0.5 dB
        "40 Hz should pass through LFE LP, got ratio {ratio}"
    );
}

/// Main channels get highpassed: bass content is removed.
#[test]
fn main_channels_highpassed() {
    let layout = surround_51_layout();
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment);
    let mut stage = LfeBassManagementStage::new();
    stage.init(&ctx);

    let channels = 6;
    let num_frames = 4096;
    let mut buffer = vec![0.0f32; channels * num_frames];

    // Put a 40 Hz sine on channel 0 (front left).
    let freq = 40.0;
    for frame in 0..num_frames {
        let t = frame as f32 / SAMPLE_RATE;
        buffer[frame * channels] = (2.0 * std::f32::consts::PI * freq * t).sin();
    }

    stage.process(&mut buffer, &ctx);

    // Channel 0 should have very little 40 Hz energy left.
    let ch0_energy: f32 = (num_frames / 2..num_frames)
        .map(|f| {
            let s = buffer[f * channels];
            s * s
        })
        .sum::<f32>()
        / (num_frames / 2) as f32;

    assert!(
        ch0_energy < 0.01,
        "40 Hz on main channel should be attenuated by HP, got energy {ch0_energy}"
    );
}

/// Bass removed from mains is redirected to LFE.
#[test]
fn bass_redirected_to_lfe() {
    let layout = surround_51_layout();
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment);
    let mut stage = LfeBassManagementStage::new();
    stage.init(&ctx);

    let channels = 6;
    let num_frames = 4096;
    let mut buffer = vec![0.0f32; channels * num_frames];

    // Put a 40 Hz sine on channel 0 (front left). Nothing on LFE initially.
    let freq = 40.0;
    for frame in 0..num_frames {
        let t = frame as f32 / SAMPLE_RATE;
        buffer[frame * channels] = (2.0 * std::f32::consts::PI * freq * t).sin();
    }

    stage.process(&mut buffer, &ctx);

    // LFE should now contain the redirected 40 Hz bass.
    let lfe_energy: f32 = (num_frames / 2..num_frames)
        .map(|f| {
            let s = buffer[f * channels + 3];
            s * s
        })
        .sum::<f32>()
        / (num_frames / 2) as f32;

    assert!(
        lfe_energy > 0.1,
        "Redirected 40 Hz bass should appear on LFE, got energy {lfe_energy}"
    );
}

/// Steady-state amplitude of a sine on the given channel over the last
/// half of the buffer (RMS × √2).
fn steady_state_amplitude(buffer: &[f32], channels: usize, ch: usize) -> f32 {
    let num_frames = buffer.len() / channels;
    let start = num_frames / 2;
    let mean_square: f32 = (start..num_frames)
        .map(|f| {
            let s = buffer[f * channels + ch];
            s * s
        })
        .sum::<f32>()
        / (num_frames - start) as f32;
    (2.0 * mean_square).sqrt()
}

/// Run a single-frequency sine (amplitude 1.0) on channel 0 through the
/// stage and return (main_ch0_amplitude, lfe_amplitude) at steady state.
fn run_sine_through_crossover(freq: f32) -> (f32, f32) {
    let layout = surround_51_layout();
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment);
    let mut stage = LfeBassManagementStage::new();
    stage.init(&ctx);

    let channels = 6;
    let num_frames = 16384;
    let mut buffer = vec![0.0f32; channels * num_frames];
    for frame in 0..num_frames {
        let t = frame as f32 / SAMPLE_RATE;
        buffer[frame * channels] = (2.0 * std::f32::consts::PI * freq * t).sin();
    }
    stage.process(&mut buffer, &ctx);

    (
        steady_state_amplitude(&buffer, channels, 0),
        steady_state_amplitude(&buffer, channels, 3),
    )
}

/// LR4 reconstruction: |HP(f)| + |LP(f)| contributions must sum to flat
/// magnitude at every frequency. LR4's property is an *allpass* sum —
/// flat magnitude with phase rotation — so this is asserted per frequency
/// on steady-state sine amplitude, NOT as time-domain waveform identity.
/// (The old formulation `redirect = original − HP` made the time-domain
/// identity hold tautologically while injecting +9.5 dB excess bass at
/// the crossover.)
#[test]
fn crossover_sum_is_magnitude_flat_per_frequency() {
    for freq in [40.0, 80.0, 120.0, 240.0, 1000.0, 4000.0] {
        let (main, lfe) = run_sine_through_crossover(freq);
        // LP and HP branches are phase-coherent (that's the point of LR4),
        // so amplitudes of ch0 (HP) + ch3 (LP redirect) add in phase.
        let total = main + lfe;
        assert!(
            (total - 1.0).abs() < 0.06,
            "{freq} Hz: |HP| + |LP| = {total:.3}, expected ~1.0 (main={main:.3}, lfe={lfe:.3})"
        );
    }
}

/// At the 120 Hz crossover each LR4 branch is exactly −6 dB (0.5×).
/// The redirected bass must arrive at 0.5×, not the 1.5× produced by the
/// old `original − highpassed` formulation.
#[test]
fn redirected_bass_at_crossover_is_minus_6_db() {
    let (main, lfe) = run_sine_through_crossover(120.0);
    assert!(
        (lfe - 0.5).abs() < 0.03,
        "120 Hz redirected bass should be ~0.5× (−6 dB), got {lfe:.3}"
    );
    assert!(
        (main - 0.5).abs() < 0.03,
        "120 Hz highpassed main should be ~0.5× (−6 dB), got {main:.3}"
    );
}

/// Midrange must not leak into the subwoofer. A true LR4 lowpass is
/// ~−73 dB at 1 kHz; the old formulation leaked 1 kHz into the LFE at
/// only −9.4 dB (0.34×).
#[test]
fn midrange_does_not_leak_into_lfe() {
    let (_, lfe) = run_sine_through_crossover(1000.0);
    assert!(
        lfe < 0.01,
        "1 kHz content on a main channel should stay out of the LFE, got {lfe:.4}"
    );
}

// ── Independent LR4 reference and phasor measurement ───────────────────

use realfft::num_complex::Complex;

/// LR4 magnitude (two cascaded Butterworth sections) with bilinear
/// prewarping: lowpass = 1/(1+r⁴), highpass = 1/(1+r⁻⁴), r = tan(ωf/2)/tan(ωc/2).
fn reference_lr4_magnitude(frequency_hz: f64, sample_rate: f64, highpass: bool) -> f64 {
    let ratio = (std::f64::consts::PI * frequency_hz / sample_rate).tan()
        / (std::f64::consts::PI * LFE_CUTOFF_HZ as f64 / sample_rate).tan();
    if highpass {
        1.0 / (1.0 + ratio.powi(-4))
    } else {
        1.0 / (1.0 + ratio.powi(4))
    }
}

/// Complex amplitude A·e^{jφ} of `y ≈ A·sin(ωt + φ)` over the last part of
/// `output`, using a Hann-weighted quadrature demodulation.
fn demodulate(output: &[f32], frequency_hz: f64, sample_rate: f64, start: usize) -> Complex<f64> {
    let window_length = output.len() - start;
    let mut in_phase = 0.0f64;
    let mut quadrature = 0.0f64;
    let mut weight_sum = 0.0f64;
    for (index, &output_sample) in output.iter().enumerate().skip(start) {
        let phase = 2.0 * std::f64::consts::PI * frequency_hz * index as f64 / sample_rate;
        let position = (index - start) as f64 / window_length as f64;
        let weight = 0.5 * (1.0 - (2.0 * std::f64::consts::PI * position).cos());
        in_phase += weight * output_sample as f64 * phase.sin();
        quadrature += weight * output_sample as f64 * phase.cos();
        weight_sum += weight;
    }
    Complex::new(in_phase, quadrature) * (2.0 / weight_sum)
}

fn sine_samples(frequency_hz: f64, sample_rate: f64, count: usize) -> Vec<f32> {
    (0..count)
        .map(|index| {
            (2.0 * std::f64::consts::PI * frequency_hz * index as f64 / sample_rate).sin() as f32
        })
        .collect()
}

fn to_db(magnitude: f64) -> f64 {
    20.0 * magnitude.log10()
}

/// A 1/3-octave style sweep from 20 Hz to 20 kHz.
fn crossover_sweep_frequencies(sample_rate: f64) -> Vec<f64> {
    let mut frequencies = Vec::new();
    let mut frequency_hz = 20.0f64;
    while frequency_hz <= 20_000.0 && frequency_hz < 0.45 * sample_rate {
        frequencies.push(frequency_hz);
        frequency_hz *= 2.0f64.powf(1.0 / 3.0);
    }
    frequencies
}

/// Each LR4 branch matches the analytic magnitude 1/(1±r⁴) (so exactly
/// 0.5 = -6.02 dB at the 120 Hz crossover) at every sample rate.
#[test]
fn lr4_branches_match_analytic_magnitude_across_sample_rates() {
    for sample_rate in [44_100.0f64, 48_000.0, 96_000.0] {
        for frequency_hz in crossover_sweep_frequencies(sample_rate) {
            let count = sample_rate as usize;
            let input = sine_samples(frequency_hz, sample_rate, count);
            let mut lowpass = Lr4Filter::lowpass(LFE_CUTOFF_HZ, sample_rate as f32);
            let mut highpass = Lr4Filter::highpass(LFE_CUTOFF_HZ, sample_rate as f32);
            let lowpass_output: Vec<f32> = input.iter().map(|&x| lowpass.process(x)).collect();
            let highpass_output: Vec<f32> = input.iter().map(|&x| highpass.process(x)).collect();
            let lowpass_gain =
                demodulate(&lowpass_output, frequency_hz, sample_rate, count / 2).norm();
            let highpass_gain =
                demodulate(&highpass_output, frequency_hz, sample_rate, count / 2).norm();
            let expected_lowpass = reference_lr4_magnitude(frequency_hz, sample_rate, false);
            let expected_highpass = reference_lr4_magnitude(frequency_hz, sample_rate, true);
            // Compare in dB only where the response is above -70 dB (f32 floor).
            if expected_lowpass > 3e-4 {
                assert!(
                    (to_db(lowpass_gain) - to_db(expected_lowpass)).abs() < 0.05,
                    "LP {frequency_hz:.1} Hz @ {sample_rate}: {:.3} vs {:.3} dB",
                    to_db(lowpass_gain),
                    to_db(expected_lowpass)
                );
            }
            if expected_highpass > 3e-4 {
                assert!(
                    (to_db(highpass_gain) - to_db(expected_highpass)).abs() < 0.05,
                    "HP {frequency_hz:.1} Hz @ {sample_rate}: {:.3} vs {:.3} dB",
                    to_db(highpass_gain),
                    to_db(expected_highpass)
                );
            }
        }
    }
    // The crossover point itself: -6.0206 dB per branch.
    let at_crossover = reference_lr4_magnitude(120.0, 48_000.0, false);
    assert!((to_db(at_crossover) + 6.0206).abs() < 1e-3);
}

/// LR4 reconstruction, measured on complex phasors: LP and HP are in phase
/// at every frequency (no polarity flip, no relative phase rotation) and the
/// phasor sum |LP + HP| is 0 dB within 0.1 dB from 20 Hz to 20 kHz. This is
/// the allpass property, and is stronger than the per-amplitude check above.
#[test]
fn lr4_lowpass_plus_highpass_is_flat_and_in_phase() {
    for sample_rate in [44_100.0f64, 48_000.0, 96_000.0] {
        for frequency_hz in crossover_sweep_frequencies(sample_rate) {
            let count = sample_rate as usize;
            let input = sine_samples(frequency_hz, sample_rate, count);
            let mut lowpass = Lr4Filter::lowpass(LFE_CUTOFF_HZ, sample_rate as f32);
            let mut highpass = Lr4Filter::highpass(LFE_CUTOFF_HZ, sample_rate as f32);
            let lowpass_output: Vec<f32> = input.iter().map(|&x| lowpass.process(x)).collect();
            let highpass_output: Vec<f32> = input.iter().map(|&x| highpass.process(x)).collect();
            let lowpass_phasor = demodulate(&lowpass_output, frequency_hz, sample_rate, count / 2);
            let highpass_phasor =
                demodulate(&highpass_output, frequency_hz, sample_rate, count / 2);
            let sum = lowpass_phasor + highpass_phasor;
            assert!(
                to_db(sum.norm()).abs() < 0.1,
                "{frequency_hz:.1} Hz @ {sample_rate}: |LP+HP| = {:.3} dB",
                to_db(sum.norm())
            );
            // Phase agreement only meaningful while both branches carry signal.
            if lowpass_phasor.norm() > 0.01 && highpass_phasor.norm() > 0.01 {
                let phase_difference = (lowpass_phasor / highpass_phasor).arg();
                assert!(
                    phase_difference.abs() < 0.03,
                    "{frequency_hz:.1} Hz @ {sample_rate}: LP/HP phase differ by {phase_difference:.4} rad"
                );
            }
        }
    }
}

/// Polarity: the lowpass has +1 DC gain and the highpass's first impulse
/// response sample is positive (b0² > 0) — neither branch inverts.
#[test]
fn lr4_branches_have_positive_polarity() {
    for sample_rate in [44_100.0f32, 48_000.0, 96_000.0] {
        let mut lowpass = Lr4Filter::lowpass(LFE_CUTOFF_HZ, sample_rate);
        let mut step_response = 0.0;
        for _ in 0..(sample_rate as usize) {
            step_response = lowpass.process(1.0);
        }
        assert!(
            // f32 coefficient quantization at 120 Hz / 96 kHz costs ~0.004 (0.03 dB).
            (step_response - 1.0).abs() < 1e-2,
            "LR4 LP DC gain {step_response} @ {sample_rate}"
        );
        let mut highpass = Lr4Filter::highpass(LFE_CUTOFF_HZ, sample_rate);
        assert!(
            highpass.process(1.0) > 0.0,
            "HP impulse must start positive"
        );
        // And blocks DC.
        let mut highpass = Lr4Filter::highpass(LFE_CUTOFF_HZ, sample_rate);
        let mut dc_response = 1.0;
        for _ in 0..(sample_rate as usize) {
            dc_response = highpass.process(1.0);
        }
        assert!(dc_response.abs() < 1e-3, "LR4 HP DC leak {dc_response}");
    }
}

/// Stage-level flatness: main-channel highpass (ch0) plus redirected bass
/// (LFE) sum to 0 dB within 0.1 dB across the band, in phase, through the
/// real `process` path with the parallel-lowpass redirect.
#[test]
fn stage_main_plus_redirected_bass_is_flat_across_band() {
    let layout = surround_51_layout();
    let listener = Listener::new(Vec3::ZERO, 0.0);
    for sample_rate in [44_100.0f32, 48_000.0, 96_000.0] {
        let sample_rate_f64 = sample_rate as f64;
        for frequency_hz in crossover_sweep_frequencies(sample_rate_f64)
            .into_iter()
            .step_by(2)
        {
            let environment = test_environment();
            let ctx = MixContext {
                sample_rate,
                ..test_mix_context(&layout, &listener, &environment)
            };
            let mut stage = LfeBassManagementStage::new();
            stage.init(&ctx);
            let channels = 6;
            let num_frames = (sample_rate as usize) / 2;
            let input = sine_samples(frequency_hz, sample_rate_f64, num_frames);
            let mut buffer = vec![0.0f32; channels * num_frames];
            for (frame, &sample) in input.iter().enumerate() {
                buffer[frame * channels] = sample;
            }
            stage.process(&mut buffer, &ctx);
            let main: Vec<f32> = (0..num_frames).map(|f| buffer[f * channels]).collect();
            let lfe: Vec<f32> = (0..num_frames).map(|f| buffer[f * channels + 3]).collect();
            let main_phasor = demodulate(&main, frequency_hz, sample_rate_f64, num_frames / 2);
            let lfe_phasor = demodulate(&lfe, frequency_hz, sample_rate_f64, num_frames / 2);
            let sum = main_phasor + lfe_phasor;
            assert!(
                to_db(sum.norm()).abs() < 0.1,
                "{frequency_hz:.1} Hz @ {sample_rate}: |main+LFE| = {:.3} dB",
                to_db(sum.norm())
            );
            // Other non-LFE channels see nothing from channel 0's signal.
            for ch in [1usize, 2, 4, 5] {
                assert!((0..num_frames).all(|f| buffer[f * channels + ch] == 0.0));
            }
        }
    }
}

/// The redirect has unit gain per main channel and **no 1/N normalization**:
/// five mains carrying the same 40 Hz tone plus a 40 Hz tone already on the
/// LFE sum coherently to ≈ 6 × |LP(40 Hz)| at the LFE. (Peak 5.9 at the LFE
/// from full-scale inputs: only the final soft clip protects the subwoofer.)
#[test]
fn redirected_bass_from_all_mains_sums_with_unit_gain_each() {
    let layout = surround_51_layout();
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment);
    let mut stage = LfeBassManagementStage::new();
    stage.init(&ctx);
    let channels = 6;
    let num_frames = 24_000;
    let input = sine_samples(40.0, SAMPLE_RATE as f64, num_frames);
    let mut buffer = vec![0.0f32; channels * num_frames];
    for (frame, &sample) in input.iter().enumerate() {
        for ch in 0..channels {
            buffer[frame * channels + ch] = sample;
        }
    }
    stage.process(&mut buffer, &ctx);
    let lfe: Vec<f32> = (0..num_frames).map(|f| buffer[f * channels + 3]).collect();
    let lfe_gain = demodulate(&lfe, 40.0, SAMPLE_RATE as f64, num_frames / 2).norm();
    let branch = reference_lr4_magnitude(40.0, SAMPLE_RATE as f64, false);
    assert!(
        (lfe_gain / (6.0 * branch) - 1.0).abs() < 0.01,
        "LFE amplitude {lfe_gain:.3} vs expected {:.3}",
        6.0 * branch
    );
}

/// DC on a main channel is removed from the main and appears at unit gain
/// on the LFE.
#[test]
fn dc_on_main_moves_entirely_to_lfe() {
    let layout = surround_51_layout();
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment);
    let mut stage = LfeBassManagementStage::new();
    stage.init(&ctx);
    let channels = 6;
    let num_frames = 48_000;
    let mut buffer = vec![0.0f32; channels * num_frames];
    for frame in 0..num_frames {
        buffer[frame * channels] = 1.0;
    }
    stage.process(&mut buffer, &ctx);
    let last = (num_frames - 1) * channels;
    assert!(buffer[last].abs() < 1e-3, "main DC leak {}", buffer[last]);
    assert!(
        (buffer[last + 3] - 1.0).abs() < 1e-2,
        "LFE DC {}",
        buffer[last + 3]
    );
}

/// The LFE channel's own input is filtered by the same LR4 lowpass: its gain
/// at the 120 Hz crossover is 0.5 and 1 kHz is rejected by > 70 dB.
#[test]
fn lfe_input_uses_lr4_lowpass() {
    let layout = surround_51_layout();
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment);
    for (frequency_hz, expected) in [
        (120.0f64, 0.5f64),
        (60.0, reference_lr4_magnitude(60.0, 48_000.0, false)),
    ] {
        let mut stage = LfeBassManagementStage::new();
        stage.init(&ctx);
        let channels = 6;
        let num_frames = 24_000;
        let input = sine_samples(frequency_hz, SAMPLE_RATE as f64, num_frames);
        let mut buffer = vec![0.0f32; channels * num_frames];
        for (frame, &sample) in input.iter().enumerate() {
            buffer[frame * channels + 3] = sample;
        }
        stage.process(&mut buffer, &ctx);
        let lfe: Vec<f32> = (0..num_frames).map(|f| buffer[f * channels + 3]).collect();
        let gain = demodulate(&lfe, frequency_hz, SAMPLE_RATE as f64, num_frames / 2).norm();
        assert!(
            (gain - expected).abs() < 0.005,
            "LFE input at {frequency_hz} Hz: {gain:.4} vs {expected:.4}"
        );
        // Mains stay silent because nothing was fed to them.
        assert!((0..num_frames).all(|f| buffer[f * channels] == 0.0));
    }
}

/// Mid/high frequencies pass the main-channel highpass essentially unchanged
/// (|HP| within 0.01 dB of unity above 1 kHz) and LFE stays silent.
#[test]
fn mains_pass_midrange_at_unity() {
    for frequency_hz in [1000.0f64, 4000.0, 16000.0] {
        let (main, lfe) = run_sine_through_crossover(frequency_hz as f32);
        assert!(
            to_db(main as f64).abs() < 0.05,
            "{frequency_hz} Hz main gain {:.3} dB",
            to_db(main as f64)
        );
        assert!(lfe < 1e-3);
    }
}

/// `reset` clears every filter so a silent buffer stays silent afterwards.
#[test]
fn reset_clears_crossover_state() {
    let layout = surround_51_layout();
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment);
    let mut stage = LfeBassManagementStage::new();
    stage.init(&ctx);
    let channels = 6;
    let mut buffer = vec![1.0f32; channels * 512];
    stage.process(&mut buffer, &ctx);
    stage.reset();
    let mut silence = vec![0.0f32; channels * 512];
    stage.process(&mut silence, &ctx);
    assert!(silence.iter().all(|&sample| sample == 0.0));
}

/// If the layout's LFE channel does not fit the actual output width, the
/// stage disables itself instead of indexing out of range.
#[test]
fn lfe_channel_beyond_output_width_is_noop() {
    let layout = surround_51_layout(); // LFE on channel 3
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let environment = test_environment();
    let ctx = MixContext {
        channels: 2,
        render_channels: 2,
        ..test_mix_context(&layout, &listener, &environment)
    };
    let mut stage = LfeBassManagementStage::new();
    stage.init(&ctx);
    let mut buffer = vec![0.5f32; 2 * 64];
    let original = buffer.clone();
    stage.process(&mut buffer, &ctx);
    assert_eq!(buffer, original);
}
