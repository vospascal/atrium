//! AmbisonicsRenderer — per-path full 3D FOA encode + decode.
//!
//! Used by Ambisonics mode. Iterates over propagation paths from the PathResolver.
//! For the **direct path**, computes full FOA encoding (azimuth + elevation +
//! distance attenuation + directivity). For **reflection paths**, computes
//! angular-only FOA encoding (direction from path.direction), with the
//! reflection's energy carried by path.gain.
//!
//! Decoding adapts to the output layout:
//! - **3+ channels (speakers):** AllRAD decoder — 12 virtual speakers decoded
//!   via mode-matching, then VBAP re-panned to the real speaker layout.
//!   Rebuilt per-buffer because speaker angles are listener-relative.
//! - **2 channels (stereo/headphones):** Bilateral ambisonics — B-format
//!   rotated per ear (accounting for head width ITD), then decoded with
//!   cardioid binaural weights.

use atrium_core::ambisonics::{foa_encode, AllRadDecoder, BilateralDecoder};
use atrium_core::directivity::directivity_gain;
use atrium_core::panner::distance_gain_at_model;
use atrium_core::speaker::{SpeakerLayout, MAX_CHANNELS};

use crate::pipeline::path::{PathKind, PathSet, MAX_PATHS};
use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// Decode strategy auto-selected based on output channel count.
///
/// 3+ channels → physical speakers → AllRAD (VBAP re-panning handles arbitrary layouts).
/// 2 channels → stereo headphones → bilateral (per-ear B-format rotation + cardioid decode).
///
/// The threshold is 3 because VBAP needs ≥2 speakers to form pairs, and with only
/// 2 speakers we're in headphone territory where bilateral binaural is the right decode.
enum DecodeMode {
    Speaker(AllRadDecoder),
    Binaural(BilateralDecoder),
}

/// Per-path FOA encode + AllRAD/bilateral decode renderer.
pub struct AmbisonicsRenderer {
    /// Previous per-channel gains per source per path (speaker mode).
    prev_gains: Vec<[[f32; MAX_CHANNELS]; MAX_PATHS]>,
    /// Previous per-path stereo gains per source (binaural mode): [left, right].
    prev_stereo: Vec<[[f32; 2]; MAX_PATHS]>,
    /// Cached speaker count for decoder rebuild.
    speaker_count: usize,
    /// Current decode mode, rebuilt once per buffer.
    decode_mode: Option<DecodeMode>,
    /// Set to false at the start of each buffer (in ensure_topology).
    /// The first active source triggers the decoder rebuild.
    decoder_built: bool,
}

impl Default for AmbisonicsRenderer {
    fn default() -> Self {
        Self {
            prev_gains: Vec::new(),
            prev_stereo: Vec::new(),
            speaker_count: 0,
            decode_mode: None,
            decoder_built: false,
        }
    }
}

impl AmbisonicsRenderer {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Encode a path to B-format in listener-local coordinates.
fn encode_path(
    path_kind: PathKind,
    path_direction: atrium_core::types::Vec3,
    ctx: &SourceContext,
    cos_y: f32,
    sin_y: f32,
) -> atrium_core::ambisonics::BFormat {
    match path_kind {
        PathKind::Direct => {
            let dx = ctx.source_pos.x - ctx.listener.position.x;
            let dy = ctx.source_pos.y - ctx.listener.position.y;
            let dz = ctx.source_pos.z - ctx.listener.position.z;
            let local_x = dx * cos_y + dy * sin_y;
            let local_y = -dx * sin_y + dy * cos_y;
            let azimuth = local_y.atan2(local_x);
            let horiz = (local_x * local_x + local_y * local_y).sqrt();
            let elevation = dz.atan2(horiz);

            let dist_gain = distance_gain_at_model(
                ctx.listener.position,
                ctx.source_pos,
                ctx.source_ref_distance,
                ctx.distance_model.max_distance,
                ctx.distance_model.rolloff,
                ctx.distance_model.model,
            );
            let dir_gain = directivity_gain(
                ctx.source_pos,
                ctx.source_orientation,
                ctx.listener.position,
                ctx.source_directivity,
            );

            foa_encode(azimuth, elevation, dist_gain * dir_gain)
        }
        _ => {
            let local_x = path_direction.x * cos_y + path_direction.y * sin_y;
            let local_y = -path_direction.x * sin_y + path_direction.y * cos_y;
            let local_z = path_direction.z;
            let azimuth = local_y.atan2(local_x);
            let horiz = (local_x * local_x + local_y * local_y).sqrt();
            let elevation = local_z.atan2(horiz);
            foa_encode(azimuth, elevation, 1.0)
        }
    }
}

impl Renderer for AmbisonicsRenderer {
    #[allow(clippy::needless_range_loop)]
    fn render_source(
        &mut self,
        source_idx: usize,
        source: &mut dyn atrium_core::source::SoundSource,
        source_stages: &mut [&mut dyn SourceStage],
        ctx: &SourceContext,
        src_out: &SourceOutput,
        paths: &PathSet,
        out: &mut OutputBuffer,
    ) {
        // Rebuild decoder once per buffer with current listener position/yaw.
        if !self.decoder_built {
            if self.speaker_count >= 3 {
                self.decode_mode = Some(DecodeMode::Speaker(AllRadDecoder::from_listener(
                    ctx.layout.speakers(),
                    self.speaker_count,
                    ctx.listener,
                )));
            } else {
                self.decode_mode = Some(DecodeMode::Binaural(BilateralDecoder::new()));
            }
            self.decoder_built = true;
        }

        let decode_mode = match &self.decode_mode {
            Some(d) => d,
            None => return,
        };

        let path_slice = paths.as_slice();
        let cos_y = ctx.listener.yaw.cos();
        let sin_y = ctx.listener.yaw.sin();

        match decode_mode {
            DecodeMode::Speaker(decoder) => {
                // AllRAD decode to speaker gains
                let mut target_gains = [[0.0f32; MAX_CHANNELS]; MAX_PATHS];
                for (pi, path) in path_slice.iter().enumerate() {
                    let bformat = encode_path(path.kind, path.direction, ctx, cos_y, sin_y);
                    let mut decoded = decoder.decode(&bformat);
                    ctx.layout.apply_mask(&mut decoded);
                    target_gains[pi] = decoded.gains;
                }

                let inv_frames = 1.0 / out.num_frames as f32;
                let prev = &self.prev_gains[source_idx];

                for frame in 0..out.num_frames {
                    let t = frame as f32 * inv_frames;
                    let raw = source.next_sample(out.sample_rate);
                    let mut sample = raw;
                    for stage in source_stages.iter_mut() {
                        sample = stage.process_sample(sample);
                    }
                    sample *= src_out.gain_modifier;

                    let base = frame * out.channels;
                    for (pi, path) in path_slice.iter().enumerate() {
                        let path_sample = sample * path.gain;
                        for ch in 0..out.channels {
                            let gain = prev[pi][ch] + (target_gains[pi][ch] - prev[pi][ch]) * t;
                            out.buffer[base + ch] += path_sample * gain;
                        }
                    }
                }

                self.prev_gains[source_idx] = target_gains;
            }
            DecodeMode::Binaural(bilateral) => {
                // Bilateral ambisonics to stereo
                let mut target_stereo = [[0.0f32; 2]; MAX_PATHS];
                for (pi, path) in path_slice.iter().enumerate() {
                    let bformat = encode_path(path.kind, path.direction, ctx, cos_y, sin_y);
                    let (l, r) = bilateral.decode_stereo(&bformat, path.distance.max(0.5));
                    target_stereo[pi] = [l, r];
                }

                let inv_frames = 1.0 / out.num_frames as f32;
                let prev = &self.prev_stereo[source_idx];

                for frame in 0..out.num_frames {
                    let t = frame as f32 * inv_frames;
                    let raw = source.next_sample(out.sample_rate);
                    let mut sample = raw;
                    for stage in source_stages.iter_mut() {
                        sample = stage.process_sample(sample);
                    }
                    sample *= src_out.gain_modifier;

                    let base = frame * out.channels;
                    for (pi, path) in path_slice.iter().enumerate() {
                        let path_sample = sample * path.gain;
                        for ear in 0..2.min(out.channels) {
                            let gain = prev[pi][ear] + (target_stereo[pi][ear] - prev[pi][ear]) * t;
                            out.buffer[base + ear] += path_sample * gain;
                        }
                    }
                }

                self.prev_stereo[source_idx] = target_stereo;
            }
        }
    }

    fn name(&self) -> &str {
        "ambisonics"
    }

    fn ensure_topology(&mut self, source_count: usize, layout: &SpeakerLayout, _sample_rate: f32) {
        self.speaker_count = layout.speaker_count();
        self.decoder_built = false;

        while self.prev_gains.len() < source_count {
            self.prev_gains.push([[0.0; MAX_CHANNELS]; MAX_PATHS]);
        }
        while self.prev_stereo.len() < source_count {
            self.prev_stereo.push([[0.0; 2]; MAX_PATHS]);
        }
    }

    fn reset(&mut self) {
        for gains in &mut self.prev_gains {
            *gains = [[0.0; MAX_CHANNELS]; MAX_PATHS];
        }
        for stereo in &mut self.prev_stereo {
            *stereo = [[0.0; 2]; MAX_PATHS];
        }
    }
}
