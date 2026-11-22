//! The headphone equalisation through the whole headphone pipeline: a
//! diffuse field reaches each ear flat, whether it is the measured
//! directions together or an ambient sound; the cues that tell one
//! direction from another survive; the switch never clicks; and what it
//! changes in level.
//!
//! The diffuse field here is the independent one of `ambient_headphones`:
//! every measured direction weighted by the solid angle its ring covers,
//! not the renderer's own Voronoi weighting.

use std::path::Path;

use realfft::RealFftPlanner;

use super::ambient_headphones::{
    facing_ahead, measured_diffuse_field, render, render_ambient, ring_solid_angle, spectra_of,
    white_noise, EarSpectra, AS_MEASURED, HIGHEST_COHERENCE_BAND, HIGHEST_COLOUR_BAND, LOWEST_BAND,
    MEASURED_RINGS, NOISE_FRAMES, REFERENCE_DISTANCE_METRES,
};
use super::{Rig, BLOCK_FRAMES};
use crate::audio::decode::decode_file;
use atrium_core::probe::Probe;
use atrium_core::source::{EmitterKind, SoundSource};
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

const SAMPLE_RATE: f32 = 48_000.0;

/// The air and the ground off: the head and the equaliser are all that is
/// measured.
const HEAD_ALONE: Probe = Probe {
    air_absorption: false,
    ground_effect: false,
    ..Probe::WHOLE
};

/// The analysis grid: a third octave at 100 Hz holds 31 bins.
const SPECTRUM_SIZE: usize = 1 << 16;

/// Each measured direction is heard for this long: the HRIRs follow a
/// moved source within four callbacks, the impulse comes in the sixth, and
/// its response, 280 samples of travel, 558 of HRIR and 960 of equaliser,
/// is over well before the next direction.
const SEGMENT_FRAMES: usize = 12 * BLOCK_FRAMES;
const IMPULSE_AT: usize = 5 * BLOCK_FRAMES;

/// Exactly 280 samples of travel at 20 °C and 48 kHz (2.003 m): a whole
/// number of samples, so the delay line does not interpolate and dull the
/// treble.
fn whole_sample_distance() -> f32 {
    280.0 * crate::audio::atmosphere::speed_of_sound(20.0) / SAMPLE_RATE
}

/// One unit impulse from a fixed point, at its own reference distance.
struct DirectionImpulse {
    position: Vec3,
    sample_index: usize,
}

impl SoundSource for DirectionImpulse {
    fn next_sample(&mut self, _sample_rate: f32) -> f32 {
        let sample = if self.sample_index == IMPULSE_AT {
            1.0
        } else {
            0.0
        };
        self.sample_index += 1;
        sample
    }
    fn position(&self) -> Vec3 {
        self.position
    }
    fn ref_distance(&self) -> f32 {
        whole_sample_distance()
    }
    fn tick(&mut self, _delta_seconds: f32) {}
}

/// Where a direction of the measured grid lies for the rig's listener, at
/// the origin facing +y: forward is +y, left is −x.
fn world_position(azimuth_degrees: f64, elevation_degrees: f64, distance: f32) -> Vec3 {
    let (azimuth, elevation) = (azimuth_degrees.to_radians(), elevation_degrees.to_radians());
    let forward = elevation.cos() * azimuth.cos();
    let left = elevation.cos() * azimuth.sin();
    let up = elevation.sin();
    Vec3::new(-left as f32, forward as f32, up as f32) * distance
}

fn power_spectrum(samples: impl Iterator<Item = f32>) -> Vec<f64> {
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(SPECTRUM_SIZE);
    let mut time = forward.make_input_vec();
    for (slot, sample) in time.iter_mut().zip(samples) {
        *slot = sample as f64;
    }
    let mut spectrum = forward.make_output_vec();
    forward.process(&mut time, &mut spectrum).expect("forward");
    spectrum.iter().map(|bin| bin.norm_sqr()).collect()
}

/// Each ear's power spectrum of the measured directions heard through the
/// headphone pipeline with `probe`, each weighted by its solid angle: what
/// two ears hear of a diffuse field made of point sources. Its input is a
/// unit impulse, white, so flat is 0 dB.
fn diffuse_field_of_point_sources(probe: Probe) -> (Vec<f64>, Vec<f64>) {
    let rig = Rig::headphones(SAMPLE_RATE);
    let mut pipeline = rig.build(RenderMode::Hrtf);
    pipeline.probe = probe;
    let bins = SPECTRUM_SIZE / 2 + 1;
    let (mut left, mut right) = (vec![0.0f64; bins], vec![0.0f64; bins]);
    let total_weight: f64 = MEASURED_RINGS
        .iter()
        .map(|&(elevation, _)| ring_solid_angle(elevation))
        .sum();
    for (elevation, count) in MEASURED_RINGS {
        let weight = ring_solid_angle(elevation) / count as f64 / total_weight;
        for step in 0..count {
            let azimuth = 360.0 * step as f64 / count as f64;
            let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(DirectionImpulse {
                position: world_position(azimuth, elevation as f64, whole_sample_distance()),
                sample_index: 0,
            })];
            let output = rig.render(&mut pipeline, &mut sources, SEGMENT_FRAMES);
            let response = &output[IMPULSE_AT * 2..];
            for (sum, ear) in [(&mut left, 0), (&mut right, 1)] {
                let spectrum = power_spectrum(response.iter().skip(ear).step_by(2).copied());
                for (total, power) in sum.iter_mut().zip(spectrum) {
                    *total += weight * power;
                }
            }
        }
    }
    (left, right)
}

/// The mean of `power` over the third octave 1000 · 2^(band/3) Hz, in dB.
fn third_octave_db(power: &[f64], band: i32) -> f64 {
    band_db(
        power,
        1000.0 * 2f64.powf((band as f64 - 0.5) / 3.0),
        1000.0 * 2f64.powf((band as f64 + 0.5) / 3.0),
    )
}

/// The mean of `power`, on the analysis grid, from `lowest` to `highest`
/// Hz, in dB.
fn band_db(power: &[f64], lowest: f64, highest: f64) -> f64 {
    let bin_hz = SAMPLE_RATE as f64 / SPECTRUM_SIZE as f64;
    let first = (lowest / bin_hz).ceil() as usize;
    let last = ((highest / bin_hz).floor() as usize).min(power.len() - 1);
    10.0 * (power[first..=last].iter().sum::<f64>() / (last - first + 1) as f64).log10()
}

/// The equaliser divides the set's diffuse-field response out: the measured
/// directions together, each weighted by its solid angle and heard through
/// the equaliser, reach each ear flat, within 1 dB in every third octave
/// from 100 Hz to 12.5 kHz. That flat is 0 dB pins the level convention: a
/// diffuse field reaches each ear at the power of the signal that made it.
/// The same field heard as measured, divided out, is the equaliser itself,
/// and the failure lists its gain at every band.
#[test]
fn a_diffuse_field_of_the_measured_directions_reaches_each_ear_flat() {
    let (left, right) = diffuse_field_of_point_sources(HEAD_ALONE);
    let (left_measured, right_measured) = diffuse_field_of_point_sources(Probe {
        headphone_equalisation: false,
        ..HEAD_ALONE
    });
    let mut report = String::from("band Hz | left dB | right dB | equaliser left dB | right dB\n");
    let mut worst = 0f64;
    for band in LOWEST_BAND..=12 {
        let (left_db, right_db) = (third_octave_db(&left, band), third_octave_db(&right, band));
        let (left_equaliser, right_equaliser) = (
            left_db - third_octave_db(&left_measured, band),
            right_db - third_octave_db(&right_measured, band),
        );
        report.push_str(&format!(
            "{:7.0} | {left_db:+.2} | {right_db:+.2} | {left_equaliser:+.2} | {right_equaliser:+.2}\n",
            1000.0 * 2f64.powf(band as f64 / 3.0)
        ));
        if band <= HIGHEST_COLOUR_BAND {
            worst = worst.max(left_db.abs()).max(right_db.abs());
        }
    }
    report.push_str(&format!(
        "worst {worst:.2} dB from 100 Hz to 12.5 kHz; 14.3 to 16 kHz: left {:+.2} dB",
        band_db(&left, 16_000.0 * 2f64.powf(-1.0 / 6.0), 16_000.0)
    ));
    eprintln!("{report}");
    assert!(worst <= 1.0, "{report}");
}

/// An ambient sound, equalised, reaches each ear flat: white noise within
/// 1.5 dB of its own level in every third octave from 100 Hz to 12.5 kHz.
/// The coherence is the measured head's as before, within 0.15 from 100 Hz
/// to 8 kHz, and the ears as balanced: within 0.5 dB of each other over
/// the whole band.
///
/// Third octave by third octave the ears are not within 0.5 dB of each
/// other, and were not before: the echo that makes them unalike has the
/// opposite sign in the two, so where it adds at one ear it takes away at
/// the other: up to 1.1 dB apart (at 100 Hz and 2 kHz), and 1.5 dB as
/// measured. The equalisation must not make that worse.
#[test]
fn ambient_sound_reaches_each_ear_flat_with_the_heads_coherence() {
    let (measured, _) = measured_diffuse_field();
    let noise = white_noise();
    let rendered = spectra_of(&render_ambient(&noise, facing_ahead), &noise);
    let as_measured = spectra_of(
        &super::ambient_headphones::render_ambient_through(AS_MEASURED, &noise, facing_ahead),
        &noise,
    );
    let mut report =
        String::from("band Hz | left dB | right dB | coherence: measured | rendered\n");
    let (mut worst_flat, mut worst_between, mut worst_coherence) = (0f64, 0f64, 0f64);
    let mut worst_between_as_measured = 0f64;
    for band in LOWEST_BAND..=HIGHEST_COLOUR_BAND {
        let (truth, heard) = (measured.band(band), rendered.band(band));
        let before = as_measured.band(band);
        worst_between_as_measured =
            worst_between_as_measured.max((before.left_db - before.right_db).abs());
        report.push_str(&format!(
            "{:7.0} | {:+.2} | {:+.2} | {:.3} | {:.3}\n",
            heard.centre, heard.left_db, heard.right_db, truth.coherence, heard.coherence
        ));
        worst_flat = worst_flat
            .max(heard.left_db.abs())
            .max(heard.right_db.abs());
        worst_between = worst_between.max((heard.left_db - heard.right_db).abs());
        if band <= HIGHEST_COHERENCE_BAND {
            worst_coherence = worst_coherence.max((heard.coherence - truth.coherence).abs());
        }
    }
    let whole_band = |spectra: &EarSpectra| {
        10.0 * (spectra.left.iter().sum::<f64>() / spectra.right.iter().sum::<f64>()).log10()
    };
    let balance = whole_band(&rendered);
    report.push_str(&format!(
        "worst {worst_flat:.2} dB from flat; ears {balance:+.2} dB apart over the whole band, by third octave up to {worst_between:.2} dB ({worst_between_as_measured:.2} dB as measured); coherence off by {worst_coherence:.3}"
    ));
    eprintln!("{report}");
    assert!(worst_flat <= 1.5, "{report}");
    assert!(balance.abs() <= 0.5, "{report}");
    assert!(
        worst_between <= worst_between_as_measured + 0.05,
        "{report}"
    );
    assert!(worst_coherence <= 0.15, "{report}");
}

/// The mean of `power` over the octave around `centre`, in dB.
fn octave_db(spectra: &EarSpectra, power: &[f64], centre: f64) -> f64 {
    let first = (centre / std::f64::consts::SQRT_2 / spectra.bin_hz).ceil() as usize;
    let last = (centre * std::f64::consts::SQRT_2 / spectra.bin_hz).floor() as usize;
    10.0 * (power[first..=last].iter().sum::<f64>()
        / spectra.input[first..=last].iter().sum::<f64>())
    .log10()
}

/// The lag, in samples, at which the right ear best matches the left
/// within ±1 ms: positive when the right ear hears later.
fn interaural_lag(output: &[f32]) -> i64 {
    let largest_lag = (0.001 * SAMPLE_RATE) as i64;
    let frames = output.len() / 2;
    let span = (0.5 * SAMPLE_RATE) as usize..(4.5 * SAMPLE_RATE) as usize;
    (-largest_lag..=largest_lag)
        .map(|lag| {
            let correlation: f64 = span
                .clone()
                .filter(|&frame| {
                    let shifted = frame as i64 + lag;
                    shifted >= 0 && (shifted as usize) < frames
                })
                .map(|frame| {
                    output[frame * 2] as f64 * output[(frame as i64 + lag) as usize * 2 + 1] as f64
                })
                .sum();
            (lag, correlation)
        })
        .fold((0, f64::MIN), |best, candidate| {
            if candidate.1 > best.1 {
                candidate
            } else {
                best
            }
        })
        .0
}

/// The interaural level difference over the octave around `centre`, as
/// the mean of the ears' level difference at each frequency in it, in dB.
fn octave_level_difference_db(spectra: &EarSpectra, centre: f64) -> f64 {
    let first = (centre / std::f64::consts::SQRT_2 / spectra.bin_hz).ceil() as usize;
    let last = (centre * std::f64::consts::SQRT_2 / spectra.bin_hz).floor() as usize;
    (first..=last)
        .map(|bin| 10.0 * (spectra.left[bin] / spectra.right[bin]).log10())
        .sum::<f64>()
        / (last - first + 1) as f64
}

/// The equaliser is per ear and nearly the same in both, so what tells the
/// ears apart survives it: for white noise straight to the left, the
/// interaural level difference, the ears' level difference at each
/// frequency averaged over each octave from 500 Hz to 8 kHz, changes by no
/// more than 0.2 dB, and the interaural time difference not at all.
///
/// The ratio of the octave's power at one ear to the other's is reported
/// too. It moves by up to 1.1 dB, at 4 kHz: not because the ears are told
/// apart differently, but because the equaliser, which changes by up to
/// 10 dB inside the 2 and 4 kHz octaves, weighs the frequencies within an
/// octave differently, and the head's level difference changes across it.
#[test]
fn the_cues_between_the_ears_survive_the_equalisation() {
    let noise = white_noise();
    let left_of_the_head = Vec3::new(-REFERENCE_DISTANCE_METRES, 0.0, 0.0);
    let heard = |probe: Probe| {
        render(
            &mut Rig::headphones(SAMPLE_RATE),
            &noise,
            EmitterKind::Object,
            left_of_the_head,
            probe,
            facing_ahead,
        )
    };
    let equalised = heard(HEAD_ALONE);
    let as_measured = heard(Probe {
        headphone_equalisation: false,
        ..HEAD_ALONE
    });
    let (with, without) = (
        spectra_of(&equalised, &noise),
        spectra_of(&as_measured, &noise),
    );
    let mut report = String::from(
        "octave Hz | level difference equalised | as measured | change | change in the power ratio\n",
    );
    let mut worst = 0f64;
    for centre in [500.0, 1000.0, 2000.0, 4000.0, 8000.0] {
        let (equalised_difference, measured_difference) = (
            octave_level_difference_db(&with, centre),
            octave_level_difference_db(&without, centre),
        );
        let change = equalised_difference - measured_difference;
        let power_ratio_change = (octave_db(&with, &with.left, centre)
            - octave_db(&with, &with.right, centre))
            - (octave_db(&without, &without.left, centre)
                - octave_db(&without, &without.right, centre));
        report.push_str(&format!(
            "{centre:5.0} | {equalised_difference:+.2} dB | {measured_difference:+.2} dB | {change:+.3} dB | {power_ratio_change:+.2} dB\n"
        ));
        worst = worst.max(change.abs());
    }
    let (lag_with, lag_without) = (interaural_lag(&equalised), interaural_lag(&as_measured));
    report.push_str(&format!(
        "interaural lag: equalised {lag_with} samples, as measured {lag_without} samples"
    ));
    eprintln!("{report}");
    assert!(worst <= 0.2, "{report}");
    assert_eq!(lag_with, lag_without, "{report}");
    assert!(
        lag_with > 0,
        "the right ear should hear a source on the left later: {report}"
    );
}

/// A tone that does not stop, as a point source and as an ambient sound.
struct Tone {
    frequency: f64,
    sample_index: usize,
    kind: EmitterKind,
    position: Vec3,
}

impl SoundSource for Tone {
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        let phase =
            std::f64::consts::TAU * self.frequency * self.sample_index as f64 / sample_rate as f64;
        self.sample_index += 1;
        0.5 * phase.sin() as f32
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

/// The largest step between two samples of either ear, from `from_frame` on.
fn largest_step(output: &[f32], from_frame: usize) -> f32 {
    output
        .chunks_exact(2)
        .skip(from_frame)
        .zip(output.chunks_exact(2).skip(from_frame + 1))
        .map(|(previous, next)| {
            (next[0] - previous[0])
                .abs()
                .max((next[1] - previous[1]).abs())
        })
        .fold(0f32, f32::max)
}

/// Switching the equalisation on and off every quarter of a second during
/// a sustained 250 Hz tone straight ahead and a 160 Hz ambient tone (the
/// equaliser lifts the one by 9.6 dB, and takes away the other's colour,
/// −9.4 dB) never clicks: no step between two samples is larger than the largest in
/// the louder of the two renders held steady, and 10 % allowed for the
/// glide's own slope. Cut instead of glided, the step would be the whole
/// difference between the two.
#[test]
fn switching_the_equalisation_never_clicks() {
    let frames = 2 * SAMPLE_RATE as usize;
    let settled = (0.1 * SAMPLE_RATE) as usize;
    let render_switching = |switch: &dyn Fn(usize) -> bool| {
        let mut rig = Rig::headphones(SAMPLE_RATE);
        rig.listener.yaw = facing_ahead(0);
        let mut pipeline = rig.build(RenderMode::Hrtf);
        let mut sources: Vec<Box<dyn SoundSource>> = vec![
            Box::new(Tone {
                frequency: 250.0,
                sample_index: 0,
                kind: EmitterKind::Object,
                position: Vec3::new(0.0, REFERENCE_DISTANCE_METRES, 0.0),
            }),
            Box::new(Tone {
                frequency: 160.0,
                sample_index: 0,
                kind: EmitterKind::Field,
                position: Vec3::ZERO,
            }),
        ];
        pipeline.ensure_topology(sources.len(), &rig.layout, rig.sample_rate);
        let mut output = Vec::with_capacity(frames * 2);
        let mut rendered = 0;
        while rendered < frames {
            pipeline.probe = Probe {
                headphone_equalisation: switch(rendered),
                ..HEAD_ALONE
            };
            output.extend(rig.render(&mut pipeline, &mut sources, BLOCK_FRAMES));
            rendered += BLOCK_FRAMES;
        }
        output
    };
    let equalised = render_switching(&|_| true);
    let as_measured = render_switching(&|_| false);
    let quarter = (0.25 * SAMPLE_RATE) as usize;
    let switching = render_switching(&|frame| (frame / quarter).is_multiple_of(2));
    let steady = largest_step(&equalised, settled).max(largest_step(&as_measured, settled));
    let difference = equalised
        .iter()
        .zip(&as_measured)
        .skip(settled * 2)
        .fold(0f32, |largest, (a, b)| largest.max((a - b).abs()));
    let switched = largest_step(&switching, settled);
    let report = format!(
        "largest step switching {switched:.4}, held steady {steady:.4}; the two renders differ by up to {difference:.4}"
    );
    eprintln!("{report}");
    assert!(switched <= 1.1 * steady, "{report}");
    assert!(
        difference > 5.0 * steady,
        "the switch made no difference: {report}"
    );
}

/// What the equalisation changes in level, reported for the record and held
/// to what follows from the level convention: a point source straight
/// ahead at its reference distance (white noise, broadband and by octave),
/// and the frog chorus made ambient against the render before it (the
/// head's colour) and before that (half the power at each ear, flat).
/// Ambient sound is now flat at unit gain, so the frog's level at each ear
/// is its own, within 0.5 dB, and 3 dB above half of it.
#[test]
fn what_the_equalisation_changes_in_level() {
    let noise = white_noise();
    let ahead = Vec3::new(0.0, REFERENCE_DISTANCE_METRES, 0.0);
    let point = |probe: Probe| {
        render(
            &mut Rig::headphones(SAMPLE_RATE),
            &noise,
            EmitterKind::Object,
            ahead,
            probe,
            facing_ahead,
        )
    };
    let equalised = spectra_of(&point(HEAD_ALONE), &noise);
    let as_measured = spectra_of(
        &point(Probe {
            headphone_equalisation: false,
            ..HEAD_ALONE
        }),
        &noise,
    );
    let broadband = |spectra: &EarSpectra, power: &[f64]| {
        10.0 * (power.iter().sum::<f64>() / spectra.input.iter().sum::<f64>()).log10()
    };
    let mut report = format!(
        "point source ahead, white noise: left {:+.2} dB, right {:+.2} dB broadband\n",
        broadband(&equalised, &equalised.left) - broadband(&as_measured, &as_measured.left),
        broadband(&equalised, &equalised.right) - broadband(&as_measured, &as_measured.right),
    );
    for centre in [125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0] {
        report.push_str(&format!(
            "  {centre:5.0} Hz octave: {:+.2} dB\n",
            octave_db(&equalised, &equalised.left, centre)
                - octave_db(&as_measured, &as_measured.left, centre)
        ));
    }

    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let frog = decode_file(&assets.join("frog.mp3")).expect("decode frog");
    let step = frog.sample_rate as f64 / SAMPLE_RATE as f64;
    // As much as the render holds with its tail: five seconds.
    let frog: Vec<f32> = (0..(((frog.samples.len() - 1) as f64 / step) as usize).min(NOISE_FRAMES))
        .map(|index| {
            let position = index as f64 * step;
            let (whole, part) = (position as usize, position.fract() as f32);
            frog.samples[whole] * (1.0 - part) + frog.samples[whole + 1] * part
        })
        .collect();
    let input_power: f64 = frog.iter().map(|&s| (s as f64).powi(2)).sum();
    let ambient = |probe: Probe| {
        super::ambient_headphones::render_ambient_through(probe, &frog, facing_ahead)
    };
    let ear_power = |output: &[f32], ear: usize| -> f64 {
        output
            .iter()
            .skip(ear)
            .step_by(2)
            .map(|&s| (s as f64).powi(2))
            .sum()
    };
    let (frog_equalised, frog_measured) = (ambient(Probe::WHOLE), ambient(AS_MEASURED));
    for (ear, name) in [(0, "left"), (1, "right")] {
        let now = 10.0 * (ear_power(&frog_equalised, ear) / input_power).log10();
        let coloured = 10.0 * (ear_power(&frog_measured, ear) / input_power).log10();
        let half_power = 10.0 * 0.5f64.log10();
        report.push_str(&format!(
            "frog, ambient, {name} ear against its own level: {now:+.2} dB now, {coloured:+.2} dB with the head's colour, {half_power:+.2} dB before that; now against the colour {:+.2} dB, against before {:+.2} dB\n",
            now - coloured,
            now - half_power
        ));
        assert!(now.abs() <= 0.5, "{report}");
    }
    eprintln!("{report}");
}
