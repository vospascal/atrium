//! The whole analysis of one signal: every channel transformed once, each
//! measure taken from what it needs, the channels combined, the report
//! assembled.

use crate::bands::{band_powers, third_octaves};
use crate::level::{block_powers, levels, LevelInputs};
use crate::onsets::onsets;
use crate::over_time::{over_time, Series};
use crate::report::{
    BandLevel, Colour, Distribution, Events, Level, LevelUnit, Report, SignalFacts, Space,
    REPORT_VERSION,
};
use crate::signal::{Calibration, Combination, Settings, Signal};
use crate::space::{binaural, octave_envelopes};
use crate::spectrum::Spectrum;
use crate::statistics::{mean_square, quantile};
use crate::texture::{average, channel_texture};
use crate::weighting::{a_weighting, c_weighting, fast};

/// The colour's centroid is taken over the audible range (Hz).
const CENTROID_LOW: f64 = 20.0;
const CENTROID_HIGH: f64 = 20_000.0;

/// Analyse `signal`: levels, colour, texture, events and, for two or more
/// channels, space.
///
/// # Panics
///
/// If the signal has no channel, no frame or a sample rate of zero.
pub fn analyse(signal: &Signal, settings: &Settings) -> Report {
    assert!(signal.channels > 0, "a signal needs at least one channel");
    assert!(signal.sample_rate > 0, "a signal needs a sample rate");
    assert!(signal.frames() > 0, "a signal needs at least one frame");
    let calibration = Calibration::of(settings);
    let combination = Combination::for_channels(signal.channels);
    let names = signal.names();
    let rate = signal.sample_rate;
    let nyquist = rate as f64 / 2.0;
    let frames = signal.frames();
    let third_octave_bands = third_octaves(nyquist);
    let pair = pair_of(&names);

    let mut channel_inputs = Vec::with_capacity(signal.channels);
    let mut channel_samples = Vec::with_capacity(signal.channels);
    let mut channel_band_powers = Vec::with_capacity(signal.channels);
    let mut channel_laf_steps = Vec::with_capacity(signal.channels);
    let mut textures = Vec::with_capacity(signal.channels);
    let mut pair_envelopes = (Vec::new(), Vec::new());
    let mut a_weighted_power = vec![0.0f64; frames];
    let (mut centroid_moment, mut centroid_power) = (0.0, 0.0);
    for index in 0..signal.channels {
        let samples = signal.channel(index);
        let spectrum = Spectrum::of(&samples, rate);
        let a_weighted: Vec<f64> = spectrum
            .filtered(a_weighting)
            .iter()
            .map(|sample| (*sample as f64).powi(2))
            .collect();
        for (sum, power) in a_weighted_power.iter_mut().zip(&a_weighted) {
            *sum += power;
        }
        let time_weighted = fast(&a_weighted, rate);
        channel_laf_steps.push(time_weighted.step_maxima.clone());
        channel_band_powers.push(band_powers(&spectrum, &third_octave_bands));
        let (moment, power) = spectrum.frequency_moments(CENTROID_LOW, CENTROID_HIGH);
        centroid_moment += moment;
        centroid_power += power;
        let channel_power = mean_square(&samples);
        textures.push((channel_texture(&spectrum), channel_power));
        if let Some((left, right)) = pair {
            if index == left {
                pair_envelopes.0 = octave_envelopes(&spectrum);
            } else if index == right {
                pair_envelopes.1 = octave_envelopes(&spectrum);
            }
        }
        channel_inputs.push(LevelInputs {
            mean_square: channel_power,
            a_weighted_mean_square: spectrum.mean_square(0.0, f64::INFINITY, a_weighting),
            c_weighted_mean_square: spectrum.mean_square(0.0, f64::INFINITY, c_weighting),
            blocks: block_powers(&samples, rate),
            fast: time_weighted,
            peak: samples
                .iter()
                .fold(0.0f64, |peak, sample| peak.max(sample.abs() as f64)),
        });
        channel_samples.push(samples);
    }
    let scale = combination.scale(signal.channels);
    a_weighted_power
        .iter_mut()
        .for_each(|power| *power *= scale);

    // The channels together.
    let combine = |value: &dyn Fn(&LevelInputs) -> f64| {
        combination.combine(&channel_inputs.iter().map(value).collect::<Vec<_>>())
    };
    let overall_fast = fast(&a_weighted_power, rate);
    let overall_inputs = LevelInputs {
        mean_square: combine(&|inputs| inputs.mean_square),
        a_weighted_mean_square: combine(&|inputs| inputs.a_weighted_mean_square),
        c_weighted_mean_square: combine(&|inputs| inputs.c_weighted_mean_square),
        blocks: (0..channel_inputs[0].blocks.len())
            .map(|block| combine(&|inputs| inputs.blocks[block]))
            .collect(),
        peak: channel_inputs
            .iter()
            .fold(0.0f64, |peak, inputs| peak.max(inputs.peak)),
        fast: overall_fast,
    };
    let overall = levels("overall", &overall_inputs, calibration);

    let third_octaves: Vec<BandLevel> = third_octave_bands
        .iter()
        .enumerate()
        .map(|(band_index, band)| {
            let power = combination.combine(
                &channel_band_powers
                    .iter()
                    .map(|powers| powers[band_index])
                    .collect::<Vec<_>>(),
            );
            let level = calibration.level(power);
            BandLevel {
                centre_hertz: band.nominal,
                level,
                relative_decibels: level - overall.leq,
            }
        })
        .collect();

    let onset_seconds = onsets(&channel_samples, rate);
    let intervals: Vec<f64> = onset_seconds
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect();
    let seconds = signal.seconds();
    let events = Events {
        onsets_per_minute: onset_seconds.len() as f64 * 60.0 / seconds,
        interval_median_seconds: quantile(&intervals, 0.5),
        interval_p10_seconds: quantile(&intervals, 0.1),
        interval_p90_seconds: quantile(&intervals, 0.9),
        onset_seconds,
    };

    let (pair_report, pair_over_time) = match pair {
        Some((left, right)) => {
            let (report, over) = binaural(
                (&names[left], &names[right]),
                &channel_samples[left],
                &channel_samples[right],
                rate,
                (&pair_envelopes.0, &pair_envelopes.1),
            );
            (Some(report), Some(over))
        }
        None => (None, None),
    };
    let distribution = distribution(&names, &channel_inputs, calibration);
    let space = (signal.channels > 1).then_some(Space {
        pair: pair_report,
        distribution,
    });

    let texture = if textures.len() == 1 {
        textures
            .pop()
            .map(|(texture, _)| texture)
            .expect("one texture")
    } else {
        average(&textures)
    };

    let over_time = over_time(&Series {
        calibration,
        sample_rate: rate,
        a_weighted_power: &a_weighted_power,
        laf_overall: &overall_inputs.fast.step_maxima,
        laf_channels: channel_laf_steps.iter().map(Vec::as_slice).collect(),
        onset_seconds: &events.onset_seconds,
        pair: pair_over_time.as_ref(),
    });

    Report {
        report_version: REPORT_VERSION,
        signal: SignalFacts {
            channel_names: names.clone(),
            sample_rate: rate,
            seconds,
            full_scale_db_spl: settings.full_scale_db_spl,
            combination: match combination {
                Combination::Single => "single",
                Combination::EarsMean => "ears_mean",
                Combination::RoomSum => "room_sum",
            }
            .to_owned(),
        },
        unit: if calibration.known {
            LevelUnit::SoundPressureLevel
        } else {
            LevelUnit::FullScale
        },
        level: Level {
            overall,
            channels: names
                .iter()
                .zip(&channel_inputs)
                .map(|(name, inputs)| levels(name, inputs, calibration))
                .collect(),
            absolute_gate: calibration.level(crate::level::absolute_gate(calibration)),
        },
        colour: Colour {
            third_octaves,
            centroid_hertz: if centroid_power > 0.0 {
                centroid_moment / centroid_power
            } else {
                0.0
            },
        },
        texture,
        events,
        space,
        over_time,
    }
}

/// The ears, or the L/R pair of a speaker layout.
fn pair_of(names: &[String]) -> Option<(usize, usize)> {
    if names.len() == 2 {
        return Some((0, 1));
    }
    let find = |wanted: &str| names.iter().position(|name| name == wanted);
    Some((find("L")?, find("R")?))
}

/// Front (L, C, R) against surround (Ls, Rs), for a layout that has both.
fn distribution(
    names: &[String],
    inputs: &[LevelInputs],
    calibration: Calibration,
) -> Option<Distribution> {
    let power = |wanted: &[&str]| -> Option<f64> {
        let found: Vec<f64> = names
            .iter()
            .zip(inputs)
            .filter(|(name, _)| wanted.contains(&name.as_str()))
            .map(|(_, inputs)| inputs.mean_square)
            .collect();
        (!found.is_empty()).then(|| found.iter().sum())
    };
    let front = calibration.level(power(&["L", "C", "R"])?);
    let surround = calibration.level(power(&["Ls", "Rs"])?);
    Some(Distribution {
        front,
        surround,
        front_minus_surround_decibels: front - surround,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signals::{interleave, sine, Noise, RATE};

    /// 5 s at −20 dBFS and 5 s at −40, repeated for 60 s (a 1 kHz sine, where
    /// A-weighting is 0 dB): L10 = −20 and L90 = −40, each ± 0.5.
    #[test]
    fn percentile_levels_find_the_two_steps() {
        let loud = sine(1000.0, 0.1 * 2f64.sqrt(), 5.0);
        let quiet = sine(1000.0, 0.01 * 2f64.sqrt(), 5.0);
        let samples: Vec<f32> = (0..6)
            .flat_map(|_| loud.iter().chain(&quiet))
            .copied()
            .collect();
        let report = analyse(
            &Signal {
                interleaved: &samples,
                channels: 1,
                sample_rate: RATE,
                channel_names: Vec::new(),
            },
            &Settings::default(),
        );
        let overall = &report.level.overall;
        assert!((overall.l10 + 20.0).abs() <= 0.5, "L10 {}", overall.l10);
        assert!((overall.l90 + 40.0).abs() <= 0.5, "L90 {}", overall.l90);
        assert!(report.space.is_none());
        assert_eq!(report.unit, LevelUnit::FullScale);
    }

    /// A calibrated stereo file: the same sine at both ears reads its level
    /// in dB SPL at each and overall (the ears are averaged, not summed),
    /// and the report survives a round trip through JSON.
    #[test]
    fn ears_are_averaged_and_levels_are_calibrated() {
        let tone = sine(1000.0, 0.1 * 2f64.sqrt(), 3.0);
        let samples = interleave(&[&tone, &tone]);
        let report = analyse(
            &Signal {
                interleaved: &samples,
                channels: 2,
                sample_rate: RATE,
                channel_names: Vec::new(),
            },
            &Settings {
                full_scale_db_spl: Some(114.0),
            },
        );
        // −20 dBFS + 114 = 94 dB SPL.
        assert!((report.level.overall.leq - 94.0).abs() < 0.01);
        assert!((report.level.overall.while_sounding - 94.0).abs() < 0.01);
        assert!((report.level.channels[1].laeq - 94.0).abs() < 0.01);
        assert_eq!(report.unit, LevelUnit::SoundPressureLevel);
        let text = serde_json::to_string(&report).expect("serialises");
        let back: Report = serde_json::from_str(&text).expect("reads back");
        assert!((back.level.overall.leq - report.level.overall.leq).abs() < 1e-9);
        assert_eq!(back.texture.subbands.len(), 32);
        assert_eq!(
            back.colour.third_octaves.len(),
            report.colour.third_octaves.len()
        );
    }

    /// Six channels of noise, front three at twice the surround's power each
    /// (ignoring LFE, which is silent here): front − surround =
    /// 10·log10(3·2 / 2) = 4.77 dB.
    #[test]
    fn five_one_front_against_surround() {
        let mut noise = Noise::new(12);
        let front = noise.white(0.1 * 2f64.sqrt(), 2.0);
        let centre = noise.white(0.1 * 2f64.sqrt(), 2.0);
        let right = noise.white(0.1 * 2f64.sqrt(), 2.0);
        let low = vec![0.0f32; front.len()];
        let left_surround = noise.white(0.1, 2.0);
        let right_surround = noise.white(0.1, 2.0);
        let samples = interleave(&[
            &front,
            &right,
            &centre,
            &low,
            &left_surround,
            &right_surround,
        ]);
        let report = analyse(
            &Signal {
                interleaved: &samples,
                channels: 6,
                sample_rate: RATE,
                channel_names: Vec::new(),
            },
            &Settings::default(),
        );
        let space = report.space.expect("six channels have space");
        let difference = space
            .distribution
            .expect("Ls and Rs")
            .front_minus_surround_decibels;
        assert!((difference - 4.77).abs() < 0.1, "{difference}");
        assert!(space.pair.is_some());
    }
}
