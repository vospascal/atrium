//! The scenes a player can choose from.

use atrium::config::{saved_by_editor, EnvironmentSummary, SceneConfig};

use crate::api::{EngineError, Position};

/// A scene file, summarised without decoding any audio.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SceneFile {
    /// The file name without `.yaml`; unique within its directory.
    pub name: String,
    /// The path to pass to `Engine`.
    pub path: String,
    /// The scene's `title`, else one made from the file name.
    pub title: String,
    /// What the scene is: a nature scene or a listening session.
    pub kind: crate::api::SceneKind,
    /// Whether the editor wrote this file, so that saving may replace it. A
    /// hand-written scene is saved under a new name instead.
    pub saved_by_editor: bool,
    /// In slot order, as they will play.
    pub sounds: Vec<SoundSummary>,
}

/// One sound of a scene, before it plays.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SoundSummary {
    pub name: String,
    /// UI color as `#rrggbb`.
    pub color: String,
    /// Where the sound starts, relative to the listener (m).
    pub offset_from_listener: Position,
}

/// The `.yaml` scene files directly in `directory`, sorted by name. A file
/// that is not a valid scene is left out, with a warning on stderr.
#[uniffi::export]
pub fn list_scenes(directory: String) -> Result<Vec<SceneFile>, EngineError> {
    let entries = std::fs::read_dir(&directory)
        .map_err(|error| EngineError::Scene(format!("{directory}: {error}")))?;
    let mut scenes: Vec<SceneFile> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "yaml")
        })
        .filter_map(|path| {
            let name = path.file_stem()?.to_str()?.to_owned();
            let path = path.to_str()?.to_owned();
            let summary = match SceneConfig::load(&path) {
                Ok(config) => config.summary(&path),
                Err(error) => {
                    eprintln!("not a scene, left out: {error}");
                    return None;
                }
            };
            Some(SceneFile {
                name,
                saved_by_editor: saved_by_editor(&path),
                path,
                title: summary.title,
                kind: summary.kind.into(),
                sounds: summary
                    .sounds
                    .into_iter()
                    .map(|sound| SoundSummary {
                        name: sound.name,
                        color: sound.color,
                        offset_from_listener: sound.offset_from_listener.into(),
                    })
                    .collect(),
            })
        })
        .collect();
    scenes.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(scenes)
}

/// A place a scene can play in, as a list of them shows it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PlaceChoice {
    /// The environment file, as `Engine::set_place` takes it and
    /// `SceneDescription::space` reports it; nothing for the dry space.
    pub space: Option<String>,
    /// Its size along x, y and z (m).
    pub width: f32,
    pub depth: f32,
    pub height: f32,
    /// Whether nothing is overhead, as in a meadow; otherwise it is enclosed.
    pub open_sky: bool,
}

/// The places a scene can play in: the dry space first, then each
/// environment file directly in `directory` (`.yaml`), sorted by name. A
/// file that is not an environment is left out, with a warning on stderr.
#[uniffi::export]
pub fn list_places(directory: String) -> Result<Vec<PlaceChoice>, EngineError> {
    let entries = std::fs::read_dir(&directory)
        .map_err(|error| EngineError::Scene(format!("{directory}: {error}")))?;
    let mut paths: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "yaml")
        })
        .filter_map(|path| path.to_str().map(str::to_owned))
        .collect();
    paths.sort();
    Ok(std::iter::once(None)
        .chain(paths.into_iter().map(Some))
        .filter_map(|space| match EnvironmentSummary::of(space.as_deref()) {
            Ok(summary) => Some(PlaceChoice {
                space,
                width: summary.width,
                depth: summary.depth,
                height: summary.height,
                open_sky: summary.open_sky,
            }),
            Err(error) => {
                eprintln!("not an environment, left out: {error}");
                None
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_repository_scenes_by_name_with_loadable_paths() {
        let directory = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scenes");
        let scenes = list_scenes(directory.into()).expect("scenes directory should list");
        let names: Vec<&str> = scenes.iter().map(|scene| scene.name.as_str()).collect();
        assert!(names.contains(&"frog"), "got {names:?}");
        assert!(
            names.windows(2).all(|pair| pair[0] <= pair[1]),
            "unsorted: {names:?}"
        );
        assert!(scenes
            .iter()
            .all(|scene| std::path::Path::new(&scene.path).is_file()));
        let frogs = scenes
            .iter()
            .find(|scene| scene.name == "frog")
            .expect("frog scene listed");
        assert_eq!(frogs.title, "Frog");
        assert!(
            !frogs.saved_by_editor,
            "the repository's scenes are written by hand"
        );
        assert!(!frogs.sounds.is_empty());
    }

    /// Dry first, then every environment file by name, each with its size
    /// and whether the sky is open above it.
    #[test]
    fn lists_the_places_a_scene_can_play_in() {
        let directory = concat!(env!("CARGO_MANIFEST_DIR"), "/../../environments");
        let places = list_places(directory.into()).expect("places list");
        let named: Vec<(Option<&str>, [f32; 3], bool)> = places
            .iter()
            .map(|place| {
                (
                    place
                        .space
                        .as_deref()
                        .and_then(|space| space.rsplit('/').next()),
                    [place.width, place.depth, place.height],
                    place.open_sky,
                )
            })
            .collect();
        assert_eq!(
            named,
            [
                (None, [20.0, 20.0, 10.0], true),
                (Some("atrium.yaml"), [14.0, 10.0, 9.0], false),
                (Some("cathedral.yaml"), [40.0, 25.0, 20.0], false),
                (Some("meadow.yaml"), [60.0, 60.0, 30.0], true),
                (Some("riverside.yaml"), [100.0, 100.0, 50.0], true),
                (Some("small_atrium.yaml"), [6.0, 4.0, 3.0], false),
            ]
        );
        assert!(places
            .iter()
            .filter_map(|place| place.space.as_deref())
            .all(|space| std::path::Path::new(space).is_file()));
    }

    #[test]
    fn a_missing_directory_is_a_scene_error() {
        assert!(matches!(
            list_scenes("no/such/directory".into()),
            Err(EngineError::Scene(_))
        ));
    }
}
