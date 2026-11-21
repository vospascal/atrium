//! Environmental fields: rendered straight to channels, never through the
//! point-source path.

use super::{
    default_distance_model, energy, layout_5_1, living_room_5_1, test_environment,
    FieldTextureSource, Rig,
};
use crate::pipeline::builders;
use crate::pipeline::wall_material;
use crate::pipeline::{render_pipeline, PipelineParams, RenderParams};
use atrium_core::listener::Listener;
use atrium_core::source::SoundSource;
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

#[test]
fn field_emitter_bypasses_position_and_room_acoustics() {
    let layout = layout_5_1();
    let dm = default_distance_model();
    let listener = Listener::new(Vec3::new(3.0, 2.0, 1.2), 0.0);
    let environment_min = Vec3::ZERO;
    let environment_max = Vec3::new(20.0, 15.0, 8.0);
    let channels = 6;
    let frames = 4096;

    let render = |position: Vec3, wall: wall_material::WallMaterial| {
        let walls: [wall_material::WallMaterial; 6] = std::array::from_fn(|_| wall.clone());
        let pipeline_params = PipelineParams {
            environment_min,
            environment_max,
            reverb_rt60_seconds: Some(3.0),
            ..PipelineParams::default()
        };
        let mut pipeline = builders::vbap::build(&pipeline_params);
        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(FieldTextureSource {
            pos: position,
            rng: crate::synth::noise::Rng::new(42),
        })];
        let mut output = vec![0.0; frames * channels];
        let environment = test_environment(environment_min, environment_max, walls.clone());
        let render_params = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48_000.0,
            master_gain: 1.0,
            distance_model: &dm,
            layout: &layout,
            environment: &environment,
            measurement_mode: true,
        };
        render_pipeline(&mut pipeline, &mut sources, &render_params, &mut output);
        output
    };

    let near_reflective = render(
        Vec3::new(3.1, 2.0, 1.2),
        wall_material::WallMaterial::hard_wall(),
    );
    let far_open = render(
        Vec3::new(19.0, 14.0, 7.0),
        wall_material::WallMaterial::open(),
    );
    assert_eq!(near_reflective, far_open);

    // The field occupies every active spatial channel while LFE remains the
    // responsibility of the downstream crossover.
    for channel in [0, 1, 2, 4, 5] {
        let energy: f32 = near_reflective
            .chunks_exact(channels)
            .map(|frame| frame[channel] * frame[channel])
            .sum();
        assert!(energy > 0.0, "field channel {channel} is silent");
    }
}

/// HRTF renders point sources to channels 0–1 only (`render_channels = 2`),
/// and ambient sound must stay there too. Fields once bypassed the renderer
/// and were spread over every active *speaker* channel of the layout: on a
/// 5.1 layout in HRTF mode, wind and rain leaked into C, Ls and Rs while
/// every object stayed binaural. Now the two ears carry them.
#[test]
fn hrtf_mode_keeps_field_emitters_on_the_binaural_pair() {
    let mut rig = Rig::free_field(48_000.0);
    rig.layout = living_room_5_1();
    rig.channels = rig.layout.total_channels();
    let mut pipeline = rig.build(RenderMode::Hrtf);
    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(FieldTextureSource {
        pos: Vec3::new(1.0, 1.0, 0.0),
        rng: crate::synth::noise::Rng::new(7),
    })];
    let output = rig.render(&mut pipeline, &mut sources, 2048);
    let surround = energy(&output, rig.channels, &[2, 3, 4, 5], 0);
    assert!(
        surround == 0.0,
        "HRTF mode wrote field energy {surround:e} to channels 2–5"
    );
    assert!(
        energy(&output, rig.channels, &[0], 0) > 0.0
            && energy(&output, rig.channels, &[1], 0) > 0.0,
        "the ears carry no field"
    );
}
