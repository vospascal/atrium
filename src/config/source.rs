//! Source entries: how a scene places a sound, how an entry resolves against
//! a referenced `sources/*.yaml` definition, and the definition formats
//! (sample file or procedural synth).

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::synth::SynthSpec;
use crate::audio::sound_profile::SoundProfile;

/// Scene entry: places a source in the scene. The sound definition comes either
/// from a referenced `sources/*.yaml` file (`source`) or from inline fields
/// (`audio` + `reference_spl` + …). Inline fields also act as overrides on top
/// of a referenced file — so a live SPL/spread/directivity edit is captured on
/// Save without needing to rewrite the source yaml, and a browsed audio file
/// with no preset yaml round-trips as a fully self-contained entry.
#[derive(Deserialize, Serialize, Clone)]
pub struct SourceEntry {
    /// Path to a source definition YAML file (e.g. "sources/djembe.yaml").
    /// Optional when the entry is defined inline via `audio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Path to the audio file (e.g. "assets/frog.mp3"). Overrides the referenced
    /// def's path; required when `source` is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<String>,
    /// Reference SPL in dB at 1 m. Overrides the referenced def.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_spl: Option<f32>,
    /// Directivity pattern ("omni", "cardioid", …). Overrides the referenced def.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directivity: Option<String>,
    /// MDAP spread (0.0 = point, 1.0 = full surround). Overrides the referenced def.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spread: Option<f32>,
    /// Display name (defaults to filename stem if omitted).
    pub name: Option<String>,
    /// UI color as hex string (e.g. "#ff6b35"). Defaults to palette by index.
    pub color: Option<String>,
    pub position: [f32; 3],
    #[serde(default)]
    pub orbit_radius: f32,
    #[serde(default)]
    pub orbit_speed: f32,
}

/// The sound identity an entry resolved to: a sample file or a synth spec.
pub(super) enum ResolvedSound {
    Sample { audio_path: String },
    Synth { spec: SynthSpec },
}

/// Resolved intrinsic sound properties for one entry, merging a referenced
/// `SourceDef` (if any) with the entry's inline overrides.
pub(super) struct ResolvedSource {
    pub(super) sound: ResolvedSound,
    pub(super) reference_spl: f32,
    pub(super) directivity: String,
    pub(super) spread: f32,
}

impl SourceEntry {
    /// Resolve this entry's sound definition: load the referenced yaml (if any)
    /// and apply inline overrides on top.
    pub(super) fn resolve(&self) -> Result<ResolvedSource, Box<dyn std::error::Error>> {
        let def: Option<SourceDef> = match &self.source {
            Some(path) => {
                let contents =
                    std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))?;
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

        // The entry-level `audio:` override replaces a sample def's file.
        // Combining it with a synth def is contradictory — reject it.
        let sound = match (self.audio.clone(), def_sound) {
            (Some(_), Some(ResolvedSound::Synth { .. })) => {
                return Err("`audio:` override cannot apply to a synth source definition".into())
            }
            (Some(audio_path), _) => ResolvedSound::Sample { audio_path },
            (None, Some(sound)) => sound,
            (None, None) => return Err("source entry needs either `source` or `audio`".into()),
        };

        Ok(ResolvedSource {
            sound,
            reference_spl: self.reference_spl.or(def_spl).unwrap_or(70.0),
            directivity: self
                .directivity
                .clone()
                .or(def_directivity)
                .unwrap_or_else(|| "omni".into()),
            spread: self.spread.or(def_spread).unwrap_or(0.0),
        })
    }

    /// A display name: explicit `name`, else the stem of the referenced yaml or
    /// the audio file.
    pub(super) fn display_name(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        let stem_of = |p: &str| {
            Path::new(p)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        };
        self.source
            .as_deref()
            .and_then(stem_of)
            .or_else(|| self.audio.as_deref().and_then(stem_of))
            .unwrap_or_else(|| "?".to_string())
    }
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
