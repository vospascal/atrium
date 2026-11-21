use atrium_core::panner::DistanceModelType;
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

use super::build::BuildResult;
use super::scene::SceneConfig;

/// The synth test scene must build end-to-end with calibrated sources.
#[test]
fn synth_test_scene_builds_with_calibrated_sources() {
    let scene = SceneConfig::load("scenes/synth-test.yaml").expect("scene should load");
    let result = scene.build().expect("scene should build");
    let expected = ["Field Wind", "Waves", "Rain v2", "Rain v1"];
    assert_eq!(&result.source_names[..expected.len()], &expected);
    for (index, name) in expected.iter().enumerate() {
        assert!(
            result.scene.source_amplitudes[index] > 0.0,
            "source {name} should have positive amplitude"
        );
    }
}

/// The description a UI draws must place every source exactly where the
/// engine renders it, and match the calibration the engine was built with.
#[test]
fn description_matches_the_built_scene() {
    let result = SceneConfig::load("scenes/frog.yaml")
        .expect("scene should load")
        .build()
        .expect("scene should build");
    let description = &result.description;

    assert_eq!(description.listener, result.scene.listener.position);
    assert_eq!(description.sources.len(), result.source_names.len());
    for (slot, source) in description.sources.iter().enumerate() {
        assert_eq!(source.name, result.source_names[slot]);
        assert_eq!(
            source.emitter_kind,
            result.scene.sources[slot].emitter_kind()
        );
        assert_eq!(
            source.position,
            result.scene.sources[slot].position(),
            "slot {slot} ({}) drawn away from where it is rendered",
            source.name
        );
        assert!(source.spl.is_finite() && source.reference_distance > 0.0);
    }
    assert!(description.width > 0.0 && description.depth > 0.0);
}

#[test]
fn canopy_wind_scene_builds_end_to_end() {
    let scene = SceneConfig::load("scenes/canopy-wind-only.yaml").expect("scene should load");
    let result = scene.build().expect("canopy scene should build");
    assert_eq!(
        result.source_names.first().map(String::as_str),
        Some("Canopy Wind")
    );
    assert!(result.scene.source_amplitudes[0] > 0.0);
    assert_eq!(
        result.scene.sources[0].emitter_kind(),
        atrium_core::source::EmitterKind::Field
    );
}

#[test]
fn storm_wind_scene_builds_end_to_end() {
    let scene = SceneConfig::load("scenes/storm-wind-only.yaml").expect("scene should load");
    let result = scene.build().expect("storm scene should build");
    assert_eq!(
        result.source_names.first().map(String::as_str),
        Some("Storm Wind")
    );
    assert!(result.scene.source_amplitudes[0] > 0.0);
    assert_eq!(
        result.scene.sources[0].emitter_kind(),
        atrium_core::source::EmitterKind::Field
    );
}

#[test]
fn soft_wind_scene_builds_end_to_end() {
    let scene = SceneConfig::load("scenes/soft-wind-only.yaml").expect("scene should load");
    let result = scene.build().expect("soft-wind scene should build");
    assert_eq!(
        result.source_names.first().map(String::as_str),
        Some("Soft Wind")
    );
    assert!(result.scene.source_amplitudes[0] > 0.0);
    assert_eq!(
        result.scene.sources[0].emitter_kind(),
        atrium_core::source::EmitterKind::Field
    );
    assert_eq!(
        result.description.sources[0].emitter_kind,
        atrium_core::source::EmitterKind::Field
    );
}

#[test]
fn river_scene_builds_end_to_end() {
    let scene = SceneConfig::load("scenes/river-only.yaml").expect("scene should load");
    let result = scene.build().expect("river scene should build");
    assert_eq!(
        result.source_names.first().map(String::as_str),
        Some("River")
    );
    assert!(result.scene.source_amplitudes[0] > 0.0);
    assert_eq!(
        result.scene.sources[0].emitter_kind(),
        atrium_core::source::EmitterKind::Object
    );
}

/// The meadow names 5.1 speakers. Asked for headphones it is the same six
/// sounds on two channels, rendered binaurally.
#[test]
fn a_speaker_scene_builds_binaural_stereo_for_headphones() {
    let scene = SceneConfig::load("scenes/nature.yaml").expect("scene should load");
    let on_speakers = scene.clone().build().expect("nature scene should build");
    assert_eq!(on_speakers.scene.speaker_layout.total_channels(), 6);

    let result = scene
        .for_headphones()
        .build()
        .expect("nature scene should build for headphones");
    assert_eq!(result.scene.active_pipeline, RenderMode::Hrtf);
    assert_eq!(result.scene.speaker_layout.total_channels(), 2);
    assert_eq!(result.source_names.len(), 6);
}

/// Render a scene in one mode, then silence every source. Returns the
/// output power while playing and at 0.3 s after silence (sum over all
/// channels, averaged over a 10 ms block).
fn playing_and_tail_power(scene_path: &str, mode: atrium_core::speaker::RenderMode) -> (f32, f32) {
    let scene = SceneConfig::load(scene_path).expect("scene should load");
    let mut result = scene.build().expect("scene should build");
    let channels = result.scene.speaker_layout.total_channels();
    result.scene.sample_rate = 48000.0;
    result.scene.active_pipeline = mode;
    result.scene.init_pipelines();
    let frames = 480;
    let mut buffer = vec![0.0f32; frames * channels];
    let power = |block: &[f32]| block.iter().map(|x| x * x).sum::<f32>() / frames as f32;

    let mut playing = 0.0f32;
    for block in 0..80 {
        result.scene.render(&mut buffer, channels);
        if block >= 30 {
            playing += power(&buffer) / 50.0;
        }
    }
    for source in result.scene.sources.iter_mut() {
        source.set_muted(true);
    }
    // 0.3 s is far longer than any sound can still be in the air: the
    // farthest meadow source is 19 m away (55 ms).
    for _ in 0..30 {
        result.scene.render(&mut buffer, channels);
    }
    result.scene.render(&mut buffer, channels);
    (playing, power(&buffer))
}

/// An open meadow has no reverb tail, in every render mode. Before the
/// fix the 5.1 scene was still only 25 dB down 0.3 s after silence, with a
/// 1.2 s decay that Sabine's equation invents for a fully absorbing box.
#[test]
fn open_meadow_is_silent_after_sources_stop_in_every_mode() {
    for mode in atrium_core::speaker::RenderMode::ALL {
        let (playing, tail) = playing_and_tail_power("scenes/nature.yaml", mode);
        assert!(playing > 0.0, "{mode:?}: scene should produce sound");
        let tail_db = 10.0 * (tail / playing).max(1e-20).log10();
        assert!(
            tail_db < -100.0,
            "{mode:?}: open field still ringing 0.3 s after silence: {tail_db:.1} dB re playing"
        );
    }
}

/// The glass atrium must still ring: the gate removes reverb only where
/// the space cannot hold it. WorldLocked is excluded — it has no late
/// reverb stage by design, only per-speaker early reflections.
#[test]
fn glass_atrium_still_rings_after_sources_stop() {
    use atrium_core::speaker::RenderMode;
    for mode in RenderMode::ALL {
        if mode == RenderMode::WorldLocked {
            continue;
        }
        let (playing, tail) = playing_and_tail_power("scenes/default.yaml", mode);
        let tail_db = 10.0 * (tail / playing).max(1e-20).log10();
        assert!(
            tail_db > -60.0,
            "{mode:?}: atrium tail missing 0.3 s after silence: {tail_db:.1} dB re playing"
        );
    }
}

/// The wisp flies by itself: rendered through a real scene, its reported
/// position moves, stays near its territory, and it makes sound both on
/// its speakers and on headphones.
#[test]
fn the_wisp_scene_builds_flies_and_sounds() {
    for headphones in [false, true] {
        let path = if headphones {
            "wisp on headphones"
        } else {
            "wisp on speakers"
        };
        let scene = SceneConfig::load("scenes/wisp.yaml").expect("scene should load");
        let scene = if headphones {
            scene.for_headphones()
        } else {
            scene
        };
        let mut result = scene.build().expect("wisp scene should build");
        assert_eq!(
            result.source_names.first().map(String::as_str),
            Some("Wisp")
        );
        let channels = result.scene.speaker_layout.total_channels();
        result.scene.sample_rate = 48000.0;
        result.scene.init_pipelines();

        let anchor = result.scene.sources[0].position() - result.scene.sources[0].motion_offset();
        let start = result.scene.sources[0].position();
        let mut buffer = vec![0.0f32; 480 * channels];
        let mut energy = 0.0f32;
        let mut farthest = 0.0f32;
        let mut travelled = 0.0f32;
        let mut previous = start;
        for _ in 0..1500 {
            result.scene.render(&mut buffer, channels);
            energy += buffer.iter().map(|x| x * x).sum::<f32>();
            assert!(
                buffer.iter().all(|x| x.is_finite()),
                "{path}: non-finite output"
            );
            let position = result.scene.sources[0].position();
            travelled += position.distance_to(previous);
            farthest = farthest.max(position.distance_to(anchor));
            previous = position;
        }
        assert!(energy > 0.0, "{path}: wisp is silent");
        assert!(
            travelled > 10.0,
            "{path}: wisp flew only {travelled:.1} m in 15 s"
        );
        // Territory radius 6 m, height up to 3 m, plus overshoot.
        assert!(
            farthest < 7.5,
            "{path}: wisp strayed {farthest:.1} m from its anchor"
        );
    }
}

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

/// Every scene in the repository reads in the current layout. The engine has
/// no reader for an older one, so a scene left behind would fail here by name
/// instead of at the moment someone tries to play it.
#[test]
fn every_repository_scene_reads_and_names_its_sounds() {
    let mut scenes: Vec<_> = std::fs::read_dir("scenes")
        .expect("the scenes directory")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "yaml")
        })
        .collect();
    scenes.sort();
    assert!(scenes.len() >= 11, "found only {} scenes", scenes.len());
    for path in scenes {
        let path = path.to_str().expect("a UTF-8 path");
        let config = SceneConfig::load(path).unwrap_or_else(|error| panic!("{path}: {error}"));
        assert!(!config.sounds.is_empty(), "{path} has no sounds");
        for (id, entity) in config.sounds.iter() {
            assert!(
                entity.group.is_some(),
                "{path}: sound `{id}` has no group for the layer list"
            );
        }
    }
}

/// The meadow of rests birds builds: each bird kind reports its takes by
/// name, utters and rests by itself, and no two birds utter the same sequence.
/// Rendered without an audio device, at 48 kHz for two minutes.
#[test]
fn the_meadow_birds_sing_their_own_takes_with_rests() {
    let scene = SceneConfig::load("scenes/meadow-birds.yaml").expect("scene should load");
    let mut result = scene.build().expect("scene should build");
    let birds: Vec<_> = result
        .description
        .sources
        .iter()
        .filter(|source| source.takes.len() > 1)
        .map(|source| {
            (
                source.name.clone(),
                source.slot as usize,
                source.takes.len(),
            )
        })
        .collect();
    assert_eq!(
        birds
            .iter()
            .map(|bird| (bird.0.as_str(), bird.2))
            .collect::<Vec<_>>(),
        [
            ("Blackbird", 3),
            ("Robin", 5),
            ("Dunnock", 5),
            ("Blue tit", 6)
        ]
    );
    assert_eq!(result.description.sources[1].takes[0].name, "Phrase 1");
    assert!(
        result.description.sources[0].takes.is_empty(),
        "the wind has no takes"
    );

    let mut uttered: Vec<Vec<u8>> = vec![Vec::new(); birds.len()];
    let mut before: Vec<Option<u8>> = vec![None; birds.len()];
    let mut resting = vec![0usize; birds.len()];
    for _ in 0..120 * 48_000 {
        for (index, (_, slot, _)) in birds.iter().enumerate() {
            let source = &mut result.scene.sources[*slot];
            source.next_sample(48_000.0);
            let now = source.sounding_take();
            if now.is_none() {
                resting[index] += 1;
            }
            if now != before[index] {
                if let Some(take) = now {
                    uttered[index].push(take);
                }
                before[index] = now;
            }
        }
    }
    for (index, (name, _, takes)) in birds.iter().enumerate() {
        let uttered_by_one = &uttered[index];
        assert!(
            uttered_by_one.len() >= 8,
            "{name} sang only {} phrases in two minutes",
            uttered_by_one.len()
        );
        assert!(
            uttered_by_one.windows(2).all(|pair| pair[0] != pair[1]),
            "{name} repeated a phrase"
        );
        assert!(uttered_by_one.iter().all(|take| (*take as usize) < *takes));
        let share = resting[index] as f32 / (120.0 * 48_000.0);
        assert!(
            share > 0.4,
            "{name} rested only {:.0} % of the time",
            share * 100.0
        );
    }
    assert_ne!(
        uttered[1], uttered[2],
        "two birds with five takes must not fall into step"
    );
}
