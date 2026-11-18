//! Late reverb: tails after the source stops and the critical-distance send law.

use super::{
    default_distance_model, energy, layout_5_1, quad_around, test_environment, uniform_walls,
    BurstSource, Rig, ToneSource,
};
use crate::pipeline::builders;
use crate::pipeline::mix_stage::MixContext;
use crate::pipeline::room_acoustics;
use crate::pipeline::wall_material;
use crate::pipeline::{render_pipeline, PipelineParams, RenderParams};
use atrium_core::listener::Listener;
use atrium_core::source::SoundSource;
use atrium_core::speaker::{RenderMode, SpeakerLayout};
use atrium_core::types::Vec3;

// ── End-to-end: a reflective room produces a reverb tail ─────────────

/// The headline fix, end-to-end: rendered through the default (VBAP) pipeline,
/// a reflective room rings out after the source stops, while an open field
/// (the old `riverside` default) goes essentially silent — proving the tail
/// comes from the environment, not from panning.
#[test]
fn reflective_room_produces_reverb_tail_open_field_does_not() {
    let layout = layout_5_1();
    let dm = default_distance_model();
    let channels = 6;
    let frames = 512;
    let environment_min = Vec3::new(0.0, 0.0, 0.0);
    let environment_max = Vec3::new(8.0, 6.0, 4.0);
    let listener = Listener::new(Vec3::new(4.0, 3.0, 0.0), std::f32::consts::FRAC_PI_2);

    let late_energy =
        |wall: wall_material::WallMaterial, reflectivity: f32, rt60: Option<f32>| -> f32 {
            let wall_materials: [wall_material::WallMaterial; 6] =
                std::array::from_fn(|_| wall.clone());
            let params = PipelineParams {
                er_wall_reflectivity: reflectivity,
                environment_min,
                environment_max,
                reverb_rt60_seconds: rt60,
                ..PipelineParams::default()
            };
            let mut pipeline = builders::vbap::build(&params);

            // render_pipeline does not init mix stages; the FDN needs it to
            // compute damping/pre-delay before it will produce a tail.
            let environment =
                test_environment(environment_min, environment_max, wall_materials.clone());
            let init_ctx = MixContext {
                listener: &listener,
                layout: &layout,
                sample_rate: 48000.0,
                channels,
                master_gain: 1.0,
                render_channels: channels,
                reverb_input: None,
                wall_reflectivity: reflectivity,
                environment: &environment,
                measurement_mode: false,
            };
            pipeline.init(&init_ctx);

            let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(BurstSource {
                pos: Vec3::new(4.0, 4.5, 0.0), // ~1.5 m in front of the listener
                n: 0,
                remaining: frames, // one buffer of tone, then silence
            })];

            // Buffer 0 = tone; buffers 1..7 = silence while the tail rings out.
            let mut late = 0.0f32;
            for b in 0..7 {
                let mut buf = vec![0.0f32; frames * channels];
                let rp = RenderParams {
                    listener: &listener,
                    channels,
                    sample_rate: 48000.0,
                    master_gain: 1.0,
                    distance_model: &dm,
                    layout: &layout,
                    environment: &environment,
                    measurement_mode: false,
                };
                render_pipeline(&mut pipeline, &mut sources, &rp, &mut buf);
                if b >= 3 {
                    // Well after the source stopped: only the tail remains.
                    late += buf.iter().map(|s| s * s).sum::<f32>();
                }
            }
            late
        };

    let reflective = late_energy(wall_material::WallMaterial::hard_wall(), 0.9, Some(2.0));
    let open = late_energy(wall_material::WallMaterial::open(), 0.0, None);

    assert!(
        reflective > 1e-6 && reflective > open * 10.0,
        "reflective room should ring out after the source stops; open field should not: \
         reflective={reflective}, open={open}"
    );
}

/// Stereo (bilateral) Ambisonics must feed the late-reverb send like every
/// other mode. Regression: the 2-channel render branch skipped the reverb
/// send entirely, so the FDN read a zeroed bus and headphone Ambisonics
/// had no tail at all.
#[test]
fn stereo_ambisonics_produces_reverb_tail() {
    let layout = SpeakerLayout::stereo(Vec3::new(3.0, 4.5, 0.0), Vec3::new(5.0, 4.5, 0.0));
    let dm = default_distance_model();
    let channels = 2;
    let frames = 512;
    let environment_min = Vec3::new(0.0, 0.0, 0.0);
    let environment_max = Vec3::new(8.0, 6.0, 4.0);
    let listener = Listener::new(Vec3::new(4.0, 3.0, 0.0), std::f32::consts::FRAC_PI_2);
    let wall_materials: [wall_material::WallMaterial; 6] =
        std::array::from_fn(|_| wall_material::WallMaterial::hard_wall());

    let params = PipelineParams {
        er_wall_reflectivity: 0.9,
        environment_min,
        environment_max,
        reverb_rt60_seconds: Some(2.0),
        ..PipelineParams::default()
    };
    let mut pipeline = builders::ambisonics::build(&params);

    let environment = test_environment(environment_min, environment_max, wall_materials.clone());
    let init_ctx = MixContext {
        listener: &listener,
        layout: &layout,
        sample_rate: 48000.0,
        channels,
        master_gain: 1.0,
        render_channels: channels,
        reverb_input: None,
        wall_reflectivity: 0.9,
        environment: &environment,
        measurement_mode: false,
    };
    pipeline.init(&init_ctx);

    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(BurstSource {
        pos: Vec3::new(4.0, 4.5, 0.0),
        n: 0,
        remaining: frames,
    })];

    let mut late = 0.0f32;
    for b in 0..7 {
        let mut buf = vec![0.0f32; frames * channels];
        let rp = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &dm,
            layout: &layout,
            environment: &environment,
            measurement_mode: false,
        };
        render_pipeline(&mut pipeline, &mut sources, &rp, &mut buf);
        if b >= 3 {
            late += buf.iter().map(|s| s * s).sum::<f32>();
        }
    }

    assert!(
        late > 1e-8,
        "stereo bilateral Ambisonics should ring out through the FDN, got late energy {late}"
    );
}

/// Late-reverb send law: speaker modes inject the direct path into the FDN
/// *before* distance attenuation, scaled by d² / (d² + d_c²). Room 30 × 30
/// × 10 m, broadband reflectivity 0.9:
///   V = 9000 m³, S = 3000 m², Sabine RT60 = 0.161·9000 / (3000·0.1) = 4.83 s
///   d_c = 0.057·√(9000 / 4.83) = 2.4605 m
///   send(2 m) = 0.3978, send(8 m) = 0.9136 → tail 8 m vs 2 m = +7.22 dB.
/// The FDN itself decays with the 1.5 s override; that only scales both
/// tails equally. Heights 0 m keep the ISO ground filter identical for
/// both distances. Window 0.40–0.60 s, after every first-order reflection.
fn late_tail_energy(mode: RenderMode, source_distance: f32) -> f64 {
    let sample_rate = 48_000.0;
    let centre = Vec3::new(15.0, 15.0, 0.0);
    let mut rig = Rig::free_field(sample_rate);
    rig.layout = quad_around(centre);
    rig.listener = Listener::new(centre, std::f32::consts::FRAC_PI_2);
    rig.environment.min = Vec3::ZERO;
    rig.environment.max = Vec3::new(30.0, 30.0, 10.0);
    rig.environment.wall_materials = uniform_walls(wall_material::WallMaterial::hard_wall());
    rig.wall_reflectivity = 0.9;
    rig.reverb_rt60_seconds = Some(1.5);
    let mut pipeline = rig.build(mode);
    let burst = (0.15 * sample_rate) as usize;
    let mut sources = vec![ToneSource::boxed(
        centre + Vec3::new(0.0, source_distance, 0.0),
        1000.0,
        burst,
    )];
    let output = rig.render(&mut pipeline, &mut sources, (0.6 * sample_rate) as usize);
    let all_channels: Vec<usize> = (0..rig.channels).collect();
    let window_start = (0.4 * sample_rate) as usize;
    energy(&output, rig.channels, &all_channels, window_start)
}

fn expected_tail_ratio_db() -> f64 {
    let (volume, surface_area) =
        room_acoustics::room_geometry(Vec3::ZERO, Vec3::new(30.0, 30.0, 10.0));
    let rt60 = room_acoustics::sabine_rt60(volume, surface_area, 0.9);
    let critical = room_acoustics::critical_distance(volume, rt60, 1.0);
    let near = room_acoustics::reverb_send(2.0, critical);
    let far = room_acoustics::reverb_send(8.0, critical);
    20.0 * (far / near).log10() as f64
}

#[test]
fn speaker_modes_late_tail_follows_the_critical_distance_send_law() {
    let expected_db = expected_tail_ratio_db();
    assert!(
        (expected_db - 7.22).abs() < 0.05,
        "hand-derived send ratio is +7.22 dB, helper says {expected_db:.3}"
    );
    for mode in [RenderMode::Vbap, RenderMode::Dbap, RenderMode::Ambisonics] {
        let measured_db =
            10.0 * (late_tail_energy(mode, 8.0) / late_tail_energy(mode, 2.0)).log10();
        assert!(
            (measured_db - expected_db).abs() < 1.0,
            "{mode:?}: late tail 8 m vs 2 m = {measured_db:+.2} dB, send law says {expected_db:+.2} dB"
        );
    }
}

/// HRTF feeds the FDN with the direct path *after* its distance gain
/// (`mono_buf` already carries dist_gain × dir_gain), then scales by the
/// same d²/(d²+d_c²) send — so distance is applied twice. Expected
/// +7.22 dB (as every speaker mode); HRTF measures +7.22 − 20·log10(4)
/// = −4.8 dB: on headphones a far source has *less* reverb than a near one.
#[test]
fn hrtf_late_tail_follows_the_same_send_law_as_speaker_modes() {
    let expected_db = expected_tail_ratio_db();
    let measured_db = 10.0
        * (late_tail_energy(RenderMode::Hrtf, 8.0) / late_tail_energy(RenderMode::Hrtf, 2.0))
            .log10();
    assert!(
        (measured_db - expected_db).abs() < 1.5,
        "HRTF late tail 8 m vs 2 m = {measured_db:+.2} dB, send law says {expected_db:+.2} dB"
    );
}
