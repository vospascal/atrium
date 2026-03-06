use std::fs::File;
use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Pre-decoded audio samples. Immutable after creation — safe to share via Arc.
pub struct AudioBuffer {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    /// Root-mean-square level of the decoded samples.
    pub rms: f32,
}

/// Decode an audio file (MP3, WAV, FLAC, etc.) into a mono f32 AudioBuffer.
pub fn decode_file(path: &Path) -> Result<AudioBuffer, Box<dyn std::error::Error>> {
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
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count())
        .unwrap_or(1);
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
            Err(_) => break,
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let spec = *decoded.spec();
        let num_frames = decoded.capacity();

        let mut sample_buf = SampleBuffer::<f32>::new(num_frames as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);

        let interleaved = sample_buf.samples();

        // Downmix to mono if needed
        if channels > 1 {
            for frame in interleaved.chunks(channels) {
                let mono: f32 = frame.iter().sum::<f32>() / channels as f32;
                all_samples.push(mono);
            }
        } else {
            all_samples.extend_from_slice(interleaved);
        }
    }

    let rms = if all_samples.is_empty() {
        0.0
    } else {
        (all_samples.iter().map(|s| s * s).sum::<f32>() / all_samples.len() as f32).sqrt()
    };

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
    })
}
