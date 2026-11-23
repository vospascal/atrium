//! A sound's layers as a scene file keeps them: set, saved, opened again,
//! and rendered the same.

use atrium_world::{EntityId, Layer};

use super::{identical, render};
use crate::config::SceneConfig;
use crate::offline::{RenderSettings, Rendering};

const SCENE: &str = "scenes/soft-wind-only.yaml";

#[test]
fn layers_come_back_from_the_scene_file_and_render_the_same() {
    let mut config = SceneConfig::load(SCENE).unwrap_or_else(|error| panic!("{SCENE}: {error}"));
    let wind = EntityId::new("soft-wind").expect("its id");
    let layers = &mut config.sounds.get_mut(&wind).expect("the wind").sound.layers;
    layers.insert(
        "presence".into(),
        Layer {
            level: -6.0,
            ..Layer::default()
        },
    );
    layers.insert(
        "mid".into(),
        Layer {
            mute: true,
            ..Layer::default()
        },
    );
    layers.insert(
        "air".into(),
        Layer {
            level: 3.5,
            solo: true,
            mute: false,
        },
    );
    let directory = std::env::temp_dir().join(format!("atrium-layers-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("scratch directory");
    let path = directory.join("soft-wind-layers.yaml");
    let path = path.to_str().expect("a path");
    config.save(path).expect("it saves");
    let text = std::fs::read_to_string(path).expect("it reads");
    println!("{}", &text[text.find("sounds:").expect("its sounds")..]);
    let reopened = SceneConfig::load(path).expect("it opens");
    assert_eq!(reopened.sounds, config.sounds, "the layers come back");

    let built = reopened.clone().build().expect("it builds").description;
    let shown: Vec<(String, f32, bool, bool)> = built.sources[0]
        .layers
        .iter()
        .map(|layer| {
            (
                layer.key.clone(),
                layer.setting.level_db,
                layer.setting.solo,
                layer.setting.mute,
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("low".to_string(), 0.0, false, false),
            ("body".to_string(), 0.0, false, false),
            ("mid".to_string(), 0.0, false, true),
            ("presence".to_string(), -6.0, false, false),
            ("air".to_string(), 3.5, true, false),
        ]
    );

    let settings = RenderSettings {
        seconds: 2.0,
        seed: Some(3),
        ..RenderSettings::default()
    };
    let as_set = Rendering::prepare(config, &settings)
        .expect("it renders")
        .run();
    let as_reopened = Rendering::prepare(reopened, &settings)
        .expect("it renders")
        .run();
    assert!(identical(as_set.frames(), as_reopened.frames()));
    // And the layers are heard: the scene as shipped sounds otherwise.
    let shipped = render(SCENE, &settings);
    assert!(!identical(shipped.frames(), as_set.frames()));
    std::fs::remove_dir_all(&directory).ok();
}

/// A layer the sound does not have is refused by name, with the layers it
/// does have; a recording, which is one stream, has none.
#[test]
fn an_unknown_layer_is_refused() {
    let mut config = SceneConfig::load(SCENE).expect("it loads");
    let wind = EntityId::new("soft-wind").expect("its id");
    config
        .sounds
        .get_mut(&wind)
        .expect("the wind")
        .sound
        .layers
        .insert("treble".into(), Layer::default());
    let refused = config.build().err().expect("refused").to_string();
    assert_eq!(
        refused,
        "sound `soft-wind` has no layer `treble`; its layers are low, body, mid, presence, air"
    );
}
