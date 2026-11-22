//! The files are right: the engine's own decoder reads the frames back with
//! their channels and rate, and the sidecar says what the frames are.

use super::{on_headphones, render, storm_on_speakers, MEADOW, STORM};
use crate::audio::decode::decode_channels;
use crate::audio::wav::SampleFormat;
use crate::offline::{write_files, Provenance};

/// A 16-bit sample is written as `x * 32767` truncated and read back as
/// `n / 32768`, as the engine's own 16-bit output and decoder do: within two
/// steps of 1/32768 of the rendered value (one for truncating, up to one for
/// the scales), and exactly the written step.
const SIXTEEN_BIT_STEP: f32 = 1.0 / 32_768.0;

#[test]
fn the_engine_reads_back_what_was_written_and_the_sidecar_describes_it() {
    let directory = std::env::temp_dir().join(format!("atrium-render-{}", std::process::id()));
    for (scene, settings, expected_channels) in [
        (MEADOW, on_headphones(2.0), ["L", "R"].as_slice()),
        (
            STORM,
            storm_on_speakers(2.0),
            ["L", "R", "C", "LFE", "Ls", "Rs"].as_slice(),
        ),
    ] {
        let rendered = render(scene, &settings);
        let frames = rendered.frames();
        assert_eq!(rendered.channel_names(), expected_channels);
        let provenance = Provenance::of(scene).expect("the scene file is there");
        for format in [SampleFormat::Float32, SampleFormat::Pcm16] {
            let name = format!("{}-{format:?}.wav", rendered.layout());
            let written = write_files(&rendered, &provenance, &directory.join(name), format)
                .expect("written");

            let decoded = decode_channels(&written.wav).expect("decodes");
            assert_eq!(decoded.channels, expected_channels.len(), "{scene}");
            assert_eq!(decoded.sample_rate, 48_000);
            assert_eq!(
                decoded.interleaved.len(),
                frames.len(),
                "{scene} {format:?}"
            );
            match format {
                SampleFormat::Float32 => assert!(
                    decoded
                        .interleaved
                        .iter()
                        .zip(frames)
                        .all(|(read, written)| read.to_bits() == written.to_bits()),
                    "{scene}: float frames do not come back exactly"
                ),
                SampleFormat::Pcm16 => {
                    for (read, written) in decoded.interleaved.iter().zip(frames) {
                        let step = (written.clamp(-1.0, 1.0) * 32_767.0) as i16;
                        assert_eq!(*read, step as f32 / 32_768.0, "{scene}");
                        assert!((read - written).abs() <= 2.0 * SIXTEEN_BIT_STEP);
                    }
                    assert_eq!(written.clipped, 0, "{scene}: nothing near full scale");
                }
            }

            let sidecar: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(&written.sidecar).expect("the sidecar is there"),
            )
            .expect("the sidecar parses");
            assert_eq!(sidecar["scene"]["path"], scene);
            assert_eq!(sidecar["seed"], rendered.seed());
            assert_eq!(sidecar["sample_rate"], 48_000);
            assert_eq!(sidecar["frames"], frames.len() / expected_channels.len());
            assert_eq!(
                sidecar["mode"],
                rendered.render_mode().as_str(),
                "{scene}: the mode"
            );
            assert_eq!(sidecar["channels"], serde_json::json!(expected_channels));
            let levels = sidecar["levels"].as_array().expect("levels");
            assert_eq!(levels.len(), expected_channels.len());
            for (channel, level) in levels.iter().enumerate() {
                let samples: Vec<f32> = frames
                    .iter()
                    .skip(channel)
                    .step_by(expected_channels.len())
                    .copied()
                    .collect();
                let peak = samples.iter().fold(0f32, |peak, s| peak.max(s.abs()));
                let rms = (samples.iter().map(|s| (*s as f64).powi(2)).sum::<f64>()
                    / samples.len() as f64)
                    .sqrt();
                assert_eq!(level["channel"], expected_channels[channel]);
                assert_eq!(level["peak"].as_f64().expect("a peak") as f32, peak);
                let read_rms = level["rms"].as_f64().expect("an rms");
                assert!((read_rms - rms).abs() <= 1e-12 * rms.max(1e-30), "{scene}");
            }
        }
    }
    std::fs::remove_dir_all(&directory).ok();
}
