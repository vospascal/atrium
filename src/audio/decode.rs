use std::fs::File;
use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::audio::spectral_profile::{self, SpectralProfile};

/// Pre-decoded audio samples. Immutable after creation — safe to share via Arc.
pub struct AudioBuffer {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    /// Root-mean-square level of the decoded samples.
    pub rms: f32,
    /// Long-term spectral profile across 24 Bark bands (computed at decode time).
    pub spectral_profile: SpectralProfile,
}

/// A file's samples as they are in it: every channel kept, interleaved.
pub struct DecodedChannels {
    pub interleaved: Vec<f32>,
    pub channels: usize,
    pub sample_rate: u32,
}

/// Decode an audio file (MP3, WAV, FLAC, etc.) into a mono f32 AudioBuffer.
pub fn decode_file(path: &Path) -> Result<AudioBuffer, Box<dyn std::error::Error>> {
    let DecodedChannels {
        interleaved,
        channels,
        sample_rate,
    } = decode_channels(path)?;

    // Downmix to mono if needed
    let all_samples: Vec<f32> = if channels > 1 {
        interleaved
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    } else {
        interleaved
    };

    let rms = if all_samples.is_empty() {
        0.0
    } else {
        (all_samples.iter().map(|s| s * s).sum::<f32>() / all_samples.len() as f32).sqrt()
    };

    let spectral_profile = spectral_profile::compute_profile(&all_samples, sample_rate);

    println!(
        "Decoded {}: {} samples, {}Hz, {}ch → mono (RMS: {:.4})",
        path.display(),
        all_samples.len(),
        sample_rate,
        channels,
        rms,
    );

    Ok(AudioBuffer {
        samples: all_samples,
        sample_rate,
        rms,
        spectral_profile,
    })
}

/// Decode an audio file with every channel it has, as it is: a WAV of
/// floats comes back sample for sample.
pub fn decode_channels(path: &Path) -> Result<DecodedChannels, Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;

    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or("no audio track found")?;

    let track_id = track.id;
    let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(1);
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);

    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut all_samples: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => {
                eprintln!("audio decode error: {e}");
                break;
            }
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("audio packet decode error: {e}");
                continue;
            }
        };

        let spec = *decoded.spec();
        let num_frames = decoded.capacity();

        let mut sample_buf = SampleBuffer::<f32>::new(num_frames as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);

        all_samples.extend_from_slice(sample_buf.samples());
    }

    Ok(DecodedChannels {
        interleaved: all_samples,
        channels,
        sample_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a minimal PCM16 WAV file and return its path.
    fn write_pcm16_wav(
        name: &str,
        channels: u16,
        sample_rate: u32,
        interleaved: &[i16],
    ) -> std::path::PathBuf {
        let data_bytes = (interleaved.len() * 2) as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * channels as u32 * 2).to_le_bytes());
        bytes.extend_from_slice(&(channels * 2).to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_bytes.to_le_bytes());
        for sample in interleaved {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let path = std::env::temp_dir().join(format!(
            "atrium_decode_test_{}_{name}.wav",
            std::process::id()
        ));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// Stereo is downmixed to mono by averaging channels (L+R)/2 — equal-weight,
    /// i.e. −6 dB per channel, not −3 dB — and the file's sample rate is kept
    /// (no resampling happens in the decoder).
    #[test]
    fn stereo_is_averaged_to_mono_and_sample_rate_is_preserved() {
        let frames = 1000;
        let mut interleaved = Vec::new();
        for _ in 0..frames {
            interleaved.push(16384i16); // L = +0.5
            interleaved.push(-8192i16); // R = −0.25
        }
        let path = write_pcm16_wav("stereo", 2, 44_100, &interleaved);
        let buffer = decode_file(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(buffer.sample_rate, 44_100);
        assert_eq!(buffer.samples.len(), frames);
        for &sample in &buffer.samples {
            assert!((sample - 0.125).abs() < 1e-6, "mono sample {sample}");
        }
        assert!((buffer.rms - 0.125).abs() < 1e-6);
    }

    /// Mono files pass through untouched: PCM16 → f32 is x / 32768.
    #[test]
    fn mono_passes_through_with_pcm16_scaling() {
        let interleaved: Vec<i16> = vec![0, 16384, -16384, 32767, -32768];
        let path = write_pcm16_wav("mono", 1, 48_000, &interleaved);
        let buffer = decode_file(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(buffer.sample_rate, 48_000);
        let expected = [0.0f32, 0.5, -0.5, 32767.0 / 32768.0, -1.0];
        assert_eq!(buffer.samples, expected);
        // RMS = sqrt(mean(x²)).
        let mean_square: f32 = expected.iter().map(|x| x * x).sum::<f32>() / 5.0;
        assert!((buffer.rms - mean_square.sqrt()).abs() < 1e-6);
    }

    /// A full-scale ±0.5 square wave has RMS exactly 0.5 and the spectral profile
    /// is attached (24 bands, not the flat default) after decoding.
    #[test]
    fn rms_and_spectral_profile_are_computed_from_decoded_samples() {
        let interleaved: Vec<i16> = (0..8192)
            .map(|index| if (index / 24) % 2 == 0 { 16384 } else { -16384 })
            .collect();
        let path = write_pcm16_wav("square", 1, 48_000, &interleaved);
        let buffer = decode_file(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert!((buffer.rms - 0.5).abs() < 1e-6);
        assert!(buffer
            .spectral_profile
            .bands
            .iter()
            .any(|&band| band != 0.0));
    }

    /// Out-of-phase stereo cancels completely under averaging. Documented
    /// consequence of a plain-average downmix: the RMS is 0, so the downstream
    /// RMS normalisation falls back to gain 1.0.
    #[test]
    fn anti_phase_stereo_cancels_to_silence() {
        let interleaved: Vec<i16> = (0..2000)
            .map(|index| if index % 2 == 0 { 10000 } else { -10000 })
            .collect();
        let path = write_pcm16_wav("antiphase", 2, 48_000, &interleaved);
        let buffer = decode_file(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(buffer.samples.iter().all(|&sample| sample == 0.0));
        assert_eq!(buffer.rms, 0.0);
    }
}
