//! The cutter's checks on synthetic signals (docs/cutter-plan.md, "How we
//! know it is right"). The checks that need a recording or the engine (the
//! frog, our soft wind, the preset in a scene, the round trip) are in the
//! engine's `src/offline/tests/cut.rs`.

use super::*;
use crate::test_signals::{Noise, RATE};

/// A sweep from `from` to `to` Hz over `seconds`, peak `amplitude`, with 2 ms
/// raised-cosine ramps.
fn sweep(from: f64, to: f64, seconds: f64, amplitude: f64) -> Vec<f32> {
    let length = (seconds * RATE as f64).round() as usize;
    let ramp = (0.002 * RATE as f64) as usize;
    let mut phase = 0.0f64;
    (0..length)
        .map(|index| {
            let frequency = from + (to - from) * index as f64 / length as f64;
            phase += std::f64::consts::TAU * frequency / RATE as f64;
            let edge = index.min(length - 1 - index);
            let gain = if edge < ramp {
                0.5 - 0.5 * (std::f64::consts::PI * edge as f64 / ramp as f64).cos()
            } else {
                1.0
            };
            (amplitude * gain * phase.sin()) as f32
        })
        .collect()
}

/// The peak amplitude of a sine `snr` dB above noise of RMS `noise_rms`.
fn amplitude(snr: f64, noise_rms: f64) -> f64 {
    2f64.sqrt() * noise_rms * 10f64.powf(snr / 20.0)
}

fn add(into: &mut [f32], at_seconds: f64, samples: &[f32]) {
    let start = (at_seconds * RATE as f64).round() as usize;
    for (slot, value) in into[start..].iter_mut().zip(samples) {
        *slot += value;
    }
}

fn mono(samples: &[f32]) -> Signal<'_> {
    Signal {
        interleaved: samples,
        channels: 1,
        sample_rate: RATE,
        channel_names: Vec::new(),
    }
}

/// A take's edges: its file less the fades.
fn edges(take: &Take) -> (f64, f64) {
    let fade = take.fade_frames as f64 / RATE as f64;
    (take.start_seconds + fade, take.end_seconds - fade)
}

const NOISE_RMS: f64 = 0.002;
const CHIRPS: usize = 20;
const CHIRP_SECONDS: f64 = 0.15;

/// 80 s of pink noise with 20 chirps (2 to 4 kHz, 150 ms) at `snr` dB over
/// the noise, 3.5 to 6.5 s apart. With `step`, the first 40 s, noise and
/// chirps, 11 dB louder: the frog's bed.
fn chirps_in_noise(snr: f64, step: bool) -> (Vec<f32>, Vec<f64>) {
    let mut samples = Noise::new(31).pink(NOISE_RMS, 80.0);
    let starts: Vec<f64> = (0..CHIRPS)
        .map(|index| 1.5 + 3.8 * index as f64 + 0.6 * ((index * 7) % 5) as f64)
        .collect();
    let chirp = sweep(2000.0, 4000.0, CHIRP_SECONDS, amplitude(snr, NOISE_RMS));
    for start in &starts {
        add(&mut samples, *start, &chirp);
    }
    if step {
        let gain = 10f32.powf(11.0 / 20.0);
        let half = samples.len() / 2;
        samples[..half]
            .iter_mut()
            .for_each(|sample| *sample *= gain);
    }
    (samples, starts)
}

/// Every chirp is one take, its edges within 10 ms of the chirp's start and
/// 20 ms of its end, and there is no other take. The worst errors are
/// returned, start and end (s).
fn every_chirp_found(cut: &Cut, starts: &[f64]) -> (f64, f64) {
    assert_eq!(
        cut.takes.len(),
        starts.len(),
        "takes at {:?}; dropped {:?}",
        cut.takes
            .iter()
            .map(|take| take.start_seconds)
            .collect::<Vec<_>>(),
        cut.dropped
    );
    let mut worst = (0.0f64, 0.0f64);
    for (take, start) in cut.takes.iter().zip(starts) {
        let (from, to) = edges(take);
        let (start_error, end_error) = (from - start, to - (start + CHIRP_SECONDS));
        assert!(
            start_error.abs() <= 0.010,
            "take {} starts {start_error:+.4} s off",
            take.number
        );
        assert!(
            end_error.abs() <= 0.020,
            "take {} ends {end_error:+.4} s off",
            take.number
        );
        worst.0 = worst.0.max(start_error.abs());
        worst.1 = worst.1.max(end_error.abs());
    }
    worst
}

/// 20 chirps in pink noise at 30, 20 and 12 dB over it: every one found,
/// starts within 10 ms, ends within 20 ms, no other takes. (Measured: see
/// the printed worst errors and docs/cutter-plan.md.)
#[test]
fn chirps_in_pink_noise_are_found() {
    for snr in [30.0, 20.0, 12.0] {
        let (samples, starts) = chirps_in_noise(snr, false);
        let cut = cut(&mono(&samples), &CutSettings::default());
        assert!(
            cut.band.low_hertz > 1500.0 && cut.band.high_hertz < 5000.0,
            "{:?}",
            cut.band.ranges
        );
        let (start, end) = every_chirp_found(&cut, &starts);
        println!(
            "SNR {snr} dB: band {:.0}-{:.0} Hz, worst start {:.1} ms, worst end {:.1} ms, criterion {:?}",
            cut.band.low_hertz,
            cut.band.high_hertz,
            1000.0 * start,
            1000.0 * end,
            cut.criterion.decision
        );
    }
}

/// The same chirps with the first 40 s 11 dB louder, noise and chirps: the
/// same takes, so the loud half is not one take.
#[test]
fn a_bed_that_steps_down_halfway() {
    for snr in [30.0, 12.0] {
        let (samples, starts) = chirps_in_noise(snr, true);
        let cut = cut(&mono(&samples), &CutSettings::default());
        let (start, end) = every_chirp_found(&cut, &starts);
        println!(
            "SNR {snr} dB, bed stepping 11 dB: worst start {:.1} ms, worst end {:.1} ms",
            1000.0 * start,
            1000.0 * end
        );
    }
}

/// The chirps at 2–4 kHz under a wind below 500 Hz that is 10 dB louder than
/// they are: the band follows the chirps, and every one is found.
#[test]
fn a_bird_under_wind() {
    let (mut samples, starts) = chirps_in_noise(30.0, false);
    let chirp_rms = amplitude(30.0, NOISE_RMS) / 2f64.sqrt();
    let wind = crate::spectrum::Spectrum::of(&Noise::new(43).pink(1.0, 80.0), RATE)
        .filtered(|frequency| f64::from(u8::from(frequency < 500.0)));
    let wind_rms = (wind
        .iter()
        .map(|sample| (*sample as f64).powi(2))
        .sum::<f64>()
        / wind.len() as f64)
        .sqrt();
    let gain = (chirp_rms * 10f64.powf(10.0 / 20.0) / wind_rms) as f32;
    for (sample, wind) in samples.iter_mut().zip(&wind) {
        *sample += gain * wind;
    }
    let cut = cut(&mono(&samples), &CutSettings::default());
    assert!(
        cut.band.low_hertz > 1500.0 && cut.band.high_hertz < 5000.0,
        "{:?}",
        cut.band.ranges
    );
    every_chirp_found(&cut, &starts);
}

/// A minute of pink noise has no takes.
#[test]
fn pink_noise_alone_has_no_takes() {
    let samples = Noise::new(8).pink(NOISE_RMS, 60.0);
    let cut = cut(&mono(&samples), &CutSettings::default());
    assert!(cut.takes.is_empty(), "{:?}", cut.takes);
    assert!(cut.dropped.is_empty(), "{:?}", cut.dropped);
    println!(
        "pink noise: {} events, band {:?}",
        cut.events.len(),
        cut.band.ranges
    );
}

/// A song: five 80 ms syllables (sweeps up from `from` Hz by a third) 60 ms
/// apart.
fn song(from: f64, peak: f64) -> Vec<f32> {
    let mut samples = Vec::new();
    for syllable in 0..5 {
        if syllable > 0 {
            samples.extend(std::iter::repeat_n(0.0, (0.06 * RATE as f64) as usize));
        }
        samples.extend(sweep(from, from * 1.33, 0.08, peak));
    }
    samples
}

/// Songs of five 80 ms syllables 60 ms apart, songs 3 to 8 s apart (20
/// songs, the 19 rests evenly spread over 3–8 s): one take per song, the
/// criterion between 0.06 and 3 s, and the two fitted processes within 10 %
/// of the rates drawn (16.7 a second inside a song, 1 / 5.5 s between).
#[test]
fn syllables_join_into_songs() {
    let rests: Vec<f64> = (0..19)
        .map(|index| 3.0 + 5.0 * ((index * 7) % 19) as f64 / 18.0)
        .collect();
    let one_song = song(2500.0, amplitude(30.0, NOISE_RMS));
    let song_seconds = one_song.len() as f64 / RATE as f64;
    let seconds = 2.0 + 20.0 * song_seconds + rests.iter().sum::<f64>() + 2.0;
    let mut samples = Noise::new(12).pink(NOISE_RMS, seconds);
    let mut at = 2.0;
    let mut starts = Vec::new();
    for index in 0..20 {
        add(&mut samples, at, &one_song);
        starts.push(at);
        at += song_seconds + rests.get(index).copied().unwrap_or(0.0);
    }
    let cut = cut(&mono(&samples), &CutSettings::default());
    assert_eq!(cut.takes.len(), 20, "{}", cut_summary("songs", &cut));
    for (take, start) in cut.takes.iter().zip(&starts) {
        assert_eq!(take.events, 5, "take {}", take.number);
        assert!(
            (edges(take).0 - start).abs() <= 0.010,
            "take {} at {:?} for {start}",
            take.number,
            edges(take)
        );
    }
    let criterion = cut.criterion.criterion_seconds.expect("two processes");
    assert!(criterion > 0.06 && criterion < 3.0, "{criterion}");
    let two = cut.criterion.two.expect("fitted");
    let drawn_fast = 1.0 / 0.06;
    let drawn_slow = 1.0 / 5.5;
    println!(
        "criterion {criterion:.3} s; fast {:.2}/s against {drawn_fast:.2} ({:+.1} %), slow {:.4}/s against {drawn_slow:.4} ({:+.1} %)",
        two.fast.rate,
        100.0 * (two.fast.rate / drawn_fast - 1.0),
        two.slow.rate,
        100.0 * (two.slow.rate / drawn_slow - 1.0)
    );
    assert!((two.fast.rate / drawn_fast - 1.0).abs() < 0.1, "{two:?}");
    assert!((two.slow.rate / drawn_slow - 1.0).abs() < 0.1, "{two:?}");
}

/// A 1 kHz tone burst in noise, cut: each take's first and last 5 ms (its
/// fades) hold none of the burst, only the bed, and no sample step at the
/// take's edges is larger than the recording's own largest step.
#[test]
fn a_cut_take_does_not_click() {
    let mut samples = Noise::new(17).pink(NOISE_RMS, 30.0);
    let burst: Vec<f32> = (0..(0.3 * RATE as f64) as usize)
        .map(|index| {
            (amplitude(30.0, NOISE_RMS)
                * (std::f64::consts::TAU * 1000.0 * index as f64 / RATE as f64).sin())
                as f32
        })
        .collect();
    let starts = [5.0, 12.5, 21.0];
    for start in starts {
        add(&mut samples, start, &burst);
    }
    let signal = mono(&samples);
    let cut = cut(&signal, &CutSettings::default());
    assert_eq!(cut.takes.len(), 3);
    let own_largest_step = samples
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0f32, f32::max);
    for (take, start) in cut.takes.iter().zip(starts) {
        let fade = take.fade_frames as f64 / RATE as f64;
        // The fades lie before the burst starts and after it ends.
        assert!(take.start_seconds + fade <= start, "{take:?}");
        assert!(take.end_seconds - fade >= start + 0.3, "{take:?}");
        let audio = take.samples(&signal);
        assert_eq!((audio[0], audio[audio.len() - 1]), (0.0, 0.0));
        let fade_frames = take.fade_frames;
        let edge_step = audio[..=fade_frames]
            .windows(2)
            .chain(audio[audio.len() - fade_frames - 1..].windows(2))
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            edge_step <= own_largest_step,
            "{edge_step} against {own_largest_step}"
        );
        // Bed level in the fades: the band's power there over the bed.
        println!(
            "take {}: fades from {:.1} ms before the burst to {:.1} ms after it; largest step at the edges {:.4} against the recording's {:.4}",
            take.number,
            1000.0 * (start - take.start_seconds - fade),
            1000.0 * (take.end_seconds - fade - start - 0.3),
            edge_step,
            own_largest_step
        );
    }
}

/// Two song shapes in the order A A A B B A A: two types, and bouts of 2 to 3.
/// Each song is sung with its own pitch (±3 %) and in its own noise, so the
/// similarity of two of one shape is less than 1. Prints the matrix, which is
/// what the provisional type threshold was set from.
#[test]
fn two_song_shapes_make_two_types_and_bouts() {
    let order = "AAABBAA";
    let mut samples = Noise::new(5).pink(NOISE_RMS, 2.0 + 5.0 * order.len() as f64);
    let peak = amplitude(30.0, NOISE_RMS);
    for (index, shape) in order.chars().enumerate() {
        let wobble = 1.0 + 0.03 * (((index * 5) % 7) as f64 / 3.0 - 1.0);
        let song = match shape {
            // Five rising syllables.
            'A' => song(2500.0 * wobble, peak),
            // Three long falling ones.
            _ => {
                let mut samples = Vec::new();
                for syllable in 0..3 {
                    if syllable > 0 {
                        samples.extend(std::iter::repeat_n(0.0, (0.06 * RATE as f64) as usize));
                    }
                    samples.extend(sweep(4200.0 * wobble, 2600.0 * wobble, 0.2, peak));
                }
                samples
            }
        };
        add(
            &mut samples,
            2.0 + 5.0 * index as f64 + 0.3 * (index % 3) as f64,
            &song,
        );
    }
    let cut = cut(&mono(&samples), &CutSettings::default());
    let text = cut_summary("two shapes", &cut);
    println!("{text}");
    let types: String = cut
        .takes
        .iter()
        .map(|take| take.type_name.clone())
        .collect();
    assert_eq!(types, "AAABBAA", "{text}");
    // The threshold lies between the two kinds of pair.
    let (mut within, mut between) = (f64::INFINITY, f64::NEG_INFINITY);
    let shapes: Vec<char> = order.chars().collect();
    for first in 0..shapes.len() {
        for second in first + 1..shapes.len() {
            let value = cut.types.similarity[first][second];
            if shapes[first] == shapes[second] {
                within = within.min(value);
            } else {
                between = between.max(value);
            }
        }
    }
    println!("one shape at least {within:.2}, two shapes at most {between:.2}");
    assert!(between < types::TYPE_SIMILARITY && types::TYPE_SIMILARITY < within);
    assert_eq!((cut.bouts.shortest, cut.bouts.longest), (Some(2), Some(3)));
}
