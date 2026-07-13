// Procedural audio synthesis — environmental sound sources.
//
// Ported from the TypeScript spatial-audio-garden AudioWorklet processors.
// All generators are allocation-free in the hot path and suitable for
// real-time audio threads (no heap alloc, no locks, no syscalls).
//
// Architecture difference from TS: the original processors included inline
// reverb (Freeverb). Here, sources produce dry mono samples — the existing
// FdnReverb processor in the audio pipeline handles reverberation.

pub mod canopy_wind;
pub mod field_wind;
pub mod noise;
pub mod rain;
pub mod rain_v2;
pub mod soft_wind;
pub mod storm_wind;
pub mod wave;

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
    let step = (sample_rate / ENVELOPE_RATE).max(1.0) as usize;

    let mut smooth = 0.0_f32;
    let mut envelope = Vec::with_capacity(samples.len() / step + 1);
    for (n, &x) in samples.iter().enumerate() {
        smooth = a * smooth + (1.0 - a) * pre(x).abs();
        if n % step == 0 {
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
