//! How much of each ear's tone of a binaural beat reaches the other ear in
//! the 3D render, measured at the ears: the alpha piece (108 Hz left, 118 Hz
//! right) on headphones, each tone held 12 cm beside its ear. The numbers
//! are recorded in `docs/binaural-beats.md` ("In this 3D version, measured").

use std::f64::consts::TAU;

use realfft::RealFftPlanner;

use super::{on_headphones, render};
use crate::config::SceneConfig;
use crate::offline::{RenderSettings, Rendering};

const SCENE: &str = "scenes/binaural-beat-test.yaml";
const LEFT_HZ: f64 = 108.0;
const RIGHT_HZ: f64 = 118.0;
/// Measured from here, well past the 2 s fade-in and the render's settling.
const FROM_SECONDS: f64 = 4.0;
/// For this long: a whole number of cycles of both tones and of the beat.
const MEASURED_SECONDS: f64 = 10.0;

/// One channel of an interleaved stereo render.
fn channel(frames: &[f32], index: usize) -> Vec<f32> {
    frames.iter().skip(index).step_by(2).copied().collect()
}

/// Amplitude of the component at `frequency_hz` in `samples`: a single-bin
/// DFT over a whole number of its cycles, so a sine of amplitude A reads A.
fn amplitude_at(samples: &[f32], sample_rate: f64, frequency_hz: f64) -> f64 {
    let step = TAU * frequency_hz / sample_rate;
    let (mut real, mut imaginary) = (0.0, 0.0);
    for (index, &sample) in samples.iter().enumerate() {
        let angle = step * index as f64;
        real += sample as f64 * angle.cos();
        imaginary -= sample as f64 * angle.sin();
    }
    2.0 * (real * real + imaginary * imaginary).sqrt() / samples.len() as f64
}

/// The envelope of `samples`: the magnitude of its analytic signal, the
/// samples with their Hilbert transform (every component turned a quarter
/// cycle back) as the imaginary part.
fn envelope(samples: &[f32]) -> Vec<f64> {
    let length = samples.len();
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(length);
    let inverse = planner.plan_fft_inverse(length);
    let mut input: Vec<f64> = samples.iter().map(|&sample| sample as f64).collect();
    let mut spectrum = forward.make_output_vec();
    forward.process(&mut input, &mut spectrum).unwrap();
    // −i on every positive frequency; nothing at 0 Hz or at Nyquist.
    for value in spectrum.iter_mut() {
        *value = realfft::num_complex::Complex::new(value.im, -value.re);
    }
    let last = spectrum.len() - 1;
    spectrum[0] = Default::default();
    spectrum[last] = Default::default();
    let mut transformed = inverse.make_output_vec();
    inverse.process(&mut spectrum, &mut transformed).unwrap();
    samples
        .iter()
        .zip(&transformed)
        .map(|(&sample, &quadrature)| (sample as f64).hypot(quadrature / length as f64))
        .collect()
}

/// What one ear hears of the two tones.
#[derive(Debug)]
struct Ear {
    /// Its own tone's amplitude.
    own: f64,
    /// Its own tone's level (dB SPL).
    own_db_spl: f64,
    /// The other ear's tone's amplitude, in dB against its own.
    other_db: f64,
    /// How far its level swings at the beat rate, loudest to softest (dB).
    swing_db: f64,
    /// The swing two tones of these amplitudes make together (dB):
    /// 20·log10((own + other) / (own − other)).
    expected_swing_db: f64,
}

/// What each ear hears, the left then the right, of `seconds` of the scene
/// on headphones turning at `turn_degrees_per_second`, with the scene's
/// environment replaced by `environment` if one is given.
fn ears(environment: Option<&str>, turn_degrees_per_second: f64) -> (Ear, Ear) {
    let mut config = SceneConfig::load(SCENE).unwrap_or_else(|error| panic!("{SCENE}: {error}"));
    if let Some(environment) = environment {
        config.environment = Some(environment.to_string());
    }
    let settings = RenderSettings {
        turn_degrees_per_second,
        ..on_headphones(FROM_SECONDS + MEASURED_SECONDS)
    };
    let rendered = Rendering::prepare(config, &settings)
        .unwrap_or_else(|error| panic!("{SCENE}: {error}"))
        .run();
    assert_eq!(rendered.channels(), 2);
    let sample_rate = rendered.sample_rate() as f64;
    let full_scale_db_spl = rendered.full_scale_db_spl();
    let first = (FROM_SECONDS * sample_rate) as usize;
    let measure = |index: usize, own_hz: f64, other_hz: f64| {
        let samples = &channel(rendered.frames(), index)[first..];
        let own = amplitude_at(samples, sample_rate, own_hz);
        let other = amplitude_at(samples, sample_rate, other_hz);
        // The envelope away from the transform's wrapped ends.
        let edge = (0.5 * sample_rate) as usize;
        let level = envelope(samples);
        let level = &level[edge..level.len() - edge];
        let loudest = level.iter().copied().fold(0.0, f64::max);
        let softest = level.iter().copied().fold(f64::INFINITY, f64::min);
        Ear {
            own,
            own_db_spl: full_scale_db_spl + 20.0 * (own / std::f64::consts::SQRT_2).log10(),
            other_db: 20.0 * (other / own).log10(),
            swing_db: 20.0 * (loudest / softest).log10(),
            expected_swing_db: 20.0 * ((own + other) / (own - other)).log10(),
        }
    };
    (measure(0, LEFT_HZ, RIGHT_HZ), measure(1, RIGHT_HZ, LEFT_HZ))
}

fn report(case: &str, (left, right): &(Ear, Ear)) {
    for (name, ear) in [("left", left), ("right", right)] {
        println!(
            "{case}, {name} ear: own tone {:.1} dB SPL, the other ear's tone {:+.2} dB, \
             level swings {:.2} dB at the beat (two tones of these levels: {:.2} dB)",
            ear.own_db_spl, ear.other_db, ear.swing_db, ear.expected_swing_db,
        );
    }
}

/// In the open meadow, still, the tones held 12 cm from the head's centre:
/// each ear hears its own tone at about 50 dB SPL and the other ear's tone
/// some 21 to 22 dB below it, so its level swings by about 1.4 dB at the
/// 10 Hz beat, as two tones of those levels do. Measured 2026-11-22: see
/// `docs/binaural-beats.md`.
#[test]
fn each_ear_hears_its_own_tone_and_the_other_leaks_in() {
    let meadow = ears(None, 0.0);
    report("meadow", &meadow);
    for (name, ear) in [("left", &meadow.0), ("right", &meadow.1)] {
        assert!(
            (ear.own_db_spl - 50.0).abs() < 1.5,
            "{name}: its own tone at {:.1} dB SPL",
            ear.own_db_spl
        );
        assert!(
            ear.other_db < -19.0,
            "{name}: the other tone at {:+.2} dB",
            ear.other_db
        );
        assert!(
            ear.other_db > -24.0,
            "{name}: the other tone at {:+.2} dB",
            ear.other_db
        );
        assert!(
            (ear.swing_db - ear.expected_swing_db).abs() < 0.3,
            "{name}: swings {:.2} dB, two tones make {:.2} dB",
            ear.swing_db,
            ear.expected_swing_db
        );
    }
    // The two ears mirror each other within a decibel.
    assert!((meadow.0.other_db - meadow.1.other_db).abs() < 1.0);
}

/// Head tracking turns the listener, and the tones turn with the head: a
/// head turning 45° a second hears what a still one does, within a tenth of
/// a decibel.
#[test]
fn the_tones_stay_at_the_ears_when_the_head_turns() {
    let still = ears(None, 0.0);
    let turning = ears(None, 45.0);
    report("meadow, the head turning 45°/s", &turning);
    for (still, turning) in [(&still.0, &turning.0), (&still.1, &turning.1)] {
        assert!((still.other_db - turning.other_db).abs() < 0.1);
        assert!((20.0 * (turning.own / still.own).log10()).abs() < 0.1);
    }
}

/// In the default glass atrium the reflections and the reverb carry each
/// tone to the other ear too: it leaks a little more than in the open
/// meadow, where all of the leak is the direct sound round the head.
#[test]
fn a_reflective_room_leaks_more_than_the_open_meadow() {
    let meadow = ears(None, 0.0);
    let atrium = ears(Some("environments/atrium.yaml"), 0.0);
    report("glass atrium", &atrium);
    for (meadow, atrium) in [(&meadow.0, &atrium.0), (&meadow.1, &atrium.1)] {
        assert!(atrium.other_db > meadow.other_db);
    }
}

/// The render is the same file every time, so the numbers above are too.
#[test]
fn the_measurement_repeats() {
    let first = render(SCENE, &on_headphones(2.0));
    let second = render(SCENE, &on_headphones(2.0));
    assert!(super::identical(first.frames(), second.frames()));
}

/// The right ear's tone muted as a layer of the sound (`layers: { right:
/// { mute: true } }`): only 108 Hz is left, at both ears. Returns, for the
/// left ear then the right, the 118 Hz component in dB against the 108 Hz
/// one.
#[test]
fn muting_the_right_ear_leaves_only_108_hz_at_both_ears() {
    let mut config = SceneConfig::load(SCENE).unwrap_or_else(|error| panic!("{SCENE}: {error}"));
    let id = atrium_world::EntityId::new("alpha-binaural-beat").expect("its id");
    config
        .sounds
        .get_mut(&id)
        .expect("the beat")
        .sound
        .layers
        .insert(
            "right".to_string(),
            atrium_world::Layer {
                mute: true,
                ..atrium_world::Layer::default()
            },
        );
    let rendered = Rendering::prepare(config, &on_headphones(FROM_SECONDS + MEASURED_SECONDS))
        .expect("it renders")
        .run();
    let sample_rate = rendered.sample_rate() as f64;
    let first = (FROM_SECONDS * sample_rate) as usize;
    let at_each_ear: Vec<(f64, f64)> = (0..2)
        .map(|index| {
            let samples = &channel(rendered.frames(), index)[first..];
            let left_tone = amplitude_at(samples, sample_rate, LEFT_HZ);
            let right_tone = amplitude_at(samples, sample_rate, RIGHT_HZ);
            (left_tone, 20.0 * (right_tone / left_tone).log10())
        })
        .collect();
    println!(
        "right ear muted: 108 Hz at the left ear {:.2e}, at the right {:.2e}; 118 Hz against it {:.1} dB left, {:.1} dB right",
        at_each_ear[0].0, at_each_ear[1].0, at_each_ear[0].1, at_each_ear[1].1
    );
    for (left_tone, right_tone_db) in at_each_ear {
        assert!(left_tone > 1e-5, "108 Hz is heard");
        assert!(
            right_tone_db < -100.0,
            "118 Hz is left at {right_tone_db:.1} dB"
        );
    }
}
