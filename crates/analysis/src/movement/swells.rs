//! Slow swells of the broadband level and their shape: whether they rise
//! quickly and fall slowly (damped, as an impact or a breaking wave), rise
//! slowly and stop quickly (ramped, which Stecker and Hafter 2000 found
//! louder at equal energy), or fade both ways alike.
//!
//! The level (1 s frames every 0.5 s) is smoothed over 2 s. A swell is a
//! peak standing at least 3 dB over the higher of its two troughs, each
//! trough the lowest point between the peak and the next higher ground on
//! that side (a topographic prominence). Its rise runs from the trough
//! before to the peak, its decay from the peak to the trough after. Swells
//! cut off by the file's start or end are not counted.

use crate::report::Swells;
use crate::statistics::{mean, quantile};

/// Frames averaged for the smoothing (2 s at 0.5 s steps).
const SMOOTHING_FRAMES: usize = 4;
/// A swell rises and falls at least this much (dB).
const PROMINENCE_DECIBELS: f64 = 3.0;

/// The swells of `levels` (dB, `step_seconds` apart).
pub(crate) fn swells(levels: &[f64], step_seconds: f64) -> Swells {
    let smooth: Vec<f64> = levels.windows(SMOOTHING_FRAMES).map(mean).collect();
    let mut rises = Vec::new();
    let mut decays = Vec::new();
    let last = smooth.len().saturating_sub(1);
    for peak in 1..last {
        let here = smooth[peak];
        if here < smooth[peak - 1] || here <= smooth[peak + 1] {
            continue;
        }
        let Some(before) = trough(&smooth, peak, -1) else {
            continue;
        };
        let Some(after) = trough(&smooth, peak, 1) else {
            continue;
        };
        if here - smooth[before].max(smooth[after]) < PROMINENCE_DECIBELS {
            continue;
        }
        rises.push((peak - before) as f64 * step_seconds);
        decays.push((after - peak) as f64 * step_seconds);
    }
    let changes: Vec<f64> = smooth.windows(2).map(|pair| pair[1] - pair[0]).collect();
    Swells {
        count: rises.len(),
        rise_median_seconds: quantile(&rises, 0.5),
        decay_median_seconds: quantile(&decays, 0.5),
        rate_of_change_skewness: skewness(&changes),
    }
}

/// The lowest point from `peak` in `direction` before the level rises above
/// the peak again; None when the file ends first and the lowest point is
/// its last frame (a swell cut off).
fn trough(levels: &[f64], peak: usize, direction: isize) -> Option<usize> {
    let mut lowest = peak;
    let mut index = peak as isize + direction;
    while index >= 0 && (index as usize) < levels.len() {
        let here = levels[index as usize];
        if here > levels[peak] {
            return Some(lowest);
        }
        if here < levels[lowest] {
            lowest = index as usize;
        }
        index += direction;
    }
    let edge = if direction < 0 { 0 } else { levels.len() - 1 };
    (lowest != edge).then_some(lowest)
}

/// The third standardised moment; None with fewer than eight values or none
/// varying.
fn skewness(values: &[f64]) -> Option<f64> {
    if values.len() < 8 {
        return None;
    }
    let average = mean(values);
    let second = mean(
        &values
            .iter()
            .map(|v| (v - average).powi(2))
            .collect::<Vec<_>>(),
    );
    let third = mean(
        &values
            .iter()
            .map(|v| (v - average).powi(3))
            .collect::<Vec<_>>(),
    );
    (second > 0.0).then(|| third / second.powf(1.5))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A level that rises 10 dB in 2 s and falls back in 8 s, every 10 s,
    /// for two minutes, sampled every 0.5 s: about eleven whole swells,
    /// quick rises skewing the changes positive. The 2 s smoothing moves the
    /// trough to where the window's two ends are level: 1.6 s of the fall
    /// (1.25 dB/s) against 0.4 s of the rise (5 dB/s), so 0.4 s after it;
    /// and the peak to 0.4 s of rise against 1.6 s of fall, 1.6 s after it.
    /// So a 2 s / 8 s swell reads 3.2 s / 6.8 s, ± a 0.5 s step: still
    /// plainly quick up and slow down. Time-reversed, 6.8 s / 3.2 s, skewed
    /// negative.
    #[test]
    fn damped_and_ramped_swells() {
        let step = 0.5;
        let damped: Vec<f64> = (0..240)
            .map(|index| {
                let phase = (index as f64 * step) % 10.0;
                if phase < 2.0 {
                    10.0 * phase / 2.0
                } else {
                    10.0 * (10.0 - phase) / 8.0
                }
            })
            .collect();
        let reading = swells(&damped, step);
        assert!((10..=12).contains(&reading.count), "{reading:?}");
        let rise = reading.rise_median_seconds.expect("swells");
        let decay = reading.decay_median_seconds.expect("swells");
        assert!((rise - 3.2).abs() <= 0.5, "{rise}");
        assert!((decay - 6.8).abs() <= 0.5, "{decay}");
        assert!(reading.rate_of_change_skewness.expect("enough") > 1.0);

        let ramped: Vec<f64> = damped.iter().rev().copied().collect();
        let reading = swells(&ramped, step);
        let rise = reading.rise_median_seconds.expect("swells");
        let decay = reading.decay_median_seconds.expect("swells");
        assert!((rise - 6.8).abs() <= 0.5, "{rise}");
        assert!((decay - 3.2).abs() <= 0.5, "{decay}");
        assert!(reading.rate_of_change_skewness.expect("enough") < -1.0);
    }

    /// A steady level has no swells and no skew to speak of.
    #[test]
    fn a_steady_level_has_no_swells() {
        let reading = swells(&[-30.0; 100], 0.5);
        assert_eq!(reading.count, 0);
        assert!(reading.rate_of_change_skewness.is_none());
    }
}
