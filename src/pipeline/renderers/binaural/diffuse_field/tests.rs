use super::ear_filters::{
    design_ear_filters, design_grid_size, echo_delay_seconds, sound_and_echo_for_coherence,
};
use super::statistics::DiffuseStatistics;
use super::*;
use realfft::RealFftPlanner;

// ── Hand-derived values ───────────────────────────────────────────────

/// a² + b² = 1 keeps each ear's power; a² − b² is the ears' coherence. So
/// identical ears have no echo, unrelated ears an echo as strong as the
/// sound, and opposite ears nothing but the echo.
#[test]
fn sound_and_echo_shares_give_the_coherence_at_unchanged_power() {
    assert_eq!(sound_and_echo_for_coherence(1.0), (1.0, 0.0));
    assert_eq!(sound_and_echo_for_coherence(-1.0), (0.0, 1.0));
    let (sound, echo) = sound_and_echo_for_coherence(0.0);
    assert!((sound - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
    assert!((echo - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
    for coherence in [-0.4, -0.2, 0.05, 0.3, 0.5, 0.67, 0.85, 0.94] {
        let (sound, echo) = sound_and_echo_for_coherence(coherence);
        assert!((sound * sound + echo * echo - 1.0).abs() < 1e-12);
        assert!((sound * sound - echo * echo - coherence).abs() < 1e-12);
    }
}

/// The delay is 1.2 ms plus two periods of the third octave's width,
/// 2 / (0.231563 f), and never more than 30 ms.
#[test]
fn echo_delay_is_a_margin_plus_two_turns_of_a_third_octave() {
    for (frequency, expected_ms) in [
        (1000.0, 1.2 + 2000.0 / 231.563),
        (4000.0, 1.2 + 2000.0 / 926.252),
        (8000.0, 1.2 + 2000.0 / 1852.504),
        (100.0, 30.0),
        (20.0, 30.0),
    ] {
        let delay_ms = echo_delay_seconds(frequency) * 1000.0;
        assert!(
            (delay_ms - expected_ms).abs() < 1e-3,
            "{frequency} Hz: {delay_ms} ms, expected {expected_ms}"
        );
    }
}

// ── The design on a head whose answer is known ───────────────────────

/// A head that hears every frequency at unit power in both ears, with the
/// ears' coherence `coherence` at every frequency.
fn flat_head(coherence: f64, sample_rate: f32) -> DiffuseStatistics {
    let bins = design_grid_size(sample_rate) / 2 + 1;
    DiffuseStatistics {
        left_power: vec![1.0; bins],
        right_power: vec![1.0; bins],
        cross_power: vec![coherence; bins],
    }
}

/// Per third octave from 400 Hz to 12.7 kHz: the coherence (largest over
/// ±1 ms of shift, as the ear compares) and each ear's level in dB.
fn third_octave_measures(
    left: &[f32],
    right: &[f32],
    sample_rate: f32,
) -> Vec<(f64, f64, f64, f64)> {
    let size = 1 << 16;
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(size);
    let spectrum_of = |response: &[f32]| {
        let mut time = forward.make_input_vec();
        for (slot, &sample) in time.iter_mut().zip(response) {
            *slot = sample as f64;
        }
        let mut spectrum = forward.make_output_vec();
        forward.process(&mut time, &mut spectrum).expect("forward");
        spectrum
    };
    let left = spectrum_of(left);
    let right = spectrum_of(right);
    let bin_hz = sample_rate as f64 / size as f64;
    let largest_lag = (0.001 * sample_rate as f64).round() as i64;
    (-4..=11)
        .map(|band| {
            let centre = 1000.0 * 2f64.powf(band as f64 / 3.0);
            let lowest = (centre * 2f64.powf(-1.0 / 6.0) / bin_hz).ceil() as usize;
            let highest = (centre * 2f64.powf(1.0 / 6.0) / bin_hz).floor() as usize;
            let left_power: f64 = left[lowest..=highest]
                .iter()
                .map(|bin| bin.norm_sqr())
                .sum();
            let right_power: f64 = right[lowest..=highest]
                .iter()
                .map(|bin| bin.norm_sqr())
                .sum();
            let coherence = (-largest_lag..=largest_lag)
                .map(|lag| {
                    (lowest..=highest)
                        .map(|bin| {
                            let shift = realfft::num_complex::Complex::from_polar(
                                1.0,
                                std::f64::consts::TAU * bin as f64 * lag as f64 / size as f64,
                            );
                            left[bin] * right[bin].conj() * shift
                        })
                        .sum::<realfft::num_complex::Complex<f64>>()
                        .norm()
                })
                .fold(0f64, f64::max)
                / (left_power * right_power).sqrt();
            let count = (highest - lowest + 1) as f64;
            (
                centre,
                coherence,
                10.0 * (left_power / count).log10(),
                10.0 * (right_power / count).log10(),
            )
        })
        .collect()
}

/// On a flat head the filters must keep both ears flat in every third
/// octave and give the coherence asked for: from 400 Hz up the echo delay
/// turns over at least twice across each.
#[test]
fn a_flat_head_keeps_its_colour_and_gets_the_coherence_asked_for() {
    let sample_rate = 48_000.0;
    for wanted in [0.0, 0.5, -0.2] {
        let filters = design_ear_filters(&flat_head(wanted, sample_rate), sample_rate);
        for (centre, coherence, left_db, right_db) in
            third_octave_measures(&filters.left, &filters.right, sample_rate)
        {
            assert!(
                left_db.abs() < 1.0 && right_db.abs() < 1.0,
                "coherence {wanted}, {centre:.0} Hz: left {left_db:+.2} dB, right {right_db:+.2} dB"
            );
            assert!(
                (coherence - wanted.abs()).abs() < 0.15,
                "coherence {wanted}, {centre:.0} Hz: measured {coherence:.3}"
            );
        }
    }
}

/// With the ears asked to be identical there are no echoes: both filters
/// are the same, a millisecond's lead-in and nothing else.
#[test]
fn identical_ears_get_identical_filters() {
    let sample_rate = 48_000.0;
    let filters = design_ear_filters(&flat_head(1.0, sample_rate), sample_rate);
    assert_eq!(filters.left.len(), 2880, "60 ms at 48 kHz");
    let strongest = filters
        .left
        .iter()
        .enumerate()
        .fold((0, 0f32), |best, (index, &value)| {
            if value.abs() > best.1.abs() {
                (index, value)
            } else {
                best
            }
        });
    assert_eq!(strongest.0, 48, "the lead-in is 1 ms");
    assert!(
        (strongest.1 - 1.0).abs() < 1e-3,
        "unit gain: {}",
        strongest.1
    );
    for (left, right) in filters.left.iter().zip(&filters.right) {
        assert!((left - right).abs() < 1e-6);
    }
}

// ── At work ──────────────────────────────────────────────────────────

fn shipped_head(sample_rate: f32) -> DiffuseField {
    let path = crate::pipeline::PipelineParams::default().hrtf_path;
    let sofa = sofar::reader::OpenOptions::new()
        .sample_rate(sample_rate)
        .open(&path)
        .expect("the shipped HRIRs");
    DiffuseField::new(&path, &sofa, sample_rate)
}

/// When nothing ambient plays, nothing is computed and nothing is added;
/// after the last ambient callback the filters ring out over their 60 ms
/// and then stop.
#[test]
fn silence_costs_nothing_and_a_sound_rings_out_then_stops() {
    let sample_rate = 48_000.0;
    let mut field = shipped_head(sample_rate);
    let frames = 512;
    let mut buffer = vec![0.25f32; frames * 2];
    let silent_bus = vec![0.0f32; frames];
    let render = |field: &mut DiffuseField, bus: &[f32], fed: bool, buffer: &mut [f32]| {
        let mut out = OutputBuffer {
            buffer,
            reverb_send: None,
            channels: 2,
            num_frames: frames,
            sample_rate,
        };
        field.render(bus, fed, &mut out);
    };
    render(&mut field, &silent_bus, false, &mut buffer);
    assert!(buffer.iter().all(|&sample| sample == 0.25), "nothing added");
    assert_eq!(field.ringing_frames, 0);

    let mut click_bus = vec![0.0f32; frames];
    click_bus[0] = 1.0;
    buffer.fill(0.0);
    render(&mut field, &click_bus, true, &mut buffer);
    assert!(buffer.iter().any(|&sample| sample != 0.0));
    let mut callbacks_ringing = 0;
    while field.ringing_frames > 0 {
        buffer.fill(0.0);
        render(&mut field, &silent_bus, false, &mut buffer);
        callbacks_ringing += 1;
    }
    // 2880 frames of filter after the first callback's 512: six more.
    assert_eq!(callbacks_ringing, 6);
    buffer.fill(0.25);
    render(&mut field, &silent_bus, false, &mut buffer);
    assert!(buffer.iter().all(|&sample| sample == 0.25), "rung out");
}
