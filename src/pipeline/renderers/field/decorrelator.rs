//! The filter that turns one mono ambient signal into mutually decorrelated
//! channels: for each channel, four short Schroeder all-passes in series.
//!
//! An all-pass keeps every frequency at its level and only moves its phase,
//! so the channels sound like the source and their powers add up to its
//! power. What it costs is time: each stage spreads an attack over an
//! exponentially decaying train of echoes. With delays of 0.5 to 2 ms and a
//! gain of 0.5 the four stages put over 90 % of an attack's energy in its
//! first 10 ms, so a frog's croak or a drop of rain is heard once.
//!
//! The structure is Schroeder and Logan's all-pass ("Colorless artificial
//! reverberation", 1961). The four numbers that size it were measured against
//! the acceptance criteria in the tests, not taken from a paper; the root
//! README ("Ambient sound") records how, and why the published velvet-noise
//! decorrelator was measured and not chosen.

use crate::synth::noise::Rng;

/// All-pass stages per channel.
const STAGES: usize = 4;
/// Feedback gain of every stage. An impulse leaves a stage as −g at once and
/// then (1 − g²)·g^(k−1) after k delays, so the train falls by 6 dB per delay.
const STAGE_GAIN: f32 = 0.5;
/// The range each stage's delay is drawn from. A stage's energy arrives on
/// average one delay late (Σ k·M·(1 − g²)²·g^(2k−2) = M), so four stages
/// averaging 1.25 ms centre an attack about 5 ms after it starts.
const SHORTEST_DELAY_MS: f32 = 0.5;
const LONGEST_DELAY_MS: f32 = 2.0;
/// Channel `c` draws its delays from `Rng::new(FIRST_SEED + c)`: fixed, so
/// every run and every machine renders the same field. 80 is the first base
/// of a scan in steps of eight (so no two bases share a channel's seed) whose
/// eight channels meet every criterion in the tests, with margin, at 44.1, 48
/// and 96 kHz. With short filters the delays decide the correlation between
/// two channels, and most bases leave one pair too alike at some rate.
const FIRST_SEED: u64 = 80;

/// One Schroeder all-pass: H(z) = (−g + z^−M) / (1 − g·z^−M).
struct AllPass {
    buffer: Vec<f32>,
    write: usize,
    feedback: f32,
}

impl AllPass {
    fn new(delay_samples: usize, feedback: f32) -> Self {
        Self {
            buffer: vec![0.0; delay_samples.max(1)],
            write: 0,
            feedback,
        }
    }

    #[inline(always)]
    fn process(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.write];
        let output = delayed - self.feedback * input;
        self.buffer[self.write] = input + self.feedback * output;
        self.write += 1;
        if self.write == self.buffer.len() {
            self.write = 0;
        }
        output
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write = 0;
    }
}

/// One channel's decorrelating filter. Its delay lines are allocated here,
/// so processing never allocates.
pub(super) struct Decorrelator {
    stages: [AllPass; STAGES],
}

impl Decorrelator {
    /// The filter for output channel `channel`. Delays are defined in
    /// milliseconds and rounded to whole samples at `sample_rate`, so the
    /// filter is the same at every rate to within half a sample.
    pub(super) fn new(channel: usize, sample_rate: f32) -> Self {
        let mut generator = Rng::new(FIRST_SEED + channel as u64);
        let mut delays_ms = [0.0f32; STAGES];
        for delay_ms in &mut delays_ms {
            *delay_ms =
                SHORTEST_DELAY_MS + (LONGEST_DELAY_MS - SHORTEST_DELAY_MS) * generator.next_f32();
        }
        Self {
            stages: delays_ms.map(|delay_ms| {
                AllPass::new(
                    (delay_ms * 0.001 * sample_rate).round() as usize,
                    STAGE_GAIN,
                )
            }),
        }
    }

    #[inline(always)]
    pub(super) fn process(&mut self, input: f32) -> f32 {
        self.stages
            .iter_mut()
            .fold(input, |signal, stage| stage.process(signal))
    }

    pub(super) fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.reset();
        }
    }
}

#[cfg(test)]
mod tests;
