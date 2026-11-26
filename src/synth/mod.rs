// Procedural audio synthesis — environmental sound sources and tones.
//
// Ported from the TypeScript spatial-audio-garden AudioWorklet processors.
// All generators are allocation-free in the hot path and suitable for
// real-time audio threads (no heap alloc, no locks, no syscalls).
//
// Architecture difference from TS: the original processors included inline
// reverb (Freeverb). Here, sources produce dry mono samples — the existing
// FdnReverb processor in the audio pipeline handles reverberation.

pub mod binaural_beat;
pub mod canopy_wind;
pub mod field_wind;
pub mod focus_tone;
pub mod noise;
pub mod noise_colour;
pub mod rain;
pub mod rain_v2;
pub mod recipe;
pub mod river;
pub mod soft_wind;
pub mod storm_wind;
pub mod thunder;
pub mod wave;
pub mod wind_behaviour;
pub mod wisp;

#[cfg(test)]
pub(crate) mod test_support;

/// Envelope sample rate (Hz) used for analysis-resynthesis. High enough to keep
/// the flutter (a few Hz) crisp; low enough that a multi-minute recording is a
/// small array.
pub const ENVELOPE_RATE: f32 = 500.0;

use noise::{OnePoleHP, OnePoleLP};

/// Crossover for the two-band analysis-resynthesis, aligned with the wind
/// synth's `body_lp` (low-mid body) and `air_hp` (HF hiss) layers.
pub const BAND_SPLIT_LOW_HZ: f32 = 800.0;
pub const BAND_SPLIT_HIGH_HZ: f32 = 2000.0;

/// Core envelope follower: apply `pre` to each sample, rectify → one-pole smooth
/// (~50 ms, ≈3 Hz cutoff: keeps the wide gust swells and their organic
/// non-linear ramps, drops the fast flutter/chop that reads as warble on noise)
/// → decimate to [`ENVELOPE_RATE`] → normalize so the mean is 1.0 (absolute
/// level is set later by SPL calibration; this only carries the *shape*).
fn envelope_core(samples: &[f32], sample_rate: f32, mut pre: impl FnMut(f32) -> f32) -> Vec<f32> {
    if samples.is_empty() || sample_rate <= 0.0 {
        return Vec::new();
    }
    let tau = 0.050; // 50 ms smoothing
    let a = (-1.0 / (tau * sample_rate)).exp();
    // Fractional decimation step: envelope sample k is taken at the first
    // input index at or after k · step, so the envelope runs at exactly
    // ENVELOPE_RATE in time even when the step is not an integer (88.2 at
    // 44.1 kHz; an integer floor of 88 would run at 501.1 Hz).
    let step = (sample_rate as f64 / ENVELOPE_RATE as f64).max(1.0);

    let mut smooth = 0.0_f32;
    let mut envelope = Vec::with_capacity((samples.len() as f64 / step) as usize + 1);
    for (n, &x) in samples.iter().enumerate() {
        smooth = a * smooth + (1.0 - a) * pre(x).abs();
        if n as f64 >= envelope.len() as f64 * step {
            envelope.push(smooth);
        }
    }

    let mean = envelope.iter().sum::<f32>() / envelope.len().max(1) as f32;
    if mean > 1e-9 {
        for v in &mut envelope {
            *v /= mean;
        }
    }
    envelope
}

/// Extract a single broadband amplitude envelope (intensity-over-time).
pub fn extract_amplitude_envelope(samples: &[f32], sample_rate: f32) -> Vec<f32> {
    envelope_core(samples, sample_rate, |x| x)
}

/// Extract SEPARATE envelopes for the low-mid "body" (< ~800 Hz) and the high
/// "air" (> ~2 kHz) bands, so each synth layer can breathe with its own real
/// dynamics — the spectrum genuinely *moves* over time rather than the whole
/// band scaling as a block. Returns (body_envelope, air_envelope).
pub fn extract_band_envelopes(samples: &[f32], sample_rate: f32) -> (Vec<f32>, Vec<f32>) {
    let mut lp = OnePoleLP::new(BAND_SPLIT_LOW_HZ, sample_rate);
    let body = envelope_core(samples, sample_rate, move |x| lp.process(x));
    let mut hp = OnePoleHP::new(BAND_SPLIT_HIGH_HZ, sample_rate);
    let air = envelope_core(samples, sample_rate, move |x| hp.process(x));
    (body, air)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn sine(frequency_hz: f32, sample_rate: f32, seconds: f32) -> Vec<f32> {
        (0..(seconds * sample_rate) as usize)
            .map(|index| (TAU * frequency_hz * index as f32 / sample_rate).sin())
            .collect()
    }

    fn correlation(first: &[f32], second: &[f32]) -> f32 {
        let count = first.len().min(second.len());
        let mean_first = first[..count].iter().sum::<f32>() / count as f32;
        let mean_second = second[..count].iter().sum::<f32>() / count as f32;
        let (mut covariance, mut variance_first, mut variance_second) = (0.0, 0.0, 0.0);
        for index in 0..count {
            let a = first[index] - mean_first;
            let b = second[index] - mean_second;
            covariance += a * b;
            variance_first += a * a;
            variance_second += b * b;
        }
        covariance / (variance_first * variance_second).sqrt()
    }

    #[test]
    fn envelope_is_mean_normalised_to_one() {
        let envelope = extract_amplitude_envelope(&sine(200.0, 48_000.0, 4.0), 48_000.0);
        let mean = envelope.iter().sum::<f32>() / envelope.len() as f32;
        assert!((mean - 1.0).abs() < 1e-4, "mean {mean}");
    }

    /// Steady-state value of the follower for a unit sine is the rectified
    /// mean (2/pi); after normalisation a constant-level tone is flat at 1.
    #[test]
    fn constant_level_tone_gives_a_flat_envelope_after_the_settling_time() {
        let envelope = extract_amplitude_envelope(&sine(220.0, 48_000.0, 4.0), 48_000.0);
        // 50 ms settling at 500 Hz is 25 samples; skip 100.
        for &value in &envelope[100..] {
            assert!((value - 1.0).abs() < 0.03, "envelope wandered to {value}");
        }
    }

    /// A 1 Hz amplitude swell on a 300 Hz carrier is recovered at the
    /// envelope rate: the one-pole at tau = 50 ms (3.2 Hz) passes 1 Hz with
    /// a 0.3 rad phase lag and 0.4 dB of loss.
    #[test]
    fn slow_amplitude_modulation_is_recovered() {
        let sample_rate = 48_000.0;
        let swell: Vec<f32> = (0..(8.0 * sample_rate) as usize)
            .map(|index| {
                let time = index as f32 / sample_rate;
                (1.0 + 0.6 * (TAU * time).sin()) * (TAU * 300.0 * time).sin()
            })
            .collect();
        let envelope = extract_amplitude_envelope(&swell, sample_rate);
        // One-pole smoother at tau = 50 ms: phase lag atan(2 pi f tau).
        let lag = (TAU * 1.0 * 0.050_f32).atan();
        let expected: Vec<f32> = (0..envelope.len())
            .map(|index| 1.0 + 0.6 * (TAU * index as f32 / ENVELOPE_RATE - lag).sin())
            .collect();
        assert!(
            correlation(&envelope[500..], &expected[500..]) > 0.985,
            "modulation not recovered"
        );
    }

    #[test]
    fn envelope_of_silence_or_invalid_input_is_empty_or_zero() {
        assert!(extract_amplitude_envelope(&[], 48_000.0).is_empty());
        assert!(extract_amplitude_envelope(&[0.1; 100], 0.0).is_empty());
        let envelope = extract_amplitude_envelope(&vec![0.0; 4_800], 48_000.0);
        assert!(!envelope.is_empty() && envelope.iter().all(|&value| value == 0.0));
    }

    #[test]
    fn envelope_runs_at_500_hz_for_a_48_khz_recording() {
        let envelope = extract_amplitude_envelope(&vec![0.5; 48_000 * 10], 48_000.0);
        assert_eq!(envelope.len(), 5_000);
    }

    /// Consumers assume `ENVELOPE_RATE` = 500. An integer decimation step of
    /// floor(44_100 / 500) = 88 would yield 501.1 envelope samples per second
    /// and run the gust timeline 0.22 % off (1.3 s over ten minutes); the
    /// fractional step gives exactly 500.
    #[test]
    fn envelope_rate_is_500_hz_for_44_khz_recordings() {
        let envelope = extract_amplitude_envelope(&vec![0.5; 44_100 * 10], 44_100.0);
        assert_eq!(envelope.len(), 5_000);
    }

    /// Body (< 800 Hz) and air (> 2 kHz) envelopes follow their own band's
    /// dynamics: a 1 Hz swell on a 300 Hz tone must show up in the body
    /// envelope only, a 0.5 Hz swell on a 6 kHz tone in the air envelope only.
    #[test]
    fn band_envelopes_follow_their_own_band() {
        let sample_rate = 48_000.0;
        let mix: Vec<f32> = (0..(10.0 * sample_rate) as usize)
            .map(|index| {
                let time = index as f32 / sample_rate;
                let low = (1.0 + 0.7 * (TAU * 1.0 * time).sin()) * (TAU * 300.0 * time).sin();
                let high = (1.0 + 0.7 * (TAU * 0.5 * time).sin()) * (TAU * 6_000.0 * time).sin();
                low + high
            })
            .collect();
        let (body, air) = extract_band_envelopes(&mix, sample_rate);
        let low_swell: Vec<f32> = (0..body.len())
            .map(|index| 1.0 + 0.7 * (TAU * index as f32 / ENVELOPE_RATE).sin())
            .collect();
        let high_swell: Vec<f32> = (0..air.len())
            .map(|index| 1.0 + 0.7 * (TAU * 0.5 * index as f32 / ENVELOPE_RATE).sin())
            .collect();
        assert!(correlation(&body[500..], &low_swell[500..]) > 0.95);
        assert!(correlation(&air[500..], &high_swell[500..]) > 0.95);
        assert!(correlation(&body[500..], &high_swell[500..]).abs() < 0.4);
        assert!(correlation(&air[500..], &low_swell[500..]).abs() < 0.4);
    }

    #[test]
    fn band_split_constants_match_the_wind_layer_crossovers() {
        assert_eq!(BAND_SPLIT_LOW_HZ, 800.0);
        assert_eq!(BAND_SPLIT_HIGH_HZ, 2_000.0);
        assert_eq!(ENVELOPE_RATE, 500.0);
    }
}
