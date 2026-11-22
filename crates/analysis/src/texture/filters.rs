//! The filters of McDermott and Simoncelli's texture model (Neuron 71, 2011,
//! Experimental Procedures): the cochlear filter bank and the two banks of
//! modulation filters.
//!
//! The cochlear bank is built as the McDermott lab's pycochleagram builds it
//! (`erbfilter.make_erb_cos_filters`, Clear BSD licence; the construction
//! is followed here, no code is copied): 32 cutoffs equally spaced on the
//! ERB-number scale from 20 Hz to 10 kHz, 30 half-cosine bandpass filters
//! each spanning two steps (so their centres run from 52 to 8844 Hz), and a
//! lowpass and a highpass at the ends shaped so the squared responses sum to
//! one at every frequency.

/// Glasberg and Moore's (1990) ERB-number scale, as pycochleagram writes it:
/// 9.265·ln(1 + f / (24.7·9.265)).
pub(crate) fn erb_number(frequency: f64) -> f64 {
    9.265 * (1.0 + frequency / (24.7 * 9.265)).ln()
}

pub(crate) fn erb_frequency(number: f64) -> f64 {
    24.7 * 9.265 * ((number / 9.265).exp() - 1.0)
}

/// Bandpass filters in the cochlear bank.
const COCHLEAR_BANDPASS: usize = 30;
const COCHLEAR_LOW_HERTZ: f64 = 20.0;
const COCHLEAR_HIGH_HERTZ: f64 = 10_000.0;

/// What a cochlear subband passes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Shape {
    /// Everything below the second cutoff, fading out where the first
    /// bandpass fades in.
    Lowpass,
    /// The `index`th half-cosine bandpass.
    Bandpass(usize),
    /// Everything above the last-but-one cutoff.
    Highpass,
}

/// The cochlear filter bank for one sample rate.
pub(crate) struct CochlearBank {
    cutoffs: Vec<f64>,
    nyquist: f64,
}

/// One subband's extent and where it is centred (Hz).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Extent {
    pub(crate) low: f64,
    pub(crate) centre: f64,
    pub(crate) high: f64,
}

impl CochlearBank {
    pub(crate) fn new(sample_rate: u32) -> Self {
        let nyquist = sample_rate as f64 / 2.0;
        let low = erb_number(COCHLEAR_LOW_HERTZ);
        let high = erb_number(COCHLEAR_HIGH_HERTZ.min(nyquist));
        let steps = COCHLEAR_BANDPASS + 1;
        let cutoffs = (0..=steps)
            .map(|step| erb_frequency(low + (high - low) * step as f64 / steps as f64))
            .collect();
        Self { cutoffs, nyquist }
    }

    /// The lowpass, the 30 bandpasses from low to high, the highpass.
    pub(crate) fn shapes(&self) -> Vec<Shape> {
        std::iter::once(Shape::Lowpass)
            .chain((0..COCHLEAR_BANDPASS).map(Shape::Bandpass))
            .chain(std::iter::once(Shape::Highpass))
            .collect()
    }

    /// Where `shape` passes anything, and its centre: a bandpass's peak, or
    /// for the lowpass and highpass the half-power point.
    pub(crate) fn extent(&self, shape: Shape) -> Extent {
        let cutoffs = &self.cutoffs;
        let midpoint =
            |first: f64, second: f64| erb_frequency((erb_number(first) + erb_number(second)) / 2.0);
        match shape {
            Shape::Lowpass => Extent {
                low: 0.0,
                centre: midpoint(cutoffs[0], cutoffs[1]),
                high: cutoffs[1],
            },
            Shape::Bandpass(index) => Extent {
                low: cutoffs[index],
                centre: cutoffs[index + 1],
                high: cutoffs[index + 2],
            },
            Shape::Highpass => Extent {
                low: cutoffs[COCHLEAR_BANDPASS],
                centre: midpoint(cutoffs[COCHLEAR_BANDPASS], cutoffs[COCHLEAR_BANDPASS + 1]),
                high: self.nyquist,
            },
        }
    }

    /// The amplitude of `shape` at `frequency` Hz.
    pub(crate) fn response(&self, shape: Shape, frequency: f64) -> f64 {
        let cutoffs = &self.cutoffs;
        match shape {
            Shape::Bandpass(index) => self.bandpass(index, frequency),
            Shape::Lowpass if frequency < cutoffs[1] => {
                (1.0 - self.bandpass(0, frequency).powi(2)).max(0.0).sqrt()
            }
            Shape::Highpass if frequency > cutoffs[COCHLEAR_BANDPASS] => (1.0
                - self.bandpass(COCHLEAR_BANDPASS - 1, frequency).powi(2))
            .max(0.0)
            .sqrt(),
            _ => 0.0,
        }
    }

    /// cos((erb(f) − mid) / range · π) strictly between the filter's
    /// cutoffs, zero elsewhere.
    fn bandpass(&self, index: usize, frequency: f64) -> f64 {
        let low = self.cutoffs[index];
        let high = self.cutoffs[index + 2];
        if frequency <= low || frequency >= high {
            return 0.0;
        }
        let low_number = erb_number(low);
        let high_number = erb_number(high);
        let middle = (low_number + high_number) / 2.0;
        ((erb_number(frequency) - middle) / (high_number - low_number) * std::f64::consts::PI).cos()
    }
}

/// A half-cosine modulation filter (the toolbox's `make_constQ_cos_filters`):
/// amplitude cos((f − centre) / (2·centre/Q) · π) within centre·(1 ± 1/Q),
/// so its half-power bandwidth is centre / Q.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ModulationFilter {
    pub(crate) centre: f64,
    quality: f64,
}

impl ModulationFilter {
    pub(crate) fn response(&self, frequency: f64) -> f64 {
        let half_span = self.centre / self.quality;
        if (frequency - self.centre).abs() >= half_span {
            return 0.0;
        }
        ((frequency - self.centre) / (2.0 * half_span) * std::f64::consts::PI).cos()
    }
}

/// The 20 filters of the modulation power statistic: centres equally spaced
/// on a log scale from 0.5 to 200 Hz, Q = 2.
pub(crate) fn modulation_power_filters() -> Vec<ModulationFilter> {
    let count = 20;
    (0..count)
        .map(|index| ModulationFilter {
            centre: 0.5 * 400f64.powf(index as f64 / (count - 1) as f64),
            quality: 2.0,
        })
        .collect()
}

/// The octave filters of the C1 statistic: centres 1.5625 to 100 Hz in
/// octave steps, Q = √2. C1 uses the second to the seventh.
pub(crate) fn c1_filters() -> Vec<ModulationFilter> {
    (0..7)
        .map(|index| ModulationFilter {
            centre: 1.5625 * 2f64.powi(index),
            quality: std::f64::consts::SQRT_2,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The paper's centres: 52 to 8844 Hz.
    #[test]
    fn bandpass_centres_span_the_papers_range() {
        let bank = CochlearBank::new(48_000);
        let first = bank.extent(Shape::Bandpass(0)).centre;
        let last = bank.extent(Shape::Bandpass(29)).centre;
        assert!((first - 52.0).abs() < 1.0, "{first}");
        assert!((last - 8844.0).abs() < 10.0, "{last}");
        assert_eq!(bank.shapes().len(), 32);
    }

    /// The squared responses sum to one from DC to Nyquist: the bank passes
    /// a signal's power unchanged.
    #[test]
    fn squared_responses_sum_to_one() {
        let bank = CochlearBank::new(48_000);
        for step in 0..2400 {
            let frequency = step as f64 * 10.0 + 0.5;
            let sum: f64 = bank
                .shapes()
                .into_iter()
                .map(|shape| bank.response(shape, frequency).powi(2))
                .sum();
            assert!((sum - 1.0).abs() < 1e-9, "{frequency} Hz: {sum}");
        }
    }

    #[test]
    fn modulation_filters_have_their_quality() {
        let filters = modulation_power_filters();
        assert_eq!(filters.len(), 20);
        assert!((filters[19].centre - 200.0).abs() < 1e-9);
        // Q = 2: half power at centre ± centre/4.
        let filter = filters[10];
        let half_power = filter.response(filter.centre * 1.25).powi(2);
        assert!((half_power - 0.5).abs() < 1e-9, "{half_power}");
        let octaves = c1_filters();
        assert_eq!(octaves[6].centre, 100.0);
    }
}
