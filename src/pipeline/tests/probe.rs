//! Listening to parts of the render alone: the parts add up to the whole,
//! and an effect switched off is really gone.

use super::{
    build_mode, energy, quad_around, uniform_walls, BurstSource, FieldTextureSource, Rig,
    ToneSource,
};
use crate::audio::turbulence::AirMotion;
use crate::pipeline::wall_material::WallMaterial;
use crate::pipeline::PipelineParams;
use atrium_core::probe::Probe;
use atrium_core::source::SoundSource;
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

const MODES: [RenderMode; 5] = [
    RenderMode::WorldLocked,
    RenderMode::Vbap,
    RenderMode::Hrtf,
    RenderMode::Dbap,
    RenderMode::Ambisonics,
];

/// A glass room 8 × 6 × 4 m around the listener: it reflects and it rings.
fn glass_room(sample_rate: f32) -> Rig {
    let mut rig = Rig::free_field(sample_rate);
    rig.environment.min = Vec3::new(-4.0, -3.0, -1.2);
    rig.environment.max = Vec3::new(4.0, 3.0, 2.8);
    rig.environment.wall_materials = uniform_walls(WallMaterial::glass());
    rig.layout = quad_around(Vec3::ZERO);
    rig
}

/// An ambient texture from a 90° arc towards `place`.
struct ArcTexture {
    rng: crate::synth::noise::Rng,
    place: Vec3,
}

impl SoundSource for ArcTexture {
    fn next_sample(&mut self, _sample_rate: f32) -> f32 {
        self.rng.next_bipolar() * 0.25
    }
    fn position(&self) -> Vec3 {
        self.place
    }
    fn emitter_kind(&self) -> atrium_core::source::EmitterKind {
        atrium_core::source::EmitterKind::Field
    }
    fn arc_degrees(&self) -> f32 {
        90.0
    }
    fn tick(&mut self, _dt: f32) {}
}

/// A tone burst in a corner of the room, an ambient texture all round and
/// another from an arc: something in every part of the render.
fn a_point_and_a_field() -> Vec<Box<dyn SoundSource>> {
    vec![
        Box::new(BurstSource {
            pos: Vec3::new(2.5, 1.5, 0.3),
            n: 0,
            remaining: 28_800,
        }),
        Box::new(FieldTextureSource {
            pos: Vec3::ZERO,
            rng: crate::synth::noise::Rng::new(5),
        }),
        Box::new(ArcTexture {
            rng: crate::synth::noise::Rng::new(6),
            place: Vec3::new(-10.0, 3.0, 0.0),
        }),
    ]
}

fn render_with(rig: &Rig, mode: RenderMode, probe: Probe, frames: usize) -> Vec<f32> {
    let mut pipeline = rig.build(mode);
    pipeline.probe = probe;
    let mut sources = a_point_and_a_field();
    pipeline.ensure_topology(sources.len(), &rig.layout, rig.sample_rate);
    pipeline.prepare_sources(&sources);
    rig.render(&mut pipeline, &mut sources, frames)
}

const NOTHING: Probe = Probe {
    direct: false,
    reflections: false,
    reverb: false,
    ambient: false,
    ..Probe::WHOLE
};

/// The four parts, each heard alone, add up to the whole, sample for sample,
/// in every render mode: a probe takes parts away and changes nothing else.
/// The reverb heard alone is still fed by the direct sound it hides.
///
/// Compared from the first sample: a part hidden before the pipeline first
/// renders has never been heard, so there is nothing to fade out.
#[test]
fn the_parts_heard_alone_add_up_to_the_whole() {
    let rig = glass_room(48_000.0);
    let frames = 48_000;
    for mode in MODES {
        let whole = render_with(&rig, mode, Probe::WHOLE, frames);
        let parts = [
            Probe {
                direct: true,
                ..NOTHING
            },
            Probe {
                reflections: true,
                ..NOTHING
            },
            Probe {
                reverb: true,
                ..NOTHING
            },
            Probe {
                ambient: true,
                ..NOTHING
            },
        ]
        .map(|probe| render_with(&rig, mode, probe, frames));

        let peak = whole
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        assert!(peak > 0.01, "{mode:?}: the whole is silent");
        for index in 0..whole.len() {
            let sum: f32 = parts.iter().map(|part| part[index]).sum();
            assert!(
                (sum - whole[index]).abs() < 2e-4 * peak.max(1.0),
                "{mode:?}, sample {index}: the parts give {sum}, the whole {}",
                whole[index]
            );
        }

        let all: Vec<usize> = (0..rig.channels).collect();
        let heard = |part: &[f32]| energy(part, rig.channels, &all, 0);
        assert!(heard(&parts[0]) > 0.0, "{mode:?}: no direct sound");
        assert!(heard(&parts[1]) > 0.0, "{mode:?}: no reflections");
        assert!(heard(&parts[3]) > 0.0, "{mode:?}: no ambient sound");
        // World-locked has no late reverb; every other mode rings.
        let rings = mode != RenderMode::WorldLocked;
        assert_eq!(heard(&parts[2]) > 0.0, rings, "{mode:?}: late reverb");
        assert_eq!(
            heard(&render_with(&rig, mode, NOTHING, frames)),
            0.0,
            "{mode:?}: with every part hidden something is still heard"
        );
    }
}

/// The level of an 8 kHz tone 50 m off in the open, in dB, through `probe`.
fn far_treble_db(rig: &Rig, mode: RenderMode, probe: Probe) -> f64 {
    let mut pipeline = rig.build(mode);
    pipeline.probe = probe;
    let frames = 48_000;
    let mut sources = vec![ToneSource::boxed(Vec3::new(0.0, 50.0, 1.5), 8000.0, frames)];
    let output = rig.render(&mut pipeline, &mut sources, frames);
    let all: Vec<usize> = (0..rig.channels).collect();
    10.0 * energy(&output, rig.channels, &all, frames / 2).log10()
}

/// Switching the air's absorption off gives back the treble the air took:
/// at 8 kHz, 20 °C and 50 % humidity the air takes some 0.1 dB a metre, so
/// 50 m of it more than 3 dB. The ground's effect off changes the level too.
#[test]
fn an_effect_switched_off_is_gone_from_the_sound() {
    let rig = Rig::free_field(48_000.0);
    for mode in [RenderMode::Vbap, RenderMode::WorldLocked, RenderMode::Hrtf] {
        let whole = far_treble_db(&rig, mode, Probe::WHOLE);
        let without_air = far_treble_db(
            &rig,
            mode,
            Probe {
                air_absorption: false,
                ..Probe::WHOLE
            },
        );
        assert!(
            without_air - whole > 3.0,
            "{mode:?}: only {} dB came back",
            without_air - whole
        );
        let without_ground = far_treble_db(
            &rig,
            mode,
            Probe {
                ground_effect: false,
                ..Probe::WHOLE
            },
        );
        assert!(
            (without_ground - whole).abs() > 0.1,
            "{mode:?}: the ground made no difference"
        );
    }
}

/// With the moving air switched off, a windy day sounds exactly like a still
/// one: every sample the same.
#[test]
fn moving_air_switched_off_is_still_air() {
    for mode in [RenderMode::Vbap, RenderMode::WorldLocked] {
        let still = Rig::free_field(48_000.0);
        let mut windy = Rig::free_field(48_000.0);
        windy.environment.air = AirMotion::from_weather(0.6, 1.0, 270.0);
        let render = |rig: &Rig, probe: Probe| {
            let mut pipeline = rig.build(mode);
            pipeline.probe = probe;
            let mut sources = vec![ToneSource::boxed(Vec3::new(0.0, 40.0, 1.5), 4000.0, 24_000)];
            rig.render(&mut pipeline, &mut sources, 24_000)
        };
        let reference = render(&still, Probe::WHOLE);
        assert_ne!(
            render(&windy, Probe::WHOLE),
            reference,
            "{mode:?}: the wind did nothing"
        );
        let switched_off = render(
            &windy,
            Probe {
                moving_air: false,
                ..Probe::WHOLE
            },
        );
        assert_eq!(switched_off, reference, "{mode:?}");
    }
}

/// The near field is not a path effect but the headphone renderer's own: a
/// correction to each ear. Switched off, a source close by sounds exactly
/// as the measured HRIRs alone render it; on, the headphones change and no
/// other mode does, since no other mode has ears. Every probe switch is one
/// of these or a path effect below.
#[test]
fn the_near_field_is_the_headphones_alone() {
    let rig = glass_room(48_000.0);
    for mode in MODES {
        let render = |probe: Probe| {
            let mut pipeline = rig.build(mode);
            pipeline.probe = probe;
            let mut sources = vec![ToneSource::boxed(Vec3::new(0.25, 0.1, 0.0), 300.0, 24_000)];
            rig.render(&mut pipeline, &mut sources, 24_000)
        };
        let off = render(Probe {
            near_field: false,
            ..Probe::WHOLE
        });
        assert_eq!(
            render(Probe::WHOLE) != off,
            mode == RenderMode::Hrtf,
            "{mode:?}: the near field switch"
        );
    }
}

/// The headphone equalisation is the headphone renderer's too: switched
/// off, the headphones change (a point source and an ambient sound alike)
/// and no other mode does.
#[test]
fn the_headphone_equalisation_is_the_headphones_alone() {
    let rig = glass_room(48_000.0);
    for mode in MODES {
        let off = render_with(
            &rig,
            mode,
            Probe {
                headphone_equalisation: false,
                ..Probe::WHOLE
            },
            24_000,
        );
        assert_eq!(
            render_with(&rig, mode, Probe::WHOLE, 24_000) != off,
            mode == RenderMode::Hrtf,
            "{mode:?}: the headphone equalisation switch"
        );
    }
}

/// Every effect a path can have is one the probe knows how to switch, or the
/// delay, which it never does. A new effect has to be decided here.
#[test]
fn every_path_effect_is_known_to_the_probe() {
    for mode in MODES {
        let pipeline = build_mode(mode, &PipelineParams::default());
        for factory in &pipeline.path_effect_factories {
            let effect = factory(48_000.0);
            assert!(
                [
                    "propagation_delay",
                    "air_absorption",
                    "turbulence",
                    "ground_effect",
                    "wall_absorption"
                ]
                .contains(&effect.name()),
                "{mode:?}: `{}` is not handled by PathEffectChain::listen",
                effect.name()
            );
        }
    }
}
