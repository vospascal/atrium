//! Step 8, first half: which takes are one type.
//!
//! The similarity of two takes is the largest normalised cross-correlation
//! of their log-mel spectrograms over a time shift, as Raven's spectrogram
//! correlator and warbleR's `cross_correlation` measure it: the shorter
//! take's spectrogram slides along the longer's, and at each shift the
//! Pearson correlation of the two over every cell is taken. The spectrograms
//! cover the detection band only, so the bed outside it does not count.
//!
//! The takes are grouped by average linkage (UPGMA): the two groups whose
//! takes are most alike on average join, until no two are alike by
//! [`TYPE_SIMILARITY`] or more.
//!
//! [`TYPE_SIMILARITY`] is provisional. The plan sets it on a recording whose
//! calls are annotated by ear, which we do not have yet. Until then it is set
//! from the synthetic songs of the tests (two shapes, five rising syllables
//! and three long falling ones, each song in noise at its own pitch within
//! ±3 %): halfway between the highest similarity of two songs of different
//! shapes (0.12) and the lowest of two of one shape (0.68). The matrix is
//! always reported, so a cut can be grouped again by eye.

use crate::onsets::{hertz_to_mel, mel_to_hertz};
use realfft::RealFftPlanner;

/// Takes alike by this much or more on average are one type. Provisional: see
/// the module's comment.
pub(super) const TYPE_SIMILARITY: f64 = 0.40;
/// The spectrogram's window (s), rounded up to a power of two in samples:
/// 1024 at 48 kHz, 512 at 24 kHz. Half of it is the hop.
const WINDOW_SECONDS: f64 = 0.02;
/// At most this many mel bands across the detection band, fewer when the
/// band holds fewer than twice as many bins.
const MEL_BANDS: usize = 32;
/// Cells further than this below the take's loudest are floored (dB), as the
/// onset spectrogram is.
const TOP_DECIBELS: f64 = 80.0;

/// A log-mel spectrogram: `frames` rows of `bands` values (dB).
struct Spectrogram {
    frames: usize,
    bands: usize,
    cells: Vec<f64>,
}

/// The similarity of every pair of `takes` (mono samples each), in the band
/// from `low` to `high` Hz.
pub(super) fn similarity(
    takes: &[Vec<f32>],
    sample_rate: u32,
    low: f64,
    high: f64,
) -> Vec<Vec<f64>> {
    let spectrograms: Vec<Spectrogram> = takes
        .iter()
        .map(|take| spectrogram(take, sample_rate, low, high))
        .collect();
    let count = takes.len();
    let mut matrix = vec![vec![1.0; count]; count];
    for first in 0..count {
        for second in first + 1..count {
            let value = cross_correlation(&spectrograms[first], &spectrograms[second]);
            matrix[first][second] = value;
            matrix[second][first] = value;
        }
    }
    matrix
}

fn spectrogram(samples: &[f32], sample_rate: u32, low: f64, high: f64) -> Spectrogram {
    let window_length = ((WINDOW_SECONDS * sample_rate as f64).ceil() as usize).next_power_of_two();
    let hop = window_length / 2;
    let bin_hertz = sample_rate as f64 / window_length as f64;
    let bins_in_band = ((high - low) / bin_hertz).floor() as usize;
    let bands = (bins_in_band / 2).clamp(1, MEL_BANDS);
    let (low_mel, high_mel) = (hertz_to_mel(low), hertz_to_mel(high));
    let edges: Vec<f64> = (0..bands + 2)
        .map(|index| {
            mel_to_hertz(low_mel + (high_mel - low_mel) * index as f64 / (bands + 1) as f64)
        })
        .collect();
    let weights: Vec<Vec<f64>> = (0..bands)
        .map(|band| {
            let (bottom, centre, top) = (edges[band], edges[band + 1], edges[band + 2]);
            (0..window_length / 2 + 1)
                .map(|bin| {
                    let frequency = bin as f64 * bin_hertz;
                    let rising = (frequency - bottom) / (centre - bottom);
                    let falling = (top - frequency) / (top - centre);
                    rising.min(falling).max(0.0)
                })
                .collect()
        })
        .collect();
    let window: Vec<f64> = (0..window_length)
        .map(|index| {
            0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / window_length as f64).cos()
        })
        .collect();
    let frames = if samples.len() >= window_length {
        1 + (samples.len() - window_length) / hop
    } else {
        1
    };
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(window_length);
    let mut input = forward.make_input_vec();
    let mut spectrum = forward.make_output_vec();
    let mut cells = Vec::with_capacity(frames * bands);
    for frame in 0..frames {
        for (offset, value) in input.iter_mut().enumerate() {
            *value = samples
                .get(frame * hop + offset)
                .map_or(0.0, |sample| *sample as f64)
                * window[offset];
        }
        forward
            .process(&mut input, &mut spectrum)
            .expect("buffers are the planner's own");
        for band in &weights {
            let power: f64 = band
                .iter()
                .zip(&spectrum)
                .map(|(weight, bin)| weight * bin.norm_sqr())
                .sum();
            cells.push(power);
        }
    }
    let loudest = cells.iter().copied().fold(0.0f64, f64::max).max(1e-300);
    for cell in &mut cells {
        *cell = (10.0 * (cell.max(1e-300) / loudest).log10()).max(-TOP_DECIBELS);
    }
    Spectrogram {
        frames,
        bands,
        cells,
    }
}

/// The largest Pearson correlation of the shorter spectrogram with the part
/// of the longer it lies over, at every shift that keeps it inside. 0 when
/// either does not vary.
fn cross_correlation(first: &Spectrogram, second: &Spectrogram) -> f64 {
    let (shorter, longer) = if first.frames <= second.frames {
        (first, second)
    } else {
        (second, first)
    };
    let bands = shorter.bands;
    let cells = shorter.cells.len();
    let shorter_mean = shorter.cells.iter().sum::<f64>() / cells as f64;
    let centred: Vec<f64> = shorter
        .cells
        .iter()
        .map(|cell| cell - shorter_mean)
        .collect();
    let shorter_square: f64 = centred.iter().map(|cell| cell * cell).sum();
    if shorter_square <= 0.0 {
        return 0.0;
    }
    (0..=longer.frames - shorter.frames)
        .map(|shift| {
            let part = &longer.cells[shift * bands..shift * bands + cells];
            let mean = part.iter().sum::<f64>() / cells as f64;
            let (mut product, mut square) = (0.0, 0.0);
            for (a, b) in centred.iter().zip(part) {
                let b = b - mean;
                product += a * b;
                square += b * b;
            }
            if square > 0.0 {
                product / (shorter_square * square).sqrt()
            } else {
                0.0
            }
        })
        .fold(0.0, f64::max)
}

/// Each take's type, numbered by first appearance (0 for the first take's),
/// by average linkage down to [`TYPE_SIMILARITY`].
pub(super) fn group(similarity: &[Vec<f64>]) -> Vec<usize> {
    let mut groups: Vec<Vec<usize>> = (0..similarity.len()).map(|take| vec![take]).collect();
    loop {
        let mut best: Option<(usize, usize, f64)> = None;
        for first in 0..groups.len() {
            for second in first + 1..groups.len() {
                let total: f64 = groups[first]
                    .iter()
                    .flat_map(|a| groups[second].iter().map(move |b| similarity[*a][*b]))
                    .sum();
                let average = total / (groups[first].len() * groups[second].len()) as f64;
                if best.is_none_or(|(_, _, value)| average > value) {
                    best = Some((first, second, average));
                }
            }
        }
        match best {
            Some((first, second, average)) if average >= TYPE_SIMILARITY => {
                let joined = groups.remove(second);
                groups[first].extend(joined);
            }
            _ => break,
        }
    }
    groups.sort_by_key(|group| group.iter().copied().min());
    let mut types = vec![0; similarity.len()];
    for (index, group) in groups.iter().enumerate() {
        for take in group {
            types[*take] = index;
        }
    }
    types
}

/// The name of type `index`: A, B, … Z, then T27, T28, ….
pub(super) fn name(index: usize) -> String {
    if index < 26 {
        char::from(b'A' + index as u8).to_string()
    } else {
        format!("T{}", index + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn average_linkage_joins_what_is_alike() {
        let similarity = vec![
            vec![1.0, 0.9, 0.1, 0.8],
            vec![0.9, 1.0, 0.2, 0.7],
            vec![0.1, 0.2, 1.0, 0.1],
            vec![0.8, 0.7, 0.1, 1.0],
        ];
        assert_eq!(group(&similarity), [0, 0, 1, 0]);
        assert_eq!(
            (name(0), name(1), name(26)),
            ("A".into(), "B".into(), "T27".into())
        );
    }
}
