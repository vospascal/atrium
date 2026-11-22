//! Levels: Leq, LAeq, LCeq, the level while it sounds (the BS.1770 gate),
//! LAFmax and the percentile levels, peak and crest.
//!
//! A preset's `reference_spl` is the sound's level at 1 m while it sounds: a
//! frog croaking, a robin singing. A field recording of a frog is mostly the
//! quiet between croaks, so its mean power over the whole file is far below
//! the croaks', and scaling the file to a target by that mean made the croaks
//! themselves 7 dB louder than the preset said.
//!
//! The level while it sounds is measured as broadcast loudness is (ITU-R
//! BS.1770-4, §§ 2.4 and 2.5): mean squares over 400 ms blocks that overlap
//! by 75 %, an absolute gate, then a relative gate 10 dB below the mean of the
//! blocks that passed it; the level is the mean of the blocks that pass both.
//! The recommendation's K-weighting is left out: `reference_spl` is a
//! physical sound pressure level, unweighted, not a perceived loudness.

use crate::report::Levels;
use crate::signal::Calibration;
use crate::statistics::{decibels_power, quantile};
use crate::weighting::TimeWeighted;

/// Length of a gating block and the step between blocks (s).
const BLOCK_SECONDS: f64 = 0.4;
const STEP_SECONDS: f64 = 0.1;
/// Blocks quieter than this are silence, whatever else plays (dB re full
/// scale, as power), when the file's calibration is unknown, as BS.1770.
pub(crate) const ABSOLUTE_GATE_DECIBELS: f64 = -70.0;
/// When it is known, blocks below the threshold of hearing are silence. A
/// render at the engine's own level puts a 42 dB SPL wind at −72 dBFS, below
/// the −70 dBFS gate; in dB SPL the gate is the same whatever gain
/// `--normalise` applied.
pub(crate) const HEARING_THRESHOLD_DB_SPL: f64 = 0.0;
/// Blocks this far below the mean of the blocks above the absolute gate are
/// the quiet between events (dB).
const RELATIVE_GATE_DECIBELS: f64 = -10.0;

/// The mean power of `takes`, each its samples and its sample rate, over the
/// blocks in which they sound. Each take is cut into its own blocks at its own
/// rate and the blocks of all takes are gated together, so a quiet take stays
/// quieter than a loud one. A take shorter than one block counts as one
/// block. Zero when nothing sounds. The absolute gate is BS.1770's −70 dBFS:
/// takes are recordings, at a recording's level.
pub fn power_while_sounding(takes: &[(&[f32], u32)]) -> f64 {
    let blocks: Vec<f64> = takes
        .iter()
        .flat_map(|&(take, sample_rate)| block_powers(take, sample_rate))
        .collect();
    gated_power(&blocks, decibels_power(ABSOLUTE_GATE_DECIBELS))
}

/// The mean squares of the gating blocks of `samples`: 400 ms every 100 ms,
/// or one block if the samples are shorter, none if there are none.
pub(crate) fn block_powers(samples: &[f32], sample_rate: u32) -> Vec<f64> {
    let block_length = ((BLOCK_SECONDS * sample_rate as f64).round() as usize).max(1);
    let step = ((STEP_SECONDS * sample_rate as f64).round() as usize).max(1);
    if samples.is_empty() {
        return Vec::new();
    }
    if samples.len() <= block_length {
        return vec![crate::statistics::mean_square(samples)];
    }
    (0..=(samples.len() - block_length) / step)
        .map(|block| {
            let start = block * step;
            crate::statistics::mean_square(&samples[start..start + block_length])
        })
        .collect()
}

/// BS.1770's two gates over block powers: the mean of the blocks above
/// `absolute_gate` and above 10 dB below the mean of those. Zero when no
/// block passes.
pub(crate) fn gated_power(blocks: &[f64], absolute_gate: f64) -> f64 {
    let above_silence: Vec<f64> = blocks
        .iter()
        .copied()
        .filter(|power| *power > absolute_gate)
        .collect();
    if above_silence.is_empty() {
        return 0.0;
    }
    let relative_gate =
        crate::statistics::mean(&above_silence) * decibels_power(RELATIVE_GATE_DECIBELS);
    let sounding: Vec<f64> = above_silence
        .into_iter()
        .filter(|power| *power > relative_gate)
        .collect();
    crate::statistics::mean(&sounding)
}

/// What one channel, or the channels together, is measured from: powers in
/// full-scale units (a signal of RMS 1 has power 1).
pub(crate) struct LevelInputs {
    pub(crate) mean_square: f64,
    pub(crate) a_weighted_mean_square: f64,
    pub(crate) c_weighted_mean_square: f64,
    pub(crate) blocks: Vec<f64>,
    pub(crate) fast: TimeWeighted,
    /// The largest sample magnitude.
    pub(crate) peak: f64,
}

/// The absolute gate, as a power in full-scale units, for a calibration.
pub(crate) fn absolute_gate(calibration: Calibration) -> f64 {
    if calibration.known {
        decibels_power(HEARING_THRESHOLD_DB_SPL - calibration.offset_decibels)
    } else {
        decibels_power(ABSOLUTE_GATE_DECIBELS)
    }
}

/// The levels of one channel (or of all together) in the report's unit.
pub(crate) fn levels(channel: &str, inputs: &LevelInputs, calibration: Calibration) -> Levels {
    let level = |power: f64| calibration.level(power);
    let exceeded = |percent: f64| {
        quantile(&inputs.fast.readings, 1.0 - percent / 100.0)
            .map(level)
            .unwrap_or_else(|| level(0.0))
    };
    let peak = level(inputs.peak * inputs.peak);
    let leq = level(inputs.mean_square);
    Levels {
        channel: channel.to_owned(),
        leq,
        laeq: level(inputs.a_weighted_mean_square),
        lceq: level(inputs.c_weighted_mean_square),
        while_sounding: level(gated_power(&inputs.blocks, absolute_gate(calibration))),
        lafmax: level(inputs.fast.maximum),
        l10: exceeded(10.0),
        l50: exceeded(50.0),
        l90: exceeded(90.0),
        peak,
        crest_decibels: peak - leq,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statistics::mean_square;
    use crate::test_signals::{sine, RATE};

    fn tone(amplitude: f32, seconds: f64) -> Vec<f32> {
        sine(1000.0, amplitude as f64, seconds)
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

    /// With the calibration known the gate is the threshold of hearing, so a
    /// sound at 42 dB SPL in a file whose full scale is 114 dB SPL (−72 dBFS,
    /// below BS.1770's −70) is still measured, at 42 dB SPL.
    #[test]
    fn a_quiet_render_passes_the_gate_in_sound_pressure() {
        let calibration = Calibration {
            offset_decibels: 114.0,
            known: true,
        };
        // RMS 10^(−72/20): amplitude √2 times that.
        let quiet = sine(1000.0, 2f64.sqrt() * 10f64.powf(-72.0 / 20.0), 3.0);
        let blocks = block_powers(&quiet, RATE);
        assert_eq!(
            gated_power(&blocks, decibels_power(ABSOLUTE_GATE_DECIBELS)),
            0.0
        );
        let level = calibration.level(gated_power(&blocks, absolute_gate(calibration)));
        assert!((level - 42.0).abs() < 0.01, "{level}");
    }
}
