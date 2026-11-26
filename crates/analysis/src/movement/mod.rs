//! How a file's tones and noise move: its partials and how each one's
//! level, pitch and interaural phase move; binaural beats; the slow
//! movement of each octave; the shape of the swells; a periodic pulse.
//! Measured for docs/tone-and-noise-movement.md, where what each tells and
//! why it is measured this way is set against the literature.
//!
//! Built in three steps so the orchestration transforms each channel once:
//! [`MovementBuilder::new`] does everything that needs only the samples
//! (the lines and their clusters, the slow levels, the swells, the pulse);
//! [`MovementBuilder::add_channel`] takes each channel's whole-file spectrum
//! as `analyse` makes it and reads each partial's band from it;
//! [`MovementBuilder::finish`] puts the report together.

mod baseband;
mod beats;
mod lines;
mod partials;
mod pulse;
mod short_time;
mod slow;
mod swells;

use realfft::num_complex::Complex;

use crate::report::{Movement, Partial, PartialInteraural};
use crate::spectrum::Spectrum;
use baseband::baseband;
use lines::Cluster;
use partials::Track;

/// The most partials reported.
const MOST_PARTIALS: usize = 12;

pub(crate) struct MovementBuilder {
    pair: Option<(usize, usize)>,
    clusters: Vec<Cluster>,
    tracks: Vec<Track>,
    /// Each partial's band in whichever ear of the pair came first.
    first_ear: Vec<Option<Vec<Complex<f32>>>>,
    interaural: Vec<Option<PartialInteraural>>,
    slow: crate::report::SlowMovement,
    swells: crate::report::Swells,
    pulse: Option<crate::report::Pulse>,
}

impl MovementBuilder {
    /// The measures that need only the samples. `pair` is the ears' (or the
    /// L/R speakers') channel indices.
    pub(crate) fn new(
        channels: &[Vec<f32>],
        sample_rate: u32,
        pair: Option<(usize, usize)>,
    ) -> Self {
        let clusters = lines::line_spectrum(channels, sample_rate)
            .map(|spectrum| {
                let found = lines::lines(&spectrum);
                lines::clusters(&spectrum, &found, MOST_PARTIALS)
            })
            .unwrap_or_default();
        let levels = slow::levels(channels, sample_rate);
        let swells = swells::swells(&levels.broadband, slow::STEP_SECONDS);
        let slow = slow::slow(&levels);
        let mut envelope: Vec<f64> = Vec::new();
        for samples in channels {
            let power = pulse::power_envelope(samples, sample_rate);
            if envelope.is_empty() {
                envelope = power;
            } else {
                envelope
                    .iter_mut()
                    .zip(&power)
                    .for_each(|(sum, value)| *sum += value);
            }
        }
        let partials_hertz: Vec<f64> = clusters
            .iter()
            .map(|cluster| cluster.strongest_line().frequency_hertz)
            .collect();
        let pulse = pulse::pulse(&envelope, &partials_hertz);
        let count = clusters.len();
        Self {
            pair,
            clusters,
            tracks: (0..count).map(|_| Track::default()).collect(),
            first_ear: vec![None; count],
            interaural: vec![None; count],
            slow,
            swells,
            pulse,
        }
    }

    /// Channel `index`'s whole-file spectrum: each partial's band.
    pub(crate) fn add_channel(&mut self, index: usize, spectrum: &Spectrum) {
        let in_pair = self
            .pair
            .is_some_and(|(left, right)| index == left || index == right);
        for (number, cluster) in self.clusters.iter().enumerate() {
            let band = baseband(spectrum, cluster.low_hertz, cluster.high_hertz);
            self.tracks[number].add(&band);
            if !in_pair {
                continue;
            }
            match self.first_ear[number].take() {
                None => self.first_ear[number] = Some(band.values),
                Some(first) => {
                    let (left, _) = self.pair.expect("in a pair");
                    let (left_band, right_band) = if index == left {
                        (&band.values, &first)
                    } else {
                        (&first, &band.values)
                    };
                    self.interaural[number] = partials::interaural(left_band, right_band);
                }
            }
        }
    }

    pub(crate) fn finish(self) -> Movement {
        let partials: Vec<Partial> = self
            .clusters
            .iter()
            .zip(&self.tracks)
            .zip(self.interaural)
            .map(|((cluster, track), interaural)| {
                let strongest = cluster.strongest_line();
                let pitch = track.pitch();
                Partial {
                    frequency_hertz: strongest.frequency_hertz,
                    low_hertz: cluster.low_hertz,
                    high_hertz: cluster.high_hertz,
                    lines: cluster.lines.len(),
                    line_spacing_hertz: cluster.line_spacing_hertz(),
                    relative_decibels: crate::statistics::power_decibels(cluster.share),
                    prominence_decibels: strongest.prominence_decibels,
                    level_p10_p90_decibels: track.level_p10_p90(),
                    pitch_standard_deviation_cents: pitch.map(|(spread, _)| spread),
                    pitch_p10_p90_cents: pitch.map(|(_, range)| range),
                    interaural,
                }
            })
            .collect();
        Movement {
            tonal_share: self.clusters.iter().map(|cluster| cluster.share).sum(),
            binaural_beats: beats::binaural_beats(&partials),
            partials,
            slow: self.slow,
            swells: self.swells,
            pulse: self.pulse,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signals::{sine, RATE};

    /// Through the whole chain: a steady 100 Hz sine and a 300 Hz tone whose
    /// pitch swings ±10 cents at 0.2 Hz (±1.73 Hz). The steady one is one
    /// line in a 1.5 Hz band that does not move. The wandering one is a hump
    /// at least 3.4 Hz wide, and its band holds the whole swing: a pitch
    /// standard deviation of 10/√2 = 7.07 cents.
    #[test]
    fn a_steady_and_a_wandering_tone() {
        let seconds = 64.0;
        let mut phase = 0.0f64;
        let wandering: Vec<f32> = (0..(seconds * RATE as f64) as usize)
            .map(|index| {
                let time = index as f64 / RATE as f64;
                let swing = (std::f64::consts::TAU * 0.2 * time).sin();
                phase +=
                    std::f64::consts::TAU * 300.0 * 2f64.powf(10.0 * swing / 1200.0) / RATE as f64;
                (0.2 * phase.sin()) as f32
            })
            .collect();
        let samples: Vec<f32> = sine(100.0, 0.3, seconds)
            .iter()
            .zip(&wandering)
            .map(|(steady, wandering)| steady + wandering)
            .collect();
        let channels = vec![samples];
        let mut builder = MovementBuilder::new(&channels, RATE, None);
        builder.add_channel(0, &Spectrum::of(&channels[0], RATE));
        let movement = builder.finish();
        let near = |hertz: f64| {
            movement
                .partials
                .iter()
                .find(|partial| (partial.frequency_hertz - hertz).abs() < 2.5)
                .unwrap_or_else(|| panic!("a partial near {hertz} Hz: {movement:#?}"))
        };
        let steady = near(100.0);
        assert_eq!(steady.lines, 1);
        assert!((steady.high_hertz - steady.low_hertz - 1.5).abs() < 0.1);
        assert!(steady.pitch_standard_deviation_cents.expect("long") < 0.1);
        let moving = near(300.0);
        assert!(moving.high_hertz - moving.low_hertz >= 3.4, "{moving:?}");
        let spread = moving.pitch_standard_deviation_cents.expect("long");
        assert!((spread - 7.07).abs() < 0.4, "{spread}");
        assert_eq!(
            movement
                .partials
                .iter()
                .filter(|partial| (partial.frequency_hertz - 300.0).abs() < 4.0)
                .count(),
            1
        );
    }
}
