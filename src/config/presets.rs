//! The presets a library offers: every sound definition in a directory, read
//! from its file alone. No audio is decoded and no generator is run.

use super::description::title_from_path;
use super::source::SourceDef;

/// One preset as a library lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct PresetSummary {
    /// The path a scene's sound names as its `preset`.
    pub path: String,
    /// The preset's own `name`, else one made from its file name.
    pub name: String,
    /// The family a sound made from it joins, when the preset names one.
    pub group: Option<String>,
    /// Whether it is a generator; otherwise it plays a recording.
    pub generated: bool,
    /// Its level in dB SPL at 1 m.
    pub level: f32,
    /// How many takes it can utter; 0 for a generator.
    pub takes: usize,
}

/// Every preset in `directory`, by file name. A file that is not a preset is
/// an error that names it, so a broken preset is noticed in the library and
/// not when a scene that uses it fails to start.
pub fn list_presets(directory: &str) -> Result<Vec<PresetSummary>, Box<dyn std::error::Error>> {
    let mut paths: Vec<String> = std::fs::read_dir(directory)
        .map_err(|error| format!("{directory}: {error}"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "yaml")
        })
        .filter_map(|path| path.to_str().map(str::to_string))
        .collect();
    paths.sort();

    paths.iter().map(|path| read_preset(path)).collect()
}

/// The preset in the file at `path`, as a library lists it.
pub fn read_preset(path: &str) -> Result<PresetSummary, Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
    let preset: SourceDef =
        serde_yaml::from_str(&contents).map_err(|error| format!("{path}: {error}"))?;
    let (name, group, generated, level, takes) = match preset {
        SourceDef::Sample(sample) => (
            sample.name,
            sample.group,
            false,
            sample.reference_spl,
            sample.takes.len(),
        ),
        SourceDef::Synth(synth) => (synth.name, synth.group, true, synth.reference_spl, 0),
    };
    Ok(PresetSummary {
        name: name.unwrap_or_else(|| title_from_path(path)),
        path: path.to_string(),
        group,
        generated,
        level,
        takes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The repository's own presets, as the app's library lists them: every
    /// one reads, has a name a person would recognise, and says which family
    /// a sound made from it joins.
    #[test]
    fn every_repository_preset_is_listed_with_a_name_and_a_group() {
        let presets = list_presets("sources").expect("the presets read");
        assert!(presets.len() >= 17, "found only {} presets", presets.len());
        for preset in &presets {
            assert!(!preset.name.is_empty(), "{} has no name", preset.path);
            assert!(preset.group.is_some(), "{} has no group", preset.path);
        }

        let frog = presets
            .iter()
            .find(|preset| preset.path == "sources/frog.yaml")
            .expect("the frog is listed");
        assert_eq!(frog.name, "Frog");
        assert_eq!(frog.group.as_deref(), Some("Creatures"));
        assert!(!frog.generated, "the frog is a recording");
        assert_eq!(frog.level, 65.0);

        let wind = presets
            .iter()
            .find(|preset| preset.path == "sources/field-wind.yaml")
            .expect("the field wind is listed");
        assert!(wind.generated, "the field wind is a generator");
        assert_eq!(wind.group.as_deref(), Some("Weather"));
    }

    #[test]
    fn a_missing_directory_is_an_error_that_names_it() {
        let refusal = list_presets("no/such/presets").expect_err("nothing to list");
        assert!(refusal.to_string().contains("no/such/presets"));
    }
}
