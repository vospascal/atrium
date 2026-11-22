//! Ambient sound on headphones: what two ears hear of a diffuse field, held
//! to the measured head itself.
//!
//! The colour is the measured head's with the headphone equalisation off;
//! with it on (`headphone_equalisation.rs`) the same field reaches the ears
//! flat. The coherence, the attack and the turning head hold either way.
//!
//! The ground truth is computed here, independently of the renderer, from
//! every measured direction of the shipped MIT KEMAR set: 710 directions on
//! 14 rings of elevation, −40° to +90° in steps of 10°. Each direction is
//! weighted by the solid angle its ring covers (±5° of elevation, shared
//! among the ring's directions); nothing below −45° was measured, and none
//! is counted. The renderer weighs the measured directions by their
//! spherical Voronoi cells over the whole sphere instead, giving the lowest
//! ring the cap below it; the two differ by at most 0.4 dB in any third
//! octave and 0.03 in coherence.

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;
use sofar::reader::{Filter, OpenOptions};

use super::{ImpulseSource, Rig};
use crate::pipeline::PipelineParams;
use crate::synth::noise::Rng;
use atrium_core::probe::Probe;
use atrium_core::source::{EmitterKind, SoundSource};
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

const SAMPLE_RATE: f32 = 48_000.0;

/// The MIT KEMAR measurement grid (Gardner and Martin 1995): each ring's
/// elevation and how many directions, evenly spaced in azimuth, it holds.
pub(super) const MEASURED_RINGS: [(i32, usize); 14] = [
    (-40, 56),
    (-30, 60),
    (-20, 72),
    (-10, 72),
    (0, 72),
    (10, 72),
    (20, 72),
    (30, 60),
    (40, 56),
    (50, 45),
    (60, 36),
    (70, 24),
    (80, 12),
    (90, 1),
];

/// Third octaves `1000 · 2^(k/3)` Hz: k = −10 is 99 Hz, 9 is 7.9 kHz, 11 is
/// 12.7 kHz.
pub(super) const LOWEST_BAND: i32 = -10;
pub(super) const HIGHEST_COHERENCE_BAND: i32 = 9;
pub(super) const HIGHEST_COLOUR_BAND: i32 = 11;

/// The render as the measured responses give it, before the headphone
/// equalisation.
pub(super) const AS_MEASURED: Probe = Probe {
    headphone_equalisation: false,
    ..Probe::WHOLE
};

/// Two ears' spectra on one grid: each ear's power, their cross-spectrum,
/// and the power of the input that made them (all ones for the diffuse
/// field itself, whose input is white by definition).
pub(super) struct EarSpectra {
    pub(super) bin_hz: f64,
    pub(super) left: Vec<f64>,
    pub(super) right: Vec<f64>,
    pub(super) cross: Vec<Complex<f64>>,
    pub(super) input: Vec<f64>,
}

/// One third octave as two ears hear it.
pub(super) struct Band {
    pub(super) centre: f64,
    /// The largest normalised cross-correlation within ±1 ms of shift, as
    /// the ear compares its two inputs (the IACC of ISO 3382-1, per band).
    pub(super) coherence: f64,
    /// Its real part without shift: negative where the ears are opposed.
    pub(super) signed_coherence: f64,
    pub(super) left_db: f64,
    pub(super) right_db: f64,
}

impl EarSpectra {
    pub(super) fn band(&self, index: i32) -> Band {
        let centre = 1000.0 * 2f64.powf(index as f64 / 3.0);
        let lowest = (centre * 2f64.powf(-1.0 / 6.0) / self.bin_hz).ceil() as usize;
        let highest = (centre * 2f64.powf(1.0 / 6.0) / self.bin_hz).floor() as usize;
        let sum = |values: &[f64]| values[lowest..=highest].iter().sum::<f64>();
        let (left, right, input) = (sum(&self.left), sum(&self.right), sum(&self.input));
        let lag_samples = (0.001 * SAMPLE_RATE as f64).round() as i64;
        let coherence = (-lag_samples..=lag_samples)
            .map(|lag| {
                let seconds = lag as f64 / SAMPLE_RATE as f64;
                (lowest..=highest)
                    .map(|bin| {
                        let turn = std::f64::consts::TAU * bin as f64 * self.bin_hz * seconds;
                        self.cross[bin] * Complex::from_polar(1.0, turn)
                    })
                    .sum::<Complex<f64>>()
                    .norm()
            })
            .fold(0f64, f64::max)
            / (left * right).sqrt();
        let signed_coherence =
            self.cross[lowest..=highest].iter().sum::<Complex<f64>>().re / (left * right).sqrt();
        Band {
            centre,
            coherence,
            signed_coherence,
            left_db: 10.0 * (left / input).log10(),
            right_db: 10.0 * (right / input).log10(),
        }
    }
}

/// The diffuse field of the measured head, from its 710 measured responses,
/// and its response straight ahead; worked out once and shared by the tests.
pub(super) fn measured_diffuse_field() -> &'static (EarSpectra, EarSpectra) {
    static MEASURED: std::sync::OnceLock<(EarSpectra, EarSpectra)> = std::sync::OnceLock::new();
    MEASURED.get_or_init(work_out_measured_diffuse_field)
}

/// Fine enough for a third octave at 100 Hz (23 Hz wide) to hold 31 bins.
pub(super) const ANALYSIS_SIZE: usize = 1 << 16;

fn work_out_measured_diffuse_field() -> (EarSpectra, EarSpectra) {
    let sofa = OpenOptions::new()
        .sample_rate(SAMPLE_RATE)
        .open(PipelineParams::default().hrtf_path)
        .expect("the shipped HRIRs");
    // Each response is 558 samples; its power and cross spectra are the
    // transforms of correlations 1115 long, so they are summed on a grid of
    // 2048 and carried to the fine grid exactly through those correlations.
    let short_size = (2 * sofa.filter_len()).next_power_of_two();
    let short_bins = short_size / 2 + 1;
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
    let mut left_power = vec![Complex::new(0.0, 0.0); short_bins];
    let mut right_power = vec![Complex::new(0.0, 0.0); short_bins];
    let mut cross = vec![Complex::new(0.0, 0.0); short_bins];
    let mut distinct = std::collections::HashSet::new();
    let total_weight: f64 = MEASURED_RINGS
        .iter()
        .map(|&(elevation, _)| ring_solid_angle(elevation))
        .sum();
    for (elevation, count) in MEASURED_RINGS {
        let weight = ring_solid_angle(elevation) / count as f64 / total_weight;
        for step in 0..count {
            let azimuth = (360.0 * step as f64 / count as f64).to_radians();
            let elevation_radians = (elevation as f64).to_radians();
            sofa.filter_nointerp(
                (elevation_radians.cos() * azimuth.cos()) as f32,
                (elevation_radians.cos() * azimuth.sin()) as f32,
                elevation_radians.sin() as f32,
                &mut filter,
            );
            assert_eq!(
                (filter.ldelay, filter.rdelay),
                (0.0, 0.0),
                "the set's delays are in its responses"
            );
            distinct.insert(
                filter
                    .left
                    .iter()
                    .map(|sample| sample.to_bits())
                    .collect::<Vec<_>>(),
            );
            let left = spectrum_of(&filter.left);
            let right = spectrum_of(&filter.right);
            for bin in 0..short_bins {
                left_power[bin] += weight * left[bin].norm_sqr();
                right_power[bin] += weight * right[bin].norm_sqr();
                cross[bin] += weight * left[bin] * right[bin].conj();
            }
        }
    }
    assert_eq!(
        distinct.len(),
        710,
        "the grid names every measured direction"
    );
    let diffuse = EarSpectra {
        bin_hz: SAMPLE_RATE as f64 / ANALYSIS_SIZE as f64,
        left: on_fine_grid(&left_power).iter().map(|bin| bin.re).collect(),
        right: on_fine_grid(&right_power)
            .iter()
            .map(|bin| bin.re)
            .collect(),
        cross: on_fine_grid(&cross),
        input: vec![1.0; ANALYSIS_SIZE / 2 + 1],
    };

    sofa.filter_nointerp(1.0, 0.0, 0.0, &mut filter);
    let (left, right) = (spectrum_of(&filter.left), spectrum_of(&filter.right));
    let power = |spectrum: &[Complex<f64>]| -> Vec<Complex<f64>> {
        spectrum
            .iter()
            .map(|bin| Complex::new(bin.norm_sqr(), 0.0))
            .collect()
    };
    let ahead_cross: Vec<Complex<f64>> = left
        .iter()
        .zip(&right)
        .map(|(left, right)| left * right.conj())
        .collect();
    let ahead = EarSpectra {
        bin_hz: diffuse.bin_hz,
        left: on_fine_grid(&power(&left))
            .iter()
            .map(|bin| bin.re)
            .collect(),
        right: on_fine_grid(&power(&right))
            .iter()
            .map(|bin| bin.re)
            .collect(),
        cross: on_fine_grid(&ahead_cross),
        input: vec![1.0; ANALYSIS_SIZE / 2 + 1],
    };
    (diffuse, ahead)
}

/// A spectrum of a correlation shorter than its grid, on the analysis grid:
/// back to the correlation, zero-padded between its positive and negative
/// lags, and forward again.
pub(super) fn on_fine_grid(short: &[Complex<f64>]) -> Vec<Complex<f64>> {
    let short_size = 2 * (short.len() - 1);
    let mut planner = RealFftPlanner::<f64>::new();
    let inverse = planner.plan_fft_inverse(short_size);
    let forward = planner.plan_fft_forward(ANALYSIS_SIZE);
    let mut spectrum = short.to_vec();
    spectrum[0].im = 0.0;
    let last = spectrum.len() - 1;
    spectrum[last].im = 0.0;
    let mut correlation = inverse.make_output_vec();
    inverse
        .process(&mut spectrum, &mut correlation)
        .expect("inverse");
    let mut padded = forward.make_input_vec();
    for lag in 0..short_size / 2 {
        padded[lag] = correlation[lag] / short_size as f64;
        padded[ANALYSIS_SIZE - short_size / 2 + lag] =
            correlation[short_size / 2 + lag] / short_size as f64;
    }
    let mut fine = forward.make_output_vec();
    forward.process(&mut padded, &mut fine).expect("forward");
    fine
}

/// The solid angle of the band ±5° of elevation around a ring, over 2π;
/// the top ring covers only 85° to 90°.
pub(super) fn ring_solid_angle(elevation: i32) -> f64 {
    let lowest = (elevation as f64 - 5.0).to_radians();
    let highest = (elevation as f64 + 5.0).min(90.0).to_radians();
    highest.sin() - lowest.sin()
}

/// A recorded or generated signal played once, as an ambient sound or a
/// point, then silence. Its level is its own at 2 m.
pub(super) struct OneShot {
    pub(super) samples: Vec<f32>,
    pub(super) index: usize,
    pub(super) kind: EmitterKind,
    pub(super) position: Vec3,
}

/// Where a point source of the comparison has its own level: far enough
/// that the near-field correction leaves it alone.
pub(super) const REFERENCE_DISTANCE_METRES: f32 = 2.0;

impl SoundSource for OneShot {
    fn next_sample(&mut self, _sample_rate: f32) -> f32 {
        let sample = self.samples.get(self.index).copied().unwrap_or(0.0);
        self.index += 1;
        sample
    }
    fn position(&self) -> Vec3 {
        self.position
    }
    fn emitter_kind(&self) -> EmitterKind {
        self.kind
    }
    fn ref_distance(&self) -> f32 {
        REFERENCE_DISTANCE_METRES
    }
    fn tick(&mut self, _delta_seconds: f32) {}
}

/// Five seconds of white noise, then room for the filters to ring out: the
/// whole fits one transform of 2^18 samples, so the spectra are exact.
pub(super) const NOISE_FRAMES: usize = 240_000;
pub(super) const RENDER_FRAMES: usize = 1 << 18;

pub(super) fn white_noise() -> Vec<f32> {
    let mut generator = Rng::new(17);
    (0..NOISE_FRAMES)
        .map(|_| 0.25 * generator.next_bipolar())
        .collect()
}

/// `input` played as `kind` at `position` through the headphone pipeline,
/// the listener turned by `yaw_at(frame)` before each callback.
pub(super) fn render(
    rig: &mut Rig,
    input: &[f32],
    kind: EmitterKind,
    position: Vec3,
    probe: Probe,
    yaw_at: impl Fn(usize) -> f32,
) -> Vec<f32> {
    let mut pipeline = rig.build(RenderMode::Hrtf);
    pipeline.probe = probe;
    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(OneShot {
        samples: input.to_vec(),
        index: 0,
        kind,
        position,
    })];
    let mut output = Vec::with_capacity(RENDER_FRAMES * 2);
    let mut rendered = 0;
    while rendered < RENDER_FRAMES {
        let block = super::BLOCK_FRAMES.min(RENDER_FRAMES - rendered);
        rig.listener.yaw = yaw_at(rendered);
        output.extend(rig.render(&mut pipeline, &mut sources, block));
        rendered += block;
    }
    output
}

pub(super) fn render_ambient(input: &[f32], yaw_at: impl Fn(usize) -> f32) -> Vec<f32> {
    render_ambient_through(Probe::WHOLE, input, yaw_at)
}

pub(super) fn render_ambient_through(
    probe: Probe,
    input: &[f32],
    yaw_at: impl Fn(usize) -> f32,
) -> Vec<f32> {
    render(
        &mut Rig::headphones(SAMPLE_RATE),
        input,
        EmitterKind::Field,
        Vec3::ZERO,
        probe,
        yaw_at,
    )
}

/// The spectra of a stereo render `output` of `input`.
pub(super) fn spectra_of(output: &[f32], input: &[f32]) -> EarSpectra {
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(RENDER_FRAMES);
    let transform = |samples: &mut dyn Iterator<Item = f32>| {
        let mut time = forward.make_input_vec();
        for (slot, sample) in time.iter_mut().zip(samples) {
            *slot = sample as f64;
        }
        let mut spectrum = forward.make_output_vec();
        forward.process(&mut time, &mut spectrum).expect("forward");
        spectrum
    };
    let left = transform(&mut output.iter().step_by(2).copied());
    let right = transform(&mut output.iter().skip(1).step_by(2).copied());
    let source = transform(&mut input.iter().copied());
    EarSpectra {
        bin_hz: SAMPLE_RATE as f64 / RENDER_FRAMES as f64,
        left: left.iter().map(|bin| bin.norm_sqr()).collect(),
        right: right.iter().map(|bin| bin.norm_sqr()).collect(),
        cross: left
            .iter()
            .zip(&right)
            .map(|(left, right)| left * right.conj())
            .collect(),
        input: source.iter().map(|bin| bin.norm_sqr()).collect(),
    }
}

/// Each band's coherence and both ears' colour against the measured head;
/// the failure lists every band.
fn assert_heard_as_the_measured_diffuse_field(rendered: &EarSpectra, measured: &EarSpectra) {
    let mut report = String::from(
        "band Hz | measured coherence (signed) | rendered (signed) | left dB vs measured | right dB\n",
    );
    let mut worst_coherence = 0f64;
    let mut worst_colour = 0f64;
    for index in LOWEST_BAND..=HIGHEST_COLOUR_BAND {
        let (truth, heard) = (measured.band(index), rendered.band(index));
        let left_error = heard.left_db - truth.left_db;
        let right_error = heard.right_db - truth.right_db;
        report.push_str(&format!(
            "{:7.0} | {:.3} ({:+.3}) | {:.3} ({:+.3}) | {:+.2} | {:+.2}\n",
            truth.centre,
            truth.coherence,
            truth.signed_coherence,
            heard.coherence,
            heard.signed_coherence,
            left_error,
            right_error
        ));
        if index <= HIGHEST_COHERENCE_BAND {
            worst_coherence = worst_coherence.max((heard.coherence - truth.coherence).abs());
        }
        worst_colour = worst_colour.max(left_error.abs()).max(right_error.abs());
    }
    report.push_str(&format!(
        "worst coherence error {worst_coherence:.3} (100 Hz–8 kHz), worst colour error {worst_colour:.2} dB (100 Hz–12.5 kHz)"
    ));
    eprintln!("{report}");
    assert!(worst_coherence <= 0.15, "{report}");
    assert!(worst_colour <= 1.5, "{report}");
}

pub(super) fn facing_ahead(_frame: usize) -> f32 {
    std::f32::consts::FRAC_PI_2
}

/// Two ears in a diffuse field are alike in the bass and unrelated from
/// about 500 Hz up. From the 710 measured directions of the shipped head,
/// the interaural coherence (largest within ±1 ms) per third octave is
/// 0.911 at 125 Hz, 0.047 at 500 Hz, 0.073 at 1 kHz, 0.026 at 2 kHz and
/// 0.004 at 4 kHz (signed: +0.911, +0.046, −0.073, −0.026, −0.004). Ambient
/// white noise must be heard within 0.15 of it in every third octave from
/// 100 Hz to 8 kHz, and, before the headphone equalisation, each ear within
/// 1.5 dB of the head's diffuse-field response from 100 Hz to 12.5 kHz.
#[test]
fn two_ears_hear_ambient_noise_as_the_measured_diffuse_field() {
    let (measured, _) = measured_diffuse_field();
    let noise = white_noise();
    let output = render_ambient_through(AS_MEASURED, &noise, facing_ahead);
    assert_heard_as_the_measured_diffuse_field(&spectra_of(&output, &noise), measured);
}

/// Left and right ear powers of ambient white noise within 0.5 dB.
#[test]
fn ambient_noise_is_balanced_between_the_ears() {
    let output = render_ambient(&white_noise(), facing_ahead);
    let left: f64 = output.iter().step_by(2).map(|&s| (s as f64).powi(2)).sum();
    let right: f64 = output
        .iter()
        .skip(1)
        .step_by(2)
        .map(|&s| (s as f64).powi(2))
        .sum();
    let difference = 10.0 * (left / right).log10();
    eprintln!("left against right: {difference:+.3} dB");
    assert!(
        difference.abs() <= 0.5,
        "left is {difference:+.2} dB against right"
    );
}

/// A diffuse field is the same from every side: facing 0°, 37°, 90° or
/// 180°, the ears hear exactly the same, sample for sample, so the
/// coherence and colour above hold at every one.
#[test]
fn ambient_sound_is_the_same_from_every_side() {
    let noise = white_noise();
    let reference = render_ambient(&noise, |_| 0.0);
    for degrees in [37.0f32, 90.0, 180.0] {
        let turned = render_ambient(&noise, |_| degrees.to_radians());
        assert!(
            turned == reference,
            "facing {degrees}° the ambient sound changed"
        );
    }
}

/// Turning the head at 180° a second through ambient noise must not click.
/// The largest step between two samples is bounded by the same noise heard
/// with the head still; as the field does not depend on the head, the two
/// renders are the same sample for sample.
#[test]
fn turning_the_head_through_ambient_sound_never_clicks() {
    let noise = white_noise();
    let largest_step = |output: &[f32]| {
        output
            .chunks_exact(2)
            .zip(output.chunks_exact(2).skip(1))
            .map(|(previous, next)| {
                (next[0] - previous[0])
                    .abs()
                    .max((next[1] - previous[1]).abs())
            })
            .fold(0f32, f32::max)
    };
    let still = render_ambient(&noise, facing_ahead);
    let turning = render_ambient(&noise, |frame| {
        std::f32::consts::PI * frame as f32 / SAMPLE_RATE
    });
    let (still_step, turning_step) = (largest_step(&still), largest_step(&turning));
    assert!(
        turning_step <= still_step,
        "turning: largest step {turning_step}, still: {still_step}"
    );
    assert!(
        turning == still,
        "turning the head changed the ambient sound"
    );
}

/// A click through an ambient sound is heard once: at each ear at least
/// 85 % of its energy within 13 ms, and nothing later than 18 ms above
/// −20 dB relative to the strongest sample.
#[test]
fn an_attack_through_ambient_sound_is_heard_once() {
    let mut rig = Rig::headphones(SAMPLE_RATE);
    let mut pipeline = rig.build(RenderMode::Hrtf);
    struct Ambient(Box<dyn SoundSource>);
    impl SoundSource for Ambient {
        fn next_sample(&mut self, sample_rate: f32) -> f32 {
            self.0.next_sample(sample_rate)
        }
        fn position(&self) -> Vec3 {
            Vec3::ZERO
        }
        fn emitter_kind(&self) -> EmitterKind {
            EmitterKind::Field
        }
        fn tick(&mut self, _delta_seconds: f32) {}
    }
    rig.listener.yaw = facing_ahead(0);
    let mut sources: Vec<Box<dyn SoundSource>> =
        vec![Box::new(Ambient(ImpulseSource::boxed(Vec3::ZERO, 0)))];
    let frames = (0.2 * SAMPLE_RATE) as usize;
    let output = rig.render(&mut pipeline, &mut sources, frames);
    let within_13_ms = (0.013 * SAMPLE_RATE) as usize;
    let after_18_ms = (0.018 * SAMPLE_RATE) as usize;
    for (ear, name) in [(0, "left"), (1, "right")] {
        let response: Vec<f64> = output
            .iter()
            .skip(ear)
            .step_by(2)
            .map(|&s| s as f64)
            .collect();
        let total: f64 = response.iter().map(|s| s * s).sum();
        let early: f64 = response[..within_13_ms].iter().map(|s| s * s).sum();
        let strongest = response.iter().fold(0f64, |peak, s| peak.max(s.abs()));
        let latest = response[after_18_ms + 1..]
            .iter()
            .fold(0f64, |peak, s| peak.max(s.abs()));
        let late_db = 20.0 * (latest / strongest).log10();
        assert!(
            early / total >= 0.85,
            "{name} ear: {:.1} % of the click's energy within 13 ms",
            100.0 * early / total
        );
        assert!(
            late_db <= -20.0,
            "{name} ear: a sample after 18 ms at {late_db:.1} dB"
        );
        eprintln!(
            "{name} ear: {:.1} % within 13 ms, strongest after 18 ms {late_db:.1} dB",
            100.0 * early / total
        );
    }
}

/// How loud an ambient sound is at the ear, against a point source of the
/// same level at its reference distance straight ahead, before the
/// headphone equalisation: the head's diffuse response over its response
/// from ahead, both from the measured responses. (Equalised, both are
/// divided by nearly the same, and `headphone_equalisation.rs` reports what
/// that leaves.) For white noise that is +0.05 dB at each ear: as loud. By third octave
/// it is −0.1 dB at 125 Hz, +0.2 dB at 500 Hz, +1.5 dB at 1 kHz, −0.3 dB at
/// 2.5 kHz, −1.5 dB at 4 kHz and +12.9 dB at 8 kHz, where the response from
/// straight ahead has a deep notch that sound from all round fills in.
#[test]
fn ambient_sound_is_as_loud_as_the_measured_head_hears_it() {
    let (measured, ahead) = measured_diffuse_field();
    let noise = white_noise();
    let ambient = render_ambient_through(AS_MEASURED, &noise, facing_ahead);
    // At 2 m the near field leaves the sound alone; the air and the ground
    // are switched off, so the head is all that is compared.
    let point = render(
        &mut Rig::headphones(SAMPLE_RATE),
        &noise,
        EmitterKind::Object,
        Vec3::new(0.0, REFERENCE_DISTANCE_METRES, 0.0),
        Probe {
            air_absorption: false,
            ground_effect: false,
            ..AS_MEASURED
        },
        facing_ahead,
    );
    let power = |output: &[f32], ear: usize| -> f64 {
        output
            .iter()
            .skip(ear)
            .step_by(2)
            .map(|&s| (s as f64).powi(2))
            .sum()
    };
    let mut report = String::new();
    for (ear, name, diffuse, frontal) in [
        (0, "left", &measured.left, &ahead.left),
        (1, "right", &measured.right, &ahead.right),
    ] {
        let expected = 10.0 * (diffuse.iter().sum::<f64>() / frontal.iter().sum::<f64>()).log10();
        let heard = 10.0 * (power(&ambient, ear) / power(&point, ear)).log10();
        report.push_str(&format!(
            "{name} ear: ambient against ahead {heard:+.2} dB, the measured head says {expected:+.2} dB\n"
        ));
        assert!((heard - expected).abs() <= 0.5, "{report}");
    }
    for index in [-9, -3, 0, 4, 6, 9] {
        let (diffuse, frontal) = (measured.band(index), ahead.band(index));
        report.push_str(&format!(
            "{:.0} Hz: ambient against ahead {:+.1} dB\n",
            diffuse.centre,
            diffuse.left_db - frontal.left_db
        ));
    }
    eprintln!("{report}");
}
