//! AmbisonicsRenderer — per-path full 3D FOA encode.
//!
//! Used by Ambisonics mode. Iterates over propagation paths from the PathResolver.
//! For the **direct path**, computes full FOA encoding (azimuth + elevation +
//! distance attenuation + directivity). For **reflection paths**, computes
//! angular-only FOA encoding (direction from path.direction), with the
//! reflection's energy carried by path.gain.
//!
//! What the mix bus carries is decided once, by [`ambisonic_bus`], and every
//! consumer (this renderer, the field renderer, `AmbiMultiDelayStage`,
//! `AmbisonicsDecodeStage`) asks that function instead of applying its own
//! channel-count rule:
//! - [`AmbisonicBus::BFormat`]: writes B-format (W,Y,Z,X) to channels 0-3.
//!   `AmbiMultiDelayStage` decorrelates it and `AmbisonicsDecodeStage`
//!   decodes it with the [`BusDecoder`] the decision names.
//! - [`AmbisonicBus::InlineBilateral`]: the bus is too narrow for four
//!   components, so the renderer decodes inline — B-format rotated ±90° per
//!   ear, then forward cardioid weights to a left and a right channel. The
//!   multi-delay and decode stages no-op.

use atrium_core::ambisonics::{foa_encode, BilateralDecoder};
use atrium_core::directivity::directivity_gain;
use atrium_core::panner::distance_gain_at_model;
use atrium_core::speaker::{Speaker, SpeakerLayout, MAX_CHANNELS};
use atrium_core::types::Vec3;

use crate::pipeline::path::{PathEffectChain, PathKind, PathSet, MAX_PATHS};
use crate::pipeline::renderer::SourceContext;
use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::renderers::field::FieldEncoding;

/// What the Ambisonics mix bus carries between the renderer and the post-mix
/// stages. Produced only by [`ambisonic_bus`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AmbisonicBus {
    /// Channels 0–3 carry first-order B-format (W, Y, Z, X), decoded by the
    /// decode stage with this decoder.
    BFormat(BusDecoder),
    /// Fewer than four bus channels: the renderer decodes inline with the
    /// bilateral cardioid pair to these channels.
    InlineBilateral(EarChannels),
}

/// The decoder the decode stage applies to a B-format bus.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BusDecoder {
    /// EPAD-corrected AllRAD designed over exactly these speakers.
    AllRad(DecodeSpeakers),
    /// Bilateral cardioid pair to these channels; every other channel is
    /// silenced.
    Bilateral(EarChannels),
}

/// Where a bilateral decode puts each ear signal. Both fields name the same
/// channel when only one speaker is reachable — the two ears then sum to the
/// omni signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EarChannels {
    pub left: usize,
    pub right: usize,
}

/// The speakers an AllRAD decoder is designed over: active (unmasked),
/// non-LFE speakers on a channel the bus can reach.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecodeSpeakers {
    speakers: [Speaker; MAX_CHANNELS],
    count: usize,
}

impl DecodeSpeakers {
    pub fn as_slice(&self) -> &[Speaker] {
        &self.speakers[..self.count]
    }
}

/// Decide what the Ambisonics mix bus carries and how it is decoded.
///
/// - `render_channels`: channels the pipeline lets the renderer write
///   (`MixContext::render_channels`).
/// - `device_channels`: interleaved channels in the output buffer.
///
/// A speaker is *reachable* when its channel is below both counts, is active
/// in the layout's mask, and is not the LFE channel.
///
/// | bus channels | reachable speakers | result                                   |
/// |--------------|--------------------|------------------------------------------|
/// | < 4          | any                | `InlineBilateral` (renderer decodes)     |
/// | ≥ 4          | ≥ 3                | `BFormat(AllRad)` over the reachable set |
/// | ≥ 4          | ≤ 2                | `BFormat(Bilateral)` (decode stage)      |
///
/// Three is the fewest speakers an AllRAD ring can surround the listener
/// with. A bilateral decode goes to the two lowest reachable channels —
/// lowest is left, the order of `SpeakerLayout::stereo` and ITU 5.1 — or to
/// channels 0 and 1 when nothing is reachable (a positionless headphone host).
///
/// Designing the decoder over reachable speakers only is the point: a decoder
/// designed over all five 5.1 speakers and masked afterwards puts a rear
/// source almost entirely into the silenced surrounds (−21 dB, antiphase).
pub fn ambisonic_bus(
    layout: &SpeakerLayout,
    render_channels: usize,
    device_channels: usize,
) -> AmbisonicBus {
    let bus_channels = render_channels.min(device_channels);
    let mut reachable = DecodeSpeakers {
        speakers: [Speaker {
            position: Vec3::ZERO,
            channel: 0,
        }; MAX_CHANNELS],
        count: 0,
    };
    for speaker in layout.speakers() {
        if speaker.channel < bus_channels
            && layout.is_channel_active(speaker.channel)
            && Some(speaker.channel) != layout.lfe_channel()
            && reachable.count < MAX_CHANNELS
        {
            reachable.speakers[reachable.count] = *speaker;
            reachable.count += 1;
        }
    }

    if bus_channels < 4 {
        AmbisonicBus::InlineBilateral(ear_channels(&reachable, bus_channels))
    } else if reachable.count >= 3 {
        AmbisonicBus::BFormat(BusDecoder::AllRad(reachable))
    } else {
        AmbisonicBus::BFormat(BusDecoder::Bilateral(ear_channels(
            &reachable,
            bus_channels,
        )))
    }
}

/// The two lowest reachable channels as left and right ear.
fn ear_channels(reachable: &DecodeSpeakers, bus_channels: usize) -> EarChannels {
    let mut lowest = usize::MAX;
    let mut second_lowest = usize::MAX;
    for speaker in reachable.as_slice() {
        if speaker.channel < lowest {
            second_lowest = lowest;
            lowest = speaker.channel;
        } else if speaker.channel < second_lowest {
            second_lowest = speaker.channel;
        }
    }
    match (lowest, second_lowest) {
        (usize::MAX, _) => EarChannels {
            left: 0,
            right: bus_channels.saturating_sub(1).min(1),
        },
        (only, usize::MAX) => EarChannels {
            left: only,
            right: only,
        },
        (left, right) => EarChannels { left, right },
    }
}

/// The bus as the post-mix stages will see it.
///
/// The renderer is not handed `MixContext::render_channels`. The Ambisonics
/// pipeline leaves `RenderPipeline::render_channels` at 0, so `render_pipeline`
/// gives the stages `layout.total_channels().min(device_channels)`; passing
/// `total_channels` here therefore reaches the identical decision (pinned by
/// `ambisonics_pipeline_does_not_override_render_channels`).
fn renderer_bus(layout: &SpeakerLayout, device_channels: usize) -> AmbisonicBus {
    ambisonic_bus(layout, layout.total_channels(), device_channels)
}

/// Per-path FOA encode renderer. Encode-only on a B-format bus, inline
/// bilateral decode otherwise (see [`ambisonic_bus`]).
#[derive(Default)]
pub struct AmbisonicsRenderer {
    /// Previous per-path B-format per source (B-format bus).
    /// [source][path] = [W, Y, Z, X] gains for ramping.
    prev_bformat: Vec<[[f32; 4]; MAX_PATHS]>,
    /// Previous per-path ear gains per source (bilateral bus): [left, right].
    prev_stereo: Vec<[[f32; 2]; MAX_PATHS]>,
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
        ctx: &SourceContext,
        paths: &PathSet,
        path_effects: &mut [PathEffectChain],
        out: &mut OutputBuffer,
    ) {
        let path_slice = paths.as_slice();
        let cos_y = ctx.listener.yaw.cos();
        let sin_y = ctx.listener.yaw.sin();

        match renderer_bus(ctx.layout, out.channels) {
            AmbisonicBus::BFormat(_) => {
                // Encode-only. Write B-format to channels 0-3; decode happens
                // later in AmbisonicsDecodeStage.
                let mut target_bf = [[0.0f32; 4]; MAX_PATHS];
                for (pi, path) in path_slice.iter().enumerate() {
                    let bf =
                        encode_path(path.kind, path.direction, path.distance, ctx, cos_y, sin_y);
                    target_bf[pi] = [bf.w, bf.y, bf.z, bf.x];
                }

                let inv_frames = 1.0 / out.num_frames as f32;
                let prev = &self.prev_bformat[source_idx];

                for frame in 0..out.num_frames {
                    let t = frame as f32 * inv_frames;
                    let raw = source.next_sample(out.sample_rate);
                    let sample = raw;

                    let base = frame * out.channels;
                    for (pi, path) in path_slice.iter().enumerate() {
                        let filtered = path_effects[pi].process_sample(sample);
                        let path_sample = filtered * path.gain;
                        for component in 0..4 {
                            let gain = prev[pi][component]
                                + (target_bf[pi][component] - prev[pi][component]) * t;
                            out.buffer[base + component] += path_sample * gain;
                        }
                        // Only the direct path feeds the late reverb send.
                        // Mono only (W channel): late reverb is physically omnidirectional,
                        // so we don't inject directional B-format into the FDN. The FDN's
                        // Z-rotation creates spatial decorrelation from the omni input.
                        if path.kind == PathKind::Direct {
                            if let Some(ref mut rev) = out.reverb_send {
                                rev[base] += path_sample * ctx.reverb_send;
                            }
                        }
                    }
                }

                self.prev_bformat[source_idx] = target_bf;
                // The other bus's ramp restarts from silence if the bus changes.
                self.prev_stereo[source_idx] = [[0.0; 2]; MAX_PATHS];
            }
            AmbisonicBus::InlineBilateral(ears) => {
                // Inline bilateral decode to the bus's left/right channels.
                let bilateral = BilateralDecoder::new();
                let ear_channels = [ears.left, ears.right];

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
                    let sample = raw;

                    let base = frame * out.channels;
                    for (pi, path) in path_slice.iter().enumerate() {
                        let filtered = path_effects[pi].process_sample(sample);
                        let path_sample = filtered * path.gain;
                        for (ear, &channel) in ear_channels.iter().enumerate() {
                            let gain = prev[pi][ear] + (target_stereo[pi][ear] - prev[pi][ear]) * t;
                            out.buffer[base + channel] += path_sample * gain;
                        }
                        // Only the direct path feeds the late reverb send — same
                        // as the B-format branch; without this, the bilateral bus
                        // starves the FDN and produces no tail at all.
                        if path.kind == PathKind::Direct {
                            if let Some(ref mut rev) = out.reverb_send {
                                rev[base] += path_sample * ctx.reverb_send;
                            }
                        }
                    }
                }

                self.prev_stereo[source_idx] = target_stereo;
                self.prev_bformat[source_idx] = [[0.0; 4]; MAX_PATHS];
            }
        }
    }

    fn name(&self) -> &str {
        "ambisonics"
    }

    fn field_encoding(&self, layout: &SpeakerLayout, device_channels: usize) -> FieldEncoding {
        match renderer_bus(layout, device_channels) {
            AmbisonicBus::BFormat(_) => FieldEncoding::Ambisonics,
            AmbisonicBus::InlineBilateral(_) => FieldEncoding::Speaker,
        }
    }

    fn ensure_topology(&mut self, source_count: usize, _layout: &SpeakerLayout, _sample_rate: f32) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::distance::DistanceModel;
    use crate::audio::propagation::GroundProperties;
    use crate::pipeline::builders;
    use crate::pipeline::environment::Environment;
    use crate::pipeline::{render_pipeline, PipelineParams, RenderParams};
    use atrium_core::directivity::DirectivityPattern;
    use atrium_core::listener::Listener;
    use atrium_core::source::SoundSource;
    use atrium_core::speaker::Speaker;
    use atrium_core::types::Vec3;
    use std::f32::consts::FRAC_PI_2;

    fn assert_close(actual: f32, expected: f32, label: &str) {
        assert!(
            (actual - expected).abs() < 1e-5,
            "{label}: got {actual}, expected {expected}"
        );
    }

    /// Encode one path for a listener at the origin with the given yaw.
    fn encode_for(
        kind: PathKind,
        direction: Vec3,
        distance: f32,
        source_position: Vec3,
        yaw: f32,
    ) -> atrium_core::ambisonics::BFormat {
        let listener = Listener::new(Vec3::ZERO, yaw);
        let directivity = DirectivityPattern::Omni;
        let environment = Environment {
            ground: GroundProperties::default(),
            ..Environment::anechoic(Vec3::new(-10.0, -10.0, -10.0), Vec3::new(10.0, 10.0, 10.0))
        };
        let distance_model = DistanceModel::default();
        let layout = SpeakerLayout::new(&[], None, 6);
        let ctx = SourceContext {
            listener: &listener,
            source_pos: source_position,
            source_orientation: Vec3::new(1.0, 0.0, 0.0),
            source_directivity: &directivity,
            source_spread: 0.0,
            source_ref_distance: 1.0,
            path_target: Vec3::ZERO,
            environment: &environment,
            sample_rate: 48000.0,
            distance_model: &distance_model,
            layout: &layout,
            reverb_send: 0.0,
        };
        encode_path(kind, direction, distance, &ctx, yaw.cos(), yaw.sin())
    }

    #[test]
    fn direct_path_encodes_listener_local_left_with_inverse_distance_gain() {
        // Listener facing +Y: world −X is local left (azimuth +π/2).
        // Inverse law, ref 1 m, rolloff 1, d = 2 m: g = 1/(1 + (2 − 1)) = ½.
        // W = g/√2 = 0.353553, Y = g·a = 0.353553, Z = X = 0.
        let bformat = encode_for(
            PathKind::Direct,
            Vec3::ZERO,
            0.0,
            Vec3::new(-2.0, 0.0, 0.0),
            FRAC_PI_2,
        );
        assert_close(bformat.w, 0.353553, "W");
        assert_close(bformat.y, 0.353553, "Y");
        assert_close(bformat.z, 0.0, "Z");
        assert_close(bformat.x, 0.0, "X");
    }

    #[test]
    fn direct_path_encodes_elevation_from_world_z() {
        // Listener facing +Y, source at (0, 2, 2): local front, elevation 45°,
        // d = 2√2 → g = 1/(1 + 2√2 − 1) = 1/(2√2) = 0.353553.
        // W = g/√2 = 0.25, X = Z = g·cos45°·a = g·½ = 0.176777, Y = 0.
        let bformat = encode_for(
            PathKind::Direct,
            Vec3::ZERO,
            0.0,
            Vec3::new(0.0, 2.0, 2.0),
            FRAC_PI_2,
        );
        assert_close(bformat.w, 0.25, "W");
        assert_close(bformat.y, 0.0, "Y");
        assert_close(bformat.z, 0.176777, "Z");
        assert_close(bformat.x, 0.176777, "X");
    }

    #[test]
    fn reflection_path_encodes_arrival_direction_and_path_length() {
        // Ceiling reflection: arrival direction +Z, unfolded length 3 m:
        // g = 1/(1 + 2) = ⅓ → W = Z = ⅓/√2 = 0.235702, X = Y = 0.
        let ceiling = encode_for(
            PathKind::Reflection,
            Vec3::new(0.0, 0.0, 1.0),
            3.0,
            Vec3::new(1.0, 0.0, 0.0),
            0.3,
        );
        assert_close(ceiling.w, 0.235702, "W");
        assert_close(ceiling.z, 0.235702, "Z");
        assert_close(ceiling.x, 0.0, "X");
        assert_close(ceiling.y, 0.0, "Y");

        // Side-wall reflection arriving from world −X for a listener facing +Y
        // is local left: Y = g·a with g = ½ at 2 m → 0.353553.
        let side = encode_for(
            PathKind::Reflection,
            Vec3::new(-1.0, 0.0, 0.0),
            2.0,
            Vec3::new(1.0, 0.0, 0.0),
            FRAC_PI_2,
        );
        assert_close(side.y, 0.353553, "Y");
        assert_close(side.x, 0.0, "X");
    }

    // ── Pipeline-level checks (renderer → multi-delay → decode) ────────────

    /// Deterministic white noise (LCG).
    struct NoiseSource {
        position: Vec3,
        state: u32,
    }
    impl SoundSource for NoiseSource {
        fn next_sample(&mut self, _sample_rate: f32) -> f32 {
            self.state = self
                .state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            ((self.state >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * 0.25
        }
        fn position(&self) -> Vec3 {
            self.position
        }
        fn tick(&mut self, _dt: f32) {}
    }

    fn itu_speaker(degrees: f32, channel: usize) -> Speaker {
        let azimuth = degrees.to_radians();
        Speaker {
            position: Vec3::new(2.0 * azimuth.cos(), 2.0 * azimuth.sin(), 0.0),
            channel,
        }
    }

    fn itu_5_1() -> SpeakerLayout {
        SpeakerLayout::new(
            &[
                itu_speaker(30.0, 0),
                itu_speaker(-30.0, 1),
                itu_speaker(0.0, 2),
                itu_speaker(110.0, 4),
                itu_speaker(-110.0, 5),
            ],
            Some(3),
            6,
        )
    }

    /// Render 8192 frames of a noise source through the full Ambisonics
    /// pipeline (open walls, listener at the origin facing +X) and return
    /// per-channel energy.
    fn per_channel_energy(layout: &SpeakerLayout, channels: usize, source: Vec3) -> Vec<f32> {
        let environment_min = Vec3::new(-20.0, -20.0, -5.0);
        let environment_max = Vec3::new(20.0, 20.0, 5.0);
        let environment = Environment {
            ground: GroundProperties::default(),
            ..Environment::anechoic(environment_min, environment_max)
        };
        let params = PipelineParams {
            environment_min,
            environment_max,
            ..PipelineParams::default()
        };
        let mut pipeline = builders::ambisonics::build(&params);
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let distance_model = DistanceModel::default();
        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(NoiseSource {
            position: source,
            state: 7,
        })];
        let frames = 8192;
        let mut output = vec![0.0f32; frames * channels];
        let render_params = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &distance_model,
            layout,
            environment: &environment,
            measurement_mode: true,
        };
        render_pipeline(&mut pipeline, &mut sources, &render_params, &mut output);
        (0..channels)
            .map(|channel| {
                output
                    .chunks_exact(channels)
                    .map(|frame| frame[channel] * frame[channel])
                    .sum()
            })
            .collect()
    }

    #[test]
    fn left_source_lands_on_left_speakers_end_to_end() {
        // World +Y is the listener's left (facing +X): L + Ls must dominate
        // R + Rs after encode → decorrelate → AllRAD/EPAD decode.
        let energy = per_channel_energy(&itu_5_1(), 6, Vec3::new(0.0, 2.0, 0.0));
        let left = energy[0] + energy[4];
        let right = energy[1] + energy[5];
        assert!(
            10.0 * (left / right).log10() > 6.0,
            "left {left} should exceed right {right} by > 6 dB"
        );
    }

    #[test]
    fn five_one_layout_on_two_channel_device_is_decoded() {
        // audio/output.rs falls back to the device's 2 channels and masks the
        // rest. Regression: the renderer used to key on speaker_count = 5 ≥ 4
        // and write W, Y into channels 0, 1 while AmbisonicsDecodeStage saw
        // render_channels = 2 and returned early — raw B-format out (front
        // source on L only; right source L = W, R = −W, no lateralisation).
        // The bus has two channels, so `ambisonic_bus` now picks the inline
        // bilateral decode for the renderer and the stages alike.
        let front = per_channel_energy(&itu_5_1(), 2, Vec3::new(2.0, 0.0, 0.0));
        let balance_db = 10.0 * (front[0] / front[1]).log10();
        assert!(
            balance_db.abs() < 1.0,
            "front source L/R balance {balance_db} dB (L {}, R {})",
            front[0],
            front[1]
        );
        let right = per_channel_energy(&itu_5_1(), 2, Vec3::new(0.0, -2.0, 0.0));
        assert!(
            right[1] > 2.0 * right[0],
            "right source: R energy {} should clearly exceed L energy {}",
            right[1],
            right[0]
        );
    }

    #[test]
    fn three_speaker_layout_with_lfe_keeps_sides() {
        // L/R/C + LFE (4 channels). Regression: the renderer used the inline
        // bilateral branch (speaker_count 3 < 4) while the decode stage
        // (render_channels 4, speaker_count ≥ 3) read ch0..3 as W/Y/Z/X, so a
        // right source's large R ear signal became positive Y = "left". Three
        // reachable speakers and four channels: both now see a B-format bus.
        let layout = SpeakerLayout::new(
            &[
                itu_speaker(30.0, 0),
                itu_speaker(-30.0, 1),
                itu_speaker(0.0, 2),
            ],
            Some(3),
            4,
        );
        let energy = per_channel_energy(&layout, 4, Vec3::new(0.0, -2.0, 0.0));
        assert!(
            energy[1] > energy[0],
            "right source: R energy {} should exceed L energy {}",
            energy[1],
            energy[0]
        );
    }

    // ── Bus decision ───────────────────────────────────────────────────────

    /// Expected outcome, compared on channels only (positions follow).
    #[derive(Debug, PartialEq)]
    enum Expected {
        AllRad(Vec<usize>),
        Bilateral(usize, usize),
        InlineBilateral(usize, usize),
    }

    fn outcome(bus: AmbisonicBus) -> Expected {
        match bus {
            AmbisonicBus::BFormat(BusDecoder::AllRad(speakers)) => Expected::AllRad(
                speakers
                    .as_slice()
                    .iter()
                    .map(|speaker| speaker.channel)
                    .collect(),
            ),
            AmbisonicBus::BFormat(BusDecoder::Bilateral(ears)) => {
                Expected::Bilateral(ears.left, ears.right)
            }
            AmbisonicBus::InlineBilateral(ears) => Expected::InlineBilateral(ears.left, ears.right),
        }
    }

    fn masked(mut layout: SpeakerLayout, active_channels: &[usize]) -> SpeakerLayout {
        layout.set_active_channels(active_channels);
        layout
    }

    #[test]
    fn bus_decision_truth_table() {
        let stereo = SpeakerLayout::stereo(Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, -1.0, 0.0));
        let quad = SpeakerLayout::quad(
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(-1.0, 1.0, 0.0),
            Vec3::new(-1.0, -1.0, 0.0),
        );
        let front_three_with_lfe = SpeakerLayout::new(
            &[
                itu_speaker(30.0, 0),
                itu_speaker(-30.0, 1),
                itu_speaker(0.0, 2),
            ],
            Some(3),
            4,
        );
        let front_three = SpeakerLayout::new(
            &[
                itu_speaker(30.0, 0),
                itu_speaker(-30.0, 1),
                itu_speaker(0.0, 2),
            ],
            None,
            3,
        );
        let headphone_host = SpeakerLayout::new(&[], None, 2);

        // (case, layout, render channels, device channels, expected)
        let cases: [(&str, SpeakerLayout, usize, usize, Expected); 14] = [
            (
                "stereo on 2",
                stereo.clone(),
                2,
                2,
                Expected::InlineBilateral(0, 1),
            ),
            (
                "stereo layout on a 6-channel device",
                stereo,
                2,
                6,
                Expected::InlineBilateral(0, 1),
            ),
            (
                "5.1 on 6",
                itu_5_1(),
                6,
                6,
                Expected::AllRad(vec![0, 1, 2, 4, 5]),
            ),
            (
                "5.1 on a 2-channel device",
                itu_5_1(),
                2,
                2,
                Expected::InlineBilateral(0, 1),
            ),
            (
                "5.1 on a 4-channel device: LFE never a decode target",
                itu_5_1(),
                4,
                4,
                Expected::AllRad(vec![0, 1, 2]),
            ),
            (
                "Stereo channel mode on 5.1",
                masked(itu_5_1(), &[0, 1]),
                6,
                6,
                Expected::Bilateral(0, 1),
            ),
            (
                "Quad channel mode on 5.1: centre is not a decode target",
                masked(itu_5_1(), &[0, 1, 4, 5]),
                6,
                6,
                Expected::AllRad(vec![0, 1, 4, 5]),
            ),
            (
                "quad layout on 6",
                quad.clone(),
                6,
                6,
                Expected::AllRad(vec![0, 1, 4, 5]),
            ),
            (
                "quad layout on a 4-channel device: rears unreachable",
                quad,
                4,
                4,
                Expected::Bilateral(0, 1),
            ),
            (
                "L/R/C + LFE on 4",
                front_three_with_lfe,
                4,
                4,
                Expected::AllRad(vec![0, 1, 2]),
            ),
            (
                "L/R/C on 3: no room for W/Y/Z/X",
                front_three,
                3,
                3,
                Expected::InlineBilateral(0, 1),
            ),
            (
                "centre only: both ears on one channel",
                masked(itu_5_1(), &[2]),
                6,
                6,
                Expected::Bilateral(2, 2),
            ),
            (
                "positionless headphone host",
                headphone_host,
                2,
                2,
                Expected::InlineBilateral(0, 1),
            ),
            (
                "render channels bound the bus like device channels",
                itu_5_1(),
                2,
                6,
                Expected::InlineBilateral(0, 1),
            ),
        ];
        for (case, layout, render_channels, device_channels, expected) in cases {
            assert_eq!(
                outcome(ambisonic_bus(&layout, render_channels, device_channels)),
                expected,
                "{case}"
            );
        }
    }

    #[test]
    fn bilateral_ears_are_the_two_lowest_reachable_channels_in_any_speaker_order() {
        // Speakers listed right-before-left: the lower channel is still left.
        let layout = SpeakerLayout::new(&[itu_speaker(-30.0, 5), itu_speaker(30.0, 4)], None, 6);
        assert_eq!(
            outcome(ambisonic_bus(&layout, 6, 6)),
            Expected::Bilateral(4, 5)
        );
    }

    #[test]
    fn decode_speakers_keep_positions() {
        let AmbisonicBus::BFormat(BusDecoder::AllRad(speakers)) =
            ambisonic_bus(&masked(itu_5_1(), &[0, 1, 4, 5]), 6, 6)
        else {
            panic!("quad mode on 5.1 is an AllRAD bus");
        };
        assert_eq!(
            speakers.as_slice(),
            &[
                itu_speaker(30.0, 0),
                itu_speaker(-30.0, 1),
                itu_speaker(110.0, 4),
                itu_speaker(-110.0, 5),
            ]
        );
    }

    #[test]
    fn field_encoding_follows_the_bus() {
        let renderer = AmbisonicsRenderer::new();
        assert_eq!(
            renderer.field_encoding(&itu_5_1(), 6),
            FieldEncoding::Ambisonics
        );
        assert_eq!(
            renderer.field_encoding(&masked(itu_5_1(), &[0, 1]), 6),
            FieldEncoding::Ambisonics,
            "a bilateral decode stage still reads a B-format bus"
        );
        assert_eq!(
            renderer.field_encoding(&itu_5_1(), 2),
            FieldEncoding::Speaker
        );
    }

    #[test]
    fn ambisonics_pipeline_does_not_override_render_channels() {
        // `renderer_bus` relies on render_pipeline handing the stages
        // `layout.total_channels().min(device_channels)`, which holds only
        // while the Ambisonics pipeline leaves render_channels at 0.
        assert_eq!(
            builders::ambisonics::build(&PipelineParams::default()).render_channels,
            0
        );
    }

    #[test]
    fn stereo_channel_mode_on_5_1_images_left_and_right_end_to_end() {
        // Renderer → multi-delay → bilateral decode on 6 channels with only
        // L and R active: a right source lands on R, nothing on the masked
        // channels.
        let layout = masked(itu_5_1(), &[0, 1]);
        let energy = per_channel_energy(&layout, 6, Vec3::new(0.0, -2.0, 0.0));
        assert!(
            energy[1] > 2.0 * energy[0],
            "right source: R energy {} should clearly exceed L energy {}",
            energy[1],
            energy[0]
        );
        for channel in [2, 3, 4, 5] {
            assert_eq!(energy[channel], 0.0, "masked channel {channel}");
        }
    }
}
