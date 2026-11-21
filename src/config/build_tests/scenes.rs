//! Every repository scene reads, and the ones with their own sounds build
//! end to end and sound.

use atrium_core::speaker::RenderMode;

use crate::config::scene::SceneConfig;

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
