//! Binaural beats: a partial mostly in the left ear and one mostly in the
//! right, close enough to beat.
//!
//! A beat of under 2 Hz puts both tones in one cluster, where it shows as
//! the cluster's interaural rotation instead. Faster ones are two partials,
//! one standing at least 10 dB higher in each ear, paired nearest first
//! within 45 Hz (binaural beats are heard up to about 30–35 Hz, Licklider
//! 1950; 45 leaves room for the engine's 40 Hz piece).

use crate::report::{BinauralBeat, Partial};

/// A partial is in one ear when it stands this much higher there (dB).
const ONE_EAR_DECIBELS: f64 = 10.0;
/// Partials further apart (Hz) do not beat.
const WIDEST_BEAT_HERTZ: f64 = 45.0;

pub(crate) fn binaural_beats(partials: &[Partial]) -> Vec<BinauralBeat> {
    let side = |partial: &Partial| {
        partial
            .interaural
            .as_ref()
            .map_or(0.0, |ears| ears.level_difference_decibels)
    };
    let lefts: Vec<&Partial> = partials
        .iter()
        .filter(|partial| side(partial) <= -ONE_EAR_DECIBELS)
        .collect();
    let rights: Vec<&Partial> = partials
        .iter()
        .filter(|partial| side(partial) >= ONE_EAR_DECIBELS)
        .collect();
    let mut candidates: Vec<(f64, usize, usize)> = Vec::new();
    for (left_index, left) in lefts.iter().enumerate() {
        for (right_index, right) in rights.iter().enumerate() {
            let gap = (right.frequency_hertz - left.frequency_hertz).abs();
            if gap <= WIDEST_BEAT_HERTZ {
                candidates.push((gap, left_index, right_index));
            }
        }
    }
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut used_left = vec![false; lefts.len()];
    let mut used_right = vec![false; rights.len()];
    let mut beats = Vec::new();
    for (_, left_index, right_index) in candidates {
        if used_left[left_index] || used_right[right_index] {
            continue;
        }
        used_left[left_index] = true;
        used_right[right_index] = true;
        let left = lefts[left_index].frequency_hertz;
        let right = rights[right_index].frequency_hertz;
        beats.push(BinauralBeat {
            left_hertz: left,
            right_hertz: right,
            beat_hertz: right - left,
        });
    }
    beats.sort_by(|a, b| a.left_hertz.total_cmp(&b.left_hertz));
    beats
}

#[cfg(test)]
mod tests {
    use crate::signal::{Settings, Signal};
    use crate::test_signals::{interleave, sine, RATE};

    /// The engine's piece: 108 Hz in the left ear, 112 Hz in the right, with
    /// the other ear hearing each 21 dB down (the 3D version's leak). One
    /// beat, 4 Hz, right minus left.
    #[test]
    fn a_binaural_beat_from_two_lateral_tones() {
        let leak = 10f64.powf(-21.0 / 20.0);
        let left: Vec<f32> = sine(108.0, 0.3, 40.0)
            .iter()
            .zip(sine(112.0, 0.3 * leak, 40.0))
            .map(|(own, other)| own + other)
            .collect();
        let right: Vec<f32> = sine(112.0, 0.3, 40.0)
            .iter()
            .zip(sine(108.0, 0.3 * leak, 40.0))
            .map(|(own, other)| own + other)
            .collect();
        let report = crate::analyse(
            &Signal {
                interleaved: &interleave(&[&left, &right]),
                channels: 2,
                sample_rate: RATE,
                channel_names: Vec::new(),
            },
            &Settings::default(),
        );
        let beats = &report.movement.binaural_beats;
        assert_eq!(beats.len(), 1, "{beats:?}");
        assert!((beats[0].left_hertz - 108.0).abs() < 0.02, "{beats:?}");
        assert!((beats[0].beat_hertz - 4.0).abs() < 0.02, "{beats:?}");
    }
}
