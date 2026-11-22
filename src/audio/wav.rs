//! Writing WAV files. Every WAV the engine writes, the rendered scenes and
//! the listening samples of the tests alike, is written here.
//!
//! Two formats. 32-bit float keeps every sample as it was rendered: nothing
//! clips and nothing is rounded, so a file analysed is the render itself.
//! 16-bit is for listening; it clips at full scale and says how often.
//!
//! Mono and stereo use the plain header every tool reads. More channels use
//! `WAVE_FORMAT_EXTENSIBLE` with a channel mask, so a 5.1 file opens as 5.1
//! (L, R, C, LFE, Ls, Rs) in common tools instead of as six unnamed channels.

use std::path::Path;

/// How the samples of a WAV file are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    /// IEEE 754 32-bit float, the rendered value exactly.
    Float32,
    /// Signed 16-bit integers. A sample becomes `(x.clamp(-1, 1) * 32767)`
    /// truncated towards zero, as the engine's own 16-bit output does.
    Pcm16,
}

impl SampleFormat {
    fn bytes_per_sample(self) -> u16 {
        match self {
            SampleFormat::Float32 => 4,
            SampleFormat::Pcm16 => 2,
        }
    }
}

/// `WAVE_FORMAT_PCM`.
const FORMAT_PCM: u16 = 1;
/// `WAVE_FORMAT_IEEE_FLOAT`.
const FORMAT_IEEE_FLOAT: u16 = 3;
/// `WAVE_FORMAT_EXTENSIBLE`: the real format is the subformat GUID.
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// The tail every `KSDATAFORMAT_SUBTYPE_*` GUID shares; the first two bytes
/// are the plain format tag.
const SUBFORMAT_TAIL: [u8; 14] = [
    0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];

/// Write `interleaved` frames of `channels` channels at `sample_rate` to
/// `path` as `format`. Returns how many samples clipped: always 0 for
/// [`SampleFormat::Float32`], which stores every value as it is.
pub fn write_wav(
    path: &Path,
    interleaved: &[f32],
    channels: u16,
    sample_rate: u32,
    format: SampleFormat,
) -> std::io::Result<usize> {
    let (bytes, clipped) = encode(interleaved, channels, sample_rate, format)?;
    std::fs::write(path, bytes)?;
    Ok(clipped)
}

/// The whole file in memory, and how many samples clipped.
fn encode(
    interleaved: &[f32],
    channels: u16,
    sample_rate: u32,
    format: SampleFormat,
) -> std::io::Result<(Vec<u8>, usize)> {
    let invalid = |message: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, message);
    if channels == 0 || channels as usize > MASKED_CHANNELS {
        return Err(invalid("a WAV file here has 1 to 18 channels"));
    }
    if !interleaved.len().is_multiple_of(channels as usize) {
        return Err(invalid("the samples are not a whole number of frames"));
    }
    let bytes_per_sample = format.bytes_per_sample();
    let data_length = u32::try_from(interleaved.len() * bytes_per_sample as usize)
        .ok()
        .filter(|length| *length <= u32::MAX - 128)
        .ok_or_else(|| invalid("too long for a WAV file (4 GiB)"))?;
    let frames = (interleaved.len() / channels as usize) as u32;

    let tag = match format {
        SampleFormat::Float32 => FORMAT_IEEE_FLOAT,
        SampleFormat::Pcm16 => FORMAT_PCM,
    };
    let extensible = channels > 2;
    // The plain PCM header has no size field for an extension; every other
    // header has one, and a non-PCM file also says how many frames it holds.
    let format_chunk_length: u32 = match (extensible, format) {
        (true, _) => 40,
        (false, SampleFormat::Pcm16) => 16,
        (false, SampleFormat::Float32) => 18,
    };
    let has_fact = format == SampleFormat::Float32;
    let riff_length =
        4 + (8 + format_chunk_length) + if has_fact { 8 + 4 } else { 0 } + (8 + data_length);

    let mut bytes = Vec::with_capacity(riff_length as usize + 8);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&riff_length.to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&format_chunk_length.to_le_bytes());
    bytes.extend_from_slice(&(if extensible { FORMAT_EXTENSIBLE } else { tag }).to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    let block_align = channels * bytes_per_sample;
    bytes.extend_from_slice(&(sample_rate * block_align as u32).to_le_bytes());
    bytes.extend_from_slice(&block_align.to_le_bytes());
    bytes.extend_from_slice(&(bytes_per_sample * 8).to_le_bytes());
    if extensible {
        bytes.extend_from_slice(&22u16.to_le_bytes());
        bytes.extend_from_slice(&(bytes_per_sample * 8).to_le_bytes());
        bytes.extend_from_slice(&channel_mask(channels).to_le_bytes());
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&SUBFORMAT_TAIL);
    } else if format_chunk_length == 18 {
        bytes.extend_from_slice(&0u16.to_le_bytes());
    }
    if has_fact {
        bytes.extend_from_slice(b"fact");
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&frames.to_le_bytes());
    }
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_length.to_le_bytes());

    let mut clipped = 0;
    match format {
        SampleFormat::Float32 => {
            for sample in interleaved {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
        }
        SampleFormat::Pcm16 => {
            for sample in interleaved {
                if sample.abs() > 1.0 {
                    clipped += 1;
                }
                let quantised = (sample.clamp(-1.0, 1.0) * 32_767.0) as i16;
                bytes.extend_from_slice(&quantised.to_le_bytes());
            }
        }
    }
    Ok((bytes, clipped))
}

/// How many speaker positions `dwChannelMask` names.
const MASKED_CHANNELS: usize = 18;

/// The speakers of the first `channels` channels. The mask's bits run FL,
/// FR, FC, LFE, BL, BR, …, which is the 5.1 order every layout here uses
/// (L, R, C, LFE, Ls, Rs), so the first bits name them. The surrounds are
/// "back" left and right: the mask Windows and most tools call 5.1.
fn channel_mask(channels: u16) -> u32 {
    (1u32 << channels) - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([bytes[at], bytes[at + 1]])
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
    }

    /// The 16-bit stereo header is the classic 44 bytes the listening
    /// samples were always written with, so they come out the same.
    #[test]
    fn sixteen_bit_stereo_has_the_classic_header() {
        let (bytes, clipped) =
            encode(&[0.5, -0.5, 1.5, -2.0], 2, 48_000, SampleFormat::Pcm16).expect("encodes");
        assert_eq!(bytes.len(), 44 + 8);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32_at(&bytes, 4), 36 + 8);
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u32_at(&bytes, 16), 16);
        assert_eq!(u16_at(&bytes, 20), 1, "PCM");
        assert_eq!(u16_at(&bytes, 22), 2);
        assert_eq!(u32_at(&bytes, 24), 48_000);
        assert_eq!(u32_at(&bytes, 28), 48_000 * 4);
        assert_eq!(u16_at(&bytes, 32), 4);
        assert_eq!(u16_at(&bytes, 34), 16);
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(u32_at(&bytes, 40), 8);
        let samples: Vec<i16> = bytes[44..]
            .chunks(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(samples, [16_383, -16_383, 32_767, -32_767]);
        assert_eq!(clipped, 2, "1.5 and -2 clipped");
    }

    /// Six channels are written as WAVE_FORMAT_EXTENSIBLE, the 5.1 mask
    /// and the float subformat, and a float file says how many frames it has.
    #[test]
    fn six_float_channels_are_extensible_5_1() {
        let frame = [0.1f32, 0.2, 0.3, 0.4, 0.5, 0.6];
        let (bytes, clipped) = encode(&frame, 6, 48_000, SampleFormat::Float32).expect("encodes");
        assert_eq!(clipped, 0);
        assert_eq!(u32_at(&bytes, 16), 40);
        assert_eq!(u16_at(&bytes, 20), FORMAT_EXTENSIBLE);
        assert_eq!(u16_at(&bytes, 22), 6);
        assert_eq!(u16_at(&bytes, 32), 24, "block align");
        assert_eq!(u16_at(&bytes, 34), 32, "bits");
        assert_eq!(u16_at(&bytes, 36), 22, "extension size");
        assert_eq!(u16_at(&bytes, 38), 32, "valid bits");
        assert_eq!(u32_at(&bytes, 40), 0x3F, "FL FR FC LFE BL BR");
        assert_eq!(u16_at(&bytes, 44), FORMAT_IEEE_FLOAT);
        assert_eq!(&bytes[46..60], &SUBFORMAT_TAIL);
        assert_eq!(&bytes[60..64], b"fact");
        assert_eq!(u32_at(&bytes, 68), 1, "one frame");
        assert_eq!(&bytes[72..76], b"data");
        assert_eq!(u32_at(&bytes, 76), 24);
        assert_eq!(u32_at(&bytes, 4) as usize, bytes.len() - 8);
        assert_eq!(f32::from_le_bytes(bytes[80..84].try_into().unwrap()), 0.1);
    }

    /// Samples that are not whole frames, or no channels, are refused.
    #[test]
    fn a_broken_frame_is_refused() {
        assert!(encode(&[0.0; 3], 2, 48_000, SampleFormat::Float32).is_err());
        assert!(encode(&[], 0, 48_000, SampleFormat::Float32).is_err());
    }
}
