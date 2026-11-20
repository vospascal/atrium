//! The scenes a player can choose from.

use atrium::config::SceneConfig;

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
                path,
                title: summary.title,
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
        assert!(!frogs.sounds.is_empty());
    }

    #[test]
    fn a_missing_directory_is_a_scene_error() {
        assert!(matches!(
            list_scenes("no/such/directory".into()),
            Err(EngineError::Scene(_))
        ));
    }
}
