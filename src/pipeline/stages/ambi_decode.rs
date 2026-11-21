//! Ambisonics decode MixStage.
//!
//! Reads B-format from channels 0–3 of the output buffer, decodes it with
//! the decoder `ambisonic_bus` names, and overwrites the buffer with decoded
//! speaker signals.
//!
//! This stage is the second half of the split Ambisonics pipeline:
//!   renderer (encode-only) → AmbiMultiDelayStage → AmbisonicsDecodeStage
//!
//! On an inline-bilateral bus the renderer has already decoded and this stage
//! is a no-op.

use atrium_core::ambisonics::{AllRadDecoder, BFormat, BilateralDecoder};
use atrium_core::speaker::MAX_CHANNELS;

use crate::pipeline::mix_stage::{MixContext, MixStage};
use crate::pipeline::renderers::ambisonics::{ambisonic_bus, AmbisonicBus, BusDecoder};

/// Ambisonics B-format → speaker decode stage.
///
/// ≥ 3 reachable speakers: AllRAD (12 virtual speakers → VBAP re-pan) with
/// EPAD, designed over the active, non-LFE speakers only — masked speakers
/// are never decode targets, so their share of the sound field is
/// redistributed instead of discarded. ≤ 2: bilateral cardioid pair.
#[derive(Default)]
pub struct AmbisonicsDecodeStage;

impl AmbisonicsDecodeStage {
    pub fn new() -> Self {
        Self
    }
}

impl MixStage for AmbisonicsDecodeStage {
    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        let bus_decoder = match ambisonic_bus(ctx.layout, ctx.render_channels, ctx.channels) {
            AmbisonicBus::BFormat(bus_decoder) => bus_decoder,
            AmbisonicBus::InlineBilateral(_) => return,
        };
        let num_frames = buffer.len() / ctx.channels;
        // A B-format bus guarantees render_channels ≥ 4, so W/Y/Z/X are all
        // overwritten; channels that are not decode targets become silent.
        let render_channels = ctx.render_channels.min(MAX_CHANNELS);

        let read_bformat = |frame: &[f32]| BFormat {
            w: frame[0],
            y: frame[1],
            z: frame[2],
            x: frame[3],
        };

        match bus_decoder {
            BusDecoder::AllRad(decode_speakers) => {
                // Rebuild decoder per-buffer (listener-relative speaker
                // angles). Always EPAD (energy-preserving AllRAD) — equalizes
                // energy across all source directions with no audible downside.
                let speakers = decode_speakers.as_slice();
                let decoder =
                    AllRadDecoder::from_listener_epad(speakers, speakers.len(), ctx.listener);
                for frame in 0..num_frames {
                    let base = frame * ctx.channels;
                    let decoded = decoder.decode(&read_bformat(&buffer[base..base + 4]));
                    // Overwrite only the render channels, not all hardware channels.
                    buffer[base..base + render_channels]
                        .copy_from_slice(&decoded.gains[..render_channels]);
                }
            }
            BusDecoder::Bilateral(ears) => {
                let bilateral = BilateralDecoder::new();
                for frame in 0..num_frames {
                    let base = frame * ctx.channels;
                    let (left, right) =
                        bilateral.decode_stereo(&read_bformat(&buffer[base..base + 4]));
                    buffer[base..base + render_channels].fill(0.0);
                    // Added, not assigned: both ears may share one channel.
                    buffer[base + ears.left] += left;
                    buffer[base + ears.right] += right;
                }
            }
        }
    }

    fn reset(&mut self) {}

    fn name(&self) -> &str {
        "ambi_decode"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::atmosphere::AtmosphericParams;
    use crate::audio::propagation::GroundProperties;
    use crate::pipeline::environment::Environment;
    use crate::pipeline::wall_material::WallMaterial;
    use atrium_core::ambisonics::foa_encode;
    use atrium_core::listener::Listener;
    use atrium_core::speaker::{Speaker, SpeakerLayout};
    use atrium_core::types::Vec3;

    fn test_environment() -> Environment {
        Environment {
            min: Vec3::new(-5.0, -5.0, -5.0),
            max: Vec3::new(5.0, 5.0, 5.0),
            wall_materials: [WallMaterial::HARD_WALL; 6],
            ground: GroundProperties::hard(),
            atmosphere: AtmosphericParams::default(),
            air: crate::audio::turbulence::AirMotion::STILL,
        }
    }

    #[test]
    fn decode_silent_input_silent_output() {
        let layout = SpeakerLayout::new(
            &[
                Speaker {
                    position: Vec3::new(-1.0, 1.0, 0.0),
                    channel: 0,
                },
                Speaker {
                    position: Vec3::new(1.0, 1.0, 0.0),
                    channel: 1,
                },
                Speaker {
                    position: Vec3::new(-1.0, -1.0, 0.0),
                    channel: 2,
                },
                Speaker {
                    position: Vec3::new(1.0, -1.0, 0.0),
                    channel: 3,
                },
            ],
            None,
            4,
        );
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let mut stage = AmbisonicsDecodeStage::new();
        let environment = test_environment();
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 4,
            environment: &environment,
            master_gain: 1.0,
            render_channels: 4,
            reverb_input: None,
            measurement_mode: false,
        };
        stage.init(&ctx);

        let mut buffer = vec![0.0f32; 4 * 64];
        stage.process(&mut buffer, &ctx);

        let max = buffer.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(max < 1e-10, "silent B-format should decode to silence");
    }

    #[test]
    fn decode_produces_nonzero_output() {
        let layout = SpeakerLayout::new(
            &[
                Speaker {
                    position: Vec3::new(-1.0, 1.0, 0.0),
                    channel: 0,
                },
                Speaker {
                    position: Vec3::new(1.0, 1.0, 0.0),
                    channel: 1,
                },
                Speaker {
                    position: Vec3::new(-1.0, -1.0, 0.0),
                    channel: 2,
                },
                Speaker {
                    position: Vec3::new(1.0, -1.0, 0.0),
                    channel: 3,
                },
            ],
            None,
            4,
        );
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let mut stage = AmbisonicsDecodeStage::new();
        let environment = test_environment();
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 4,
            environment: &environment,
            master_gain: 1.0,
            render_channels: 4,
            reverb_input: None,
            measurement_mode: false,
        };
        stage.init(&ctx);

        // Encode a source directly ahead (azimuth=0, elevation=0).
        let bformat = foa_encode(0.0, 0.0, 1.0);
        let mut buffer = vec![0.0f32; 4];
        buffer[0] = bformat.w;
        buffer[1] = bformat.y;
        buffer[2] = bformat.z;
        buffer[3] = bformat.x;

        stage.process(&mut buffer, &ctx);

        // Decode should produce nonzero output in at least some channels.
        let energy: f32 = buffer.iter().map(|s| s * s).sum();
        assert!(
            energy > 1e-6,
            "decoded output should have energy, got {energy}"
        );
    }

    #[test]
    fn noop_for_stereo() {
        let layout = SpeakerLayout::stereo(Vec3::new(-1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0));
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let mut stage = AmbisonicsDecodeStage::new();
        let environment = test_environment();
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 2,
            environment: &environment,
            master_gain: 1.0,
            render_channels: 2,
            reverb_input: None,
            measurement_mode: false,
        };
        stage.init(&ctx);

        let mut buffer = vec![0.5f32; 2 * 32];
        let original = buffer.clone();
        stage.process(&mut buffer, &ctx);

        assert_eq!(buffer, original, "2-channel buffer should be unchanged");
    }

    // ── Audit 2026-11: decoded values and invariants ──────────────────────

    fn mix_context<'a>(
        layout: &'a SpeakerLayout,
        listener: &'a Listener,
        environment: &'a Environment,
        channels: usize,
    ) -> MixContext<'a> {
        MixContext {
            listener,
            layout,
            sample_rate: 48000.0,
            channels,
            environment,
            master_gain: 1.0,
            render_channels: channels,
            reverb_input: None,
            measurement_mode: false,
        }
    }

    /// Decode one B-format frame through the stage and return the output frame.
    fn decode_one_frame(
        layout: &SpeakerLayout,
        listener: &Listener,
        azimuth: f32,
        channels: usize,
    ) -> Vec<f32> {
        let environment = test_environment();
        let ctx = mix_context(layout, listener, &environment, channels);
        let mut stage = AmbisonicsDecodeStage::new();
        stage.init(&ctx);
        let bformat = foa_encode(azimuth, 0.0, 1.0);
        let mut buffer = vec![0.0f32; channels];
        buffer[..4].copy_from_slice(&[bformat.w, bformat.y, bformat.z, bformat.x]);
        stage.process(&mut buffer, &ctx);
        buffer
    }

    /// Regular quad (FL 45°, FR −45°, RL 135°, RR −135°) around a listener at
    /// the origin facing +X, on masked-5.1 channels 0, 1, 4, 5.
    fn regular_quad() -> SpeakerLayout {
        SpeakerLayout::quad(
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(-1.0, 1.0, 0.0),
            Vec3::new(-1.0, -1.0, 0.0),
        )
    }

    #[test]
    fn epad_regular_quad_is_scaled_mode_matching() {
        // On a regular quad, AllRAD(12) has singular values (0.91068, 0.70711,
        // 0.70711) (numpy reference); EPAD replaces them by their mean
        // σ̄ = 0.774966, so D_EPAD = (σ̄/σ_mm)·D_mm with σ_mm = 1/√2 — the
        // mode-matching decoder scaled by 1.095972. Mode-matching gives
        //   front: FL = FR = ½, RL = RR = 0
        //   45°:   FL = ¼(1+√2), FR = RL = ¼, RR = ¼(1−√2)
        // and the energy is σ̄² = 0.600573 for every azimuth.
        let layout = regular_quad();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let scale = 1.095972;
        let front = decode_one_frame(&layout, &listener, 0.0, 6);
        let expected_front = [0.5 * scale, 0.5 * scale, 0.0, 0.0, 0.0, 0.0];
        let at_speaker = decode_one_frame(&layout, &listener, 45.0_f32.to_radians(), 6);
        let quarter = 0.25 * scale;
        let root_two = std::f32::consts::SQRT_2;
        let expected_at_speaker = [
            quarter * (1.0 + root_two),
            quarter,
            0.0,
            0.0,
            quarter,
            quarter * (1.0 - root_two),
        ];
        for channel in 0..6 {
            assert!(
                (front[channel] - expected_front[channel]).abs() < 1e-4,
                "front ch{channel}: {} vs {}",
                front[channel],
                expected_front[channel]
            );
            assert!(
                (at_speaker[channel] - expected_at_speaker[channel]).abs() < 1e-4,
                "45° ch{channel}: {} vs {}",
                at_speaker[channel],
                expected_at_speaker[channel]
            );
        }
    }

    #[test]
    fn epad_regular_quad_energy_flat_mirror_symmetric_and_peaks_at_speaker() {
        let layout = regular_quad();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        for degrees in (0..360).step_by(5) {
            let azimuth = (degrees as f32).to_radians();
            let frame = decode_one_frame(&layout, &listener, azimuth, 6);
            let energy: f32 = frame.iter().map(|gain| gain * gain).sum();
            assert!(
                (energy - 0.600573).abs() < 2e-4,
                "EPAD quad energy at {degrees}°: {energy}"
            );
            assert_eq!(frame[2], 0.0, "masked centre must stay silent");
            assert_eq!(frame[3], 0.0, "LFE is not a decode target");
            let mirror = decode_one_frame(&layout, &listener, -azimuth, 6);
            assert!(
                (frame[0] - mirror[1]).abs() < 1e-4,
                "FL↔FR mirror at {degrees}°"
            );
            assert!(
                (frame[4] - mirror[5]).abs() < 1e-4,
                "RL↔RR mirror at {degrees}°"
            );
        }
        for (degrees, channel) in [(45.0_f32, 0), (-45.0, 1), (135.0, 4), (-135.0, 5)] {
            let frame = decode_one_frame(&layout, &listener, degrees.to_radians(), 6);
            let loudest = (0..6)
                .max_by(|&a, &b| frame[a].partial_cmp(&frame[b]).unwrap())
                .unwrap();
            assert_eq!(
                loudest, channel,
                "source at {degrees}° should peak at ch{channel}"
            );
        }
    }

    #[test]
    fn stereo_channel_mode_on_5_1_keeps_rear_sources_audible() {
        // SetChannelMode(Stereo) on 5.1 hardware only masks channels 2–5.
        // Regression: the AllRAD/EPAD decoder was built over all five
        // speakers and masked afterwards, so a rear source was decoded almost
        // entirely into the masked Ls/Rs. ITU 5.0 (numpy reference): front
        // source L²+R² = 0.405, rear source 0.0031 (−21 dB, and in antiphase).
        // With two reachable speakers `ambisonic_bus` now names the bilateral
        // decoder. Front and rear both have Y = 0, so each ear gets
        // W·√2·½ = ½ and L² + R² = ½ for both: 0 dB, in phase.
        let place = |degrees: f32, channel: usize| {
            let azimuth = degrees.to_radians();
            Speaker {
                position: Vec3::new(2.0 * azimuth.cos(), 2.0 * azimuth.sin(), 0.0),
                channel,
            }
        };
        let mut layout = SpeakerLayout::new(
            &[
                place(30.0, 0),
                place(-30.0, 1),
                place(0.0, 2),
                place(110.0, 4),
                place(-110.0, 5),
            ],
            Some(3),
            6,
        );
        layout.set_active_channels(&[0, 1]);
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let stereo_energy = |azimuth: f32| {
            let frame = decode_one_frame(&layout, &listener, azimuth, 6);
            frame[0] * frame[0] + frame[1] * frame[1]
        };
        let front = stereo_energy(0.0);
        let rear = stereo_energy(std::f32::consts::PI);
        let rear_relative_db = 10.0 * (rear / front).log10();
        assert!(
            rear_relative_db > -6.0,
            "rear source is {rear_relative_db:.1} dB below a front source in stereo mode"
        );
    }
}
