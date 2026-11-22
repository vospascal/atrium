//! The near field through the whole headphone render: the level difference
//! the sphere says, where it says it, and nothing at all from 1.2 m on.

use super::{energy, Rig, ToneSource};
use crate::audio::near_field::reference::distance_variation_db;
use atrium_core::probe::Probe;
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

const NEAR_FIELD_OFF: Probe = Probe {
    near_field: false,
    ..Probe::WHOLE
};

/// One second of a 200 Hz tone at `position` through the HRTF pipeline.
/// The rig's listener is at the origin facing +y, so +x is to the right.
fn render(position: Vec3, probe: Probe) -> Vec<f32> {
    let rig = Rig::free_field(48_000.0);
    let mut pipeline = rig.build(RenderMode::Hrtf);
    pipeline.probe = probe;
    let mut sources = vec![ToneSource::boxed(position, 200.0, usize::MAX)];
    rig.render(&mut pipeline, &mut sources, 48_000)
}

/// How much louder the right ear is than the left (dB), over the second
/// half of the render.
fn right_over_left_db(output: &[f32]) -> f64 {
    let channels = Rig::free_field(48_000.0).channels;
    let left = energy(output, channels, &[0], 24_000);
    let right = energy(output, channels, &[1], 24_000);
    10.0 * (right / left).log10()
}

/// A 200 Hz tone 20 cm to the right of the head's centre is louder in the
/// right ear than the left by what the measured HRIRs give plus what the
/// sphere adds at 20 cm over 1.4 m: 10.1 dB more, at 10° from the right ear
/// and 170° from the left. At 5 m the difference is the measured one only,
/// the same with the near field on or off, sample for sample.
#[test]
fn a_tone_beside_the_ear_has_the_spheres_level_difference() {
    let near = right_over_left_db(&render(Vec3::new(0.2, 0.0, 0.0), Probe::WHOLE));
    let far_output = render(Vec3::new(5.0, 0.0, 0.0), Probe::WHOLE);
    assert_eq!(
        far_output,
        render(Vec3::new(5.0, 0.0, 0.0), NEAR_FIELD_OFF),
        "at 5 m the near field changed the sound"
    );
    let far = right_over_left_db(&far_output);
    let near_without = right_over_left_db(&render(Vec3::new(0.2, 0.0, 0.0), NEAR_FIELD_OFF));
    assert!(
        (near_without - far).abs() < 0.05,
        "without the near field 20 cm and 5 m differ by {:.2} dB",
        near_without - far
    );
    let sphere = distance_variation_db(0.2, 1.4, 10.0, 200.0)
        - distance_variation_db(0.2, 1.4, 170.0, 200.0);
    let added = near - far;
    eprintln!(
        "near field, 200 Hz at 20 cm: {near:.2} dB right over left, {far:.2} dB at 5 m; \
         added {added:.2} dB, the sphere {sphere:.2} dB"
    );
    assert!(
        (added - sphere).abs() <= 1.5,
        "20 cm adds {added:.2} dB to the 5 m difference of {far:.2} dB; the sphere says {sphere:.2}"
    );
}

/// From 1.2 m on, the near field is not in the signal: the render is the
/// same, bit for bit, whether it is on or off. Off, it is never in it.
#[test]
fn from_the_switch_out_distance_the_render_is_untouched() {
    for position in [
        Vec3::new(1.2, 0.0, 0.0),
        Vec3::new(-0.9, 0.9, 0.2),
        Vec3::new(0.0, -3.0, 0.0),
        Vec3::new(4.0, 4.0, 1.0),
    ] {
        assert_eq!(
            render(position, Probe::WHOLE),
            render(position, NEAR_FIELD_OFF),
            "at {position:?}"
        );
    }
    assert_ne!(
        render(Vec3::new(0.3, 0.0, 0.0), Probe::WHOLE),
        render(Vec3::new(0.3, 0.0, 0.0), NEAR_FIELD_OFF),
        "at 30 cm the switch does nothing"
    );
}

/// A sound whose samples come from `signal` and whose place is `path` of
/// the time since it started (s), the listener at the origin facing +y.
struct Moving {
    signal: Box<dyn FnMut() -> f32 + Send>,
    path: fn(f32) -> Vec3,
    samples: usize,
}

impl atrium_core::source::SoundSource for Moving {
    fn next_sample(&mut self, _sample_rate: f32) -> f32 {
        self.samples += 1;
        (self.signal)()
    }
    fn position(&self) -> Vec3 {
        (self.path)(self.samples as f32 / 48_000.0)
    }
    fn tick(&mut self, _dt: f32) {}
}

/// Listening samples, for judging the near field by ear: nothing happens
/// unless `NEAR_FIELD_OUT` names a directory. Each is rendered through the
/// whole headphone pipeline twice, into `on/` and `off/` under the same
/// name, and both are scaled by the same factor so the near field's level
/// is heard as it is. Distances are from the head's centre.
///
/// - `1-wisp-circling-25cm`: the wisp's whirr circling the head at ear
///   height, 25 cm out, once every 4 s, for 12 s.
/// - `2-wisp-circling-1m`: the same at 1 m.
/// - `3-robin-passing-left`: a robin's phrase from 2 m to the left, in to
///   15 cm beside the left ear, and away to 2 m behind, over 8 s.
/// - `4-clicks-20cm-right`: four clicks a second, held 20 cm to the right.
#[test]
fn write_listening_samples() {
    let Ok(directory) = std::env::var("NEAR_FIELD_OUT") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    for side in ["on", "off"] {
        std::fs::create_dir_all(directory.join(side)).expect("directory");
    }

    let robin = crate::audio::decode::decode_file(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/birds/roodborst/roodborst-zang_1.mp3"
    )))
    .expect("robin");
    // To 48 kHz by linear interpolation, then repeated to fill the pass.
    let step = robin.sample_rate as f64 / 48_000.0;
    let resampled: Vec<f32> = (0..((robin.samples.len() - 1) as f64 / step) as usize)
        .map(|index| {
            let position = index as f64 * step;
            let (whole, part) = (position as usize, (position.fract()) as f32);
            robin.samples[whole] * (1.0 - part) + robin.samples[whole + 1] * part
        })
        .collect();
    let robin_peak = resampled.iter().fold(0f32, |peak, s| peak.max(s.abs()));

    type Sample = (
        &'static str,
        f32,
        fn() -> Box<dyn FnMut() -> f32 + Send>,
        fn(f32) -> Vec3,
    );
    let wisp = || -> Box<dyn FnMut() -> f32 + Send> {
        let mut wisp = crate::synth::wisp::WispSource::new(Vec3::ZERO, 11);
        Box::new(move || {
            use atrium_core::source::SoundSource;
            wisp.next_sample(48_000.0)
        })
    };
    let clicks = || -> Box<dyn FnMut() -> f32 + Send> {
        let mut index = 0usize;
        Box::new(move || {
            index += 1;
            if index % 12_000 == 6_000 {
                0.5
            } else {
                0.0
            }
        })
    };
    fn circle(radius: f32, seconds: f32) -> Vec3 {
        let angle = std::f32::consts::TAU * seconds / 4.0;
        Vec3::new(radius * angle.cos(), radius * angle.sin(), 0.0)
    }
    let samples: [Sample; 4] = [
        ("1-wisp-circling-25cm", 12.0, wisp, |seconds| {
            circle(0.25, seconds)
        }),
        ("2-wisp-circling-1m", 12.0, wisp, |seconds| {
            circle(1.0, seconds)
        }),
        (
            "3-robin-passing-left",
            8.0,
            || Box::new(|| 0.0),
            |seconds| {
                // 2 m left → 15 cm left → 2 m behind, at one speed.
                let (start, beside, end) = (
                    Vec3::new(-2.0, 0.0, 0.0),
                    Vec3::new(-0.15, 0.0, 0.0),
                    Vec3::new(0.0, -2.0, 0.0),
                );
                let (first, second) = (start.distance_to(beside), beside.distance_to(end));
                let travelled = (first + second) * (seconds / 8.0).min(1.0);
                if travelled < first {
                    start + (beside - start) * (travelled / first)
                } else {
                    beside + (end - beside) * ((travelled - first) / second)
                }
            },
        ),
        ("4-clicks-20cm-right", 4.0, clicks, |_| {
            Vec3::new(0.2, 0.0, 0.0)
        }),
    ];

    for (name, seconds, signal, path) in samples {
        let frames = (seconds * 48_000.0) as usize;
        let mut rendered = Vec::new();
        for probe in [Probe::WHOLE, NEAR_FIELD_OFF] {
            let rig = Rig::free_field(48_000.0);
            let mut pipeline = rig.build(RenderMode::Hrtf);
            pipeline.probe = probe;
            let signal: Box<dyn FnMut() -> f32 + Send> = if name.starts_with("3-") {
                let robin = resampled.clone();
                let mut index = 0usize;
                Box::new(move || {
                    index += 1;
                    robin[index % robin.len()] * 0.5 / robin_peak
                })
            } else {
                signal()
            };
            let mut sources: Vec<Box<dyn atrium_core::source::SoundSource>> =
                vec![Box::new(Moving {
                    signal,
                    path,
                    samples: 0,
                })];
            let output = rig.render(&mut pipeline, &mut sources, frames);
            let left = super::channel_of(&output, rig.channels, 0);
            let right = super::channel_of(&output, rig.channels, 1);
            rendered.push((left, right));
        }
        let peak = rendered
            .iter()
            .flat_map(|(left, right)| left.iter().chain(right))
            .fold(0f32, |peak, sample| peak.max(sample.abs()))
            .max(1e-9);
        for ((left, right), side) in rendered.iter().zip(["on", "off"]) {
            let interleaved: Vec<f32> = left
                .iter()
                .zip(right)
                .flat_map(|(left, right)| [left * 0.8 / peak, right * 0.8 / peak])
                .collect();
            crate::audio::wav::write_wav(
                &directory.join(side).join(format!("{name}.wav")),
                &interleaved,
                2,
                48_000,
                crate::audio::wav::SampleFormat::Pcm16,
            )
            .expect("write");
        }
    }
}
