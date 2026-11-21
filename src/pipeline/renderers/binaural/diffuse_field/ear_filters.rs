//! The two filters that turn one ambient signal into what two ears hear of
//! a diffuse field: this head's diffuse-field colour at each ear, and the
//! diffuse-field coherence between them.
//!
//! Each ear's filter is a minimum-phase filter of that ear's diffuse-field
//! magnitude, then the sound itself and one echo of it:
//!
//! ```text
//! left  = a + b · D(f)        right = a − b · D(f)
//! ```
//!
//! where D is a delay whose length depends on frequency, and the echo has
//! the opposite sign in the two ears. If, within any third octave, the
//! delay's phase turns over often enough that the echo is unrelated to the
//! sound, each ear's power there is a² + b² and the ears' cross-power
//! a² − b² (the cross terms average out). With a² + b² = 1 the ears keep
//! their colour and their coherence is a² − b², so any coherence the head
//! has, from its measured 0.94 at 100 Hz to its −0.2 at 800 Hz, is one pair
//! a = √((1 + γ)/2), b = √((1 − γ)/2). This is Lauridsen's complementary
//! comb (1954) with the coherence set per frequency by the measured head.
//!
//! Within one bin the echo makes a comb: the ear hears peaks and notches
//! every 1/D Hz, complementary between the ears. Two turns per third octave
//! put the notches closer together than the ear's own frequency resolution
//! (an equivalent rectangular bandwidth), and smoothed by it the comb is
//! gone: under 0.2 dB from 300 Hz to 10 kHz on the shipped head.
//!
//! Unrelated within a third octave means the delay's phase turns over at
//! least once across it; turned twice it averages out well. And the echo
//! must come later than the ±1 ms over which the ear compares its two
//! inputs, or the ear finds it again by shifting one ear against the other.
//! So the delay at frequency f is that millisecond and a bit, plus two
//! periods of the third octave's width: 9.8 ms at 1 kHz, 3.4 ms at 4 kHz,
//! 2.3 ms at 8 kHz. Below 300 Hz that would exceed 30 ms, and there it
//! stays at 30 ms, well within the 50 ms over which the ear still fuses a
//! later sound with the first; in that bass the ears are alike and the echo
//! is weak (b is 0.17 at 100 Hz). The sound itself always comes first: an
//! attack is heard at once, and only where the head makes the ears unalike
//! is half its energy repeated a few milliseconds later.
//!
//! No published filter does this with a head's own coherence; the structure
//! is Lauridsen's and the coherence the head's, and the three numbers sizing
//! the delay were chosen by measurement on a coarse grid (the root README,
//! "Ambient sound", records it).

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

use super::statistics::DiffuseStatistics;

/// Below this the set's own coherence is not taken. The MIT KEMAR
/// measurement loudspeaker gave little bass (the diffuse response is 13 dB
/// down at 100 Hz), and the set's coherence falls towards 0.91 at 0 Hz,
/// where two ears hear the same pressure. Its value here is held below.
const COHERENCE_HELD_BELOW_HZ: f64 = 100.0;

/// Width of a third octave over its centre frequency: 2^(1/6) − 2^(−1/6).
const THIRD_OCTAVE_RELATIVE_WIDTH: f64 = 0.231_563;

/// The lag over which the ear compares its two inputs is ±1 ms (the range
/// of the interaural cross-correlation in ISO 3382-1); every echo starts at
/// least this much after the sound it belongs to.
const INTERAURAL_LAG_MARGIN_SECONDS: f64 = 0.0012;

/// How often the delay's phase turns over across one third octave: the
/// fewest at which every acceptance test holds on the whole measured grid.
const TURNS_PER_THIRD_OCTAVE: f64 = 2.0;

/// The longest echo delay. Of the grid's 30, 40 and 50 ms (all within the
/// 50 ms over which the ear fuses a later sound with the first, the boundary
/// of the clarity index C50), the one with the most margin on every test.
const LONGEST_ECHO_DELAY_SECONDS: f64 = 0.03;

/// Every ear's response starts this late. The coherence changes with
/// frequency, so the shares of sound and echo do, and a share that changes
/// with frequency spreads a little in time to both sides; without this the
/// part before time zero would be cut off.
const LEAD_IN_SECONDS: f64 = 0.001;

/// How long each filter is: twice the longest echo, room for the echo and
/// the minimum-phase colour it carries.
const FILTER_SECONDS: f64 = 0.06;

/// The last part of each filter fades out over this, so it ends without a
/// step.
const FADE_OUT_SECONDS: f64 = 0.005;

/// The frequency grid the filters are designed on spans this long in time,
/// far beyond the filters, so nothing wraps round.
const DESIGN_SECONDS: f64 = 0.6;

/// The two ears' filters, `FILTER_SECONDS` long at the sample rate they
/// were designed for.
pub(in crate::pipeline::renderers::binaural) struct EarFilters {
    pub(in crate::pipeline::renderers::binaural) left: Vec<f32>,
    pub(in crate::pipeline::renderers::binaural) right: Vec<f32>,
}

/// The length of the frequency grid the statistics must be measured on for
/// `design_ear_filters` at `sample_rate`.
pub(in crate::pipeline::renderers::binaural) fn design_grid_size(sample_rate: f32) -> usize {
    ((DESIGN_SECONDS * sample_rate as f64).ceil() as usize).next_power_of_two()
}

/// The shares (a, b) of the sound and its echo that give two ears
/// coherence `coherence` at unchanged power: a² + b² = 1, a² − b² = γ.
pub(in crate::pipeline::renderers::binaural) fn sound_and_echo_for_coherence(
    coherence: f64,
) -> (f64, f64) {
    let coherence = coherence.clamp(-1.0, 1.0);
    (
        ((1.0 + coherence) / 2.0).sqrt(),
        ((1.0 - coherence) / 2.0).sqrt(),
    )
}

/// How long the echo delay is at `frequency`, in seconds.
pub(in crate::pipeline::renderers::binaural) fn echo_delay_seconds(frequency: f64) -> f64 {
    let width = THIRD_OCTAVE_RELATIVE_WIDTH * frequency.max(1.0);
    (INTERAURAL_LAG_MARGIN_SECONDS + TURNS_PER_THIRD_OCTAVE / width).min(LONGEST_ECHO_DELAY_SECONDS)
}

/// The two ears' filters at `sample_rate` from the diffuse-field
/// `statistics`, measured on a grid of `design_grid_size(sample_rate)` bins.
pub(in crate::pipeline::renderers::binaural) fn design_ear_filters(
    statistics: &DiffuseStatistics,
    sample_rate: f32,
) -> EarFilters {
    let grid_size = design_grid_size(sample_rate);
    let bins = grid_size / 2 + 1;
    assert_eq!(
        statistics.left_power.len(),
        bins,
        "statistics on another grid"
    );
    let bin_hz = sample_rate as f64 / grid_size as f64;

    let coherence = smoothed_coherence(statistics, bin_hz);

    // The echo delay's phase: the integral of its delay over frequency.
    let mut delay_phase = vec![0.0f64; bins];
    for bin in 1..bins {
        let previous = echo_delay_seconds((bin - 1) as f64 * bin_hz);
        let current = echo_delay_seconds(bin as f64 * bin_hz);
        delay_phase[bin] =
            delay_phase[bin - 1] - std::f64::consts::TAU * 0.5 * (previous + current) * bin_hz;
    }

    let filter_length = (FILTER_SECONDS * sample_rate as f64).round() as usize;
    let fade_length = (FADE_OUT_SECONDS * sample_rate as f64).round() as usize;
    let mut ears = [&statistics.left_power, &statistics.right_power]
        .into_iter()
        .zip([1.0f64, -1.0])
        .map(|(power, sign)| {
            let minimum_phase = minimum_phase_spectrum(power, grid_size);
            let spectrum: Vec<Complex<f64>> = (0..bins)
                .map(|bin| {
                    let (sound, echo) = sound_and_echo_for_coherence(coherence[bin]);
                    let delay = Complex::from_polar(sign * echo, delay_phase[bin]);
                    let sound_and_echo = delay + sound;
                    let lead_in = Complex::from_polar(
                        1.0,
                        -std::f64::consts::TAU * bin as f64 * bin_hz * LEAD_IN_SECONDS,
                    );
                    minimum_phase[bin] * sound_and_echo * lead_in
                })
                .collect();
            let mut response = to_time(spectrum, grid_size);
            response.truncate(filter_length);
            for (index, sample) in response[filter_length - fade_length..]
                .iter_mut()
                .enumerate()
            {
                let progress = (index as f64 + 0.5) / fade_length as f64;
                *sample *= 0.5 * (1.0 + (std::f64::consts::PI * progress).cos());
            }
            response
                .iter()
                .map(|&sample| sample as f32)
                .collect::<Vec<f32>>()
        });
    let left = ears.next().expect("left ear");
    let right = ears.next().expect("right ear");
    EarFilters { left, right }
}

/// The head's diffuse-field coherence per bin, each spectrum averaged over
/// the third octave around the bin as the ear averages it, and held below
/// `COHERENCE_HELD_BELOW_HZ`.
fn smoothed_coherence(statistics: &DiffuseStatistics, bin_hz: f64) -> Vec<f64> {
    let running_sum = |values: &[f64]| {
        let mut sums = Vec::with_capacity(values.len() + 1);
        sums.push(0.0);
        for value in values {
            sums.push(sums.last().copied().unwrap_or(0.0) + value);
        }
        sums
    };
    let left = running_sum(&statistics.left_power);
    let right = running_sum(&statistics.right_power);
    let cross = running_sum(&statistics.cross_power);
    let bins = statistics.left_power.len();
    let half_band = 2f64.powf(1.0 / 6.0);
    let mut coherence: Vec<f64> = (0..bins)
        .map(|bin| {
            let centre = (bin as f64 * bin_hz).max(COHERENCE_HELD_BELOW_HZ);
            let lowest = ((centre / half_band / bin_hz).ceil() as usize).min(bins - 1);
            let highest =
                ((centre * half_band / bin_hz).floor() as usize).clamp(lowest, bins - 1) + 1;
            let band = |sums: &[f64]| sums[highest] - sums[lowest];
            (band(&cross) / (band(&left) * band(&right)).sqrt()).clamp(-1.0, 1.0)
        })
        .collect();
    let held_from = ((COHERENCE_HELD_BELOW_HZ / bin_hz).ceil() as usize).min(bins - 1);
    let held = coherence[held_from];
    coherence[..held_from].fill(held);
    coherence
}

/// The minimum-phase spectrum whose power is `power`, by the real cepstrum
/// (Oppenheim and Schafer, "Discrete-Time Signal Processing", ch. 12): the
/// most compact response in time of all with that magnitude.
fn minimum_phase_spectrum(power: &[f64], grid_size: usize) -> Vec<Complex<f64>> {
    let mut planner = RealFftPlanner::<f64>::new();
    let inverse = planner.plan_fft_inverse(grid_size);
    let forward = planner.plan_fft_forward(grid_size);
    let floor = power.iter().fold(0f64, |peak, &value| peak.max(value)) * 1e-10;
    let mut log_magnitude: Vec<Complex<f64>> = power
        .iter()
        .map(|&value| Complex::new(0.5 * value.max(floor).ln(), 0.0))
        .collect();
    let mut cepstrum = inverse.make_output_vec();
    inverse
        .process(&mut log_magnitude, &mut cepstrum)
        .expect("inverse transform of a log magnitude");
    let half = grid_size / 2;
    for (index, value) in cepstrum.iter_mut().enumerate() {
        let fold = match index {
            0 => 1.0,
            index if index < half => 2.0,
            index if index == half => 1.0,
            _ => 0.0,
        };
        *value *= fold / grid_size as f64;
    }
    let mut folded = forward.make_output_vec();
    forward
        .process(&mut cepstrum, &mut folded)
        .expect("forward transform of a folded cepstrum");
    folded.iter().map(|bin| bin.exp()).collect()
}

/// The real response whose spectrum is `spectrum`, on a grid `grid_size`.
fn to_time(mut spectrum: Vec<Complex<f64>>, grid_size: usize) -> Vec<f64> {
    let mut planner = RealFftPlanner::<f64>::new();
    let inverse = planner.plan_fft_inverse(grid_size);
    spectrum[0].im = 0.0;
    let last = spectrum.len() - 1;
    spectrum[last].im = 0.0;
    let mut response = inverse.make_output_vec();
    inverse
        .process(&mut spectrum, &mut response)
        .expect("inverse transform of an ear filter");
    for sample in &mut response {
        *sample /= grid_size as f64;
    }
    response
}
