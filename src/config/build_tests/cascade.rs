//! Config → engine parameter cascade: every unit conversion between the
//! YAML and the engine, the device channel mask, the air and the shape.

use atrium_core::panner::DistanceModelType;
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

use crate::config::build::BuildResult;
use crate::config::scene::SceneConfig;

// ── Config → engine parameter cascade ───────────────────────────────────

/// A source-less scene (the pool is filled with silent placeholders), so
/// building it decodes no audio and renders no synth preview.
fn build_inline_scene(yaml: &str) -> BuildResult {
    let scene: SceneConfig = serde_yaml::from_str(yaml).expect("inline scene should parse");
    scene.build().expect("inline scene should build")
}

const SMALL_ATRIUM_SCENE: &str = "\
environment: environments/small_atrium.yaml
atmosphere: atmospheres/default.yaml
listener:
  position: [0.5, -0.25, 1.2]
  yaw_degrees: 90
master_gain: 0.5
distance_model:
  model: exponential
  ref_distance: 2.0
  max_distance: 50.0
  rolloff: 1.5
speakers:
  layout: \"5.1\"
  render_mode: dbap
  dbap_rolloff_db: 4.5
sounds: {}
";

/// Every unit conversion between the YAML and the engine, by hand:
///   environments/small_atrium.yaml is 6 × 4 × 3 m with no `spawn`, so the
///   spawn is the floor centre (3, 2, 0) and the bounds are (0,0,0)–(6,4,3);
///   listener (0.5, −0.25, 1.2) atrium-local → (3.5, 1.75, 1.2) world;
///   yaw 90° → π/2 rad; default FL (−3, 2, 0) → (0, 4, 0) world;
///   walls default hard_wall, floor carpet, ceiling ceiling_tile;
///   no rt60_seconds, so no pipeline overrides the decay; ground_factor 0 → G = 0;
///   atmosphere 20 °C, 50 % RH, 101.325 kPa → c = 343.42 m/s.
#[test]
fn scene_config_units_reach_the_engine() {
    let result = build_inline_scene(SMALL_ATRIUM_SCENE);
    let scene = &result.scene;

    let (environment_min, environment_max) = (scene.environment.min, scene.environment.max);
    assert_eq!(environment_min, Vec3::ZERO);
    assert_eq!(environment_max, Vec3::new(6.0, 4.0, 3.0));

    assert_eq!(scene.listener.position, Vec3::new(3.5, 1.75, 1.2));
    assert!((scene.listener.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    assert_eq!(scene.initial_listener_yaw, scene.listener.yaw);
    let forward = scene.listener.forward();
    assert!(
        forward.x.abs() < 1e-6 && (forward.y - 1.0).abs() < 1e-6,
        "yaw 90° must face +y, faces {forward:?}"
    );

    let front_left = scene
        .speaker_layout
        .speaker_by_index(0)
        .expect("5.1 has a front-left speaker");
    assert_eq!(front_left.channel, 0);
    assert_eq!(front_left.position, Vec3::new(0.0, 4.0, 0.0));
    assert_eq!(scene.speaker_layout.total_channels(), 6);
    assert_eq!(scene.speaker_layout.lfe_channel(), Some(3));

    assert_eq!(scene.master_gain, 0.5);
    assert_eq!(scene.active_pipeline, RenderMode::Dbap);
    assert!(matches!(
        scene.distance_model.model,
        DistanceModelType::Exponential
    ));
    assert_eq!(scene.distance_model.ref_distance, 2.0);
    assert_eq!(scene.distance_model.max_distance, 50.0);
    assert_eq!(scene.distance_model.rolloff, 1.5);

    let names: Vec<&str> = scene
        .environment
        .wall_materials
        .iter()
        .map(|wall| wall.name)
        .collect();
    assert_eq!(
        names,
        [
            "hard_wall",
            "hard_wall",
            "hard_wall",
            "hard_wall",
            "carpet",
            "ceiling_tile"
        ]
    );
    // small_atrium.yaml sets no rt60_seconds: every pipeline derives the
    // reverb's decay from the wall materials.
    for pipeline in &scene.pipelines {
        assert_eq!(pipeline.reverb_rt60_override, None);
    }
    assert_eq!(scene.environment.ground.g_source, 0.0);
    assert_eq!(scene.environment.ground.g_middle, 0.0);
    assert_eq!(scene.environment.ground.g_receiver, 0.0);

    assert_eq!(scene.environment.atmosphere.temperature_c, 20.0);
    assert_eq!(scene.environment.atmosphere.humidity_pct, 50.0);
    assert_eq!(scene.environment.atmosphere.pressure_kpa, 101.325);
    assert!((scene.environment.atmosphere.speed_of_sound() - 343.42).abs() < 1e-3);

    // The 16-slot pool is pre-warmed with silent placeholders.
    assert_eq!(scene.sources.len(), atrium_core::telemetry::MAX_SOURCES);
    assert!(scene.sources.iter().all(|source| !source.is_active()));
}

/// `output.rs` masks the layout channels the device cannot open (5.1
/// scene on a 2-channel device → active [0, 1]). `Command::ResetScene`
/// then re-enables the full 5.1 mask, so VBAP/DBAP route rear sources to
/// channels 4/5 that the 2-channel callback never writes: they go silent.
#[test]
fn reset_scene_keeps_the_device_channel_mask() {
    let mut result = build_inline_scene(SMALL_ATRIUM_SCENE);
    let scene = &mut result.scene;
    // What output.rs does when the device opened with 2 channels.
    scene.fit_to_output_device(2);

    let (mut producer, mut consumer) = rtrb::RingBuffer::new(4);
    producer
        .push(atrium_core::commands::Command::ResetScene)
        .unwrap();
    scene.process_commands(&mut consumer);

    for channel in 2..scene.speaker_layout.total_channels() {
        assert!(
            !scene.speaker_layout.is_channel_active(channel),
            "ResetScene re-enabled channel {channel}, which the 2-channel device does not have"
        );
    }
}

/// Asking for 5.1 on a 2-channel device must not switch on the centre, LFE
/// and rear speakers: the callback never writes those channels, so whatever
/// is panned to them is lost. The mode is recorded as asked; the speakers
/// stay limited to what the device has. A 6-channel device gets all six.
#[test]
fn channel_mode_never_switches_on_channels_the_device_lacks() {
    use atrium_core::commands::Command;
    use atrium_core::speaker::ChannelMode;

    let active_channels_on = |device_channels: usize| -> Vec<usize> {
        let mut result = build_inline_scene(SMALL_ATRIUM_SCENE);
        let scene = &mut result.scene;
        scene.fit_to_output_device(device_channels);
        let (mut producer, mut consumer) = rtrb::RingBuffer::new(4);
        producer
            .push(Command::SetChannelMode {
                mode: ChannelMode::Surround51,
            })
            .unwrap();
        scene.process_commands(&mut consumer);
        assert_eq!(scene.active_channel_mode, ChannelMode::Surround51);
        (0..scene.speaker_layout.total_channels())
            .filter(|channel| scene.speaker_layout.is_channel_active(*channel))
            .collect()
    };

    assert_eq!(active_channels_on(2), vec![0, 1]);
    assert_eq!(active_channels_on(6), vec![0, 1, 2, 3, 4, 5]);
}

/// A scene can hold its own air in place of an atmosphere file: 8 °C gives
/// c = 331.3 + 0.606 × 8 = 336.15 m/s. It is written back as it was read,
/// and `set_air` keeps the pressure it had.
#[test]
fn a_scene_holds_its_own_air() {
    let yaml = SMALL_ATRIUM_SCENE.replace(
        "atmosphere: atmospheres/default.yaml",
        "atmosphere: { temperature_c: 8, humidity_pct: 85, pressure_kpa: 99.0 }",
    );
    let result = build_inline_scene(&yaml);
    let air = result.scene.environment.atmosphere;
    assert_eq!(
        (air.temperature_c, air.humidity_pct, air.pressure_kpa),
        (8.0, 85.0, 99.0)
    );
    assert!((air.speed_of_sound() - 336.15).abs() < 0.2);
    assert_eq!(
        (
            result.description.temperature_c,
            result.description.humidity_pct
        ),
        (8.0, 85.0)
    );

    let mut scene: SceneConfig = serde_yaml::from_str(&yaml).expect("parses");
    scene.set_air(-3.0, 40.0).expect("air");
    let written = serde_yaml::to_string(&scene).expect("writes");
    let read: SceneConfig = serde_yaml::from_str(&written).expect("reads back");
    assert_eq!(
        read.atmosphere,
        Some(crate::config::Atmosphere::Air(
            crate::config::AtmosphereConfig {
                temperature_c: -3.0,
                humidity_pct: 40.0,
                pressure_kpa: 99.0,
            }
        ))
    );
}

/// `shape.ambient` makes a recording a field, and a wind a point: the frog
/// is then heard at its level wherever the listener stands, so telemetry
/// reports no distance loss for it, and the wind 10 m off loses 20 dB.
#[test]
fn any_sound_can_be_ambient_or_a_point() {
    let yaml = SMALL_ATRIUM_SCENE
        .replace(
            "sounds: {}",
            "sounds:
  frogs:
    sound: { preset: sources/creatures/frog.yaml }
    shape: { ambient: true }
    place: [10.0, 0.0, 1.2]
  wind:
    sound: { preset: sources/weather/wind/field-wind.yaml }
    shape: { ambient: false }
    place: [10.5, -0.25, 1.2]
",
        )
        .replace("model: exponential", "model: inverse")
        .replace("ref_distance: 2.0", "ref_distance: 1.0")
        .replace("rolloff: 1.5", "rolloff: 1.0");
    let result = build_inline_scene(&yaml);
    use atrium_core::source::EmitterKind;
    assert_eq!(result.scene.sources[0].emitter_kind(), EmitterKind::Field);
    assert_eq!(result.scene.sources[1].emitter_kind(), EmitterKind::Object);
    assert_eq!(
        result.description.sources[0].emitter_kind,
        EmitterKind::Field
    );
    assert_eq!(result.description.sources[0].beam_half_angle, None);

    let frame = crate::engine::telemetry::compute_telemetry(
        &result.scene.sources,
        &result.scene.listener,
        &result.scene.distance_model,
    );
    assert!(
        frame.sources[0].gain_db.abs() < 1e-3,
        "a field loses nothing"
    );
    assert!(
        (frame.sources[1].gain_db + 20.0).abs() < 0.1,
        "a point 10 m off loses 20 dB, got {}",
        frame.sources[1].gain_db
    );
}

/// `shape.arc` reaches the playing sound and the scene description; left
/// out it is all round, and an arc narrower than 10° or wider than all
/// round is refused rather than bent into range.
#[test]
fn an_ambient_sound_takes_its_arc_from_the_scene() {
    let scene_with = |shape: &str| {
        SMALL_ATRIUM_SCENE.replace(
            "sounds: {}",
            &format!(
                "sounds:
  shore:
    sound: {{ preset: sources/weather/wind/field-wind.yaml }}
    shape: {shape}
    place: [0.0, 10.0, 1.2]
"
            ),
        )
    };
    let narrowed = build_inline_scene(&scene_with("{ arc: 90 }"));
    assert_eq!(narrowed.scene.sources[0].arc_degrees(), 90.0);
    assert_eq!(narrowed.description.sources[0].arc, 90.0);
    let all_round = build_inline_scene(&scene_with("{ size: 0.5 }"));
    assert_eq!(all_round.scene.sources[0].arc_degrees(), 360.0);
    assert_eq!(all_round.description.sources[0].arc, 360.0);
    for refused in ["{ arc: 5 }", "{ arc: 361 }"] {
        let config: SceneConfig = serde_yaml::from_str(&scene_with(refused)).expect("parses");
        let error = config.build().err().expect("refused").to_string();
        assert!(error.contains("`shape.arc`"), "{refused} gave {error}");
    }
}

/// A name the scene file gets wrong is said, not played as something else:
/// before, an unknown render mode became VBAP, an unknown distance law the
/// inverse one, and an unknown speaker layout a stereo pair, all silently.
#[test]
fn an_unknown_name_in_a_scene_file_is_refused() {
    let refused = |from: &str, to: &str| {
        let yaml = SMALL_ATRIUM_SCENE.replace(from, to);
        assert_ne!(yaml, SMALL_ATRIUM_SCENE, "the scene names {from}");
        let scene: SceneConfig = serde_yaml::from_str(&yaml).expect("parses");
        match scene.build() {
            Ok(_) => panic!("{to} was accepted"),
            Err(error) => error.to_string(),
        }
    };
    assert_eq!(
        refused("render_mode: dbap", "render_mode: dbpa"),
        "speakers.render_mode `dbpa` is not one of world_locked, vbap, hrtf, dbap, ambisonics"
    );
    assert_eq!(
        refused("model: exponential", "model: exponentail"),
        "distance_model.model `exponentail` is not one of linear, inverse, exponential"
    );
    assert_eq!(
        refused("layout: \"5.1\"", "layout: surround"),
        "speakers.layout `surround` is not one of 5.1, quad, stereo"
    );
}

/// A probe on one output channel silences every other, after everything
/// else, and letting everything through again brings them back. The command
/// reaches every pipeline, so it holds across a change of render mode.
#[test]
fn one_channel_can_be_heard_alone() {
    use atrium_core::commands::Command;
    use atrium_core::probe::Probe;

    let yaml = SMALL_ATRIUM_SCENE.replace(
        "sounds: {}",
        "sounds:
  frogs:
    sound: { preset: sources/creatures/frog.yaml }
    when: { gap: [0, 0] }
    place: [1.0, 1.0, 1.2]
",
    );
    let mut scene = build_inline_scene(&yaml).scene;
    scene.sample_rate = 48_000.0;
    scene.init_pipelines();
    let channels = scene.speaker_layout.total_channels();
    let (mut commands, mut inbox) = rtrb::RingBuffer::<Command>::new(8);
    let mut energies = |scene: &mut crate::engine::scene::AudioScene| {
        let mut buffer = vec![0.0f32; 512 * channels];
        let mut energy = vec![0.0f64; channels];
        for _ in 0..40 {
            scene.process_commands(&mut inbox);
            scene.render(&mut buffer, channels);
            for frame in buffer.chunks_exact(channels) {
                for (channel, sample) in frame.iter().enumerate() {
                    energy[channel] += (*sample as f64).powi(2);
                }
            }
        }
        energy
    };

    let whole = energies(&mut scene);
    let sounding: Vec<usize> = (0..channels)
        .filter(|channel| whole[*channel] > 0.0)
        .collect();
    assert!(sounding.len() >= 3, "the frogs reach {sounding:?} only");

    let only = sounding[1] as u8;
    commands
        .push(Command::SetProbe {
            probe: Probe {
                only_channel: Some(only),
                ..Probe::WHOLE
            },
        })
        .expect("sent");
    let alone = energies(&mut scene);
    for (channel, energy) in alone.iter().enumerate() {
        assert_eq!(*energy > 0.0, channel == only as usize, "channel {channel}");
    }
    assert!(scene
        .pipelines
        .iter()
        .all(|pipeline| pipeline.probe.only_channel == Some(only)));

    commands
        .push(Command::SetProbe {
            probe: Probe::WHOLE,
        })
        .expect("sent");
    let again = energies(&mut scene);
    for channel in &sounding {
        assert!(again[*channel] > 0.0, "channel {channel} did not come back");
    }
}
