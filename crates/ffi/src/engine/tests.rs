//! Tests that cross the engine's modules: one playing scene taken through
//! everything an editor does to it.

use atrium::config::SceneConfig;

use super::test_support::telemetry_when;
use super::{Engine, AUDITION_SLOT};
use crate::api::{EngineError, Position, Telemetry};

/// What the editor's layers and library do to a playing scene, at zero
/// volume: `cargo test -p atrium-ffi -- --ignored --test-threads=1`.
#[test]
#[ignore = "opens the default output device"]
fn layers_and_library_change_the_playing_scene() {
    std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .expect("repository root");
    let engine = Engine::new("scenes/frog.yaml".into()).expect("scene should play");
    engine.set_volume(0.0).expect("volume");
    let playing = |telemetry: &Telemetry, slot: usize| {
        telemetry
            .sources
            .iter()
            .any(|source| source.slot as usize == slot)
    };
    let scene = engine.scene();
    assert_eq!(scene.sources.len(), 1);
    assert_eq!(scene.sources[0].id, "frogs");
    assert_eq!(scene.sources[0].group.as_deref(), Some("Creatures"));

    // Hearing a preset before placing it: it plays, and is not in the scene.
    engine
        .audition(Some("sources/creatures/cricket.yaml".into()))
        .expect("the cricket builds");
    telemetry_when(&engine, |telemetry| playing(telemetry, AUDITION_SLOT));
    assert_eq!(
        engine.scene().sources.len(),
        1,
        "an audition is not a layer"
    );
    engine.audition(None).expect("the audition stops");
    telemetry_when(&engine, |telemetry| !playing(telemetry, AUDITION_SLOT));

    // Adding one: the next voice, an id from the preset's name, its group,
    // and a place 2 m ahead of a listener who faces north.
    let added = engine
        .add_sound("sources/creatures/frog.yaml".into(), None)
        .expect("the frog builds");
    assert_eq!((added.id.as_str(), added.name.as_str()), ("frog", "Frog"));
    assert_eq!(added.group.as_deref(), Some("Creatures"));
    assert!((added.position.y - scene.listener.y - 2.0).abs() < 1e-3);
    assert!((added.position.x - scene.listener.x).abs() < 1e-3);
    let second = engine
        .add_sound("sources/creatures/frog.yaml".into(), None)
        .expect("a second frog builds");
    assert_eq!(second.id, "frog-2", "ids stay unique");
    assert_eq!(engine.scene().sources.len(), 3);
    telemetry_when(&engine, |telemetry| {
        playing(telemetry, 1) && playing(telemetry, 2)
    });

    // A layer's mute and level.
    engine.set_source_muted(1, true).expect("mute");
    telemetry_when(&engine, |telemetry| {
        telemetry
            .sources
            .iter()
            .any(|source| source.slot == 1 && source.muted)
    });
    let level_of = |slot: u16| {
        engine
            .scene()
            .sources
            .iter()
            .find(|source| source.slot == slot)
            .map(|source| source.spl)
    };
    engine.set_source_level(1, 50.0).expect("level");
    assert_eq!(level_of(1), Some(50.0));

    // Removing a layer: its voice falls silent, the scene forgets it, and
    // the next sound added takes the freed voice and the freed id.
    engine.remove_sound(1).expect("remove");
    telemetry_when(&engine, |telemetry| !playing(telemetry, 1));
    let left: Vec<(String, u16)> = engine
        .scene()
        .sources
        .iter()
        .map(|source| (source.id.clone(), source.slot))
        .collect();
    assert_eq!(left, [("frogs".to_string(), 0), ("frog-2".to_string(), 2)]);
    engine
        .remove_sound(1)
        .expect("an empty voice is left alone");
    let again = engine
        .add_sound("sources/creatures/frog.yaml".into(), None)
        .expect("a frog builds again");
    assert_eq!((again.id.as_str(), again.slot), ("frog", 1));
    telemetry_when(&engine, |telemetry| playing(telemetry, 1));
    assert_eq!(engine.scene().sources.len(), 3);
    assert_eq!(engine.scene().sources[2].id, "frog", "added ones go last");

    // A shape and a direction: a half circle's beam is 90° to each side, and
    // a sound turned north faces +Y on the audio thread too.
    assert_eq!(engine.scene().sources[0].width, 360.0);
    assert_eq!(engine.scene().sources[0].beam_half_angle, None);
    engine.set_source_beam(0, 180.0, -60.0).expect("width");
    let shaped = engine.scene().sources[0].clone();
    assert_eq!((shaped.width, shaped.behind), (180.0, -60.0));
    let beam = shaped.beam_half_angle.expect("a half circle has a beam");
    assert!((beam - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    let north = Position {
        x: 0.0,
        y: 3.0,
        z: 9.0,
    };
    engine.set_source_facing(0, north).expect("facing");
    let turned = engine.scene().sources[0].facing;
    assert!(turned.x.abs() < 1e-6 && (turned.y - 1.0).abs() < 1e-6 && turned.z == 0.0);
    telemetry_when(&engine, |telemetry| {
        telemetry.sources.iter().any(|source| {
            source.slot == 0 && source.facing.x.abs() < 1e-3 && source.facing.y > 0.999
        })
    });

    // A sound dragged on the map: the scene remembers where it was put.
    let moved = Position {
        x: added.position.x + 4.0,
        y: added.position.y - 1.0,
        z: added.position.z,
    };
    engine.set_source_position(1, moved).expect("move");
    let placed = engine
        .scene()
        .sources
        .iter()
        .find(|source| source.slot == 1)
        .map(|source| source.position);
    assert_eq!(placed, Some(moved));
    telemetry_when(&engine, |telemetry| {
        telemetry
            .sources
            .iter()
            .any(|source| source.slot == 1 && source.position == moved)
    });
    assert_eq!(level_of(2), Some(65.0), "only that layer");

    // Saving: every change above is in the file, and a hand-written scene
    // is never saved over.
    let saved = std::env::temp_dir().join(format!("atrium-saved-{}.yaml", std::process::id()));
    let saved = saved.to_str().expect("utf-8").to_owned();
    let _ = std::fs::remove_file(&saved);
    assert!(engine
        .save_scene("scenes/frog.yaml".into(), "Frog".into())
        .is_err());
    engine
        .save_scene(saved.clone(), "Three frogs".into())
        .expect("a new file");
    let written = SceneConfig::load(&saved).expect("the saved scene reads");
    assert_eq!(written.title.as_deref(), Some("Three frogs"));
    let ids: Vec<&str> = written.sounds.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["frogs", "frog-2", "frog"]);
    let (_, frogs) = written.sounds.iter().next().expect("frogs");
    assert_eq!(frogs.shape.width, Some(180.0));
    assert_eq!(frogs.shape.facing, Some(0.0), "north");
    let (_, frog) = written.sounds.iter().nth(2).expect("frog");
    let expected = engine.sounds().place_at(moved.into());
    assert!((frog.place.x - expected.x).abs() < 0.01 && (frog.place.y - expected.y).abs() < 0.01);
    let rebuilt = written.build().expect("the saved scene builds");
    assert_eq!(rebuilt.description.sources.len(), 3);
    let _ = std::fs::remove_file(&saved);
}

#[test]
fn a_missing_scene_fails_before_any_audio_starts() {
    match Engine::new("scenes/does-not-exist.yaml".into()) {
        Err(EngineError::Scene(reason)) => assert!(reason.contains("does-not-exist")),
        Err(other) => panic!("expected a scene error, got {other}"),
        Ok(_) => panic!("a missing scene must not start an engine"),
    }
}
