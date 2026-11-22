//! The level of a recording while it sounds.
//!
//! A preset's `reference_spl` is the sound's level at 1 m while it sounds: a
//! frog croaking, a robin singing. A field recording of a frog is mostly the
//! quiet between croaks, so its mean power over the whole file is far below
//! the croaks', and scaling the file to a target by that mean made the croaks
//! themselves 7 dB louder than the preset said.
//!
//! The level is measured as broadcast loudness is (ITU-R BS.1770-4, §§ 2.4 and
//! 2.5): mean squares over 400 ms blocks that overlap by 75 %, an absolute gate
//! at −70 dB below full scale, then a relative gate 10 dB below the mean of
//! the blocks that passed it; the level is the mean of the blocks that pass
//! both. The recommendation's K-weighting is left out: `reference_spl` is a
//! physical sound pressure level, unweighted, not a perceived loudness.

/// Length of a gating block and the step between blocks (s).
const BLOCK_SECONDS: f64 = 0.4;
const STEP_SECONDS: f64 = 0.1;
/// Blocks quieter than this are silence, whatever else plays (dB re full
/// scale, as power).
const ABSOLUTE_GATE_DECIBELS: f64 = -70.0;
/// Blocks this far below the mean of the blocks above the absolute gate are
/// the quiet between events (dB).
const RELATIVE_GATE_DECIBELS: f64 = -10.0;

/// The mean power of `takes`, each its samples and its sample rate, over the
/// blocks in which they sound. Each take is cut into its own blocks at its own
/// rate and the blocks of all takes are gated together, so a quiet take stays
/// quieter than a loud one. A take shorter than one block counts as one
/// block. Zero when nothing sounds.
pub(super) fn power_while_sounding(takes: &[(&[f32], u32)]) -> f64 {
    let mut blocks = Vec::new();
    for &(take, sample_rate) in takes {
        let block_length = ((BLOCK_SECONDS * sample_rate as f64).round() as usize).max(1);
        let step = ((STEP_SECONDS * sample_rate as f64).round() as usize).max(1);
        if take.is_empty() {
            continue;
        }
        if take.len() <= block_length {
            blocks.push(mean_square(take));
            continue;
        }
        let mut start = 0;
        while start + block_length <= take.len() {
            blocks.push(mean_square(&take[start..start + block_length]));
            start += step;
        }
    }
    let absolute_gate = decibels_to_power(ABSOLUTE_GATE_DECIBELS);
    let above_silence: Vec<f64> = blocks
        .into_iter()
        .filter(|power| *power > absolute_gate)
        .collect();
    if above_silence.is_empty() {
        return 0.0;
    }
    let relative_gate = mean(&above_silence) * decibels_to_power(RELATIVE_GATE_DECIBELS);
    let sounding: Vec<f64> = above_silence
        .into_iter()
        .filter(|power| *power > relative_gate)
        .collect();
    mean(&sounding)
}

fn mean_square(samples: &[f32]) -> f64 {
    samples
        .iter()
        .map(|sample| (*sample as f64).powi(2))
        .sum::<f64>()
        / samples.len() as f64
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn decibels_to_power(decibels: f64) -> f64 {
    10f64.powf(decibels / 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn tone(amplitude: f32, seconds: f64) -> Vec<f32> {
        (0..(seconds * RATE as f64) as usize)
            .map(|index| {
                amplitude * (std::f32::consts::TAU * 1000.0 * index as f32 / RATE as f32).sin()
            })
            .collect()
    }

    fn close(value: f64, expected: f64, tolerance: f64) -> bool {
        (value - expected).abs() <= tolerance * expected
    }

    /// A steady sound is its own mean power: a sine of amplitude 0.5 has
    /// 0.5² / 2 = 0.125.
    #[test]
    fn a_steady_sound_is_its_mean_power() {
        let steady = tone(0.5, 3.0);
        assert!(close(power_while_sounding(&[(&steady, RATE)]), 0.125, 1e-3));
    }

    /// One second of a tone of power P followed by four seconds of digital
    /// silence. The blocks start every 100 ms and are 400 ms long, so seven
    /// lie wholly in the tone (starting at 0 to 600 ms), three overlap its end
    /// by 300, 200 and 100 ms (0.75, 0.5 and 0.25 P), and the rest are silent
    /// and fall below the absolute gate. The mean of the ten is 8.5 P / 10, and
    /// all ten lie above the relative gate at 0.085 P: the level is 0.85 P,
    /// where the mean over the whole file is P / 5. Before, this recording was
    /// played 6.3 dB louder than its preset said.
    #[test]
    fn silence_after_a_sound_does_not_count() {
        let mut recording = tone(0.5, 1.0);
        recording.extend(std::iter::repeat_n(0.0, 4 * RATE as usize));
        let level = power_while_sounding(&[(&recording, RATE)]);
        assert!(close(level, 0.85 * 0.125, 2e-3), "{level}");
        let whole_file = mean_square(&recording);
        assert!(close(whole_file, 0.125 / 5.0, 2e-3));
    }

    /// Five calls of 2 s with 6 s of background noise 30 dB below them in
    /// between. The background is above the absolute gate but far below the
    /// relative one, so only blocks with some call in them count. Per call, 17
    /// blocks lie wholly in it (starting at 0 to 1.6 s); 3 overlap its end by
    /// 300, 200 and 100 ms (0.75, 0.5 and 0.25 of its power) and, for the last
    /// four calls, 3 overlap its start the same way. So the level is
    /// (5 × 17 + 9 × 1.5) / (5 × 17 + 27) = 98.5 / 112 of the call's power,
    /// −0.557 dB, the background's share of the edge blocks aside.
    #[test]
    fn quiet_background_between_calls_does_not_count() {
        let mut recording = Vec::new();
        for _ in 0..5 {
            recording.extend(tone(0.5, 2.0));
            recording.extend(tone(0.5 * 10f32.powf(-30.0 / 20.0), 6.0));
        }
        let level = power_while_sounding(&[(&recording, RATE)]);
        assert!(close(level, 0.125 * 98.5 / 112.0, 2e-3), "{level}");
    }

    /// Takes are gated together: a take 6 dB quieter than another keeps its
    /// share of the mean, and does not count as silence.
    #[test]
    fn a_quieter_take_still_counts() {
        let loud = tone(0.5, 2.0);
        let quiet = tone(0.25, 2.0);
        let level = power_while_sounding(&[(&loud, RATE), (&quiet, RATE)]);
        assert!(close(level, (0.125 + 0.03125) / 2.0, 1e-3), "{level}");
    }

    /// Each take is cut at its own rate: the same second of tone at 24 and at
    /// 48 kHz has the same level.
    #[test]
    fn each_take_is_cut_at_its_own_rate() {
        let at_48 = tone(0.5, 1.0);
        let at_24: Vec<f32> = at_48.iter().step_by(2).copied().collect();
        let mixed = power_while_sounding(&[(&at_48, RATE), (&at_24, RATE / 2)]);
        let alone = power_while_sounding(&[(&at_48, RATE)]);
        assert!(close(mixed, alone, 1e-3), "{mixed} against {alone}");
    }

    /// A take shorter than one block is one block, and silence is zero.
    #[test]
    fn short_takes_and_silence() {
        let chirp = tone(0.5, 0.2);
        assert!(close(power_while_sounding(&[(&chirp, RATE)]), 0.125, 5e-3));
        let silence = vec![0.0f32; RATE as usize];
        assert_eq!(power_while_sounding(&[(&silence, RATE)]), 0.0);
        assert_eq!(power_while_sounding(&[]), 0.0);
    }
}
