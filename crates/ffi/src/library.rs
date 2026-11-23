//! The library: the presets a sound can be made from.

use crate::api::EngineError;

/// One preset as a library lists it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Preset {
    /// What `Engine::audition` and `Engine::add_sound` take.
    pub path: String,
    pub name: String,
    /// The family a sound made from it joins, when the preset names one.
    pub group: Option<String>,
    /// A few sentences on what it is and where it comes from, for the
    /// library to show with it, when the preset has them.
    pub about: Option<String>,
    /// Whether it is a generator; otherwise it plays a recording.
    pub generated: bool,
    /// How many takes it can utter: 1 for a recording that loops, 0 for a
    /// generator.
    pub takes: u32,
}

/// Every preset in `directory` and the folders inside it, sorted by path.
/// Read from the files alone: no audio is decoded.
#[uniffi::export]
pub fn list_presets(directory: String) -> Result<Vec<Preset>, EngineError> {
    let presets = atrium::config::list_presets(&directory)
        .map_err(|error| EngineError::Scene(error.to_string()))?;
    Ok(presets
        .into_iter()
        .map(|preset| Preset {
            path: preset.path,
            name: preset.name,
            group: preset.group,
            about: preset.about,
            generated: preset.generated,
            takes: preset.takes as u32,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_repository_presets_with_their_groups() {
        let directory = concat!(env!("CARGO_MANIFEST_DIR"), "/../../sources");
        let presets = list_presets(directory.into()).expect("the presets read");
        let frog = presets
            .iter()
            .find(|preset| preset.name == "Frog")
            .expect("the frog is listed");
        assert!(frog.path.ends_with("sources/creatures/frog.yaml"));
        assert_eq!(frog.group.as_deref(), Some("Creatures"));
        assert!(!frog.generated);
        assert_eq!(frog.takes, 1);
        assert!(presets
            .iter()
            .any(|preset| preset.generated && preset.takes == 0));
        let tone = presets
            .iter()
            .find(|preset| preset.path.ends_with("sources/tones/solfeggio/528.yaml"))
            .expect("the 528 Hz tone is listed");
        assert_eq!(tone.group.as_deref(), Some("Solfeggio"));
        assert!(tone
            .about
            .as_deref()
            .is_some_and(|about| about.contains("528, 264, 132 and 66 Hz")));
        assert_eq!(frog.about, None);
        let robin = presets
            .iter()
            .find(|preset| preset.name == "Robin")
            .expect("the robin is listed");
        assert_eq!(robin.takes, 5);
    }

    #[test]
    fn a_missing_directory_is_a_scene_error() {
        assert!(matches!(
            list_presets("no/such/presets".into()),
            Err(EngineError::Scene(_))
        ));
    }
}
