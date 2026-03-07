//! AmbisonicsRenderer — per-path FOA encode + decode gain ramp × sample per channel.
//!
//! Used by Ambisonics mode. Iterates over propagation paths from the PathResolver.
//! For the **direct path**, computes full FOA encoding (angular panning +
//! distance attenuation + directivity). For **reflection paths**, computes
//! angular-only FOA encoding (direction from path.direction), with the
//! reflection's energy carried by path.gain.
//!
//! Each path's B-format is decoded to speaker gains via FoaDecoder, then
//! interpolated per-sample (click-free gain ramp). The decoder is rebuilt
//! per-buffer because speaker azimuths are listener-relative.

use atrium_core::ambisonics::{foa_encode, FoaDecoder};
use atrium_core::directivity::directivity_gain;
use atrium_core::panner::distance_gain_at_model;
use atrium_core::speaker::{SpeakerLayout, MAX_CHANNELS};

use crate::pipeline::path::{PathKind, PathSet, MAX_PATHS};
use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// Per-path FOA encode + decode gain-ramp renderer.
#[derive(Default)]
pub struct AmbisonicsRenderer {
    /// Previous per-channel gains per source per path.
    /// Indexed [source_idx][path_idx][channel].
    prev_gains: Vec<[[f32; MAX_CHANNELS]; MAX_PATHS]>,
    /// Cached speaker count for decoder rebuild.
    speaker_count: usize,
    /// Rebuilt once per buffer (listener can move/turn each frame).
    decoder: Option<FoaDecoder>,
    /// Set to false at the start of each buffer (in ensure_topology).
    /// The first active source triggers the decoder rebuild.
    decoder_built: bool,
}

impl AmbisonicsRenderer {
    pub fn new() -> Self {
        Self::default()
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
            self.decoder = Some(FoaDecoder::from_listener(
                ctx.layout.speakers(),
                self.speaker_count,
                ctx.listener,
            ));
            self.decoder_built = true;
        }

        let decoder = match &self.decoder {
            Some(d) => d,
            None => return,
        };

        let path_slice = paths.as_slice();

        // Listener yaw rotation for world→local direction transform
        let cos_y = ctx.listener.yaw.cos();
        let sin_y = ctx.listener.yaw.sin();

        // Compute per-path FOA encode → decode to speaker gains
        let mut target_gains = [[0.0f32; MAX_CHANNELS]; MAX_PATHS];
        for (pi, path) in path_slice.iter().enumerate() {
            let bformat = match path.kind {
                PathKind::Direct => {
                    // Full encoding: azimuth + distance attenuation + directivity
                    let dx = ctx.source_pos.x - ctx.listener.position.x;
                    let dy = ctx.source_pos.y - ctx.listener.position.y;
                    let local_x = dx * cos_y + dy * sin_y; // forward
                    let local_y = -dx * sin_y + dy * cos_y; // left
                    let azimuth = local_y.atan2(local_x);

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

                    foa_encode(azimuth, dist_gain * dir_gain)
                }
                _ => {
                    // Reflection/diffraction: angular panning only, energy in path.gain
                    let local_x = path.direction.x * cos_y + path.direction.y * sin_y;
                    let local_y = -path.direction.x * sin_y + path.direction.y * cos_y;
                    let azimuth = local_y.atan2(local_x);
                    foa_encode(azimuth, 1.0)
                }
            };

            let mut decoded = decoder.decode(&bformat);
            ctx.layout.apply_mask(&mut decoded);
            target_gains[pi] = decoded.gains;
        }

        let inv_frames = 1.0 / out.num_frames as f32;
        let prev = &self.prev_gains[source_idx];

        for frame in 0..out.num_frames {
            let t = frame as f32 * inv_frames;
            let raw = source.next_sample(out.sample_rate);

            // Per-sample source stage DSP (air absorption filter, ground effect)
            let mut sample = raw;
            for stage in source_stages.iter_mut() {
                sample = stage.process_sample(sample);
            }

            // Apply broadband modifiers (ground effect gain, etc.)
            sample *= src_out.gain_modifier;

            // Accumulate all propagation paths, each with its own decoded gains.
            let base = frame * out.channels;
            for (pi, path) in path_slice.iter().enumerate() {
                let path_sample = sample * path.gain;
                for ch in 0..out.channels {
                    let gain = prev[pi][ch] + (target_gains[pi][ch] - prev[pi][ch]) * t;
                    out.buffer[base + ch] += path_sample * gain;
                }
            }
        }

        // Store targets as prev for next buffer
        self.prev_gains[source_idx] = target_gains;
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
    }

    fn reset(&mut self) {
        for gains in &mut self.prev_gains {
            *gains = [[0.0; MAX_CHANNELS]; MAX_PATHS];
        }
    }
}
