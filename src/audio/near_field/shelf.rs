//! The shape of one ear's correction: a gain below a corner frequency,
//! another above it, and a first-order slope between.

/// One ear's correction for a source close by. Its response is
///
/// ```text
/// |C(f)|² = (low² + high² w²) / (1 + w²),   w = f / corner
/// ```
///
/// a first-order shelf, as Spagnol et al. use, written as the two gains it
/// tends to and the corner of the low-pass that blends them: the form
/// [`super::EarFilter`] runs, and one whose parameters glide without a click.
/// The filter is its bilinear transform with the corner prewarped, so at a
/// sample rate `fs` the frequency axis bends to w = tan(π f / fs) / tan(π
/// corner / fs). The fit is made on the analogue shelf, once for every
/// rate; the tests hold the filter as it runs at 48 kHz to the sphere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EarCorrection {
    /// The gain at zero frequency, linear.
    pub(crate) low_gain: f32,
    /// The gain towards the top of the band, linear.
    pub(crate) high_gain: f32,
    /// The low-pass corner that blends the two (Hz).
    pub(crate) corner_hz: f32,
}

impl EarCorrection {
    /// No correction: the measured response as it is.
    pub(crate) const UNITY: Self = Self {
        low_gain: 1.0,
        high_gain: 1.0,
        corner_hz: 1000.0,
    };

    /// Whether this changes nothing at any frequency.
    pub(crate) fn is_unity(&self) -> bool {
        self.low_gain == 1.0 && self.high_gain == 1.0
    }

    /// |C(f)| at `frequency_hz` of the filter running at `sample_rate`.
    #[cfg(test)]
    pub(crate) fn magnitude_at(&self, frequency_hz: f64, sample_rate: f64) -> f64 {
        let corner =
            (self.corner_hz as f64).min(super::filter::HIGHEST_CORNER_SHARE as f64 * sample_rate);
        let warped = (std::f64::consts::PI * frequency_hz / sample_rate).tan()
            / (std::f64::consts::PI * corner / sample_rate).tan();
        shelf(self.low_gain as f64, self.high_gain as f64, warped)
    }
}

/// |C(f)| of the analogue shelf with gains `low` and `high` (linear) and
/// `corner_hz`, which the fit works with.
pub(super) fn analogue_magnitude(low: f64, high: f64, corner_hz: f64, frequency_hz: f64) -> f64 {
    shelf(low, high, frequency_hz / corner_hz)
}

/// The shelf's magnitude at `w` corners.
fn shelf(low: f64, high: f64, w: f64) -> f64 {
    ((low * low + high * high * w * w) / (1.0 + w * w)).sqrt()
}
