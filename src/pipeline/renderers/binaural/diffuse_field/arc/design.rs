//! One arc's two ear filters, designed so that through the headphone
//! equaliser the two ears get what this head hears of the arc: each ear's
//! power, and the two ears' cross-spectrum, with its level difference, its
//! coherence and its phase, the time by which the near ear hears it first.
//!
//! All round, the ears' cross-spectrum is real and the existing pair of
//! filters (`ear_filters`) gives it: the sound and one echo of it, of
//! opposite sign in the two ears, the echo as strong as the coherence asks.
//! From an arc it is complex, γ = |γ| e^{iφ}, and the same two parts give it
//! when the right ear's mix of them is turned:
//!
//! ```text
//! left  = a + b·D          right = γ*·(a + b·D) + s·(b − a·D)
//! ```
//!
//! with a² + b² = 1 and s = √(1 − |γ|²). Within a third octave the echo D
//! is unrelated to the sound, so each ear keeps its power and the ears'
//! cross-power is γ: a·b·D terms average out, and b − a·D is unrelated to
//! a + b·D. With γ real and a, b chosen as all round, the right ear is
//! a − b·D: the all-round pair is this one's special case.
//!
//! The coherence is the head's, averaged over the third octave around each
//! frequency as the ear averages it. An arc to one side reaches the far ear
//! later, so its cross-spectrum turns with frequency; averaged as it is,
//! the turn would cancel what is alike. It is first turned back by the
//! arc's interaural time difference (`interaural_lag`), averaged, and the
//! time difference given back, half to each ear: the near ear early, the far
//! ear late. What remains of the turn within a band is the head's.
//!
//! Each ear's colour is a minimum-phase filter whose power is the arc's
//! over the head's diffuse field, each averaged over the same third octave
//! and held below 100 Hz and above 16 kHz, divided by what the headphone
//! equaliser does to it. Through the equaliser each ear then gets exactly
//! the arc's share of the diffuse field; all round that share is one, flat,
//! as the all-round filters give it. The two minimum phases differ, and the
//! right ear's coherence is turned back by that difference, so the ears'
//! cross-spectrum is the head's after the equaliser too.

use realfft::num_complex::Complex;

use super::super::ear_filters::{echo_delay_phase, EarFilters, FADE_OUT_SECONDS, LEAD_IN_SECONDS};
use super::super::inverse::{held_smoothed_power, EqualiserFilters, HELD_ABOVE_HZ, HELD_BELOW_HZ};
use super::super::minimum_phase::{minimum_phase_spectrum, to_time, truncated_with_fade_out};
use super::super::statistics::DiffuseStatistics;
use super::super::third_octave::{third_octave_bins, RunningSum};
use super::horizon::{ArcSpectra, Horizon};

/// The interaural time difference is never taken as more than this, either
/// way: the ±1 ms over which the ear compares its two inputs, more than the
/// largest the head has.
const LARGEST_LAG_SECONDS: f64 = 0.001;

/// The time difference is read from the cross-spectrum up to here, where
/// it is carried by the phase; above it the ear follows level differences
/// and envelopes.
const TIME_DIFFERENCE_BELOW_HZ: f64 = 1500.0;

/// How often the echo's phase turns over across a third octave: five, where
/// all round it is two. An arc to one side, or a half circle ahead, asks
/// for level and time differences between the ears that all round does
/// not: within a third octave the echo adds a little to one ear and takes
/// from the other, and that has to average out better. Measured on a grid
/// of two to six turns and a longest echo of 25 to 40 ms (root README, "An
/// arc of ambient sound"), five turns and 30 ms keep the ears' level and
/// time differences closest to the head's while 95 % of a click still
/// arrives within 13 ms.
const TURNS_PER_THIRD_OCTAVE: f64 = 5.0;

/// The longest echo delay, as all round: below 750 Hz the echo waits
/// 30 ms.
const LONGEST_ECHO_SECONDS: f64 = 0.03;

/// How long each filter is: twice the longest echo, as all round.
pub(in crate::pipeline::renderers::binaural) const FILTER_SECONDS: f64 = 0.06;

/// What the arc filters are designed from: the head along the horizon, its
/// diffuse field and its headphone equaliser, on one frequency grid.
pub(in crate::pipeline::renderers::binaural) struct ArcHead {
    horizon: Horizon,
    sample_rate: f32,
    grid_size: usize,
    /// Each ear's diffuse-field power averaged over the third octave around
    /// every bin and held, as the equaliser divides it out.
    diffuse_left: Vec<f64>,
    diffuse_right: Vec<f64>,
    /// The headphone equaliser's response on the grid, as it was designed.
    equaliser_left: Vec<Complex<f64>>,
    equaliser_right: Vec<Complex<f64>>,
    echo_phase: Vec<f64>,
}

impl ArcHead {
    /// Called when the HRIRs are loaded, never on the audio thread.
    pub(in crate::pipeline::renderers::binaural) fn new(
        horizon: Horizon,
        sample_rate: f32,
        grid_size: usize,
        statistics: &DiffuseStatistics,
        equaliser: &EqualiserFilters,
    ) -> Self {
        let bin_hz = sample_rate as f64 / grid_size as f64;
        let response = |filter: &[f32]| {
            let mut planner = realfft::RealFftPlanner::<f64>::new();
            let forward = planner.plan_fft_forward(grid_size);
            let mut time = forward.make_input_vec();
            for (slot, &sample) in time.iter_mut().zip(filter) {
                *slot = sample as f64;
            }
            let mut spectrum = forward.make_output_vec();
            forward
                .process(&mut time, &mut spectrum)
                .expect("forward transform of the equaliser");
            spectrum
        };
        Self {
            horizon,
            sample_rate,
            grid_size,
            diffuse_left: held_smoothed_power(&statistics.left_power, bin_hz),
            diffuse_right: held_smoothed_power(&statistics.right_power, bin_hz),
            equaliser_left: response(&equaliser.left),
            equaliser_right: response(&equaliser.right),
            echo_phase: echo_delay_phase(
                grid_size / 2 + 1,
                bin_hz,
                TURNS_PER_THIRD_OCTAVE,
                LONGEST_ECHO_SECONDS,
            ),
        }
    }

    /// The three spectra of the arc `width_degrees` wide centred
    /// `centre_degrees` anticlockwise from straight ahead.
    pub(in crate::pipeline::renderers::binaural) fn spectra(
        &self,
        centre_degrees: f64,
        width_degrees: f64,
    ) -> ArcSpectra {
        self.horizon
            .arc(centre_degrees, width_degrees, self.grid_size)
    }

    /// The two ears' filters for the arc `width_degrees` wide centred
    /// `centre_degrees` anticlockwise from straight ahead.
    pub(in crate::pipeline::renderers::binaural) fn design(
        &self,
        centre_degrees: f64,
        width_degrees: f64,
    ) -> EarFilters {
        let spectra = self.spectra(centre_degrees, width_degrees);
        let bins = self.grid_size / 2 + 1;
        let bin_hz = self.sample_rate as f64 / self.grid_size as f64;
        let lag = interaural_lag(&spectra.cross, bin_hz);
        let turn = |bin: usize, seconds: f64| {
            Complex::from_polar(1.0, std::f64::consts::TAU * bin as f64 * bin_hz * seconds)
        };
        let turned_back: Vec<Complex<f64>> = spectra
            .cross
            .iter()
            .enumerate()
            .map(|(bin, cross)| cross * turn(bin, -lag))
            .collect();
        let left = held_smoothed_power(&spectra.left_power, bin_hz);
        let right = held_smoothed_power(&spectra.right_power, bin_hz);
        let cross = held_smoothed_cross(&turned_back, bin_hz);

        let coherence: Vec<Complex<f64>> = (0..bins)
            .map(|bin| within_unit(cross[bin] / (left[bin] * right[bin]).sqrt()))
            .collect();
        let colour = |share: &[f64], diffuse: &[f64], equaliser: &[Complex<f64>]| {
            let power: Vec<f64> = (0..bins)
                .map(|bin| share[bin] / diffuse[bin] / equaliser[bin].norm_sqr())
                .collect();
            minimum_phase_spectrum(&power, self.grid_size)
        };
        let left_colour = colour(&left, &self.diffuse_left, &self.equaliser_left);
        let right_colour = colour(&right, &self.diffuse_right, &self.equaliser_right);
        // What the two minimum phases add between the ears, after the
        // equaliser, taken back out of the coherence the right ear is given.
        let asked: Vec<Complex<f64>> = (0..bins)
            .map(|bin| {
                let between = (left_colour[bin] * self.equaliser_left[bin])
                    * (right_colour[bin] * self.equaliser_right[bin]).conj();
                if between.norm() > 0.0 {
                    coherence[bin] * (between / between.norm()).conj()
                } else {
                    coherence[bin]
                }
            })
            .collect();
        let (left_parts, right_parts) = self.sound_and_echo(&asked);
        let mut left_spectrum = Vec::with_capacity(bins);
        let mut right_spectrum = Vec::with_capacity(bins);
        for bin in 0..bins {
            let lead_in = turn(bin, -LEAD_IN_SECONDS);
            left_spectrum.push(left_colour[bin] * turn(bin, lag / 2.0) * left_parts[bin] * lead_in);
            right_spectrum
                .push(right_colour[bin] * turn(bin, -lag / 2.0) * right_parts[bin] * lead_in);
        }
        let filter_length = (FILTER_SECONDS * self.sample_rate as f64).round() as usize;
        let fade_length = (FADE_OUT_SECONDS * self.sample_rate as f64).round() as usize;
        let finish = |spectrum: Vec<Complex<f64>>| {
            truncated_with_fade_out(
                to_time(spectrum, self.grid_size),
                filter_length,
                fade_length,
            )
        };
        EarFilters {
            left: finish(left_spectrum),
            right: finish(right_spectrum),
        }
    }
}

impl ArcHead {
    /// The sound and its echo at each ear, per bin, that give the two ears
    /// the cross-spectrum `coherence` at unit power, once the echo has
    /// turned enough to be unrelated: left a + b·D, right
    /// γ*·(a + b·D) + s·(b − a·D).
    fn sound_and_echo(&self, coherence: &[Complex<f64>]) -> (Vec<Complex<f64>>, Vec<Complex<f64>>) {
        coherence
            .iter()
            .zip(&self.echo_phase)
            .map(|(coherence, &phase)| {
                let alike = coherence.norm();
                let sound = ((1.0 + alike) / 2.0).sqrt();
                let echo_share = ((1.0 - alike) / 2.0).sqrt();
                let unrelated = (1.0 - alike * alike).max(0.0).sqrt();
                let echo = Complex::from_polar(1.0, phase);
                let sound_and_echo = echo * echo_share + sound;
                let echo_and_sound = echo * -sound + echo_share;
                (
                    sound_and_echo,
                    coherence.conj() * sound_and_echo + echo_and_sound * unrelated,
                )
            })
            .unzip()
    }
}

/// `value`, brought within the unit circle if it lies outside.
fn within_unit(value: Complex<f64>) -> Complex<f64> {
    if value.norm() > 1.0 {
        value / value.norm()
    } else {
        value
    }
}

/// The arc's interaural time difference, positive when the left ear hears
/// first: the slope of the cross-spectrum's phase over angular frequency
/// from 100 Hz to 1.5 kHz, where the time is carried by the phase, each
/// frequency weighted by how strongly the ears are alike there. The phase is
/// followed up from 100 Hz to the nearest half turn: where the ears turn
/// from alike to opposed (all round about 500 Hz) the cross-spectrum
/// changes sign, which is not a time.
pub(in crate::pipeline::renderers::binaural) fn interaural_lag(
    cross: &[Complex<f64>],
    bin_hz: f64,
) -> f64 {
    let lowest = (HELD_BELOW_HZ / bin_hz).ceil() as usize;
    let highest = ((TIME_DIFFERENCE_BELOW_HZ / bin_hz).floor() as usize).min(cross.len() - 1);
    let half_turn = std::f64::consts::PI;
    let mut previous = 0.0;
    let (mut numerator, mut denominator) = (0.0, 0.0);
    for (bin, value) in cross.iter().enumerate().take(highest + 1).skip(lowest) {
        let phase = value.arg();
        let phase = phase + half_turn * ((previous - phase) / half_turn).round();
        previous = phase;
        let angular = std::f64::consts::TAU * bin as f64 * bin_hz;
        let weight = value.norm();
        numerator += weight * phase * angular;
        denominator += weight * angular * angular;
    }
    if denominator > 0.0 {
        (numerator / denominator).clamp(-LARGEST_LAG_SECONDS, LARGEST_LAG_SECONDS)
    } else {
        0.0
    }
}

/// `cross` averaged over the third octave around every bin, held below
/// 100 Hz and above 16 kHz: over the same bands as `held_smoothed_power`.
fn held_smoothed_cross(cross: &[Complex<f64>], bin_hz: f64) -> Vec<Complex<f64>> {
    let real: Vec<f64> = cross.iter().map(|value| value.re).collect();
    let imaginary: Vec<f64> = cross.iter().map(|value| value.im).collect();
    let (real, imaginary) = (RunningSum::new(&real), RunningSum::new(&imaginary));
    let bins = cross.len();
    (0..bins)
        .map(|bin| {
            let centre = (bin as f64 * bin_hz).clamp(HELD_BELOW_HZ, HELD_ABOVE_HZ);
            let band = third_octave_bins(centre, bin_hz, bins);
            let count = (band.1 - band.0) as f64;
            Complex::new(real.over(band) / count, imaginary.over(band) / count)
        })
        .collect()
}
