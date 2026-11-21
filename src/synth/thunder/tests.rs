use super::channel::{build_channel, MAX_PULSES};
use super::kinds::kind;
use super::*;
use crate::synth::test_support::{averaged_power_spectrum, band_power, render};

const SAMPLE_RATE: f32 = 48_000.0;
const KIND_NUMBERS: [u8; 4] = [KIND_CRACK, KIND_CLAP, KIND_ROLLING, KIND_DISTANT];

fn strike(take: u8, at_sample: u64) -> Utterance {
    Utterance {
        take,
        at_sample,
        gain: 1.0,
        rate: 1.0,
        skip_seconds: 0.0,
    }
}

fn one_clap(take: u8, seed: u64) -> Vec<f32> {
    let mut thunder = ThunderSource::new(Vec3::ZERO, seed);
    thunder.schedule(strike(take, 0));
    render(clap_seconds(take) + 1.0, SAMPLE_RATE, |rate| {
        thunder.next_sample(rate)
    })
}

fn energy(samples: &[f32]) -> f64 {
    samples.iter().map(|sample| (*sample as f64).powi(2)).sum()
}

/// The share of a stretch's energy above `hertz`.
fn share_above(samples: &[f32], hertz: f32) -> f64 {
    let spectrum = averaged_power_spectrum(samples);
    band_power(&spectrum, SAMPLE_RATE, hertz, 20_000.0)
        / band_power(&spectrum, SAMPLE_RATE, 1.0, 20_000.0)
}

#[test]
fn it_is_silent_until_told_to_strike() {
    let mut thunder = ThunderSource::new(Vec3::ZERO, 1);
    let samples = render(2.0, SAMPLE_RATE, |rate| thunder.next_sample(rate));
    assert!(samples.iter().all(|sample| *sample == 0.0));
    assert_eq!(thunder.sounding_take(), None);
}

#[test]
fn a_strike_sounds_at_its_sample_and_ends_within_its_clap_length() {
    for take in KIND_NUMBERS {
        let mut thunder = ThunderSource::new(Vec3::ZERO, 5 + take as u64);
        let at = 4_800;
        thunder.schedule(strike(take, at));
        let samples = render(clap_seconds(take) + 0.3, SAMPLE_RATE, |rate| {
            thunder.next_sample(rate)
        });
        assert!(
            samples[..at as usize].iter().all(|sample| *sample == 0.0),
            "kind {take}: silent before"
        );
        assert!(
            energy(&samples[at as usize..at as usize + 9_600]) > 0.0,
            "kind {take}: heard at once"
        );
        assert_eq!(
            thunder.sounding_take(),
            None,
            "kind {take}: over within its length"
        );
    }
}

#[test]
fn rate_two_ends_in_half_the_time() {
    let mut thunder = ThunderSource::new(Vec3::ZERO, 3);
    thunder.schedule(Utterance {
        rate: 2.0,
        ..strike(KIND_ROLLING, 0)
    });
    render(
        clap_seconds(KIND_ROLLING) / 2.0 + 0.2,
        SAMPLE_RATE,
        |rate| thunder.next_sample(rate),
    );
    assert_eq!(thunder.sounding_take(), None);
}

#[test]
fn appointments_follow_the_sessions_clock_in_time_order() {
    let mut thunder = ThunderSource::new(Vec3::ZERO, 4);
    thunder.set_clock(100_000);
    thunder.schedule(strike(KIND_DISTANT, 100_480));
    thunder.schedule(strike(KIND_CRACK, 0));
    thunder.next_sample(SAMPLE_RATE);
    assert_eq!(
        thunder.sounding_take(),
        Some(KIND_CRACK),
        "a moment already past is kept at once"
    );
    render(0.02, SAMPLE_RATE, |rate| thunder.next_sample(rate));
    assert_eq!(
        thunder.sounding_take(),
        Some(KIND_DISTANT),
        "then the later one"
    );
}

#[test]
fn at_most_two_claps_sound_at_once() {
    let mut thunder = ThunderSource::new(Vec3::ZERO, 8);
    for at in [0, 10, 20] {
        thunder.schedule(strike(KIND_ROLLING, at));
    }
    render(0.05, SAMPLE_RATE, |rate| thunder.next_sample(rate));
    assert_eq!(
        thunder.sounding_claps(),
        2,
        "the third took the oldest's place"
    );
}

/// Near thunder keeps its crack, far thunder is a low grumble, and every
/// clap darkens as it goes on: its later parts come from farther away.
#[test]
fn near_is_bright_far_is_dull_and_a_clap_darkens() {
    let crack = one_clap(KIND_CRACK, 2);
    let distant = one_clap(KIND_DISTANT, 2);
    assert!(share_above(&crack, 1_000.0) > 3.0 * share_above(&distant, 1_000.0));
    for seed in [3, 4, 5] {
        let clap = one_clap(KIND_CLAP, seed);
        let second = SAMPLE_RATE as usize;
        let early = &clap[..2 * second];
        let late = &clap[3 * second..7 * second];
        assert!(
            share_above(early, 400.0) > share_above(late, 400.0),
            "seed {seed}: the tail is darker"
        );
    }
}

/// Like the reference recording, a clap a few kilometres off has most of
/// its energy low: centred well under 500 Hz, little above 2 kHz.
#[test]
fn a_clap_sounds_low_like_the_recording() {
    for seed in [6, 7] {
        let clap = one_clap(KIND_CLAP, seed);
        let spectrum = averaged_power_spectrum(&clap);
        let total = band_power(&spectrum, SAMPLE_RATE, 20.0, 20_000.0);
        let low = band_power(&spectrum, SAMPLE_RATE, 20.0, 500.0);
        assert!(
            low / total > 0.75,
            "seed {seed}: {:.2} of it under 500 Hz",
            low / total
        );
        assert!(share_above(&clap, 2_000.0) < 0.03);
    }
}

/// A cloud flash rolls on long after a crack is over.
#[test]
fn rolling_thunder_rolls_on_long_after_a_crack() {
    let second = SAMPLE_RATE as usize;
    for seed in [1, 2] {
        let crack = one_clap(KIND_CRACK, seed);
        let rolling = one_clap(KIND_ROLLING, seed);
        let tail = |samples: &[f32]| energy(&samples[6 * second..]) / energy(samples);
        assert!(tail(&rolling) > 4.0 * tail(&crack), "seed {seed}");
    }
}

/// The farther, the more of what arrives is the infrasonic swell.
#[test]
fn distant_thunder_is_more_infrasound_than_a_crack() {
    let infra = |samples: &[f32]| {
        let spectrum = averaged_power_spectrum(samples);
        band_power(&spectrum, SAMPLE_RATE, 1.0, 25.0)
            / band_power(&spectrum, SAMPLE_RATE, 1.0, 20_000.0)
    };
    assert!(infra(&one_clap(KIND_DISTANT, 3)) > 2.0 * infra(&one_clap(KIND_CRACK, 3)));
}

#[test]
fn claps_come_from_the_seed_and_never_repeat() {
    assert_eq!(one_clap(KIND_CLAP, 11), one_clap(KIND_CLAP, 11));
    assert_ne!(one_clap(KIND_CLAP, 11), one_clap(KIND_CLAP, 12));
    let mut thunder = ThunderSource::new(Vec3::ZERO, 13);
    thunder.schedule(strike(KIND_CRACK, 0));
    let first = render(clap_seconds(KIND_CRACK), SAMPLE_RATE, |rate| {
        thunder.next_sample(rate)
    });
    thunder.schedule(strike(KIND_CRACK, 0));
    let second = render(clap_seconds(KIND_CRACK), SAMPLE_RATE, |rate| {
        thunder.next_sample(rate)
    });
    assert_ne!(first, second, "a new flash every time");
}

#[test]
fn a_minute_of_thunder_is_finite() {
    let mut thunder = ThunderSource::new(Vec3::ZERO, 21);
    let mut at = 0;
    for (index, take) in KIND_NUMBERS.iter().cycle().take(12).enumerate() {
        thunder.schedule(Utterance {
            gain: 0.5 + index as f32 * 0.3,
            rate: 0.6 + (index % 4) as f32 * 0.5,
            ..strike(*take, at)
        });
        at += 220_000;
    }
    let samples = render(60.0, SAMPLE_RATE, |rate| thunder.next_sample(rate));
    assert!(samples.iter().all(|sample| sample.is_finite()));
}

#[test]
fn every_kind_peaks_under_one_at_every_sample_rate() {
    for sample_rate in [44_100.0, 48_000.0, 96_000.0] {
        for take in KIND_NUMBERS {
            for seed in [1, 2, 3] {
                let mut thunder = ThunderSource::new(Vec3::ZERO, seed);
                thunder.schedule(strike(take, 0));
                let samples = render(clap_seconds(take), sample_rate, |rate| {
                    thunder.next_sample(rate)
                });
                let peak = samples
                    .iter()
                    .fold(0f32, |peak, sample| peak.max(sample.abs()));
                assert!(
                    peak < 1.0,
                    "kind {take} at {sample_rate} Hz, seed {seed}: {peak}"
                );
                assert!(peak > 0.05, "kind {take}: heard at all");
            }
        }
    }
}

#[test]
fn master_gain_zero_silences_and_nan_is_ignored() {
    let mut thunder = ThunderSource::new(Vec3::ZERO, 2);
    thunder.set_synth_param(SynthParam::MasterGain, f32::NAN);
    assert_eq!(thunder.master_gain, 1.0);
    thunder.set_synth_param(SynthParam::MasterGain, 0.0);
    thunder.schedule(strike(KIND_CRACK, 0));
    let samples = render(1.0, SAMPLE_RATE, |rate| thunder.next_sample(rate));
    assert!(samples.iter().all(|sample| *sample == 0.0));
}

/// Moved into the middle of a clap, it carries on from there instead of
/// starting over.
#[test]
fn a_clap_can_start_partway() {
    let mut thunder = ThunderSource::new(Vec3::ZERO, 9);
    thunder.schedule(Utterance {
        skip_seconds: clap_seconds(KIND_ROLLING) - 0.5,
        ..strike(KIND_ROLLING, 0)
    });
    render(0.6, SAMPLE_RATE, |rate| thunder.next_sample(rate));
    assert_eq!(
        thunder.sounding_take(),
        None,
        "only its last half second was left"
    );
}

#[test]
fn a_channel_fits_its_storage_and_its_length() {
    let mut random = Rng::new(3);
    let mut pulses = Vec::with_capacity(MAX_PULSES);
    for take in KIND_NUMBERS {
        for _ in 0..5 {
            build_channel(kind(take), &mut random, &mut pulses);
            assert!(
                pulses.len() > 300 && pulses.len() <= MAX_PULSES,
                "kind {take}: {} pulses",
                pulses.len()
            );
            assert_eq!(pulses.capacity(), MAX_PULSES, "nothing grew");
            assert!(
                pulses.windows(2).all(|pair| pair[0].at <= pair[1].at),
                "in arrival order"
            );
            assert!(pulses
                .iter()
                .all(|pulse| pulse.at + pulse.lifetime() < clap_seconds(take)));
        }
    }
}
