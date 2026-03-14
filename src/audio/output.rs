// Audio output abstraction.
//
// Currently: cpal (https://github.com/RustAudio/cpal)
// Alternative backends for future:
//   - cubeb-rs (https://github.com/mozilla/cubeb-rs) — Mozilla's Firefox audio backend,
//     pure-Rust CoreAudio + PulseAudio implementations, potentially lower latency
//   - web-audio-api-rs (https://github.com/orottier/web-audio-api-rs) — uses cpal or cubeb internally
//
// See REFERENCES.md for full list.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};

use crate::engine::commands::Command;
use crate::engine::scene::AudioScene;
use crate::profile_span;

/// Abstraction over audio output backends.
/// Future: implement for cubeb-rs or other backends.
pub trait AudioOutput {
    fn start(
        self,
        scene: AudioScene,
        commands: rtrb::Consumer<Command>,
    ) -> Result<Box<dyn StreamHandle>, Box<dyn std::error::Error>>;
}

/// Handle that keeps the audio stream alive. Drop it to stop playback.
/// Not Send — cpal::Stream contains platform-specific non-Send types (e.g. CoreAudio pointers).
/// Keep this on the main thread.
pub trait StreamHandle {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u16;
    fn device_name(&self) -> &str;
}

/// Decide how many output channels to open.
///
/// If the speaker layout requests a different channel count than the device
/// default (and the layout count is non-zero), prefer the layout.
/// This allows forcing multichannel output (e.g. 6ch for 5.1) on capable devices.
///
/// Channel ordering assumption: all layouts use ITU channel order
/// (L, R, C, LFE, Ls, Rs for 5.1). This holds for all target output paths:
/// - NUC / ALSA: `snd_pcm_set_chmap()` defaults to ITU for multichannel
/// - AirPlay / Apple TV: RAOP negotiates channel layout metadata
/// - HDMI / AVR: EDID carries channel map; receivers expect ITU order
pub fn resolve_channels(layout_channels: u16, device_channels: u16) -> u16 {
    if layout_channels != device_channels && layout_channels > 0 {
        layout_channels
    } else {
        device_channels
    }
}

/// cpal-based audio output.
pub struct CpalOutput;

struct CpalStreamHandle {
    _stream: Stream,
    sample_rate: u32,
    channels: u16,
    device_name: String,
}

impl StreamHandle for CpalStreamHandle {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn device_name(&self) -> &str {
        &self.device_name
    }
}

impl AudioOutput for CpalOutput {
    fn start(
        self,
        mut scene: AudioScene,
        mut commands: rtrb::Consumer<Command>,
    ) -> Result<Box<dyn StreamHandle>, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("no output device found")?;

        let device_name = device.name().unwrap_or_default();
        println!("Audio device: {}", device_name);

        let supported = device.default_output_config()?;
        let sample_rate = supported.sample_rate().0;
        let device_channels = supported.channels();

        let layout_channels = scene.speaker_layout.total_channels() as u16;
        let mut channels = resolve_channels(layout_channels, device_channels);

        // Validate that the device actually supports the requested channel count.
        // Without this, cpal may error or silently downmix (platform-dependent).
        if channels != device_channels {
            let device_supports = device
                .supported_output_configs()?
                .any(|cfg| cfg.channels() >= channels);
            if device_supports {
                println!(
                    "Speaker layout requests {} channels (device default: {})",
                    channels, device_channels
                );
            } else {
                eprintln!(
                    "Device does not support {} channels — falling back to device default ({})",
                    channels, device_channels
                );
                channels = device_channels;
            }
        }

        println!(
            "Output config: {}Hz, {} channels, {:?}",
            sample_rate,
            channels,
            supported.sample_format()
        );

        scene.sample_rate = sample_rate as f32;
        scene.init_pipelines();

        let config = StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let stream = match supported.sample_format() {
            SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    #[cfg(target_arch = "x86_64")]
                    unsafe {
                        // Enable FTZ (bit 15) and DAZ (bit 6) in the MXCSR register to flush
                        // denormal floats to zero. Without this, reverb tail decay produces
                        // denormals that cause 10-100× slowdowns on x86 microcode assist paths.
                        // Reference: Intel® 64 and IA-32 Architectures SDM, Vol. 1, §10.2.3.
                        std::arch::x86_64::_mm_setcsr(std::arch::x86_64::_mm_getcsr() | 0x8040);
                    }
                    let _cb = profile_span!("callback", samples = data.len()).entered();
                    scene.process_commands(&mut commands);
                    scene.render(data, channels as usize);
                },
                |err| eprintln!("audio stream error: {err}"),
                None,
            )?,
            SampleFormat::I16 => {
                // Pre-allocate for typical buffer sizes to avoid allocation in audio callback
                let mut float_buf: Vec<f32> = vec![0.0; 2048];
                device.build_output_stream(
                    &config,
                    move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                        #[cfg(target_arch = "x86_64")]
                        unsafe {
                            // FTZ | DAZ — see F32 callback above for rationale.
                            std::arch::x86_64::_mm_setcsr(std::arch::x86_64::_mm_getcsr() | 0x8040);
                        }
                        let _cb = profile_span!("callback", samples = data.len()).entered();
                        let len = data.len();
                        // Grow once on first callback (or if buffer size changes)
                        if float_buf.len() < len {
                            float_buf.resize(len, 0.0);
                        }
                        float_buf[..len].fill(0.0);
                        scene.process_commands(&mut commands);
                        scene.render(&mut float_buf[..len], channels as usize);
                        for (out, &sample) in data.iter_mut().zip(float_buf.iter()) {
                            *out = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        }
                    },
                    |err| eprintln!("audio stream error: {err}"),
                    None,
                )?
            }
            format => return Err(format!("unsupported sample format: {format:?}").into()),
        };

        stream.play()?;

        Ok(Box::new(CpalStreamHandle {
            _stream: stream,
            sample_rate,
            channels,
            device_name,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atrium_core::speaker::SpeakerLayout;
    use atrium_core::types::Vec3;

    // ── resolve_channels logic ──────────────────────────────────────────

    #[test]
    fn resolve_channels_prefers_layout_over_device() {
        // 5.1 layout (6ch) on a stereo device → should request 6
        assert_eq!(resolve_channels(6, 2), 6);
    }

    #[test]
    fn resolve_channels_uses_device_when_layout_matches() {
        // layout and device agree → use device default
        assert_eq!(resolve_channels(2, 2), 2);
        assert_eq!(resolve_channels(6, 6), 6);
    }

    #[test]
    fn resolve_channels_uses_device_when_layout_is_zero() {
        // layout_channels=0 means no layout configured → fall back to device
        assert_eq!(resolve_channels(0, 2), 2);
        assert_eq!(resolve_channels(0, 8), 8);
    }

    #[test]
    fn resolve_channels_layout_can_be_fewer_than_device() {
        // stereo layout on a 7.1 device → should request 2
        assert_eq!(resolve_channels(2, 8), 2);
    }

    // ── SpeakerLayout channel counts ────────────────────────────────────

    #[test]
    fn surround_5_1_has_6_total_channels() {
        let layout = SpeakerLayout::surround_5_1(
            Vec3::new(0.0, 4.0, 0.0),
            Vec3::new(6.0, 4.0, 0.0),
            Vec3::new(3.0, 4.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(6.0, 0.0, 0.0),
        );
        assert_eq!(
            layout.total_channels(),
            6,
            "5.1 = 5 spatial + 1 LFE = 6 channels"
        );
        assert_eq!(layout.speaker_count(), 5, "5 spatial speakers (no LFE)");
        assert_eq!(layout.lfe_channel(), Some(3), "LFE is channel 3");
    }

    #[test]
    fn surround_5_1_speaker_channel_assignments() {
        let layout = SpeakerLayout::surround_5_1(
            Vec3::new(0.0, 4.0, 0.0), // FL
            Vec3::new(6.0, 4.0, 0.0), // FR
            Vec3::new(3.0, 4.0, 0.0), // C
            Vec3::new(0.0, 0.0, 0.0), // RL
            Vec3::new(6.0, 0.0, 0.0), // RR
        );
        // ITU 5.1 channel order: L=0, R=1, C=2, LFE=3, Ls=4, Rs=5
        let expected_channels = [0, 1, 2, 4, 5]; // spatial speakers (LFE=3 is separate)
        for (i, &expected_ch) in expected_channels.iter().enumerate() {
            let speaker = layout.speaker_by_index(i).expect("speaker should exist");
            assert_eq!(
                speaker.channel, expected_ch,
                "speaker {i} should be channel {expected_ch}"
            );
        }
    }

    #[test]
    fn stereo_layout_has_2_channels() {
        let layout = SpeakerLayout::stereo(Vec3::new(-1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0));
        assert_eq!(layout.total_channels(), 2);
        assert_eq!(layout.speaker_count(), 2);
        assert_eq!(layout.lfe_channel(), None);
    }

    #[test]
    fn quad_layout_is_masked_5_1() {
        let layout = SpeakerLayout::quad(
            Vec3::new(-1.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
        );
        // Quad is 5.1 with center + LFE masked
        assert_eq!(layout.total_channels(), 6);
        assert_eq!(layout.speaker_count(), 4);
        assert_eq!(layout.lfe_channel(), Some(3));
        // Active channels: FL(0), FR(1), RL(4), RR(5) — center(2) and LFE(3) masked
        assert!(layout.is_channel_active(0));
        assert!(layout.is_channel_active(1));
        assert!(!layout.is_channel_active(2));
        assert!(!layout.is_channel_active(3));
        assert!(layout.is_channel_active(4));
        assert!(layout.is_channel_active(5));
    }

    #[test]
    fn surround_5_1_resolves_to_6_channels_on_stereo_device() {
        let layout =
            SpeakerLayout::surround_5_1(Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
        let device_default = 2u16;
        let channels = resolve_channels(layout.total_channels() as u16, device_default);
        assert_eq!(
            channels, 6,
            "5.1 layout should override stereo device default"
        );
    }
}
