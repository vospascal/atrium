use atrium_core::telemetry::SourceSignal;
use std::f32::consts::TAU;

/// Two broad crossovers and short RMS windows, with fixed storage for the audio thread.
pub(super) struct SignalMeter {
    reference_rms: f32,
    sample_rate: f32,
    low_step: f32,
    upper_step: f32,
    window: u32,
    rise: f32,
    fall: f32,
    low: f32,
    below_high: f32,
    powers: [f32; 4],
    averages: [f32; 4],
    samples: u32,
}

impl SignalMeter {
    pub(super) fn new(reference_rms: f32) -> Self {
        Self {
            reference_rms: reference_rms.max(1e-8),
            sample_rate: 0.0,
            low_step: 0.0,
            upper_step: 0.0,
            window: 0,
            rise: 0.0,
            fall: 0.0,
            low: 0.0,
            below_high: 0.0,
            powers: [0.0; 4],
            averages: [0.0; 4],
            samples: 0,
        }
    }

    /// A meter of its own on the same scale: for a second voice of the sound.
    pub(super) fn fresh(&self) -> Self {
        Self::new(self.reference_rms)
    }

    pub(super) fn reset(&mut self) {
        self.low = 0.0;
        self.below_high = 0.0;
        self.powers = [0.0; 4];
        self.averages = [0.0; 4];
        self.samples = 0;
    }

    #[inline]
    pub(super) fn push(&mut self, sample: f32, sample_rate: f32) {
        if sample_rate != self.sample_rate {
            self.reset();
            self.sample_rate = sample_rate;
            self.low_step = 1.0 - (-TAU * 350.0 / sample_rate).exp();
            self.upper_step = 1.0 - (-TAU * 2_400.0 / sample_rate).exp();
            // 10 ms per measurement. Smooth the display without losing quick attacks.
            self.window = (sample_rate * 0.010).round().max(1.0) as u32;
            self.rise = 1.0 - (-0.010f32 / 0.035).exp();
            self.fall = 1.0 - (-0.010f32 / 0.11).exp();
        }
        if self.window == 0 || !sample.is_finite() {
            return;
        }
        self.low += (sample - self.low) * self.low_step;
        self.below_high += (sample - self.below_high) * self.upper_step;
        let high = sample - self.below_high;
        let middle = self.below_high - self.low;
        self.powers[0] += sample * sample;
        self.powers[1] += self.low * self.low;
        self.powers[2] += middle * middle;
        self.powers[3] += high * high;
        self.samples += 1;
        if self.samples == self.window {
            for (average, sum) in self.averages.iter_mut().zip(&mut self.powers) {
                let power = *sum / self.window as f32;
                let step = if power > *average {
                    self.rise
                } else {
                    self.fall
                };
                *average += (power - *average) * step;
                *sum = 0.0;
            }
            self.samples = 0;
        }
    }

    pub(super) fn signal(&self) -> SourceSignal {
        let [full, low, middle, high] = self.averages;
        let sum = low + middle + high;
        if sum <= f32::MIN_POSITIVE || !sum.is_finite() || !full.is_finite() {
            return SourceSignal::default();
        }
        let relative_rms = full.sqrt() / self.reference_rms;
        let mut layer_rms = [0.0; atrium_core::layers::MOST_LAYERS];
        layer_rms[0] = relative_rms;
        SourceSignal {
            relative_rms,
            low_share: low / sum,
            high_share: high / sum,
            layer_count: 1,
            layer_rms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, amplitude: f32, sample_rate: f32) -> SourceSignal {
        let mut meter = SignalMeter::new(0.5);
        for i in 0..sample_rate as u32 / 2 {
            meter.push(
                amplitude * (TAU * hz * i as f32 / sample_rate).sin(),
                sample_rate,
            );
        }
        meter.signal()
    }

    #[test]
    fn low_and_high_tones_drive_different_bands() {
        for sample_rate in [44_100.0, 48_000.0] {
            let low = tone(80.0, 0.5, sample_rate);
            let high = tone(8_000.0, 0.5, sample_rate);
            assert!(low.low_share > 0.85 && low.high_share < 0.05, "{low:?}");
            assert!(high.high_share > 0.80 && high.low_share < 0.05, "{high:?}");
            assert!((low.relative_rms - high.relative_rms).abs() < 0.02);
        }
    }

    #[test]
    fn amplitude_changes_the_absolute_level_and_silence_stays_zero() {
        let quiet = tone(600.0, 0.25, 48_000.0);
        let loud = tone(600.0, 0.50, 48_000.0);
        assert!((loud.relative_rms / quiet.relative_rms - 2.0).abs() < 0.01);
        assert_eq!(tone(600.0, 0.0, 48_000.0).relative_rms, 0.0);
    }
}
