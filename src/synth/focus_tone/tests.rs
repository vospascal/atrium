use std::f64::consts::TAU;

use atrium_core::commands::SynthParam;
use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;

use super::*;
use crate::synth::test_support::{
    amplitude_at, averaged_power_spectrum, largest_step, peak_frequency, render, ANALYSIS_FFT_SIZE,
};

fn tone(settings: FocusToneSettings) -> FocusToneSource {
    FocusToneSource::new(Vec3::ZERO, settings)
}

/// The tone at `pitch_hz` with the rotation held still.
fn still(pitch_hz: f32) -> FocusToneSettings {
    FocusToneSettings {
        pitch_hz,
        rotation_depth_db: 0.0,
        ..FocusToneSettings::default()
    }
}

/// The stacks `docs/focus-tones.md` lists for these pitches, and nothing
/// else: two measured on harmonance.com (333 and 666 Hz, the second at the
/// most of three octaves below), 528 Hz that its Harmonics page gives as its
/// example (down to 66 Hz), and 111 Hz whose octave, 55.5 Hz, falls below the
/// 60 Hz floor, so it plays alone.
const STACKS: [(f32, &[f64]); 4] = [
    (333.0, &[333.0, 166.5, 83.25]),
    (666.0, &[666.0, 333.0, 166.5, 83.25]),
    (528.0, &[528.0, 264.0, 132.0, 66.0]),
    (111.0, &[111.0]),
];

#[test]
fn the_stacks_have_the_lines_the_notes_list() {
    for (pitch_hz, lines) in STACKS {
        assert_eq!(line_count(pitch_hz, 60.0), lines.len(), "{pitch_hz} Hz");
    }
    // 222 Hz reaches 111 but not 55.5 Hz; 963 Hz stops at three octaves
    // below (120.375 Hz), though 60.19 Hz would still be above the floor.
    assert_eq!(line_count(222.0, 60.0), 2);
    assert_eq!(line_count(963.0, 60.0), 4);
    // Never more than four lines, and the pitch always plays.
    assert_eq!(line_count(5_000.0, 10.0), MOST_LINES);
    assert_eq!(line_count(50.0, 60.0), 1);
}

/// Each line, measured: the loudest component near each expected line of the
/// stack, one 20 s FFT (0.05 Hz bins) with the rotation still, sits on the
/// line; halfway between two octaves and one octave below the lowest line
/// there is nothing.
#[test]
fn each_line_sits_at_its_octave_and_nothing_else_sounds() {
    let sample_rate = 48_000.0;
    for (pitch_hz, lines) in STACKS {
        let mut source = tone(still(pitch_hz));
        let samples = render(21.0, sample_rate, |rate| source.next_sample(rate));
        let stretch = &samples[sample_rate as usize..];
        for &line in lines {
            let measured = peak_frequency(stretch, sample_rate, line * 0.9, line * 1.1);
            println!("{pitch_hz} Hz stack: line {line} Hz measured at {measured:.5} Hz");
            assert!(
                (measured - line).abs() < 0.01,
                "{pitch_hz} Hz stack: line {line} measured at {measured:.5} Hz"
            );
        }
        let lowest = *lines.last().unwrap();
        let quietest = lines
            .iter()
            .map(|&line| amplitude_at(stretch, sample_rate, line))
            .fold(f64::MAX, f64::min);
        for silent in lines.iter().map(|&line| line * 0.75).chain([lowest / 2.0]) {
            let stray = amplitude_at(stretch, sample_rate, silent);
            assert!(
                stray < 1e-4 * quietest,
                "{pitch_hz} Hz stack: {stray:.2e} at {silent} Hz"
            );
        }
    }
}

/// The level curve passes through harmonance.com's measured lines and
/// extends its end slopes: −13.3 dB per octave below 83.25 Hz, −7.0 above
/// 666 Hz; between, linear in octaves.
#[test]
fn the_level_curve_follows_the_measured_lines() {
    let cases = [
        (83.25, 0.0),
        (166.5, -13.3),
        (333.0, -23.8),
        (666.0, -30.8),
        (41.625, 13.3),
        // The low segment's slope carried down to the 60 Hz floor.
        (66.0, 13.3 * (83.25_f32 / 66.0).log2()),
        (1_332.0, -37.8),
        // Half an octave above 83.25 Hz: halfway along the first segment.
        (83.25 * std::f32::consts::SQRT_2, -6.65),
        (264.0, -13.3 - 10.5 * (264.0_f32 / 166.5).log2()),
    ];
    for (frequency_hz, expected_db) in cases {
        let level = line_level_db(frequency_hz);
        assert!(
            (level - expected_db).abs() < 1e-3,
            "{frequency_hz} Hz: {level:.4} dB, expected {expected_db:.4}"
        );
    }
}

/// Every line's amplitude, measured with the rotation still, is the
/// curve's level for its own frequency: the same 333 Hz line plays at the
/// same level in the 333 Hz stack and in the 666 Hz one. 20 s holds a whole
/// number of cycles of every line, so the windowed DFT reads each exactly.
#[test]
fn each_line_plays_at_the_curve_level_for_its_frequency() {
    let sample_rate = 48_000.0;
    for (pitch_hz, lines) in STACKS {
        let mut source = tone(still(pitch_hz));
        let samples = render(21.0, sample_rate, |rate| source.next_sample(rate));
        let stretch = &samples[sample_rate as usize..];
        for &line in lines {
            let measured_db = 20.0
                * (amplitude_at(stretch, sample_rate, line) / REFERENCE_AMPLITUDE as f64).log10();
            let expected_db = line_level_db(line as f32) as f64;
            println!(
                "{pitch_hz} Hz stack: {line} Hz at {measured_db:.3} dB re 83.25 Hz (curve {expected_db:.3})"
            );
            assert!(
                (measured_db - expected_db).abs() < 0.1,
                "{pitch_hz} Hz stack, {line} Hz: {measured_db:.3} dB against {expected_db:.3}"
            );
        }
    }
}

/// Each line's level over time, from the output: demodulated at the line's
/// frequency in blocks of 20 cycles of the lowest line. At 7992 Hz
/// (83.25 · 96) every line of the 333 Hz stack has a whole number of samples
/// per cycle, so a block holds whole cycles of all three and they do not leak
/// into one another.
fn line_envelopes_db(samples: &[f32], sample_rate: f32, lines: &[f64]) -> Vec<Vec<(f64, f64)>> {
    let block = (20.0 * sample_rate as f64 / lines.last().unwrap()).round() as usize;
    lines
        .iter()
        .map(|&line| {
            let step = TAU * line / sample_rate as f64;
            samples
                .chunks_exact(block)
                .enumerate()
                .map(|(block_index, chunk)| {
                    let (mut real, mut imaginary) = (0.0, 0.0);
                    for (offset, &sample) in chunk.iter().enumerate() {
                        let angle = step * (block_index * block + offset) as f64;
                        real += sample as f64 * angle.cos();
                        imaginary -= sample as f64 * angle.sin();
                    }
                    let amplitude =
                        2.0 * (real * real + imaginary * imaginary).sqrt() / block as f64;
                    let centre_seconds =
                        (block_index as f64 + 0.5) * block as f64 / sample_rate as f64;
                    (centre_seconds, 20.0 * amplitude.log10())
                })
                .collect()
        })
        .collect()
}

/// Times the envelope rises through its mean, interpolated between blocks.
fn upward_crossings(envelope: &[(f64, f64)]) -> Vec<f64> {
    let mean = envelope.iter().map(|(_, level)| level).sum::<f64>() / envelope.len() as f64;
    envelope
        .windows(2)
        .filter(|pair| pair[0].1 < mean && pair[1].1 >= mean)
        .map(|pair| {
            let fraction = (mean - pair[0].1) / (pair[1].1 - pair[0].1);
            pair[0].0 + fraction * (pair[1].0 - pair[0].0)
        })
        .collect()
}

/// The rotation, measured from the output of the default 333 Hz tone over
/// 96 s (four rotations): each line's level swings 8 dB from low to high,
/// once every 24 s, and the weight passes from the pitch downwards: each
/// line a third of a turn (8 s) after the one above it.
#[test]
fn the_emphasis_rotates_through_the_lines_at_its_period_and_depth() {
    let sample_rate = 83.25 * 96.0;
    let settings = FocusToneSettings::default();
    let lines = [333.0, 166.5, 83.25];
    let mut source = tone(settings);
    let samples = render(97.0, sample_rate, |rate| source.next_sample(rate));
    // From one second in, past the onset; times are from the start.
    let skip = sample_rate as usize;
    let envelopes = line_envelopes_db(&samples[skip..], sample_rate, &lines);
    let mut first_crossings = Vec::new();
    for (line, envelope) in lines.iter().zip(&envelopes) {
        let highest = envelope
            .iter()
            .map(|(_, level)| *level)
            .fold(f64::MIN, f64::max);
        let lowest = envelope
            .iter()
            .map(|(_, level)| *level)
            .fold(f64::MAX, f64::min);
        let crossings = upward_crossings(envelope);
        let period = (crossings.last().unwrap() - crossings[0]) / (crossings.len() - 1) as f64;
        println!(
            "{line} Hz: swings {:.3} dB, period {period:.3} s over {} crossings",
            highest - lowest,
            crossings.len()
        );
        assert!(
            (highest - lowest - 8.0).abs() < 0.05,
            "{line} Hz swings {:.3} dB",
            highest - lowest
        );
        assert!(
            (period - 24.0).abs() < 0.05,
            "{line} Hz period {period:.3} s"
        );
        first_crossings.push(crossings[0] + skip as f64 / sample_rate as f64);
    }
    // Line index i is behind line 0 by i/3 of a turn: it rises through its
    // mean 8 · i s later.
    for (index, crossing) in first_crossings.iter().enumerate().skip(1) {
        let lag = (crossing - first_crossings[0]).rem_euclid(24.0);
        let expected = 8.0 * index as f64;
        println!("line {index} rises {lag:.3} s after line 0");
        assert!((lag - expected).abs() < 0.1, "line {index}: {lag:.3} s");
    }
}

/// A different period and depth, set live, are what the output then does.
#[test]
fn a_new_period_and_depth_apply_while_it_plays() {
    let sample_rate = 83.25 * 96.0;
    let mut source = tone(FocusToneSettings::default());
    render(2.0, sample_rate, |rate| source.next_sample(rate));
    source.set_synth_param(SynthParam::RotationPeriod, 6.0);
    source.set_synth_param(SynthParam::RotationDepth, 12.0);
    let samples = render(25.0, sample_rate, |rate| source.next_sample(rate));
    let envelopes = line_envelopes_db(&samples[..], sample_rate, &[333.0, 166.5, 83.25]);
    let lowest_line = &envelopes[2];
    let highest = lowest_line
        .iter()
        .map(|(_, level)| *level)
        .fold(f64::MIN, f64::max);
    let lowest = lowest_line
        .iter()
        .map(|(_, level)| *level)
        .fold(f64::MAX, f64::min);
    let crossings = upward_crossings(lowest_line);
    let period = (crossings.last().unwrap() - crossings[0]) / (crossings.len() - 1) as f64;
    println!(
        "set live: swings {:.3} dB, period {period:.3} s",
        highest - lowest
    );
    // Blocks of 0.24 s average the swing a little at a 6 s period:
    // sinc(0.04) loses 0.03 % of it.
    assert!((highest - lowest - 12.0).abs() < 0.1);
    assert!((period - 6.0).abs() < 0.05);
}

/// A high stack at a low rate: at 8 kHz the ceiling is 3.8 kHz, so a 5 kHz
/// pitch plays no top line, only its three octaves below; 2.5 kHz sits below
/// the fade (3.42 kHz) and sounds in full. Every bin away from the lines that may sound is more
/// than 100 dB below the total.
#[test]
fn nothing_is_made_at_or_above_the_ceiling() {
    let sample_rate = 8_000.0;
    let mut source = tone(still(5_000.0));
    let samples = render(3.0, sample_rate, |rate| source.next_sample(rate));
    let stretch = &samples[sample_rate as usize..];
    let spectrum = averaged_power_spectrum(stretch);
    let bin_hz = sample_rate as f64 / ANALYSIS_FFT_SIZE as f64;
    let allowed = [2_500.0, 1_250.0, 625.0];
    let total: f64 = spectrum.iter().sum();
    let stray: f64 = spectrum
        .iter()
        .enumerate()
        .filter(|(bin, _)| {
            let centre = *bin as f64 * bin_hz;
            allowed
                .iter()
                .all(|frequency: &f64| (centre - frequency).abs() > 40.0 * bin_hz)
        })
        .map(|(_, power)| power)
        .sum();
    let stray_db = 10.0 * (stray / total).log10();
    println!("5 kHz at 8 kHz: stray power {stray_db:.1} dB");
    assert!(stray_db < -100.0, "stray power {stray_db:.1} dB");
    // 5 kHz would fold to 3 kHz: nothing there.
    assert!(amplitude_at(stretch, sample_rate, 3_000.0) < 1e-6);
    let measured = amplitude_at(stretch, sample_rate, 2_500.0);
    let expected = REFERENCE_AMPLITUDE as f64 * 10.0_f64.powf(line_level_db(2_500.0) as f64 / 20.0);
    assert!((20.0 * (measured / expected).log10()).abs() < 0.05);
}

/// The same settings give the same samples; other settings do not.
#[test]
fn the_same_settings_give_the_same_samples() {
    let render_settings = |settings: FocusToneSettings| {
        let mut source = tone(settings);
        render(5.0, 48_000.0, |rate| source.next_sample(rate))
    };
    let first = render_settings(FocusToneSettings::default());
    assert_eq!(first, render_settings(FocusToneSettings::default()));
    assert_ne!(first, render_settings(still(333.0)));
}

/// It starts from silence and fades in over half a second without a step
/// larger than the steady tone's own over the same half second: the same
/// tone with its fade already done, rotating in step with it.
#[test]
fn it_starts_without_a_click() {
    let sample_rate = 48_000.0;
    let mut source = tone(FocusToneSettings::default());
    let mut steady_source = tone(FocusToneSettings::default());
    steady_source.onset_elapsed_seconds = ONSET_SECONDS;
    let onset_samples = (0.5 * sample_rate) as usize;
    let samples = render(0.5, sample_rate, |rate| source.next_sample(rate));
    let steady_samples = render(0.5, sample_rate, |rate| steady_source.next_sample(rate));
    assert_eq!(samples[0], 0.0);
    let onset = largest_step(&samples[..onset_samples]);
    let steady = largest_step(&steady_samples[..onset_samples]);
    assert!(onset <= steady, "onset step {onset}, steady {steady}");
}

/// A change while it plays glides: the largest step across the change is no
/// larger than the larger of the two steady tones' own over the same
/// stretch. The rotation does not depend on the pitch, the floor, the depth
/// or the gain, so both references rotate in step with the changed tone; for
/// a new period the reference after is started where it meets it.
/// The pitch changes include ones that add a line (333 to 666 and to 528 Hz),
/// take two away (333 to 111 Hz) and keep the count (333 to 444 Hz).
#[test]
fn a_change_while_it_plays_steps_no_more_than_the_tone_itself() {
    let sample_rate = 48_000.0;
    let changes = [
        (SynthParam::Pitch, 666.0),
        (SynthParam::Pitch, 528.0),
        (SynthParam::Pitch, 111.0),
        (SynthParam::Pitch, 444.0),
        (SynthParam::Pitch, 1_111.0),
        (SynthParam::Floor, 40.0),
        (SynthParam::Floor, 200.0),
        (SynthParam::RotationDepth, 24.0),
        (SynthParam::RotationPeriod, 2.0),
        (SynthParam::MasterGain, 2.0),
    ];
    let change_at = 2 * sample_rate as usize;
    let window = change_at - 4_800..change_at + 24_000;
    for (param, value) in changes {
        let mut changed = tone(FocusToneSettings::default());
        let mut before = tone(FocusToneSettings::default());
        let mut after = tone(FocusToneSettings::default());
        after.set_synth_param(param, value);
        after.settle();
        // A new period keeps the rotation where it is and turns it faster
        // from there: start the reference so that it reaches the same place
        // at the change, and it turns in step with the changed tone after.
        let change_seconds = change_at as f64 / sample_rate as f64;
        after.rotation_phase = (change_seconds / before.settings.rotation_period_seconds as f64
            - change_seconds / after.settings.rotation_period_seconds as f64)
            .rem_euclid(1.0);
        let mut samples = Vec::new();
        let mut reference_before = Vec::new();
        let mut reference_after = Vec::new();
        for index in 0..change_at + sample_rate as usize {
            if index == change_at {
                changed.set_synth_param(param, value);
            }
            samples.push(changed.next_sample(sample_rate));
            reference_before.push(before.next_sample(sample_rate));
            reference_after.push(after.next_sample(sample_rate));
        }
        let step = largest_step(&samples[window.clone()]);
        let own = largest_step(&reference_before[window.clone()])
            .max(largest_step(&reference_after[window.clone()]));
        println!("{param:?} to {value}: step {step:.6}, the tone's own {own:.6}");
        assert!(
            step <= own * 1.02,
            "{param:?} to {value}: step {step:.6}, the tone's own {own:.6}"
        );
    }
}

/// The step test above would catch a click: the same gain change applied at
/// once, on a sample near the crest, steps far more than the tone does.
#[test]
fn without_the_glide_a_gain_change_would_step() {
    let sample_rate = 48_000.0;
    let mut source = tone(still(333.0));
    let samples = render(2.0, sample_rate, |rate| source.next_sample(rate));
    let own = largest_step(&samples[sample_rate as usize..]);
    let crest = samples
        .iter()
        .fold(0.0_f32, |largest, sample| largest.max(*sample));
    let mut last = *samples.last().unwrap();
    while last < 0.8 * crest {
        last = source.next_sample(sample_rate);
    }
    source.set_synth_param(SynthParam::MasterGain, 2.0);
    source.settle();
    let jumped = source.next_sample(sample_rate);
    assert!(
        (jumped - last).abs() > 2.0 * own,
        "jump {}, own {own}",
        (jumped - last).abs()
    );
}

/// A pitch held outside its range, a depth past 24 dB or a period under a
/// second is clamped, not refused; another generator's setting and a
/// non-finite value are ignored.
#[test]
fn settings_are_clamped_and_others_ignored() {
    let mut source = tone(FocusToneSettings::default());
    source.set_synth_param(SynthParam::Pitch, 1.0);
    source.set_synth_param(SynthParam::Floor, 1e6);
    source.set_synth_param(SynthParam::RotationDepth, 90.0);
    source.set_synth_param(SynthParam::RotationPeriod, 0.0);
    source.set_synth_param(SynthParam::MasterGain, -1.0);
    assert_eq!(source.settings.pitch_hz, 40.0);
    assert_eq!(source.settings.floor_hz, 5_000.0);
    assert_eq!(source.settings.rotation_depth_db, 24.0);
    assert_eq!(source.settings.rotation_period_seconds, 1.0);
    assert_eq!(source.settings.master_gain, 0.0);
    assert_eq!(source.line_count(), 1);
    let before = source.settings;
    source.set_synth_param(SynthParam::GustStrength, 0.5);
    source.set_synth_param(SynthParam::Pitch, f32::NAN);
    assert_eq!(source.settings, before);
}
