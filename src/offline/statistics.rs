//! Peak and RMS of each channel of a render.

/// One channel's level over the whole render, on the rendered values
/// (before any 16-bit quantisation), 1.0 being full scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelStatistics {
    /// The largest absolute sample.
    pub peak: f32,
    /// Root mean square, summed in `f64` so a long render loses nothing.
    pub rms: f64,
}

impl ChannelStatistics {
    /// The level of each of `channels` channels of `interleaved` frames.
    pub(super) fn of(interleaved: &[f32], channels: usize) -> Vec<Self> {
        let frames = interleaved.len() / channels.max(1);
        (0..channels)
            .map(|channel| {
                let samples = interleaved.iter().skip(channel).step_by(channels);
                let (peak, sum_of_squares) = samples.fold((0f32, 0f64), |(peak, sum), sample| {
                    (peak.max(sample.abs()), sum + (*sample as f64).powi(2))
                });
                Self {
                    peak,
                    rms: if frames == 0 {
                        0.0
                    } else {
                        (sum_of_squares / frames as f64).sqrt()
                    },
                }
            })
            .collect()
    }
}
