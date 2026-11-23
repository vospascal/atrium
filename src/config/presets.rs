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
    /// A few sentences on what it is and where it comes from, when the
    /// preset has them.
    pub about: Option<String>,
    /// Whether it is a generator; otherwise it plays a recording.
    pub generated: bool,
    /// Its level in dB SPL at 1 m.
    pub level: f32,
    /// How many takes it can utter; 0 for a generator.
    pub takes: usize,
}

/// Every preset in `directory` and the folders inside it, sorted by path. A
/// file that is not a preset is an error that names it, so a broken preset
/// is noticed in the library and not when a scene that uses it fails to
/// start.
pub fn list_presets(directory: &str) -> Result<Vec<PresetSummary>, Box<dyn std::error::Error>> {
    let mut paths = Vec::new();
    collect_preset_paths(std::path::Path::new(directory), &mut paths)?;
    paths.sort();

    paths.iter().map(|path| read_preset(path)).collect()
}

/// The `.yaml` files in `directory` and every folder below it.
fn collect_preset_paths(
    directory: &std::path::Path,
    paths: &mut Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("{}: {error}", directory.display()))?
            .path();
        if path.is_dir() {
            collect_preset_paths(&path, paths)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "yaml")
        {
            if let Some(path) = path.to_str() {
                paths.push(path.to_string());
            }
        }
    }
    Ok(())
}

/// The preset in the file at `path`, as a library lists it.
pub fn read_preset(path: &str) -> Result<PresetSummary, Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
    let preset: SourceDef =
        serde_yaml::from_str(&contents).map_err(|error| format!("{path}: {error}"))?;
    let (name, group, about, generated, level, takes) = match preset {
        SourceDef::Sample(sample) => (
            sample.name,
            sample.group,
            sample.about,
            false,
            sample.reference_spl,
            sample.takes.len(),
        ),
        SourceDef::Synth(synth) => (
            synth.name,
            synth.group,
            synth.about,
            true,
            synth.reference_spl,
            0,
        ),
    };
    Ok(PresetSummary {
        name: name.unwrap_or_else(|| title_from_path(path)),
        path: path.to_string(),
        group,
        about,
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
            .find(|preset| preset.path == "sources/creatures/frog.yaml")
            .expect("the frog is listed");
        assert_eq!(frog.name, "Frog");
        assert_eq!(frog.group.as_deref(), Some("Creatures"));
        assert!(!frog.generated, "the frog is a recording");
        assert_eq!(frog.level, 65.0);

        let wind = presets
            .iter()
            .find(|preset| preset.path == "sources/weather/wind/field-wind.yaml")
            .expect("the field wind is listed");
        assert!(wind.generated, "the field wind is a generator");
        assert_eq!(wind.group.as_deref(), Some("Weather"));
    }

    /// The focus tones: one preset per frequency in `docs/focus-tones.md`,
    /// grouped by the set they come from, each with a note for the library.
    #[test]
    fn every_focus_tone_is_grouped_and_has_a_note() {
        let presets = list_presets("sources").expect("the presets read");
        let tones: Vec<&PresetSummary> = presets
            .iter()
            .filter(|preset| {
                preset.path.starts_with("sources/tones/")
                    && !preset.path.starts_with("sources/tones/binaural/")
            })
            .collect();
        let in_group = |group: &str| {
            tones
                .iter()
                .filter(|preset| preset.group.as_deref() == Some(group))
                .count()
        };
        assert_eq!(in_group("Solfeggio"), 10);
        assert_eq!(in_group("Angel numbers"), 10);
        assert_eq!(in_group("Planetary"), 13);
        assert_eq!(in_group("Concert pitch"), 1);
        assert_eq!(tones.len(), 34);
        for tone in &tones {
            assert!(tone.generated, "{} is a generator", tone.path);
            assert_eq!(tone.level, 50.0, "{}", tone.path);
            let about = tone.about.as_deref().unwrap_or("");
            assert!(about.len() > 100, "{} has no note", tone.path);
        }

        let earth_day = presets
            .iter()
            .find(|preset| preset.path == "sources/tones/planetary/earth-day.yaml")
            .expect("a preset in a folder inside a folder is listed")
            .clone();
        assert_eq!(earth_day.name, "Earth's day · 194.18 Hz");
        assert!(earth_day
            .about
            .expect("it has a note")
            .contains("Earth's day, 24 hours (86 400 s)"));
        // A preset without a note has none.
        assert_eq!(
            read_preset("sources/creatures/frog.yaml").unwrap().about,
            None
        );
    }

    /// The binaural beats: one preset per piece in `docs/binaural-beats.md`,
    /// grouped together, quiet, each with a note that says it needs
    /// headphones, what leaks between the ears here, and that no effect is
    /// measured.
    #[test]
    fn every_binaural_beat_is_grouped_and_has_a_note() {
        let presets = list_presets("sources").expect("the presets read");
        let beats: Vec<&PresetSummary> = presets
            .iter()
            .filter(|preset| preset.path.starts_with("sources/tones/binaural/"))
            .collect();
        assert_eq!(beats.len(), 13);
        for beat in &beats {
            assert_eq!(
                beat.group.as_deref(),
                Some("Binaural beats"),
                "{}",
                beat.path
            );
            assert!(beat.generated, "{} is a generator", beat.path);
            assert_eq!(beat.level, 18.0, "{}", beat.path);
            let about = beat.about.as_deref().unwrap_or("");
            for needed in [
                "108 Hz on the left",
                "needs headphones",
                "21 dB",
                "has been measured",
            ] {
                assert!(about.contains(needed), "{}: no “{needed}”", beat.path);
            }
        }
        let alpha = read_preset("sources/tones/binaural/alpha.yaml").unwrap();
        assert_eq!(alpha.name, "Alpha Binaural Beat · 10 Hz");
        let session = read_preset("sources/tones/binaural/falling-theta.yaml").unwrap();
        assert_eq!(session.name, "Falling Theta Session · 7 → 5 Hz");
        assert!(session.about.unwrap().contains("115 Hz moving to 113 Hz"));
    }

    /// The noise colours: one preset per colour, grouped together, each with
    /// a note; the colours without an agreed definition say so and that
    /// theirs follows an example recording, and red and brown say how they
    /// differ.
    #[test]
    fn every_noise_colour_is_grouped_and_has_a_note() {
        let presets = list_presets("sources").expect("the presets read");
        let colours: Vec<&PresetSummary> = presets
            .iter()
            .filter(|preset| preset.path.starts_with("sources/noise/"))
            .collect();
        assert_eq!(colours.len(), 9);
        for colour in &colours {
            assert_eq!(
                colour.group.as_deref(),
                Some("Noise colours"),
                "{}",
                colour.path
            );
            assert!(colour.generated, "{} is a generator", colour.path);
            let about = colour.about.as_deref().unwrap_or("");
            assert!(about.len() > 200, "{} has no note", colour.path);
        }
        let about = |name: &str| {
            read_preset(&format!("sources/noise/{name}.yaml"))
                .unwrap()
                .about
                .unwrap()
        };
        assert!(about("green").contains("no agreed definition"));
        assert!(about("green").contains("example recording"));
        assert!(about("orange").contains("no agreed definition"));
        assert!(about("orange").contains("not a defined spectrum"));
        assert!(about("orange").contains("example recording"));
        assert!(about("red").contains("what physics calls Brownian noise"));
        assert!(about("brown").contains("Darker than the physicist's Brownian noise"));
        assert!(about("grey").contains("does not cure it"));
        for bright in ["blue", "violet"] {
            assert!(about(bright).contains("best kept quiet"), "{bright}");
        }
    }

    /// Presets in folders, and folders inside those, are listed with the
    /// rest, sorted by path; a file that is not YAML is not.
    #[test]
    fn presets_in_folders_are_listed_sorted_by_path() {
        let directory = std::env::temp_dir().join(format!("atrium-presets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let nested = directory.join("tones").join("solfeggio");
        std::fs::create_dir_all(&nested).unwrap();
        let preset = "synth: focus_tone\nreference_spl: 50\n";
        std::fs::write(directory.join("wind.yaml"), preset).unwrap();
        std::fs::write(directory.join("about.txt"), "not a preset").unwrap();
        std::fs::write(nested.join("528.yaml"), preset).unwrap();
        std::fs::write(directory.join("tones").join("432.yaml"), preset).unwrap();

        let listed: Vec<String> = list_presets(directory.to_str().unwrap())
            .expect("the presets read")
            .into_iter()
            .map(|preset| {
                preset
                    .path
                    .strip_prefix(directory.to_str().unwrap())
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            listed,
            ["/tones/432.yaml", "/tones/solfeggio/528.yaml", "/wind.yaml"]
        );
    }

    #[test]
    fn a_missing_directory_is_an_error_that_names_it() {
        let refusal = list_presets("no/such/presets").expect_err("nothing to list");
        assert!(refusal.to_string().contains("no/such/presets"));
    }
}
