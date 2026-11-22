//! Step 7: what each take is: how long, how loud while it sounds, how far
//! above its bed, and how bright.
//!
//! The level is `power_while_sounding` of the take as the engine will play
//! it (its channels' mean, the fades in), the level the engine calibrates a
//! kind's takes with. A take whose band level stands less than
//! [`FLAG_BELOW_SNR_DECIBELS`] above its local bed is flagged: its bed will
//! gate in and out with it.

use super::edges::faded;
use super::events::Found;
use crate::level::power_while_sounding;
use crate::signal::Signal;
use crate::spectrum::Spectrum;
use crate::statistics::{decibels_power, power_decibels};

pub(super) const FLAG_BELOW_SNR_DECIBELS: f64 = 20.0;
/// The centroid is taken from here up to Nyquist (Hz).
const LOWEST_HERTZ: f64 = 20.0;

pub(super) struct Measured {
    pub(super) level_decibels: f64,
    pub(super) snr_decibels: f64,
    pub(super) centroid_hertz: f64,
}

/// The take of frames `first` to `end` (fades of `fade` frames), whose events
/// are `events`, over `levels` and `bed` (dB, reading by reading).
pub(super) fn measure(
    signal: &Signal,
    (first, end, fade): (usize, usize, usize),
    events: &[Found],
    levels: &[f64],
    bed: &[f64],
) -> Measured {
    let mono = mono(&faded(signal, first, end, fade), signal.channels);
    let level_decibels = power_decibels(power_while_sounding(&[(&mono, signal.sample_rate)]));
    let (mut sounding, mut under) = (0.0, 0.0);
    for event in events {
        for reading in event.first..=event.last {
            sounding += decibels_power(levels[reading]);
            under += decibels_power(bed[reading]);
        }
    }
    let spectrum = Spectrum::of(&mono, signal.sample_rate);
    let (moment, total) = spectrum.frequency_moments(LOWEST_HERTZ, signal.sample_rate as f64 / 2.0);
    Measured {
        level_decibels,
        snr_decibels: power_decibels(sounding) - power_decibels(under),
        centroid_hertz: if total > 0.0 { moment / total } else { 0.0 },
    }
}

/// The channels' mean, frame by frame.
pub(super) fn mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}
