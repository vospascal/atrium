//! What the scene is: a nature scene or a listening session. The kind
//! decides what runs: a listening session runs no weather story and keeps
//! standard, still air, whatever its file keeps of either, so switching to
//! listening and back brings them back as they were.

#[cfg(test)]
use atrium::config::SceneConfig;
use atrium::config::SceneKind;

use super::{lock, Engine, Live};
use crate::api::EngineError;

#[uniffi::export]
impl Engine {
    /// Make the scene a nature scene or a listening session from now on.
    /// Its weather and air stay in the scene file either way; a listening
    /// session only stops running them. `scene()` and `weather()` report the
    /// change at once. Its place stays what it is; `set_place` changes it.
    pub fn set_scene_kind(&self, kind: crate::api::SceneKind) -> Result<(), EngineError> {
        let mut scene = lock(&self.scene);
        self.play_kind(&mut scene, kind.into())
    }
}

impl Engine {
    /// Make `scene` a scene of `kind`: its weather and air as that kind runs
    /// them.
    pub(super) fn play_kind(&self, scene: &mut Live, kind: SceneKind) -> Result<(), EngineError> {
        scene.document.kind = kind;
        scene.description.kind = kind.into();
        self.play_weather(&scene.document)?;
        let air = scene
            .document
            .air()
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        self.play_air(scene, air.temperature_c, air.humidity_pct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{SceneKind as Kind, Story};

    fn at_the_repository_root() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
    }

    fn scratch(name: &str) -> String {
        let directory =
            std::env::temp_dir().join(format!("atrium-scene-kind-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("scratch directory");
        directory.join(name).to_str().expect("a path").to_owned()
    }

    /// A new empty scene is asked only for its kind, and plays, saves, and
    /// opens again as that kind: a nature scene in the meadow with a calm
    /// day, a listening session in the dry space with no weather.
    #[test]
    fn a_new_scene_of_each_kind_saves_and_reopens_as_that_kind() {
        at_the_repository_root();
        for (kind, environment) in [
            (Kind::Nature, Some("environments/meadow.yaml".to_string())),
            (Kind::Listening, None),
        ] {
            let engine = Engine::new_scene(kind).expect("it plays");
            engine.set_volume(0.0).expect("volume");
            let scene = engine.scene();
            assert_eq!(
                (scene.kind, scene.space.clone()),
                (kind, environment.clone())
            );
            assert!(scene.sources.is_empty());
            assert_eq!(engine.weather().is_some(), kind == Kind::Nature);
            let path = scratch(&format!("new-{kind:?}.yaml"));
            engine
                .save_scene(path.clone(), format!("New {kind:?}"))
                .expect("it saves");
            drop(engine);
            let reopened = Engine::new(path.clone()).expect("it opens");
            reopened.set_volume(0.0).expect("volume");
            let scene = reopened.scene();
            assert_eq!((scene.kind, scene.space), (kind, environment));
            assert_eq!(reopened.weather().is_some(), kind == Kind::Nature);
            let text = std::fs::read_to_string(&path).expect("it reads");
            assert_eq!(
                text.contains("kind: listening"),
                kind == Kind::Listening,
                "{text}"
            );
        }
    }

    /// A passing storm switched to a listening session runs no weather and
    /// keeps standard air, while its file keeps both; switched back, the
    /// storm and its air are as they were. Each switch undoes.
    #[test]
    fn switching_kind_and_back_restores_the_weather() {
        at_the_repository_root();
        let engine = Engine::new("scenes/passing-storm.yaml".into()).expect("it plays");
        engine.set_volume(0.0).expect("volume");
        engine.set_atmosphere(8.0, 85.0).expect("air");
        let weather = engine.weather().expect("a storm");
        assert_eq!(weather.story, Story::PassingStorm);
        let air = |engine: &Engine| (engine.scene().temperature_c, engine.scene().humidity_pct);
        assert_eq!(air(&engine), (8.0, 85.0));

        engine.remember("Listening".into());
        engine.set_scene_kind(Kind::Listening).expect("listening");
        assert_eq!(engine.scene().kind, Kind::Listening);
        assert_eq!(engine.weather(), None, "no weather runs");
        assert_eq!(air(&engine), (20.0, 50.0), "standard air");
        assert!(
            engine.set_weather(weather.clone()).is_err(),
            "nor can it be set"
        );
        assert!(engine.set_atmosphere(5.0, 50.0).is_err());
        let path = scratch("storm-as-listening.yaml");
        engine
            .save_scene(path.clone(), String::new())
            .expect("it saves");
        let kept = SceneConfig::load(&path).expect("it reads");
        assert!(
            kept.weather.is_some() && kept.atmosphere.is_some(),
            "kept in the file"
        );

        engine.remember("Nature".into());
        engine.set_scene_kind(Kind::Nature).expect("nature");
        assert_eq!(engine.weather(), Some(weather.clone()), "the storm again");
        assert_eq!(air(&engine), (8.0, 85.0), "its air again");

        assert!(engine.undo().expect("undo"));
        assert_eq!(engine.scene().kind, Kind::Listening);
        assert_eq!(engine.weather(), None);
        assert!(engine.undo().expect("undo"));
        assert_eq!(engine.scene().kind, Kind::Nature);
        assert_eq!(engine.weather(), Some(weather));
        assert_eq!(air(&engine), (8.0, 85.0));
    }
}
