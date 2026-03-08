//! AmbisonicsRenderer — per-path full 3D FOA encode.
//!
//! Used by Ambisonics mode. Iterates over propagation paths from the PathResolver.
//! For the **direct path**, computes full FOA encoding (azimuth + elevation +
//! distance attenuation + directivity). For **reflection paths**, computes
//! angular-only FOA encoding (direction from path.direction), with the
//! reflection's energy carried by path.gain.
//!
//! Output depends on channel count:
//! - **≥4 channels:** Writes B-format (W,Y,Z,X) to channels 0-3. Decoding to
//!   speakers happens later via `AmbisonicsDecodeStage` (allows B-format
//!   decorrelation via `AmbiMultiDelayStage` before decode).
//! - **2 channels (stereo/headphones):** Inline bilateral ambisonics — B-format
//!   rotated per ear (accounting for head width ITD), then decoded with
//!   cardioid binaural weights. Multi-delay stage no-ops for stereo.

use atrium_core::ambisonics::{foa_encode, BilateralDecoder};
use atrium_core::directivity::directivity_gain;
use atrium_core::panner::distance_gain_at_model;
use atrium_core::speaker::SpeakerLayout;

use crate::pipeline::path::{PathEffectChain, PathKind, PathSet, MAX_PATHS};
use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// Per-path FOA encode renderer.
///
/// For ≥4ch: encode-only (writes B-format). For 2ch: inline bilateral decode.
#[derive(Default)]
pub struct AmbisonicsRenderer {
    /// Previous per-path B-format per source (encode-only mode, ≥4ch).
    /// [source][path] = [W, Y, Z, X] gains for ramping.
    prev_bformat: Vec<[[f32; 4]; MAX_PATHS]>,
    /// Previous per-path stereo gains per source (binaural mode): [left, right].
    prev_stereo: Vec<[[f32; 2]; MAX_PATHS]>,
    /// Cached speaker count.
    speaker_count: usize,
    /// Bilateral decoder for 2ch mode.
    bilateral: Option<BilateralDecoder>,
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
    path_distance: f32,
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
            // Distance model for reflections — same law as the direct path.
            // path.gain carries reflectivity only; distance is applied here.
            let dist_gain = distance_gain_at_model(
                ctx.listener.position,
                ctx.listener.position + path_direction * path_distance,
                ctx.source_ref_distance,
                ctx.distance_model.max_distance,
                ctx.distance_model.rolloff,
                ctx.distance_model.model,
            );
            foa_encode(azimuth, elevation, dist_gain)
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
        path_effects: &mut [PathEffectChain],
        out: &mut OutputBuffer,
    ) {
        let path_slice = paths.as_slice();
        let cos_y = ctx.listener.yaw.cos();
        let sin_y = ctx.listener.yaw.sin();

        if self.speaker_count >= 4 {
            // ≥4ch: encode-only mode. Write B-format to channels 0-3.
            // Decode happens later in AmbisonicsDecodeStage.
            let mut target_bf = [[0.0f32; 4]; MAX_PATHS];
            for (pi, path) in path_slice.iter().enumerate() {
                let bf = encode_path(path.kind, path.direction, path.distance, ctx, cos_y, sin_y);
                target_bf[pi] = [bf.w, bf.y, bf.z, bf.x];
            }

            let inv_frames = 1.0 / out.num_frames as f32;
            let prev = &self.prev_bformat[source_idx];

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
                    let filtered = path_effects[pi].process_sample(sample);
                    let path_sample = filtered * path.gain;
                    for ch in 0..4.min(out.channels) {
                        let gain = prev[pi][ch] + (target_bf[pi][ch] - prev[pi][ch]) * t;
                        out.buffer[base + ch] += path_sample * gain;
                    }
                    // Only the direct path feeds the late reverb send.
                    // Mono only (W channel): late reverb is physically omnidirectional,
                    // so we don't inject directional B-format into the FDN. The FDN's
                    // Z-rotation creates spatial decorrelation from the omni input.
                    if path.kind == PathKind::Direct {
                        if let Some(ref mut rev) = out.reverb_send {
                            rev[base] += path_sample * src_out.reverb_send;
                        }
                    }
                }
            }

            self.prev_bformat[source_idx] = target_bf;
        } else {
            // 2ch: inline binaural ambisonics decode.
            let bilateral = match &self.bilateral {
                Some(b) => b,
                None => return,
            };

            let mut target_stereo = [[0.0f32; 2]; MAX_PATHS];
            for (pi, path) in path_slice.iter().enumerate() {
                let bformat =
                    encode_path(path.kind, path.direction, path.distance, ctx, cos_y, sin_y);
                let (l, r) = bilateral.decode_stereo(&bformat);
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
                    let filtered = path_effects[pi].process_sample(sample);
                    let path_sample = filtered * path.gain;
                    for ear in 0..2.min(out.channels) {
                        let gain = prev[pi][ear] + (target_stereo[pi][ear] - prev[pi][ear]) * t;
                        out.buffer[base + ear] += path_sample * gain;
                    }
                }
            }

            self.prev_stereo[source_idx] = target_stereo;
        }
    }

    fn name(&self) -> &str {
        "ambisonics"
    }

    fn ensure_topology(&mut self, source_count: usize, layout: &SpeakerLayout, _sample_rate: f32) {
        self.speaker_count = layout.speaker_count();

        if self.speaker_count < 4 && self.bilateral.is_none() {
            self.bilateral = Some(BilateralDecoder::new());
        }

        while self.prev_bformat.len() < source_count {
            self.prev_bformat.push([[0.0; 4]; MAX_PATHS]);
        }
        while self.prev_stereo.len() < source_count {
            self.prev_stereo.push([[0.0; 2]; MAX_PATHS]);
        }
    }

    fn reset(&mut self) {
        for bf in &mut self.prev_bformat {
            *bf = [[0.0; 4]; MAX_PATHS];
        }
        for stereo in &mut self.prev_stereo {
            *stereo = [[0.0; 2]; MAX_PATHS];
        }
    }
}
