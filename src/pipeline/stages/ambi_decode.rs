//! Ambisonics decode MixStage.
//!
//! Reads B-format from channels 0–3 of the output buffer, decodes via
//! AllRAD (≥3 speakers) or bilateral (2ch), and overwrites the buffer
//! with decoded speaker signals.
//!
//! This stage is the second half of the split Ambisonics pipeline:
//!   renderer (encode-only) → AmbiMultiDelayStage → AmbisonicsDecodeStage

use atrium_core::ambisonics::{AllRadDecoder, BFormat, BilateralDecoder};

use crate::pipeline::mix_stage::{MixContext, MixStage};

/// Ambisonics B-format → speaker decode stage.
///
/// For ≥3 speakers: AllRAD decode (12 virtual speakers → VBAP re-pan).
/// For 2 speakers: bilateral binaural decode (per-ear rotation + cardioid weights).
/// For <4 channels in the buffer: no-op (renderer did inline decode).
pub struct AmbisonicsDecodeStage {
    /// Cached speaker count for mode detection.
    speaker_count: usize,
}

impl Default for AmbisonicsDecodeStage {
    fn default() -> Self {
        Self::new()
    }
}

impl AmbisonicsDecodeStage {
    pub fn new() -> Self {
        Self { speaker_count: 0 }
    }
}

impl MixStage for AmbisonicsDecodeStage {
    fn init(&mut self, ctx: &MixContext) {
        self.speaker_count = ctx.layout.speaker_count();
    }

    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        // Only decode if the renderer wrote B-format (≥4 render channels).
        // For stereo layouts, the renderer does inline bilateral decode.
        // Use render_channels (layout-based), not channels (hardware output).
        if ctx.render_channels < 4 {
            return;
        }

        // Rebuild decoder per-buffer (listener-relative speaker angles).
        let num_frames = buffer.len() / ctx.channels;
        let speaker_count = ctx.layout.speaker_count();
        let render_channels = ctx.render_channels;

        if speaker_count >= 3 {
            // Always use EPAD (energy-preserving AllRAD) — equalizes energy
            // across all source directions with no audible downside.
            let decoder = AllRadDecoder::from_listener_epad(
                ctx.layout.speakers(),
                speaker_count,
                ctx.listener,
            );

            for frame in 0..num_frames {
                let base = frame * ctx.channels;
                let bformat = BFormat {
                    w: buffer[base],
                    y: buffer[base + 1],
                    z: buffer[base + 2],
                    x: buffer[base + 3],
                };

                let mut decoded = decoder.decode(&bformat);
                ctx.layout.apply_mask(&mut decoded);

                // Overwrite only the render channels, not all hardware channels.
                buffer[base..base + render_channels]
                    .copy_from_slice(&decoded.gains[..render_channels]);
            }
        } else {
            // 2-channel bilateral binaural decode.
            let bilateral = BilateralDecoder::new();

            for frame in 0..num_frames {
                let base = frame * ctx.channels;
                let bformat = BFormat {
                    w: buffer[base],
                    y: buffer[base + 1],
                    z: buffer[base + 2],
                    x: buffer[base + 3],
                };

                let (l, r) = bilateral.decode_stereo(&bformat);

                // Write stereo to channels 0-1, zero others.
                buffer[base] = l;
                buffer[base + 1] = r;
                for ch in 2..ctx.channels {
                    buffer[base + ch] = 0.0;
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
    use crate::pipeline::path::WallMaterial;
    use atrium_core::ambisonics::foa_encode;
    use atrium_core::listener::Listener;
    use atrium_core::speaker::{Speaker, SpeakerLayout};
    use atrium_core::types::Vec3;

    const TEST_MATERIALS: [WallMaterial; 6] = [WallMaterial::HARD_WALL; 6];

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
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 4,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            master_gain: 1.0,
            render_channels: 4,
            reverb_input: None,
            wall_reflectivity: 0.9,
            wall_materials: &TEST_MATERIALS,
            atmosphere: &AtmosphericParams::default(),
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
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 4,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            master_gain: 1.0,
            render_channels: 4,
            reverb_input: None,
            wall_reflectivity: 0.9,
            wall_materials: &TEST_MATERIALS,
            atmosphere: &AtmosphericParams::default(),
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
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 2,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            master_gain: 1.0,
            render_channels: 2,
            reverb_input: None,
            wall_reflectivity: 0.9,
            wall_materials: &TEST_MATERIALS,
            atmosphere: &AtmosphericParams::default(),
            measurement_mode: false,
        };
        stage.init(&ctx);

        let mut buffer = vec![0.5f32; 2 * 32];
        let original = buffer.clone();
        stage.process(&mut buffer, &ctx);

        assert_eq!(buffer, original, "2-channel buffer should be unchanged");
    }
}
