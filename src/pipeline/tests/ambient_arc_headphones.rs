//! Ambient sound from an arc on headphones, held to the measured head.
//!
//! The ground truth is computed here, independently of the renderer, from
//! the measured responses of the three rings nearest the horizon, at −10°,
//! 0° and +10° (72 directions each, 5° apart): each direction weighted by
//! the solid angle of its ring's ±5° band and by the share of its ±2.5° of
//! azimuth the arc covers. That is the band the renderer's arcs are made
//! of; the renderer finds each direction's share by its own route (the
//! nearest measured direction to points half a degree apart).
//!
//! "After the equaliser" an ear hears the arc's power over the head's
//! diffuse-field power, which is what the headphone equaliser divides out;
//! the equaliser itself is within 0.71 dB of that from 100 Hz to 12.5 kHz
//! (`headphone_equalisation.rs`).

use std::sync::OnceLock;

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;
use sofar::reader::{Filter, OpenOptions};

use super::ambient_headphones::{
    measured_diffuse_field, on_fine_grid, ring_solid_angle, spectra_of, white_noise, EarSpectra,
    ANALYSIS_SIZE, HIGHEST_COHERENCE_BAND, LOWEST_BAND, RENDER_FRAMES,
};
use super::Rig;
use crate::pipeline::PipelineParams;
use atrium_core::probe::Probe;
use atrium_core::source::{EmitterKind, SoundSource};
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

const SAMPLE_RATE: f32 = 48_000.0;

/// The measured rings the band is made of, and how many directions each.
const BAND_RINGS: [i32; 3] = [-10, 0, 10];
const RING_DIRECTIONS: usize = 72;

/// One measured direction's spectra on the short grid.
struct Measured {
    elevation: i32,
    azimuth_degrees: f64,
    left: Vec<Complex<f64>>,
    right: Vec<Complex<f64>>,
}

fn measured_band() -> &'static Vec<Measured> {
    static BAND: OnceLock<Vec<Measured>> = OnceLock::new();
    BAND.get_or_init(|| {
        let sofa = OpenOptions::new()
            .sample_rate(SAMPLE_RATE)
            .open(PipelineParams::default().hrtf_path)
            .expect("the shipped HRIRs");
        let short_size = (2 * sofa.filter_len()).next_power_of_two();
        let mut planner = RealFftPlanner::<f64>::new();
        let forward = planner.plan_fft_forward(short_size);
        let mut filter = Filter::new(sofa.filter_len());
        let spectrum_of = |response: &[f32]| {
            let mut time = forward.make_input_vec();
            for (slot, &sample) in time.iter_mut().zip(response) {
                *slot = sample as f64;
            }
            let mut spectrum = forward.make_output_vec();
            forward.process(&mut time, &mut spectrum).expect("forward");
            spectrum
        };
        let mut band = Vec::new();
        for elevation in BAND_RINGS {
            for step in 0..RING_DIRECTIONS {
                let azimuth_degrees = 360.0 * step as f64 / RING_DIRECTIONS as f64;
                let (azimuth, up) = (
                    azimuth_degrees.to_radians(),
                    (elevation as f64).to_radians(),
                );
                sofa.filter_nointerp(
                    (up.cos() * azimuth.cos()) as f32,
                    (up.cos() * azimuth.sin()) as f32,
                    up.sin() as f32,
                    &mut filter,
                );
                band.push(Measured {
                    elevation,
                    azimuth_degrees,
                    left: spectrum_of(&filter.left),
                    right: spectrum_of(&filter.right),
                });
            }
        }
        band
    })
}

/// The head's spectra for an arc `width` degrees wide centred `centre`
/// degrees anticlockwise from straight ahead: the weighted mean over the
/// band's directions, on the analysis grid.
fn arc_truth(width: f64, centre: f64) -> EarSpectra {
    let band = measured_band();
    let bins = band[0].left.len();
    let mut left = vec![Complex::new(0.0, 0.0); bins];
    let mut right = vec![Complex::new(0.0, 0.0); bins];
    let mut cross = vec![Complex::new(0.0, 0.0); bins];
    let half_cell = 180.0 / RING_DIRECTIONS as f64;
    let mut total = 0.0;
    for direction in band {
        // The share of the direction's cell inside the arc, the arc
        // unwrapped round the direction.
        let offset = (direction.azimuth_degrees - centre + 540.0).rem_euclid(360.0) - 180.0;
        let inside = ((width / 2.0).min(offset + half_cell)
            - (-width / 2.0).max(offset - half_cell))
        .max(0.0)
            / (2.0 * half_cell);
        if inside == 0.0 {
            continue;
        }
        let weight = inside * ring_solid_angle(direction.elevation) / RING_DIRECTIONS as f64;
        total += weight;
        for bin in 0..bins {
            left[bin] += weight * direction.left[bin].norm_sqr();
            right[bin] += weight * direction.right[bin].norm_sqr();
            cross[bin] += weight * direction.left[bin] * direction.right[bin].conj();
        }
    }
    let mean = |spectrum: &[Complex<f64>]| -> Vec<Complex<f64>> {
        on_fine_grid(
            &spectrum
                .iter()
                .map(|value| value / total)
                .collect::<Vec<_>>(),
        )
    };
    EarSpectra {
        bin_hz: SAMPLE_RATE as f64 / ANALYSIS_SIZE as f64,
        left: mean(&left).iter().map(|bin| bin.re).collect(),
        right: mean(&right).iter().map(|bin| bin.re).collect(),
        cross: mean(&cross),
        input: vec![1.0; ANALYSIS_SIZE / 2 + 1],
    }
}

/// The interaural time difference of `spectra`, from the phase of its
/// cross-spectrum below 1.5 kHz: each third octave from 100 Hz to 1.25 kHz
/// gives the phase of its summed cross-spectrum, followed from the lowest
/// band up to the nearest half turn (a change of sign, where the ears turn
/// from alike to opposed, is not a time), and the time is the slope of
/// phase over angular frequency through the origin, each band weighted by
/// how strongly the ears are alike in it. Positive when the left ear hears
/// first (seconds).
fn interaural_time_difference(spectra: &EarSpectra) -> f64 {
    let mut previous_phase = 0.0;
    let (mut numerator, mut denominator) = (0.0, 0.0);
    for index in LOWEST_BAND..=1 {
        let centre = 1000.0 * 2f64.powf(index as f64 / 3.0);
        let lowest = (centre * 2f64.powf(-1.0 / 6.0) / spectra.bin_hz).ceil() as usize;
        let highest = (centre * 2f64.powf(1.0 / 6.0) / spectra.bin_hz).floor() as usize;
        let summed: Complex<f64> = spectra.cross[lowest..=highest].iter().sum();
        let mut phase = summed.arg();
        while phase - previous_phase > std::f64::consts::FRAC_PI_2 {
            phase -= std::f64::consts::PI;
        }
        while phase - previous_phase < -std::f64::consts::FRAC_PI_2 {
            phase += std::f64::consts::PI;
        }
        previous_phase = phase;
        let angular = std::f64::consts::TAU * centre;
        let weight = summed.norm();
        numerator += weight * phase * angular;
        denominator += weight * angular * angular;
    }
    numerator / denominator
}

/// White noise or a recording played as an ambient sound from an arc
/// `degrees` wide whose middle lies towards `place`.
pub(super) struct ArcSound {
    pub(super) samples: Vec<f32>,
    pub(super) index: usize,
    pub(super) degrees: f32,
    pub(super) place: Vec3,
    pub(super) looped: bool,
}

impl SoundSource for ArcSound {
    fn next_sample(&mut self, _sample_rate: f32) -> f32 {
        let index = self.index;
        self.index += 1;
        if self.looped {
            self.samples[index % self.samples.len()]
        } else {
            self.samples.get(index).copied().unwrap_or(0.0)
        }
    }
    fn position(&self) -> Vec3 {
        self.place
    }
    fn emitter_kind(&self) -> EmitterKind {
        EmitterKind::Field
    }
    fn arc_degrees(&self) -> f32 {
        self.degrees
    }
    fn tick(&mut self, _delta_seconds: f32) {}
}

/// Where to place a sound so its arc's middle is `centre` degrees
/// anticlockwise from where a listener at the origin facing +y faces.
pub(super) fn place_towards(centre: f64) -> Vec3 {
    let heading = std::f64::consts::FRAC_PI_2 + centre.to_radians();
    Vec3::new(
        20.0 * heading.cos() as f32,
        20.0 * heading.sin() as f32,
        0.0,
    )
}

/// `input` as an ambient sound from an arc `width` degrees wide towards
/// `place`, through the headphone pipeline, the listener at the origin
/// turned by `yaw_at(frame)` before each callback.
pub(super) fn render_arc(
    input: &[f32],
    width: f32,
    place: Vec3,
    probe: Probe,
    frames: usize,
    yaw_at: impl Fn(usize) -> f32,
) -> Vec<f32> {
    let mut rig = Rig::headphones(SAMPLE_RATE);
    let mut pipeline = rig.build(RenderMode::Hrtf);
    pipeline.probe = probe;
    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ArcSound {
        samples: input.to_vec(),
        index: 0,
        degrees: width,
        place,
        looped: false,
    })];
    pipeline.prepare_sources(&sources);
    let mut output = Vec::with_capacity(frames * 2);
    let mut rendered = 0;
    while rendered < frames {
        let block = super::BLOCK_FRAMES.min(frames - rendered);
        rig.listener.yaw = yaw_at(rendered);
        output.extend(rig.render(&mut pipeline, &mut sources, block));
        rendered += block;
    }
    output
}

fn facing_ahead(_frame: usize) -> f32 {
    std::f32::consts::FRAC_PI_2
}

/// One arc's comparison, band by band, and its worst errors.
struct Comparison {
    report: String,
    worst_level: f64,
    /// The worst error in the ears' level difference from 500 Hz up, and in
    /// the third octaves from 100 to 400 Hz.
    worst_level_difference: f64,
    worst_level_difference_in_the_bass: f64,
    worst_coherence: f64,
    time_difference_error: f64,
}

/// The third octave index of 500 Hz: below it the echo's 30 ms turns fewer
/// than three and a half times across a band.
const FIRST_BAND_ABOVE_THE_BASS: i32 = -3;

fn compare(width: f64, centre: f64) -> Comparison {
    let (diffuse, _) = measured_diffuse_field();
    let truth = arc_truth(width, centre);
    let noise = white_noise();
    let output = render_arc(
        &noise,
        width as f32,
        place_towards(centre),
        Probe::WHOLE,
        RENDER_FRAMES,
        facing_ahead,
    );
    let heard = spectra_of(&output, &noise);
    let mut report = format!(
        "{width}° arc at {centre}°\nband Hz | left dB truth/heard | right dB truth/heard | ILD truth/heard | coherence truth/heard\n"
    );
    let (mut worst_level, mut worst_coherence) = (0f64, 0f64);
    let (mut worst_level_difference, mut worst_level_difference_in_the_bass) = (0f64, 0f64);
    for index in LOWEST_BAND..=HIGHEST_COHERENCE_BAND {
        let (expected, diffuse_band, rendered) =
            (truth.band(index), diffuse.band(index), heard.band(index));
        let left = expected.left_db - diffuse_band.left_db;
        let right = expected.right_db - diffuse_band.right_db;
        let level_difference = left - right;
        let heard_difference = rendered.left_db - rendered.right_db;
        report.push_str(&format!(
            "{:6.0} | {left:+6.2} {:+6.2} | {right:+6.2} {:+6.2} | {level_difference:+6.2} {heard_difference:+6.2} | {:.3} {:.3}\n",
            expected.centre, rendered.left_db, rendered.right_db, expected.coherence, rendered.coherence
        ));
        worst_level = worst_level
            .max((rendered.left_db - left).abs())
            .max((rendered.right_db - right).abs());
        let error = (heard_difference - level_difference).abs();
        if index >= FIRST_BAND_ABOVE_THE_BASS {
            worst_level_difference = worst_level_difference.max(error);
        } else {
            worst_level_difference_in_the_bass = worst_level_difference_in_the_bass.max(error);
        }
        worst_coherence = worst_coherence.max((rendered.coherence - expected.coherence).abs());
    }
    let (expected_time, heard_time) = (
        interaural_time_difference(&truth),
        interaural_time_difference(&heard),
    );
    report.push_str(&format!(
        "ITD truth {:+.0} µs, heard {:+.0} µs\n",
        expected_time * 1e6,
        heard_time * 1e6
    ));
    Comparison {
        report,
        worst_level,
        worst_level_difference,
        worst_level_difference_in_the_bass,
        worst_coherence,
        time_difference_error: (heard_time - expected_time).abs(),
    }
}

/// Ambient white noise from an arc 30°, 90° or 180° wide, centred ahead, at
/// 60°, at 90° (to the left) or behind, is heard as the measured head hears
/// it, from 100 Hz to 8 kHz: each ear's third-octave level within 1.5 dB of
/// the arc's share of the diffuse field, the ears' level difference within
/// 1 dB from 500 Hz up, their coherence (the largest within ±1 ms) within
/// 0.15, and the interaural time difference read from the phase below
/// 1.5 kHz within 50 µs.
///
/// Below 500 Hz the level difference is held to 1.5 dB, not 1: there the
/// echo's 30 ms turn fewer than three and a half times across a third
/// octave, too few for what it adds to one ear and takes from the other to
/// average out, and a half circle ahead or behind, whose ears are only
/// partly alike there, comes to 1.43 dB at 200 Hz and 1.28 dB at 400 Hz.
/// No longest delay from 25 to 40 ms brings every band under 1 dB (root
/// README, "An arc of ambient sound").
///
/// The head itself, for the 30° arc to the left (at 90°): the ears differ by
/// 4.5 dB at 500 Hz and 11.2 dB at 4 kHz, and their coherence is 0.995 at
/// 500 Hz and 0.57 at 4 kHz; the left ear hears it 700 µs first.
#[test]
fn two_ears_hear_an_arc_as_the_measured_head_does() {
    let mut failures = Vec::new();
    let mut summary = String::new();
    for (width, centre) in [30.0, 90.0, 180.0]
        .into_iter()
        .flat_map(|width| [0.0, 60.0, 90.0, 180.0].map(|centre| (width, centre)))
    {
        let comparison = compare(width, centre);
        let line = format!(
            "{width}° at {centre}°: level {:.2} dB, ILD {:.2} dB (below 500 Hz {:.2} dB), coherence {:.3}, ITD {:.0} µs",
            comparison.worst_level,
            comparison.worst_level_difference,
            comparison.worst_level_difference_in_the_bass,
            comparison.worst_coherence,
            comparison.time_difference_error * 1e6
        );
        summary.push_str(&line);
        summary.push('\n');
        if comparison.worst_level > 1.5
            || comparison.worst_level_difference > 1.0
            || comparison.worst_level_difference_in_the_bass > 1.5
            || comparison.worst_coherence > 0.15
            || comparison.time_difference_error > 50e-6
        {
            failures.push(comparison.report);
        }
    }
    eprintln!("{summary}");
    assert!(failures.is_empty(), "{summary}\n{}", failures.join("\n"));
}

/// An arc straight ahead is balanced, as the head is: 30°, 90° and 180°
/// wide, the two ears within 0.5 dB of each other over the whole band, and
/// the time difference zero within 20 µs.
#[test]
fn an_arc_straight_ahead_is_balanced() {
    let noise = white_noise();
    let mut report = String::new();
    let mut balanced = true;
    for width in [30.0f32, 90.0, 180.0] {
        let output = render_arc(
            &noise,
            width,
            place_towards(0.0),
            Probe::WHOLE,
            RENDER_FRAMES,
            facing_ahead,
        );
        let power = |ear: usize| -> f64 {
            output
                .iter()
                .skip(ear)
                .step_by(2)
                .map(|&sample| (sample as f64).powi(2))
                .sum()
        };
        let difference = 10.0 * (power(0) / power(1)).log10();
        let time = interaural_time_difference(&spectra_of(&output, &noise));
        report.push_str(&format!(
            "{width}° ahead: left against right {difference:+.3} dB, ITD {:+.1} µs\n",
            time * 1e6
        ));
        balanced &= difference.abs() <= 0.5 && time.abs() <= 20e-6;
    }
    eprintln!("{report}");
    assert!(balanced, "{report}");
}

/// The largest step between two samples of either ear.
fn largest_step(output: &[f32]) -> f32 {
    output
        .chunks_exact(2)
        .zip(output.chunks_exact(2).skip(1))
        .map(|(previous, next)| {
            (next[0] - previous[0])
                .abs()
                .max((next[1] - previous[1]).abs())
        })
        .fold(0f32, f32::max)
}

/// Turning the head at 180° a second through white noise from a 60° arc
/// never clicks, and the arc stays where it is in the world.
///
/// The arc's filters are swapped for the next bearing's, 5° on, with a
/// crossfade over a callback, so each sample is a mix of what two held
/// bearings would give: the steepest step between two samples is held to
/// the steepest of the same noise heard with the head still at any bearing
/// the turn passes, as the near field's flight past the ear is. Heard while
/// turning, the ears' level difference is the head's at each bearing:
/// left louder by the held arc's own difference when the arc is to the left,
/// right louder by as much when it is to the right.
#[test]
fn turning_the_head_through_an_arc_never_clicks() {
    let noise = white_noise();
    let seconds = 2.0;
    let frames = (seconds * SAMPLE_RATE) as usize;
    let place = Vec3::new(0.0, 20.0, 0.0);
    let turning_yaw = |frame: usize| {
        std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * frame as f32 / SAMPLE_RATE
    };
    let turning = render_arc(&noise, 60.0, place, Probe::WHOLE, frames, turning_yaw);
    let held_steepest = (0..72)
        .map(|bearing| {
            let yaw = std::f32::consts::FRAC_PI_2 - (bearing as f32 * 5.0).to_radians();
            largest_step(&render_arc(
                &noise,
                60.0,
                place,
                Probe::WHOLE,
                frames,
                |_| yaw,
            ))
        })
        .fold(0f32, f32::max);
    let turning_steepest = largest_step(&turning);
    eprintln!(
        "turning: steepest step {turning_steepest:.5}, held at any bearing {held_steepest:.5}"
    );
    assert!(
        turning_steepest <= held_steepest * 1.02,
        "turning: steepest step {turning_steepest:.5}, held at any bearing {held_steepest:.5}"
    );

    // A quarter turn in, the arc (due +y) is 90° to the right of a head
    // that faced it; three quarters in, 90° to its left. 50 ms each.
    let level_difference = |output: &[f32], centre_frame: usize| {
        let window = (0.05 * SAMPLE_RATE) as usize;
        let range = (centre_frame - window / 2)..(centre_frame + window / 2);
        let power = |ear: usize| -> f64 {
            range
                .clone()
                .map(|frame| (output[2 * frame + ear] as f64).powi(2))
                .sum()
        };
        10.0 * (power(0) / power(1)).log10()
    };
    let at_right = level_difference(&turning, frames / 4);
    let at_left = level_difference(&turning, 3 * frames / 4);
    let held_left = level_difference(
        &render_arc(&noise, 60.0, place, Probe::WHOLE, frames, |_| 0.0),
        3 * frames / 4,
    );
    let held_right = level_difference(
        &render_arc(&noise, 60.0, place, Probe::WHOLE, frames, |_| {
            std::f32::consts::PI
        }),
        frames / 4,
    );
    eprintln!(
        "turning: {at_right:+.2} dB with the arc to the right (held {held_right:+.2}), {at_left:+.2} dB to the left (held {held_left:+.2})"
    );
    assert!(
        (at_right - held_right).abs() <= 1.0 && (at_left - held_left).abs() <= 1.0,
        "turning: {at_right:+.2} dB to the right (held {held_right:+.2}), {at_left:+.2} dB to the left (held {held_left:+.2})"
    );
    assert!(
        held_left > 3.0 && held_right < -3.0,
        "held {held_left:+.2} and {held_right:+.2}"
    );
}

/// A click through an arc is heard once: at each ear at least 85 % of its
/// energy within 13 ms, for arcs to the side, ahead and behind.
#[test]
fn an_attack_through_an_arc_is_heard_once() {
    let mut report = String::new();
    let mut heard_once = true;
    for (width, centre) in [(30.0f32, 60.0), (90.0, 90.0), (180.0, 0.0), (120.0, 180.0)] {
        let mut click = vec![0.0f32; 4800];
        click[0] = 1.0;
        let output = render_arc(
            &click,
            width,
            place_towards(centre),
            Probe::WHOLE,
            9600,
            facing_ahead,
        );
        for (ear, name) in [(0, "left"), (1, "right")] {
            let response: Vec<f64> = output
                .iter()
                .skip(ear)
                .step_by(2)
                .map(|&sample| sample as f64)
                .collect();
            let total: f64 = response.iter().map(|sample| sample * sample).sum();
            let early: f64 = response[..(0.013 * SAMPLE_RATE) as usize]
                .iter()
                .map(|sample| sample * sample)
                .sum();
            report.push_str(&format!(
                "{width}° at {centre}°, {name} ear: {:.1} % within 13 ms\n",
                100.0 * early / total
            ));
            heard_once &= early / total >= 0.85;
        }
    }
    eprintln!("{report}");
    assert!(heard_once, "{report}");
}
