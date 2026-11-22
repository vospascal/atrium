//! The spectrogram's frequency axis and window, one definition for the
//! picture and the live analyser: rows log-spaced from 20 Hz to 20 kHz and
//! an 85 ms Hann window.

pub(crate) const LOWEST_HERTZ: f64 = 20.0;
pub(crate) const HIGHEST_HERTZ: f64 = 20_000.0;
/// The analysis window (s): 4096 points at 48 kHz, 12 Hz bins.
const WINDOW_SECONDS: f64 = 0.085;

/// Where `frequency` sits between 20 Hz (0) and 20 kHz (1).
pub(crate) fn frequency_fraction(frequency: f64) -> f64 {
    (frequency / LOWEST_HERTZ).ln() / (HIGHEST_HERTZ / LOWEST_HERTZ).ln()
}

/// The frequency at `fraction` of the way from 20 Hz to 20 kHz.
pub(crate) fn fraction_frequency(fraction: f64) -> f64 {
    LOWEST_HERTZ * (HIGHEST_HERTZ / LOWEST_HERTZ).powf(fraction)
}

/// The window's length at `sample_rate`: [`WINDOW_SECONDS`] rounded up to a
/// power of two, at least 256.
pub(crate) fn window_length(sample_rate: u32) -> usize {
    ((WINDOW_SECONDS * sample_rate as f64) as usize)
        .next_power_of_two()
        .max(256)
}

/// A periodic Hann window of `length` points.
pub(crate) fn hann(length: usize) -> Vec<f32> {
    (0..length)
        .map(|index| {
            (0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / length as f64).cos()) as f32
        })
        .collect()
}
