//! The spectral lines of a file and the clusters they form.
//!
//! The spectrum is the mean of 16 s Hann frames (0.0625 Hz bins), half
//! overlapping, over all channels: long enough to separate voices a few
//! tenths of a hertz apart, which is how ambient pads and bowl doublets are
//! built. A line is a local maximum standing at least 10 dB over the median
//! of the spectrum within a sixth of an octave either side; its frequency is
//! read between bins from a parabola through the decibels. Lines within
//! 2 Hz of each other, and within 20 dB of the strongest of them, form one
//! cluster, which is then one partial: a tone, a group of detuned voices, or
//! a tone's vibrato sidebands. A line counts only if the spectrum dips 6 dB
//! between it and any higher point near it, so the ripple on a wandering
//! partial's smeared peak is not taken for voices. The partial's band runs
//! from 0.75 to 2 Hz past its outer lines, to where the spectrum falls
//! 20 dB under its strongest line, and stops half-way to a neighbour. A
//! partial more than 60 dB under the whole file is dropped.

use super::short_time::{for_each_frame, frame_count};

/// The frame length (s): 0.0625 Hz bins.
const FRAME_SECONDS: f64 = 16.0;
/// A line stands at least this far over its surroundings (dB).
const PROMINENCE_DECIBELS: f64 = 10.0;
/// The surroundings: this many octaves either side, at least
/// [`FLOOR_MINIMUM_BINS`].
const FLOOR_OCTAVES: f64 = 1.0 / 6.0;
const FLOOR_MINIMUM_BINS: usize = 8;
/// The floor is computed this many times an octave and interpolated.
const FLOOR_STEPS_PER_OCTAVE: f64 = 48.0;
/// Lines are looked for from here to the lower of these (Hz) and 0.45 of
/// the sample rate.
const LOWEST_HERTZ: f64 = 20.0;
const HIGHEST_HERTZ: f64 = 8000.0;
/// A line dips at least this far (dB) before any higher point within 2 Hz:
/// two voices 0.25 Hz apart (four bins) dip more than this between them,
/// the ripple on a wandering partial's smeared peak (a mean of 14 frames
/// scatters about 1.2 dB) does not.
const DIP_DECIBELS: f64 = 6.0;
/// A weaker maximum this close to a stronger one is its window's lobe (bins).
const SAME_LINE_BINS: f64 = 3.0;
/// Lines this close (Hz) belong to one cluster.
pub(crate) const CLUSTER_GAP_HERTZ: f64 = 2.0;
/// A line in a cluster is a voice when it is within this much (dB) of the
/// cluster's strongest.
const VOICE_WITHIN_DECIBELS: f64 = 20.0;
/// A cluster's band reaches past its outer lines to where the spectrum is
/// this far (dB) under its strongest line: a partial whose pitch wanders is
/// a hump, not a line, and its band must hold the whole hump.
const WIDTH_DECIBELS: f64 = 20.0;
/// The band reaches at least this far (Hz) past the outer lines, at most
/// [`CLUSTER_GAP_HERTZ`].
const BAND_MARGIN_HERTZ: f64 = 0.75;
/// A partial holds at least this share of the file's power (−60 dB re Leq).
/// A clean render has no noise floor, so a sine's rounding products 140 dB
/// down stood 10 dB over its surroundings and read as partials.
const SMALLEST_SHARE: f64 = 1e-6;
/// At most this many lines are kept, strongest first, before clustering.
const MOST_LINES: usize = 400;

/// The mean power spectrum of all channels' frames.
pub(crate) struct LineSpectrum {
    pub(crate) hertz_per_bin: f64,
    /// |X|², mean over frames and channels.
    pub(crate) power: Vec<f64>,
    pub(crate) highest_hertz: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Line {
    pub(crate) frequency_hertz: f64,
    pub(crate) power_decibels: f64,
    pub(crate) prominence_decibels: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cluster {
    /// Lowest frequency first.
    pub(crate) lines: Vec<Line>,
    pub(crate) strongest: usize,
    /// The band the partial is measured in (Hz): its −20 dB width.
    pub(crate) low_hertz: f64,
    pub(crate) high_hertz: f64,
    /// The band's share of the file's power, 20 Hz to Nyquist, 0 to 1.
    pub(crate) share: f64,
}

impl Cluster {
    pub(crate) fn strongest_line(&self) -> &Line {
        &self.lines[self.strongest]
    }

    /// The median gap between neighbouring lines; None for one line.
    pub(crate) fn line_spacing_hertz(&self) -> Option<f64> {
        let gaps: Vec<f64> = self
            .lines
            .windows(2)
            .map(|pair| pair[1].frequency_hertz - pair[0].frequency_hertz)
            .collect();
        crate::statistics::quantile(&gaps, 0.5)
    }
}

/// The mean spectrum of 16 s frames, shorter for a file under 40 s so it
/// still has four (one frame's noise peaks stand 10 dB over the median one
/// bin in a thousand; a mean of four, one in 10¹²), down to half a second;
/// None below that.
pub(crate) fn line_spectrum(channels: &[Vec<f32>], sample_rate: u32) -> Option<LineSpectrum> {
    let length = channels.first().map_or(0, Vec::len);
    let window = ((FRAME_SECONDS * sample_rate as f64) as usize).min(length * 2 / 5);
    if window < sample_rate as usize / 2 {
        return None;
    }
    let hop = (window / 2).max(1);
    let frames = frame_count(length, window, hop) * channels.len();
    let mut power = vec![0.0f64; window / 2 + 1];
    for samples in channels {
        for_each_frame(samples, window, hop, |_, frame| {
            for (sum, value) in power.iter_mut().zip(frame) {
                *sum += value;
            }
        });
    }
    power
        .iter_mut()
        .for_each(|sum| *sum /= frames.max(1) as f64);
    Some(LineSpectrum {
        hertz_per_bin: sample_rate as f64 / window as f64,
        power,
        highest_hertz: HIGHEST_HERTZ.min(0.45 * sample_rate as f64),
    })
}

/// The lines of `spectrum`, strongest first, at most [`MOST_LINES`].
pub(crate) fn lines(spectrum: &LineSpectrum) -> Vec<Line> {
    let decibels: Vec<f64> = spectrum
        .power
        .iter()
        .map(|power| crate::statistics::power_decibels(*power))
        .collect();
    let spacing = spectrum.hertz_per_bin;
    let floor = Floor::of(&decibels, spacing, spectrum.highest_hertz);
    let first = ((LOWEST_HERTZ / spacing).ceil() as usize).max(1);
    let last = ((spectrum.highest_hertz / spacing) as usize).min(decibels.len().saturating_sub(2));
    let mut found = Vec::new();
    for index in first..=last {
        let here = decibels[index];
        if here < decibels[index - 1] || here <= decibels[index + 1] {
            continue;
        }
        let frequency = index as f64 * spacing;
        let prominence = here - floor.at(frequency);
        if prominence < PROMINENCE_DECIBELS
            || !stands_apart(&decibels, index, (CLUSTER_GAP_HERTZ / spacing) as usize)
        {
            continue;
        }
        let (offset, peak) = parabola(decibels[index - 1], here, decibels[index + 1]);
        found.push(Line {
            frequency_hertz: (index as f64 + offset) * spacing,
            power_decibels: peak,
            prominence_decibels: peak - floor.at(frequency),
        });
    }
    found.sort_by(|a, b| b.power_decibels.total_cmp(&a.power_decibels));
    let mut kept: Vec<Line> = Vec::new();
    for line in found {
        let lobe = kept.iter().any(|stronger| {
            (stronger.frequency_hertz - line.frequency_hertz).abs() <= SAME_LINE_BINS * spacing
        });
        if !lobe {
            kept.push(line);
            if kept.len() == MOST_LINES {
                break;
            }
        }
    }
    kept
}

/// The lines grouped into clusters, the strongest `most` by power, strongest
/// first.
pub(crate) fn clusters(spectrum: &LineSpectrum, lines: &[Line], most: usize) -> Vec<Cluster> {
    let mut by_frequency = lines.to_vec();
    by_frequency.sort_by(|a, b| a.frequency_hertz.total_cmp(&b.frequency_hertz));
    // Within each run of close lines only those near the strongest are
    // voices; faint ones (a pure tone's −28 dB sidebands) neither count nor
    // join two partials into one.
    let groups: Vec<Vec<Line>> = split_by_gap(by_frequency)
        .into_iter()
        .flat_map(|group| {
            let strongest = group
                .iter()
                .map(|line| line.power_decibels)
                .fold(f64::NEG_INFINITY, f64::max);
            split_by_gap(
                group
                    .into_iter()
                    .filter(|line| line.power_decibels >= strongest - VOICE_WITHIN_DECIBELS)
                    .collect(),
            )
        })
        .collect();
    let decibels: Vec<f64> = spectrum
        .power
        .iter()
        .map(|power| crate::statistics::power_decibels(*power))
        .collect();
    let mut clusters: Vec<Cluster> = groups
        .into_iter()
        .map(|lines| {
            let strongest = lines
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.power_decibels.total_cmp(&b.1.power_decibels))
                .map_or(0, |(index, _)| index);
            let edge = lines[strongest].power_decibels - WIDTH_DECIBELS;
            let low_hertz = reach(
                &decibels,
                spectrum.hertz_per_bin,
                lines[0].frequency_hertz,
                edge,
                -1.0,
            );
            let high_hertz = reach(
                &decibels,
                spectrum.hertz_per_bin,
                lines.last().expect("never empty").frequency_hertz,
                edge,
                1.0,
            );
            Cluster {
                lines,
                strongest,
                low_hertz,
                high_hertz,
                share: 0.0,
            }
        })
        .collect();
    // Neighbouring bands meet half-way rather than overlap.
    for index in 1..clusters.len() {
        if clusters[index - 1].high_hertz > clusters[index].low_hertz {
            let middle = 0.5
                * (clusters[index - 1]
                    .lines
                    .last()
                    .expect("never empty")
                    .frequency_hertz
                    + clusters[index].lines[0].frequency_hertz);
            clusters[index - 1].high_hertz = middle;
            clusters[index].low_hertz = middle;
        }
    }
    let total = band_power(spectrum, LOWEST_HERTZ, f64::INFINITY);
    for cluster in &mut clusters {
        cluster.share = if total > 0.0 {
            band_power(spectrum, cluster.low_hertz, cluster.high_hertz) / total
        } else {
            0.0
        };
    }
    clusters.retain(|cluster| cluster.share >= SMALLEST_SHARE);
    clusters.sort_by(|a, b| b.share.total_cmp(&a.share));
    clusters.truncate(most);
    clusters
}

/// Whether the maximum at `index` is a line of its own and not a ripple on a
/// stronger one: towards each higher point within `reach` bins, the
/// spectrum must dip at least [`DIP_DECIBELS`] under it first. A maximum
/// with no higher point near it on either side (a line, or the top of a
/// partial whose wander has smeared it into a hump) always stands apart.
fn stands_apart(decibels: &[f64], index: usize, reach: usize) -> bool {
    let here = decibels[index];
    let col = |direction: isize| -> Option<f64> {
        let mut lowest = here;
        for step in 1..=reach as isize {
            let at = index as isize + direction * step;
            if at < 0 || at as usize >= decibels.len() {
                return None;
            }
            let value = decibels[at as usize];
            if value > here {
                return Some(lowest);
            }
            lowest = lowest.min(value);
        }
        None
    };
    match (col(-1), col(1)) {
        (None, None) => true,
        (left, right) => {
            let highest_col = left
                .unwrap_or(f64::NEG_INFINITY)
                .max(right.unwrap_or(f64::NEG_INFINITY));
            here - highest_col >= DIP_DECIBELS
        }
    }
}

/// Lines (lowest first) in runs whose neighbours are at most
/// [`CLUSTER_GAP_HERTZ`] apart.
fn split_by_gap(lines: Vec<Line>) -> Vec<Vec<Line>> {
    let mut groups: Vec<Vec<Line>> = Vec::new();
    for line in lines {
        match groups.last_mut() {
            Some(group)
                if line.frequency_hertz
                    - group
                        .last()
                        .expect("groups are never empty")
                        .frequency_hertz
                    <= CLUSTER_GAP_HERTZ =>
            {
                group.push(line)
            }
            _ => groups.push(vec![line]),
        }
    }
    groups
}

/// Where the spectrum, going from the line at `from` Hz in `direction`,
/// first falls under `edge` dB: at least [`BAND_MARGIN_HERTZ`] and at most
/// [`CLUSTER_GAP_HERTZ`] past the line.
fn reach(decibels: &[f64], spacing: f64, from: f64, edge: f64, direction: f64) -> f64 {
    let mut distance = BAND_MARGIN_HERTZ;
    while distance < CLUSTER_GAP_HERTZ {
        let bin = ((from + direction * distance) / spacing).round();
        if bin < 1.0 || bin as usize >= decibels.len() || decibels[bin as usize] < edge {
            break;
        }
        distance += spacing;
    }
    (from + direction * distance).max(0.0)
}

/// The power of the bins whose centres lie in `[low, high)` Hz.
fn band_power(spectrum: &LineSpectrum, low: f64, high: f64) -> f64 {
    let spacing = spectrum.hertz_per_bin;
    spectrum
        .power
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            let frequency = *index as f64 * spacing;
            frequency >= low && frequency < high
        })
        .map(|(_, power)| power)
        .sum()
}

/// The vertex of the parabola through three equally spaced values: its
/// offset from the middle one (bins, within ±0.5) and its height.
fn parabola(left: f64, centre: f64, right: f64) -> (f64, f64) {
    let curvature = left - 2.0 * centre + right;
    if curvature >= 0.0 {
        return (0.0, centre);
    }
    let offset = (0.5 * (left - right) / curvature).clamp(-0.5, 0.5);
    (offset, centre - 0.25 * (left - right) * offset)
}

/// The median of the spectrum's decibels around each frequency, on a grid
/// 48 steps an octave, interpolated in log frequency.
struct Floor {
    grid: Vec<(f64, f64)>,
}

impl Floor {
    fn of(decibels: &[f64], spacing: f64, highest: f64) -> Self {
        let mut grid = Vec::new();
        let mut step = 0.0;
        loop {
            let frequency = LOWEST_HERTZ * 2f64.powf(step / FLOOR_STEPS_PER_OCTAVE);
            if frequency > highest * 2f64.powf(FLOOR_OCTAVES) {
                break;
            }
            let centre = (frequency / spacing).round() as usize;
            let reach = ((frequency * (2f64.powf(FLOOR_OCTAVES) - 1.0) / spacing).ceil() as usize)
                .max(FLOOR_MINIMUM_BINS);
            let low = centre.saturating_sub(reach).max(1);
            let high = (centre + reach + 1).min(decibels.len());
            if low < high {
                let median = crate::statistics::quantile(&decibels[low..high], 0.5)
                    .expect("a non-empty slice");
                grid.push((frequency.log2(), median));
            }
            step += 1.0;
        }
        Self { grid }
    }

    fn at(&self, frequency: f64) -> f64 {
        let position = frequency.max(LOWEST_HERTZ).log2();
        match self.grid.iter().position(|(log, _)| *log >= position) {
            Some(0) => self.grid[0].1,
            Some(above) => {
                let (low_log, low_value) = self.grid[above - 1];
                let (high_log, high_value) = self.grid[above];
                let share = (position - low_log) / (high_log - low_log);
                low_value + share * (high_value - low_value)
            }
            None => self.grid.last().map_or(0.0, |(_, value)| *value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signals::{sine, Noise, RATE};

    fn mixed(parts: &[Vec<f32>]) -> Vec<f32> {
        (0..parts[0].len())
            .map(|index| parts.iter().map(|part| part[index]).sum())
            .collect()
    }

    /// A 100 Hz sine, a pair at 220.0 and 220.6 Hz, and a 3 kHz sine over
    /// white noise: three clusters, the pair as one of two lines 0.6 Hz
    /// apart, every frequency within 0.01 Hz. The 100 Hz sine has power
    /// 0.125 of a total of 0.125 + 2·0.02 + 0.005 + 0.01 (the noise, of which
    /// the bands hold almost nothing): a share of 0.694 ± 0.01.
    #[test]
    fn lines_and_clusters_of_known_tones() {
        let seconds = 64.0;
        let mut noise = Noise::new(3);
        let signal = mixed(&[
            sine(100.0, 0.5, seconds),
            sine(220.0, 0.2, seconds),
            sine(220.6, 0.2, seconds),
            sine(3000.0, 0.1, seconds),
            noise.white(0.1, seconds),
        ]);
        let spectrum = line_spectrum(&[signal], RATE).expect("long enough");
        assert!((spectrum.hertz_per_bin - 0.0625).abs() < 1e-12);
        let found = lines(&spectrum);
        let clusters = clusters(&spectrum, &found, 12);
        assert_eq!(clusters.len(), 3, "{clusters:#?}");
        let strongest = &clusters[0];
        assert_eq!(strongest.lines.len(), 1);
        assert!((strongest.strongest_line().frequency_hertz - 100.0).abs() < 0.01);
        assert!(
            (strongest.share - 0.694).abs() < 0.01,
            "{}",
            strongest.share
        );
        let pair = &clusters[1];
        assert_eq!(pair.lines.len(), 2);
        assert!((pair.lines[0].frequency_hertz - 220.0).abs() < 0.01);
        assert!((pair.lines[1].frequency_hertz - 220.6).abs() < 0.01);
        let spacing = pair.line_spacing_hertz().expect("two lines");
        assert!((spacing - 0.6).abs() < 0.01, "{spacing}");
        assert!((clusters[2].strongest_line().frequency_hertz - 3000.0).abs() < 0.01);
    }

    /// White noise alone has no lines.
    #[test]
    fn noise_has_no_lines() {
        let mut noise = Noise::new(9);
        let signal = noise.white(0.1, 40.0);
        let spectrum = line_spectrum(&[signal], RATE).expect("long enough");
        assert!(lines(&spectrum).is_empty());
    }
}
