//! Small numerical helpers shared by the measures: decibels, means and
//! percentiles. Nothing here knows what it measures.

/// The power a silent signal is reported at, so a level is always a finite
/// number of decibels (−200 dB) and the report stays valid JSON.
pub(crate) const SILENCE_POWER: f64 = 1e-20;

/// A power (mean square) in decibels, silence floored at [`SILENCE_POWER`].
pub(crate) fn power_decibels(power: f64) -> f64 {
    10.0 * power.max(SILENCE_POWER).log10()
}

/// Decibels back to a power.
pub(crate) fn decibels_power(decibels: f64) -> f64 {
    10f64.powf(decibels / 10.0)
}

pub(crate) fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

pub(crate) fn mean_square(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples
        .iter()
        .map(|sample| (*sample as f64).powi(2))
        .sum::<f64>()
        / samples.len() as f64
}

pub(crate) fn standard_deviation(values: &[f64]) -> f64 {
    let average = mean(values);
    mean(
        &values
            .iter()
            .map(|value| (value - average).powi(2))
            .collect::<Vec<_>>(),
    )
    .sqrt()
}

/// The value below which `fraction` of `values` lie, linearly interpolated
/// between neighbours (the definition numpy and most acoustics software use
/// by default). `None` for no values.
pub(crate) fn quantile(values: &[f64], fraction: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let position = fraction.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let below = position.floor() as usize;
    let above = position.ceil() as usize;
    let share = position - below as f64;
    Some(sorted[below] * (1.0 - share) + sorted[above] * share)
}

/// Pearson's correlation of two equally long series; `None` when either does
/// not vary.
pub(crate) fn correlation(first: &[f64], second: &[f64]) -> Option<f64> {
    let first_mean = mean(first);
    let second_mean = mean(second);
    let mut product = 0.0;
    let mut first_square = 0.0;
    let mut second_square = 0.0;
    for (a, b) in first.iter().zip(second) {
        product += (a - first_mean) * (b - second_mean);
        first_square += (a - first_mean).powi(2);
        second_square += (b - second_mean).powi(2);
    }
    let scale = (first_square * second_square).sqrt();
    (scale > 0.0).then(|| product / scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_interpolate_between_neighbours() {
        let values = [4.0, 1.0, 3.0, 2.0, 5.0];
        assert_eq!(quantile(&values, 0.5), Some(3.0));
        assert_eq!(quantile(&values, 0.0), Some(1.0));
        assert_eq!(quantile(&values, 1.0), Some(5.0));
        // Position 0.1 × 4 = 0.4 between 1 and 2.
        assert!((quantile(&values, 0.1).unwrap() - 1.4).abs() < 1e-12);
        assert_eq!(quantile(&[], 0.5), None);
    }

    #[test]
    fn decibels_round_trip_and_floor_silence() {
        assert!((power_decibels(0.01) + 20.0).abs() < 1e-12);
        assert!((decibels_power(-20.0) - 0.01).abs() < 1e-15);
        assert_eq!(power_decibels(0.0), -200.0);
    }

    #[test]
    fn correlation_of_a_line_and_its_negative() {
        let rising = [1.0, 2.0, 3.0, 4.0];
        let falling = [4.0, 3.0, 2.0, 1.0];
        assert!((correlation(&rising, &rising).unwrap() - 1.0).abs() < 1e-12);
        assert!((correlation(&rising, &falling).unwrap() + 1.0).abs() < 1e-12);
        assert_eq!(correlation(&rising, &[1.0; 4]), None);
    }
}
