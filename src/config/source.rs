//! How a scene's sound resolves against the preset it names, and the preset
//! formats in `sources/*.yaml` (sample file or procedural synth).

use atrium_world::{Entity, EntityId};
use serde::Deserialize;

use super::synth::SynthSpec;
use crate::audio::sound_profile::SoundProfile;

/// The sound identity an entity resolved to: a sample file or a synth spec.
pub(super) enum ResolvedSound {
    Sample { audio_path: String },
    Synth { spec: SynthSpec },
}

/// Resolved intrinsic sound properties for one entity: its preset (if any)
/// with the entity's own changes on top.
pub(super) struct ResolvedSource {
    pub(super) sound: ResolvedSound,
    pub(super) reference_spl: f32,
    pub(super) directivity: String,
    pub(super) spread: f32,
}

/// Resolve an entity's sound: load the preset it names (if any) and apply
/// what the entity changes on top.
pub(super) fn resolve(entity: &Entity) -> Result<ResolvedSource, Box<dyn std::error::Error>> {
    let def: Option<SourceDef> = match &entity.sound.preset {
        Some(path) => {
            let contents = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))?;
            Some(serde_yaml::from_str(&contents).map_err(|e| format!("{}: {}", path, e))?)
        }
        None => None,
    };

    // Split the def into its sound identity + intrinsic defaults.
    let (def_sound, def_spl, def_directivity, def_spread) = match def {
        Some(SourceDef::Sample(sample)) => (
            Some(ResolvedSound::Sample {
                audio_path: sample.path,
            }),
            Some(sample.reference_spl),
            Some(sample.directivity),
            Some(sample.spread),
        ),
        Some(SourceDef::Synth(synth)) => (
            Some(ResolvedSound::Synth { spec: synth.spec }),
            Some(synth.reference_spl),
            Some(synth.directivity),
            Some(synth.spread),
        ),
        None => (None, None, None, None),
    };

    // The entity's own `audio:` replaces a sample preset's file.
    // Combining it with a generator preset is contradictory — reject it.
    let sound = match (entity.sound.audio.clone(), def_sound) {
        (Some(_), Some(ResolvedSound::Synth { .. })) => {
            return Err("`audio:` cannot replace the sound of a generator preset".into())
        }
        (Some(audio_path), _) => ResolvedSound::Sample { audio_path },
        (None, Some(sound)) => sound,
        // `Entities` refuses a sound with neither, so a scene never gets here.
        (None, None) => return Err("a sound needs either `preset` or `audio`".into()),
    };

    Ok(ResolvedSource {
        sound,
        reference_spl: entity.sound.level.or(def_spl).unwrap_or(70.0),
        directivity: entity
            .shape
            .pattern
            .map(|pattern| pattern.name().to_string())
            .or(def_directivity)
            .unwrap_or_else(|| "omni".into()),
        spread: entity.shape.size.or(def_spread).unwrap_or(0.0),
    })
}

/// The name a list shows: the entity's own `name`, else its id.
pub(super) fn display_name(id: &EntityId, entity: &Entity) -> String {
    entity.name.clone().unwrap_or_else(|| id.to_string())
}

/// Source definition (loaded from `sources/*.yaml`): intrinsic sound identity.
/// Either a sample player (`path:` to an audio file) or a procedural synth
/// generator (`synth:` kind plus generator parameters).
#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum SourceDef {
    Sample(SampleSourceDef),
    Synth(SynthSourceDef),
}

/// Sample-file source definition.
#[derive(Deserialize)]
pub(super) struct SampleSourceDef {
    /// The name a library shows; without one, it comes from the file name.
    #[serde(default)]
    pub name: Option<String>,
    /// The family a sound made from this preset joins in a list.
    #[serde(default)]
    pub group: Option<String>,
    pub path: String,
    pub reference_spl: SplValue,
    #[serde(default = "default_directivity")]
    pub directivity: String,
    #[serde(default)]
    pub spread: f32,
}

/// Procedural synth source definition. Generator parameters sit at the same
/// YAML level as the calibration fields (`synth: field_wind` +
/// `min_speed: 1.0` + …).
#[derive(Deserialize)]
pub(super) struct SynthSourceDef {
    /// The name a library shows; without one, it comes from the file name.
    #[serde(default)]
    pub name: Option<String>,
    /// The family a sound made from this preset joins in a list.
    #[serde(default)]
    pub group: Option<String>,
    #[serde(flatten)]
    pub spec: SynthSpec,
    pub reference_spl: SplValue,
    #[serde(default = "default_directivity")]
    pub directivity: String,
    #[serde(default)]
    pub spread: f32,
}

fn default_directivity() -> String {
    "omni".into()
}

/// SPL value in dB at 1 meter (IEC 61672 measurement distance).
/// Always a numeric value — no presets, no magic strings, just real-world dB.
type SplValue = f32;

pub(super) fn resolve_spl(db: f32) -> SoundProfile {
    SoundProfile { reference_spl: db }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sample def must still parse into the Sample variant.
    #[test]
    fn sample_source_def_parses() {
        let yaml = "path: assets/campfire.mp3\nreference_spl: 68.0\nspread: 0.6\n";
        match serde_yaml::from_str::<SourceDef>(yaml).expect("sample def should parse") {
            SourceDef::Sample(sample) => {
                assert_eq!(sample.path, "assets/campfire.mp3");
                assert_eq!(sample.reference_spl, 68.0);
            }
            SourceDef::Synth(_) => panic!("parsed as synth"),
        }
    }
}
