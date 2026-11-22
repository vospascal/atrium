//! Space: how alike the two ears (or the L/R pair) are, where sounds sit
//! between them, and for 5.1 where the energy goes in the room.
//!
//! - IACC per octave band (ISO 3382-1): the largest |normalised
//!   cross-correlation| within ±1 ms, over the whole file and per 100 ms
//!   frame. Each frame is transformed once, zero-padded so the correlation
//!   is linear, not circular; a band's correlation is the inverse transform
//!   of the frame's cross-spectrum in that band. The long-term value sums
//!   the frames' cross-spectra first.
//! - Width: 1 − IACC averaged over 500 Hz, 1 and 2 kHz (Hidaka et al. 1995,
//!   their IACC_E3 taken over the running sound).
//! - Envelope coherence per octave band: the correlation of the two band
//!   envelopes. Above about 1.5 kHz hearing follows envelopes, not the
//!   waveform, so waveform IACC of any diffuse sound is near zero there and
//!   says little; what is heard is whether the envelopes move together.
//! - ILD per octave band, right minus left, mean and spread over frames.
//! - ITD: the lag of the largest positive correlation, 88 Hz to 1.4 kHz
//!   (where hearing uses timing), over the frames where that peak exceeds
//!   0.5, refined by a parabola through the peak.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::bands::{octaves, Band};
use crate::envelope::band_envelope;
use crate::report::{Binaural, BinauralBand};
use crate::spectrum::Spectrum;
use crate::statistics::{correlation, mean, power_decibels, quantile, standard_deviation};

/// The frames the space measures and their series are taken over (s).
pub(crate) const FRAME_SECONDS: f64 = 0.1;
/// ISO 3382-1's lag range (s).
const LAG_SECONDS: f64 = 0.001;
/// The band ITD is read in (Hz): the lower edge of 125 Hz to the upper of 1 kHz.
const TIMING_LOW: f64 = 88.388;
const TIMING_HIGH: f64 = 1414.2;
/// A frame's ITD counts when its correlation peak exceeds this.
const TIMING_CORRELATION: f64 = 0.5;
/// The bands width averages (nominal Hz).
const WIDTH_BANDS: [f64; 3] = [500.0, 1000.0, 2000.0];

/// The octave band envelopes of one channel (Hilbert, 400 Hz,
/// uncompressed), for the envelope coherence.
pub(crate) fn octave_envelopes(spectrum: &Spectrum) -> Vec<Vec<f64>> {
    octaves(spectrum.sample_rate() as f64 / 2.0)
        .iter()
        .map(|band| band_envelope(spectrum, band.low, band.high, |_| 1.0, 1.0))
        .collect()
}

/// What a pair's frames give over time.
pub(crate) struct PairOverTime {
    pub(crate) width: Vec<Option<f64>>,
    pub(crate) ild_decibels: Vec<Option<f64>>,
}

/// Energies and cross-spectrum of one band, summed over what was added.
struct BandSums {
    cross: Vec<Complex<f64>>,
    left: f64,
    right: f64,
}

/// One octave band of a frame.
pub(crate) struct BandReading {
    pub(crate) left_energy: f64,
    pub(crate) right_energy: f64,
    /// IACC and ILD (R − L, dB), when both ears sound in the band.
    pub(crate) iacc: Option<f64>,
    pub(crate) ild_decibels: Option<f64>,
}

/// What one frame of the pair says.
pub(crate) struct FrameReading {
    pub(crate) bands: Vec<BandReading>,
    /// 1 − IACC over [`WIDTH_BANDS`], when all three sound.
    pub(crate) width: Option<f64>,
    /// ILD over every octave band at once (R − L, dB), when both sound.
    pub(crate) ild_decibels: Option<f64>,
    /// The lag of the timing band's largest positive correlation (samples,
    /// fractional), when that correlation exceeds [`TIMING_CORRELATION`].
    pub(crate) timing_lag: Option<f64>,
}

/// The pair read [`FRAME_SECONDS`] at a time: each frame transformed once,
/// zero-padded so the correlation is linear. The whole-file measures and the
/// live analyser read their frames through this one type, so a width or an
/// ILD cannot differ between them.
pub(crate) struct PairFrames {
    sample_rate: f64,
    frame_length: usize,
    size: usize,
    bands: Vec<(Band, std::ops::Range<usize>)>,
    timing_bins: std::ops::Range<usize>,
    broadband_bins: std::ops::Range<usize>,
    forward: Arc<dyn RealToComplex<f64>>,
    input: Vec<f64>,
    left: Vec<Complex<f64>>,
    right: Vec<Complex<f64>>,
    correlator: Correlator,
}

impl PairFrames {
    pub(crate) fn new(sample_rate: u32) -> Self {
        let rate = sample_rate as f64;
        let frame_length = ((FRAME_SECONDS * rate).round() as usize).max(1);
        let maximum_lag = (LAG_SECONDS * rate).round() as usize;
        let size = (frame_length + maximum_lag + 1).next_power_of_two();
        let bin_hertz = rate / size as f64;
        let bins_of = |low: f64, high: f64| {
            ((low / bin_hertz).ceil() as usize).max(1)
                ..((high / bin_hertz).ceil() as usize).min(size / 2)
        };
        let band_list = octaves(rate / 2.0);
        let broadband_bins = bins_of(
            band_list.first().map_or(0.0, |band| band.low),
            band_list.last().map_or(0.0, |band| band.high),
        );
        let mut planner = RealFftPlanner::<f64>::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        Self {
            sample_rate: rate,
            frame_length,
            size,
            bands: band_list
                .iter()
                .map(|band| (*band, bins_of(band.low, band.high)))
                .collect(),
            timing_bins: bins_of(TIMING_LOW, TIMING_HIGH),
            broadband_bins,
            input: forward.make_input_vec(),
            left: forward.make_output_vec(),
            right: forward.make_output_vec(),
            forward,
            correlator: Correlator {
                buffer: inverse.make_input_vec(),
                output: inverse.make_output_vec(),
                inverse,
                maximum_lag,
            },
        }
    }

    /// Samples in a frame.
    pub(crate) fn frame_length(&self) -> usize {
        self.frame_length
    }

    /// Read one frame: `left` and `right` are a frame long.
    pub(crate) fn read(&mut self, left: &[f32], right: &[f32]) -> FrameReading {
        self.transform(left, right);
        let bands: Vec<BandReading> = (0..self.bands.len())
            .map(|index| self.band(index))
            .collect();
        let width = self.width_of(&bands);
        FrameReading {
            width,
            ild_decibels: self.broadband_ild(),
            timing_lag: self.timing_lag(),
            bands,
        }
    }

    /// Only the width and the ILD of one frame, as `read` gives them, for
    /// the live analyser: the bands width does not use and the timing are
    /// left out, five correlations of the eight.
    pub(crate) fn width_and_ild(
        &mut self,
        left: &[f32],
        right: &[f32],
    ) -> (Option<f64>, Option<f64>) {
        self.transform(left, right);
        let bands: Vec<BandReading> = (0..self.bands.len())
            .map(|index| {
                if WIDTH_BANDS.contains(&self.bands[index].0.nominal) {
                    self.band(index)
                } else {
                    BandReading {
                        left_energy: 0.0,
                        right_energy: 0.0,
                        iacc: None,
                        ild_decibels: None,
                    }
                }
            })
            .collect();
        (self.width_of(&bands), self.broadband_ild())
    }

    fn transform(&mut self, left: &[f32], right: &[f32]) {
        transform(self.forward.as_ref(), &mut self.input, left, &mut self.left);
        transform(
            self.forward.as_ref(),
            &mut self.input,
            right,
            &mut self.right,
        );
    }

    /// 2·Σ|X|²/N over `bins`: the frame's energy there.
    fn energy(&self, spectrum: &[Complex<f64>], bins: std::ops::Range<usize>) -> f64 {
        2.0 * spectrum[bins].iter().map(Complex::norm_sqr).sum::<f64>() / self.size as f64
    }

    fn sounding(&self, energy: f64) -> bool {
        energy / self.frame_length as f64 > crate::statistics::SILENCE_POWER
    }

    /// Octave band `index` of the transformed frame.
    fn band(&mut self, index: usize) -> BandReading {
        let bins = self.bands[index].1.clone();
        let left_energy = self.energy(&self.left, bins.clone());
        let right_energy = self.energy(&self.right, bins.clone());
        let mut reading = BandReading {
            left_energy,
            right_energy,
            iacc: None,
            ild_decibels: None,
        };
        if self.sounding(left_energy) && self.sounding(right_energy) {
            let cross = cross_spectrum(&self.left, &self.right, bins.clone());
            let peak = self
                .correlator
                .peak(bins.start, &cross, left_energy, right_energy);
            reading.iacc = Some(peak.absolute);
            reading.ild_decibels = Some(power_decibels(right_energy) - power_decibels(left_energy));
        }
        reading
    }

    /// 1 − the mean IACC of [`WIDTH_BANDS`], when all three sound.
    fn width_of(&self, bands: &[BandReading]) -> Option<f64> {
        let width_bands: Vec<f64> = self
            .bands
            .iter()
            .zip(bands)
            .filter(|((band, _), _)| WIDTH_BANDS.contains(&band.nominal))
            .filter_map(|(_, reading)| reading.iacc)
            .collect();
        (width_bands.len() == WIDTH_BANDS.len()).then(|| 1.0 - mean(&width_bands))
    }

    fn broadband_ild(&self) -> Option<f64> {
        let left_energy = self.energy(&self.left, self.broadband_bins.clone());
        let right_energy = self.energy(&self.right, self.broadband_bins.clone());
        (self.sounding(left_energy) && self.sounding(right_energy))
            .then(|| power_decibels(right_energy) - power_decibels(left_energy))
    }

    fn timing_lag(&mut self) -> Option<f64> {
        let left_energy = self.energy(&self.left, self.timing_bins.clone());
        let right_energy = self.energy(&self.right, self.timing_bins.clone());
        if !(self.sounding(left_energy) && self.sounding(right_energy)) {
            return None;
        }
        let cross = cross_spectrum(&self.left, &self.right, self.timing_bins.clone());
        let peak = self
            .correlator
            .peak(self.timing_bins.start, &cross, left_energy, right_energy);
        (peak.positive > TIMING_CORRELATION).then_some(peak.positive_lag)
    }
}

/// `samples`, zero-padded to the transform's length, transformed into
/// `output`. The input is refilled every time: the transform uses it as
/// scratch.
fn transform(
    forward: &dyn RealToComplex<f64>,
    input: &mut [f64],
    samples: &[f32],
    output: &mut [Complex<f64>],
) {
    input.fill(0.0);
    for (slot, sample) in input.iter_mut().zip(samples) {
        *slot = *sample as f64;
    }
    forward
        .process(input, output)
        .expect("buffers are the planner's own");
}

/// conj(L)·R over `bins`.
fn cross_spectrum(
    left: &[Complex<f64>],
    right: &[Complex<f64>],
    bins: std::ops::Range<usize>,
) -> Vec<Complex<f64>> {
    left[bins.clone()]
        .iter()
        .zip(&right[bins])
        .map(|(l, r)| l.conj() * r)
        .collect()
}

/// The interaural measures of `left` and `right`.
pub(crate) fn binaural(
    names: (&str, &str),
    left: &[f32],
    right: &[f32],
    sample_rate: u32,
    envelopes: (&[Vec<f64>], &[Vec<f64>]),
) -> (Binaural, PairOverTime) {
    let mut frames = PairFrames::new(sample_rate);
    let frame_length = frames.frame_length;
    let mut long_term: Vec<BandSums> = frames
        .bands
        .iter()
        .map(|(_, bins)| BandSums {
            cross: vec![Complex::new(0.0, 0.0); bins.len()],
            left: 0.0,
            right: 0.0,
        })
        .collect();

    let frame_count = left.len().min(right.len()) / frame_length;
    let mut frame_iacc: Vec<Vec<f64>> = vec![Vec::new(); frames.bands.len()];
    let mut frame_ild: Vec<Vec<f64>> = vec![Vec::new(); frames.bands.len()];
    let mut itd_milliseconds = Vec::new();
    let mut over_time = PairOverTime {
        width: Vec::with_capacity(frame_count),
        ild_decibels: Vec::with_capacity(frame_count),
    };
    for frame in 0..frame_count {
        let span = frame * frame_length..(frame + 1) * frame_length;
        let reading = frames.read(&left[span.clone()], &right[span]);
        for (index, (sums, band)) in long_term.iter_mut().zip(&reading.bands).enumerate() {
            let bins = frames.bands[index].1.clone();
            for ((slot, l), r) in sums
                .cross
                .iter_mut()
                .zip(&frames.left[bins.clone()])
                .zip(&frames.right[bins])
            {
                *slot += l.conj() * r;
            }
            sums.left += band.left_energy;
            sums.right += band.right_energy;
            if let (Some(iacc), Some(ild)) = (band.iacc, band.ild_decibels) {
                frame_iacc[index].push(iacc);
                frame_ild[index].push(ild);
            }
        }
        over_time.width.push(reading.width);
        over_time.ild_decibels.push(reading.ild_decibels);
        if let Some(lag) = reading.timing_lag {
            itd_milliseconds.push(lag * 1000.0 / frames.sample_rate);
        }
    }

    let octave_bands: Vec<BinauralBand> = long_term
        .iter()
        .enumerate()
        .map(|(index, sums)| {
            let (band, bins) = &frames.bands[index];
            let sounding = sums.left > 0.0 && sums.right > 0.0;
            BinauralBand {
                centre_hertz: band.nominal,
                iacc: sounding.then(|| {
                    frames
                        .correlator
                        .peak(bins.start, &sums.cross, sums.left, sums.right)
                        .absolute
                }),
                iacc_median_of_frames: quantile(&frame_iacc[index], 0.5),
                envelope_coherence: match (envelopes.0.get(index), envelopes.1.get(index)) {
                    (Some(left), Some(right)) => correlation(left, right),
                    _ => None,
                },
                ild_mean_decibels: (!frame_ild[index].is_empty()).then(|| mean(&frame_ild[index])),
                ild_standard_deviation_decibels: (!frame_ild[index].is_empty())
                    .then(|| standard_deviation(&frame_ild[index])),
            }
        })
        .collect();
    let width_values: Vec<f64> = octave_bands
        .iter()
        .filter(|band| WIDTH_BANDS.contains(&band.centre_hertz))
        .filter_map(|band| band.iacc)
        .collect();
    let width = (width_values.len() == WIDTH_BANDS.len()).then(|| 1.0 - mean(&width_values));
    let interquartile = match (
        quantile(&itd_milliseconds, 0.25),
        quantile(&itd_milliseconds, 0.75),
    ) {
        (Some(lower), Some(upper)) => Some(upper - lower),
        _ => None,
    };
    (
        Binaural {
            left: names.0.to_owned(),
            right: names.1.to_owned(),
            octave_bands,
            width,
            itd_median_milliseconds: quantile(&itd_milliseconds, 0.5),
            itd_interquartile_milliseconds: interquartile,
            itd_frames: itd_milliseconds.len(),
            frames: frame_count,
        },
        over_time,
    )
}

/// The peak of a band's normalised cross-correlation within the lag range.
struct Peak {
    /// The largest |c(τ)|: IACC.
    absolute: f64,
    /// The largest c(τ), and its lag (samples, fractional, positive when
    /// the right lags the left).
    positive: f64,
    positive_lag: f64,
}

struct Correlator {
    inverse: Arc<dyn ComplexToReal<f64>>,
    buffer: Vec<Complex<f64>>,
    output: Vec<f64>,
    maximum_lag: usize,
}

impl Correlator {
    /// c(τ) = (1/N)·irfft(conj(L)·R) restricted to the band, from bin
    /// `first` on; normalised by √(E_L·E_R) of the same band.
    fn peak(&mut self, first: usize, cross: &[Complex<f64>], left: f64, right: f64) -> Peak {
        self.buffer
            .iter_mut()
            .for_each(|bin| *bin = Complex::new(0.0, 0.0));
        self.buffer[first..first + cross.len()].copy_from_slice(cross);
        self.inverse
            .process(&mut self.buffer, &mut self.output)
            .expect("the band holds neither DC nor Nyquist");
        let size = self.output.len();
        let norm = size as f64 * (left * right).sqrt();
        let lag = self.maximum_lag as isize;
        let at = |tau: isize| self.output[tau.rem_euclid(size as isize) as usize] / norm;
        let mut peak = Peak {
            absolute: 0.0,
            positive: f64::NEG_INFINITY,
            positive_lag: 0.0,
        };
        let mut best = 0isize;
        for tau in -lag..=lag {
            let value = at(tau);
            peak.absolute = peak.absolute.max(value.abs());
            if value > peak.positive {
                peak.positive = value;
                best = tau;
            }
        }
        // A parabola through the peak and its neighbours.
        let (before, here, after) = (at(best - 1), at(best), at(best + 1));
        let curvature = before - 2.0 * here + after;
        let shift = if curvature < 0.0 {
            (0.5 * (before - after) / curvature).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        peak.positive_lag = best as f64 + shift;
        peak
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signals::{Noise, RATE};

    fn pair(left: &[f32], right: &[f32]) -> Binaural {
        let envelopes = (
            octave_envelopes(&Spectrum::of(left, RATE)),
            octave_envelopes(&Spectrum::of(right, RATE)),
        );
        binaural(("L", "R"), left, right, RATE, (&envelopes.0, &envelopes.1)).0
    }

    #[test]
    fn identical_ears_are_fully_alike() {
        let noise = Noise::new(1).white(0.1, 10.0);
        let found = pair(&noise, &noise);
        for band in &found.octave_bands {
            assert!((band.iacc.unwrap() - 1.0).abs() < 1e-6, "{band:?}");
            assert!((band.envelope_coherence.unwrap() - 1.0).abs() < 1e-6);
            assert!(band.ild_mean_decibels.unwrap().abs() < 1e-9);
        }
        assert!(found.width.unwrap().abs() < 1e-6);
    }

    #[test]
    fn independent_ears_are_unalike() {
        let left = Noise::new(2).white(0.1, 30.0);
        let right = Noise::new(3).white(0.1, 30.0);
        for band in &pair(&left, &right).octave_bands {
            assert!(band.iacc.unwrap() < 0.1, "{band:?}");
            assert!(band.envelope_coherence.unwrap().abs() < 0.1, "{band:?}");
        }
    }

    /// Right is left 0.5 ms (24 samples) later: ITD +0.50 ms.
    #[test]
    fn a_delay_is_read_as_timing() {
        let left = Noise::new(4).white(0.1, 10.0);
        let delay = 24;
        let mut right = vec![0.0f32; delay];
        right.extend_from_slice(&left[..left.len() - delay]);
        let found = pair(&left, &right);
        let itd = found.itd_median_milliseconds.unwrap();
        assert!((itd - 0.5).abs() <= 0.03, "{itd}");
    }

    /// Right is left at half the amplitude: ILD −6.02 dB in every band.
    #[test]
    fn a_gain_is_read_as_level() {
        let left = Noise::new(5).white(0.1, 5.0);
        let right: Vec<f32> = left.iter().map(|sample| sample * 0.5).collect();
        for band in &pair(&left, &right).octave_bands {
            let ild = band.ild_mean_decibels.unwrap();
            assert!((ild + 6.02).abs() <= 0.05, "{ild}");
        }
    }

    /// Independent noise carriers at each ear sharing one 2 Hz swing of
    /// depth 1: the waveforms are unalike (IACC below 0.1) but the envelopes
    /// move together (coherence above 0.8) at 2 and 4 kHz.
    ///
    /// By hand: an envelope m(t)·R(t), m = 1 + sin, R Rayleigh and
    /// independent at each ear, correlates with the other ear's at
    /// var(m)·E[R]² / (E[m²]·E[R²] − E[m]²·E[R]²). The 200 Hz lowpass leaves
    /// about 26 % of R's own fluctuation in the 2 kHz band (1.4 kHz wide) and
    /// 14 % at 4 kHz, which gives about 0.82 and 0.90.
    #[test]
    fn a_shared_swing_is_coherent_without_correlation() {
        let swing = |samples: Vec<f32>| -> Vec<f32> {
            samples
                .iter()
                .enumerate()
                .map(|(index, sample)| {
                    let time = index as f64 / RATE as f64;
                    (*sample as f64 * (1.0 + (std::f64::consts::TAU * 2.0 * time).sin())) as f32
                })
                .collect()
        };
        let left = swing(Noise::new(6).white(0.1, 20.0));
        let right = swing(Noise::new(8).white(0.1, 20.0));
        let found = pair(&left, &right);
        for band in found
            .octave_bands
            .iter()
            .filter(|band| band.centre_hertz == 2000.0 || band.centre_hertz == 4000.0)
        {
            assert!(band.iacc.unwrap() < 0.1, "{band:?}");
            assert!(band.envelope_coherence.unwrap() > 0.8, "{band:?}");
        }
    }
}
