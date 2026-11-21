use super::*;
use crate::audio::distance::DistanceModel;
use crate::audio::propagation::GroundProperties;
use crate::pipeline::builders;
use crate::pipeline::environment::Environment;
use crate::pipeline::renderers::ambisonics::BusDecoder;
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
