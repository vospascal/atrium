//! The comparison's acceptance checks that need the engine or a recording
//! (docs/compare-plan.md, "How we know it is right"): the meadow's wind
//! against itself at another seed, against a blackbird take, and against
//! the soft-wind recording the hand comparison was made with.

use std::path::Path;

use atrium_analysis::{analyse, compare, Compared, Report, Settings, Signal, TextureClass};
use atrium_core::probe::Probe;

use super::{on_headphones, render, MEADOW};
use crate::audio::decode::decode_channels;
use crate::offline::{RenderSettings, Rendered};

/// A file's samples, rate and calibration.
struct Samples {
    interleaved: Vec<f32>,
    channels: usize,
    sample_rate: u32,
    full_scale_db_spl: Option<f64>,
}

impl Samples {
    fn signal(&self, first: usize, end: usize) -> Signal<'_> {
        Signal {
            interleaved: &self.interleaved[first * self.channels..end * self.channels],
            channels: self.channels,
            sample_rate: self.sample_rate,
            channel_names: Vec::new(),
        }
    }

    fn frames(&self) -> usize {
        self.interleaved.len() / self.channels
    }

    /// The report of frames `first` to `end`, and of its two halves.
    fn reports(&self, first: usize, end: usize) -> (Report, [Report; 2]) {
        let settings = Settings {
            full_scale_db_spl: self.full_scale_db_spl,
        };
        let middle = (first + end) / 2;
        (
            analyse(&self.signal(first, end), &settings),
            [
                analyse(&self.signal(first, middle), &settings),
                analyse(&self.signal(middle, end), &settings),
            ],
        )
    }
}

fn rendered(rendered: &Rendered) -> Samples {
    Samples {
        interleaved: rendered.frames().to_vec(),
        channels: rendered.channels(),
        sample_rate: rendered.sample_rate(),
        full_scale_db_spl: Some(rendered.full_scale_db_spl()),
    }
}

fn recording(path: &str) -> Samples {
    let decoded =
        decode_channels(Path::new(path)).unwrap_or_else(|error| panic!("{path}: {error}"));
    Samples {
        interleaved: decoded.interleaved,
        channels: decoded.channels,
        sample_rate: decoded.sample_rate,
        full_scale_db_spl: None,
    }
}

/// The meadow's wind alone, a minute on headphones, at `seed`.
fn wind(seed: u64) -> Samples {
    rendered(&render(
        MEADOW,
        &RenderSettings {
            seed: Some(seed),
            probe: Probe {
                direct: false,
                reflections: false,
                reverb: false,
                ambient: true,
                ..Probe::WHOLE
            },
            ..on_headphones(60.0)
        },
    ))
}

fn compared<'a>(name: &str, report: &'a Report, halves: Option<&'a [Report; 2]>) -> Compared<'a> {
    Compared {
        name: name.to_owned(),
        heading: name.to_owned(),
        report,
        halves: halves.map(|[first, second]| [first, second]),
        timeline: None,
        signal: None,
    }
}

/// Our wind at seed 1 against seed 2: every texture class at 20 dB or above,
/// the same generator. (Measured: 38.8 dB at the lowest, C; the wind follows
/// its slot's seed more than the session's, and the two renders differ by
/// −44 dB of their power, so this says less than the plan meant. The
/// generator's own randomness is the next check.)
#[test]
fn our_wind_at_two_seeds_has_one_texture() {
    let (first, second) = (wind(1), wind(2));
    let (reference, halves) = first.reports(0, first.frames());
    let (other, _) = second.reports(0, second.frames());
    let comparison = compare(
        &compared("seed 1", &reference, Some(&halves)),
        &[compared("seed 2", &other, None)],
    );
    for class in &comparison.others[0].texture.classes {
        assert!(class.converged, "{class:?}");
    }
}

/// The wind's first half against its second: the same generator, its
/// randomness truly different. Every class converged, or within the scatter
/// two takes of the wind reach by chance, measured from the first half's own
/// halves.
#[test]
fn our_wind_against_itself_later_differs_only_by_chance() {
    let samples = wind(1);
    let middle = samples.frames() / 2;
    let (reference, halves) = samples.reports(0, middle);
    let (other, _) = samples.reports(middle, samples.frames());
    let comparison = compare(
        &compared("first half", &reference, Some(&halves)),
        &[compared("second half", &other, None)],
    );
    for class in &comparison.others[0].texture.classes {
        assert!(class.converged || class.within_scatter, "{class:?}");
    }
}

/// Our wind against a blackbird take: most texture classes far below 20 dB
/// (here: below 10 dB) and beyond the wind's own scatter. (Measured: all
/// seven, from 4.1 dB, M1, down to −15.4, M2: the song's envelopes are
/// sparse, kurtosis 12.6 at 1.3 kHz against the wind's 2.7.)
#[test]
fn our_wind_against_a_blackbird() {
    let samples = wind(1);
    let (reference, halves) = samples.reports(0, samples.frames());
    let bird = recording("assets/birds/merel/merel-zang_1.mp3");
    let (other, _) = bird.reports(0, bird.frames());
    let comparison = compare(
        &compared("wind", &reference, Some(&halves)),
        &[compared("blackbird", &other, None)],
    );
    let far = comparison.others[0]
        .texture
        .classes
        .iter()
        .filter(|class| class.snr_decibels.is_some_and(|snr| snr < 10.0) && !class.within_scatter)
        .count();
    assert!(far >= 5, "{:#?}", comparison.others[0].texture.classes);
}

/// `soft wind.mp3`, its first 28 s, against our wind at seed 1: the
/// differences found by hand the day the comparison was planned, read from
/// the comparison alone. The colour within 3 dB from 200 Hz to 12.5 kHz and
/// the centroids within 5 % (5.9 and 6.0 kHz); the reference has more below
/// 100 Hz (25 to 63 Hz at least 5 dB more re Leq), more slow swelling
/// (modulation power at 0.5 Hz 2.7 % against 1.4 % of the envelope's
/// variance), bands that move together more (C at offset 1, 0.26 against
/// 0.13) and a wider level spread (L10 − L90 2.6 against 2.0 dB).
///
/// The recording is not in git (`research papers/` is ignored), so this
/// runs only when asked: `cargo test --release soft_wind -- --ignored`.
#[test]
#[ignore = "needs research papers/soft wind.mp3, which is not in git"]
fn soft_wind_against_ours_shows_the_hand_findings() {
    let reference_samples = recording("research papers/soft wind.mp3");
    let end = 28 * reference_samples.sample_rate as usize;
    let (reference, halves) = reference_samples.reports(0, end);
    let samples = wind(1);
    let (other, _) = samples.reports(0, samples.frames());
    let comparison = compare(
        &compared("soft wind.mp3@0-28", &reference, Some(&halves)),
        &[compared("our wind", &other, None)],
    );
    let found = &comparison.others[0];
    for band in &found.colour.bands {
        let difference = band.decibels.difference;
        match band.centre_hertz {
            centre if (25.0..=63.0).contains(&centre) => {
                assert!(difference <= -5.0, "{centre} Hz: {difference:+.1} dB")
            }
            centre if (200.0..=12_500.0).contains(&centre) => {
                assert!(difference.abs() <= 3.0, "{centre} Hz: {difference:+.1} dB")
            }
            _ => {}
        }
    }
    let ratio = found.colour.centroid_hertz.ratio.expect("a centroid");
    assert!((0.95..=1.05).contains(&ratio), "centroid ratio {ratio}");
    let slowest = found.texture.modulation_spectrum[0].values;
    assert!(
        (slowest.reference - 0.027).abs() < 0.003 && (slowest.other - 0.014).abs() < 0.003,
        "{slowest:?}"
    );
    let neighbours = found.texture.correlation_by_offset[0];
    assert_eq!(neighbours.offset, 1);
    assert!(
        (neighbours.values.reference - 0.26).abs() < 0.02
            && (neighbours.values.other - 0.13).abs() < 0.02,
        "{neighbours:?}"
    );
    let spread = found.level.l10_minus_l90;
    assert!(
        (spread.reference - 2.6).abs() < 0.1 && (spread.other - 2.0).abs() < 0.1,
        "{spread:?}"
    );
    // And the texture says the comodulation differs beyond chance.
    let correlation = found
        .texture
        .classes
        .iter()
        .find(|class| class.class == TextureClass::Correlation)
        .expect("every class");
    assert!(!correlation.converged && !correlation.within_scatter);
}
