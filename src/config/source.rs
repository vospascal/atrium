//! How a scene's sound resolves against the preset it names, and the preset
//! formats in `sources/*.yaml` (sample file or procedural synth).

use atrium_world::{Entity, EntityId, When};
use serde::Deserialize;

use super::description::title_from_path;
use crate::audio::voice_node::Rests;
use atrium_core::directivity::SILENT_BEHIND_DB;

/// The width of a sound that goes out all round (degrees).
pub(super) const ALL_ROUND: f32 = 360.0;

use super::synth::SynthSpec;
use crate::audio::sound_profile::SoundProfile;

/// The sound identity an entity resolved to: a sample file or a synth spec.
pub(super) enum ResolvedSound {
    /// Recordings: one that loops, or several uttered with rests between.
    Sample {
        takes: Vec<ResolvedTake>,
        rests: Option<Rests>,
    },
    Synth {
        spec: SynthSpec,
    },
}

/// Resolved intrinsic sound properties for one entity: its preset (if any)
/// with the entity's own changes on top.
pub(super) struct ResolvedSource {
    pub(super) sound: ResolvedSound,
    pub(super) reference_spl: f32,
    /// How wide it goes out (degrees, 360 all round) and how much is heard
    /// straight behind it (dB below the front).
    pub(super) width: f32,
    pub(super) behind: f32,
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
    let (def_sound, def_spl, def_beam, def_spread) = match def {
        Some(SourceDef::Sample(sample)) => {
            let level = sample.reference_spl;
            let beam = (sample.width, sample.behind);
            let spread = sample.spread;
            (Some(sample.sound()?), Some(level), Some(beam), Some(spread))
        }
        Some(SourceDef::Synth(synth)) => (
            Some(ResolvedSound::Synth { spec: synth.spec }),
            Some(synth.reference_spl),
            Some((synth.width, synth.behind)),
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
        // One recording in place of the preset's takes; it keeps the preset's way of rests.
        (Some(file), preset) => ResolvedSound::Sample {
            takes: vec![ResolvedTake {
                name: title_from_path(&file),
                file,
                chance: 1.0,
            }],
            rests: match preset {
                Some(ResolvedSound::Sample { rests, .. }) => rests,
                _ => None,
            },
        },
        (None, Some(sound)) => sound,
        // `Entities` refuses a sound with neither, so a scene never gets here.
        (None, None) => return Err("a sound needs either `preset` or `audio`".into()),
    };

    Ok(ResolvedSource {
        sound: with_when(sound, &entity.when)?,
        reference_spl: entity.sound.level.or(def_spl).unwrap_or(70.0),
        width: entity
            .shape
            .width
            .or(def_beam.and_then(|(width, _)| width))
            .unwrap_or(ALL_ROUND),
        behind: entity
            .shape
            .behind
            .or(def_beam.and_then(|(_, behind)| behind))
            .unwrap_or(SILENT_BEHIND_DB),
        spread: entity.shape.size.or(def_spread).unwrap_or(0.0),
    })
}

/// `sound` with what an entity's `when:` changes: the rest between takes and
/// each take's chance. A gap of `[0, 0]` on a sound with one take is that take
/// round and round, as a kind that loops plays it.
fn with_when(
    sound: ResolvedSound,
    when: &When,
) -> Result<ResolvedSound, Box<dyn std::error::Error>> {
    if *when == When::default() {
        return Ok(sound);
    }
    let ResolvedSound::Sample { mut takes, rests } = sound else {
        return Err("`when:` changes takes and rests, and a generator has neither".into());
    };
    for (name, chance) in &when.chance {
        if chance.is_nan() || *chance < 0.0 {
            return Err(format!("`when.chance` for `{name}` must be 0 or more").into());
        }
        let take = takes
            .iter_mut()
            .find(|take| take.name == *name)
            .ok_or_else(|| {
                format!("`when.chance` names `{name}`, which is not one of its takes")
            })?;
        take.chance = *chance;
    }
    if takes.iter().all(|take| take.chance <= 0.0) {
        return Err("`when.chance` leaves no take that can be chosen".into());
    }
    let rests = match when.gap {
        None => rests,
        Some([shortest, longest]) => {
            if !(shortest >= 0.0 && longest >= shortest) {
                return Err(
                    format!("`gap: [{shortest}, {longest}]` must run from short to long").into(),
                );
            }
            if longest == 0.0 && takes.len() == 1 {
                None
            } else {
                // A kind that loops has no variation of its own to keep.
                let kind = rests.unwrap_or(Rests {
                    gap_seconds: (0.0, 0.0),
                    level_variation_db: 0.0,
                    pitch_variation_cents: 0.0,
                });
                Some(Rests {
                    gap_seconds: (shortest, longest),
                    ..kind
                })
            }
        }
    };
    Ok(ResolvedSound::Sample { takes, rests })
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
    // Tried first: only a generator has a `synth:` key, while a recording
    // preset needs none of its own that a generator could not also have.
    Synth(SynthSourceDef),
    Sample(SampleSourceDef),
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
    /// What it can utter: each one a recording played from start to end.
    pub takes: Vec<TakeDef>,
    /// What plays when.
    pub rule: RuleDef,
    pub reference_spl: SplValue,
    /// How wide it goes out, in degrees; 360, all round, when left out.
    #[serde(default)]
    pub width: Option<f32>,
    /// How much is heard straight behind, in dB below the front; nothing
    /// when left out.
    #[serde(default)]
    pub behind: Option<f32>,
    #[serde(default)]
    pub spread: f32,
}

/// One take of a kind: a recording, the name a list shows for it, and how
/// often it is chosen next to the others.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TakeDef {
    pub file: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "default_chance")]
    pub chance: f32,
}

fn default_chance() -> f32 {
    1.0
}

/// The rule of a kind: what plays when. Written `rule: loops`, or
/// `rule: { rests: { gap: [3, 8] } }`.
#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum RuleDef {
    /// A rule that needs no settings, by name. Only `loops` exists: the one
    /// take plays round and round.
    Named(String),
    /// Utter a take, rest, utter another.
    Rests { rests: RestsDef },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RestsDef {
    /// Shortest and longest rest between two takes (s).
    pub gap: [f32; 2],
    #[serde(default)]
    pub vary: VaryDef,
}

/// How much one utterance may differ from the next.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct VaryDef {
    /// Up to this much louder or softer (dB).
    #[serde(default)]
    pub level: f32,
    /// Up to this much higher or lower (cents).
    #[serde(default)]
    pub pitch: f32,
}

/// A take with everything filled in.
pub(super) struct ResolvedTake {
    pub file: String,
    pub name: String,
    pub chance: f32,
}

impl SampleSourceDef {
    /// The recordings and the rule this preset names. A kind that loops has
    /// exactly one take; a gap has to run from short to long.
    pub(super) fn sound(self) -> Result<ResolvedSound, Box<dyn std::error::Error>> {
        if self.takes.is_empty() {
            return Err("a kind needs at least one take under `takes`".into());
        }
        if self.takes.iter().any(|take| !(take.chance > 0.0)) {
            return Err("a take's `chance` must be more than 0".into());
        }
        let takes: Vec<ResolvedTake> = self
            .takes
            .into_iter()
            .map(|take| ResolvedTake {
                name: take.name.unwrap_or_else(|| title_from_path(&take.file)),
                file: take.file,
                chance: take.chance,
            })
            .collect();
        let rests = match self.rule {
            RuleDef::Named(name) if name == "loops" => {
                if takes.len() != 1 {
                    return Err(format!(
                        "`rule: loops` plays one take round and round, and this kind has {}",
                        takes.len()
                    )
                    .into());
                }
                None
            }
            RuleDef::Named(name) => {
                return Err(format!("`rule: {name}` is not a rule; use `loops` or `rests`").into())
            }
            RuleDef::Rests { rests } => {
                let [shortest, longest] = rests.gap;
                if !(shortest >= 0.0 && longest >= shortest) {
                    return Err(format!(
                        "`gap: [{shortest}, {longest}]` must run from short to long"
                    )
                    .into());
                }
                Some(Rests {
                    gap_seconds: (shortest, longest),
                    level_variation_db: rests.vary.level,
                    pitch_variation_cents: rests.vary.pitch,
                })
            }
        };
        Ok(ResolvedSound::Sample { takes, rests })
    }
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
    /// How wide it goes out, in degrees; 360, all round, when left out.
    #[serde(default)]
    pub width: Option<f32>,
    /// How much is heard straight behind, in dB below the front; nothing
    /// when left out.
    #[serde(default)]
    pub behind: Option<f32>,
    #[serde(default)]
    pub spread: f32,
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

    fn kind(yaml: &str) -> Result<ResolvedSound, String> {
        match serde_yaml::from_str::<SourceDef>(yaml).map_err(|error| error.to_string())? {
            SourceDef::Sample(sample) => sample.sound().map_err(|error| error.to_string()),
            SourceDef::Synth(_) => Err("parsed as a generator".into()),
        }
    }

    /// Every recording kind is written the same way: its takes, and a rule.
    /// A looping kind has one take and does not rest.
    #[test]
    fn a_looping_kind_has_one_take_and_the_loops_rule() {
        let yaml = "reference_spl: 68.0\ntakes:\n  - { file: assets/campfire.mp3, name: Fire }\nrule: loops\n";
        match kind(yaml).expect("a looping kind reads") {
            ResolvedSound::Sample { takes, rests } => {
                assert_eq!(takes.len(), 1);
                assert_eq!(
                    (takes[0].file.as_str(), takes[0].name.as_str()),
                    ("assets/campfire.mp3", "Fire")
                );
                assert_eq!(takes[0].chance, 1.0);
                assert!(rests.is_none());
            }
            ResolvedSound::Synth { .. } => panic!("a recording, not a generator"),
        }
    }

    fn two_takes() -> ResolvedSound {
        kind("reference_spl: 60\ntakes:\n  - { file: a.mp3, name: Low }\n  - { file: b.mp3, name: High }\nrule: { rests: { gap: [3, 8], vary: { level: 2 } } }\n")
            .expect("reads")
    }

    /// A sound's own `when:` changes its kind's rests and chances, by take
    /// name, and keeps the kind's variation.
    #[test]
    fn when_changes_rests_and_chances_of_one_sound() {
        let when = When {
            gap: Some([20.0, 40.0]),
            chance: [("High".to_string(), 0.0)].into(),
        };
        let ResolvedSound::Sample { takes, rests } =
            with_when(two_takes(), &when).expect("applies")
        else {
            panic!("a recording");
        };
        assert_eq!((takes[0].chance, takes[1].chance), (1.0, 0.0));
        let rests = rests.expect("still rests");
        assert_eq!(rests.gap_seconds, (20.0, 40.0));
        assert_eq!(rests.level_variation_db, 2.0, "the kind's variation stays");
    }

    /// Anything that loops can be made to come and go, and one take with no
    /// rest at all loops again.
    #[test]
    fn a_gap_makes_a_loop_come_and_go_and_none_makes_it_loop() {
        let fire = || {
            kind("reference_spl: 68\ntakes:\n  - { file: fire.mp3 }\nrule: loops\n").expect("reads")
        };
        let coming = When {
            gap: Some([5.0, 9.0]),
            ..When::default()
        };
        let ResolvedSound::Sample { rests, .. } = with_when(fire(), &coming).expect("applies")
        else {
            panic!("a recording");
        };
        assert_eq!(rests.expect("rests now").gap_seconds, (5.0, 9.0));
        let always = When {
            gap: Some([0.0, 0.0]),
            ..When::default()
        };
        let ResolvedSound::Sample { rests, .. } = with_when(fire(), &always).expect("applies")
        else {
            panic!("a recording");
        };
        assert!(rests.is_none(), "one take with no rest loops");
    }

    #[test]
    fn when_refuses_what_it_cannot_mean() {
        let unknown = When {
            chance: [("Song".to_string(), 2.0)].into(),
            ..When::default()
        };
        assert!(
            with_when(two_takes(), &unknown).is_err(),
            "no take of that name"
        );
        let silent = When {
            chance: [("Low".to_string(), 0.0), ("High".to_string(), 0.0)].into(),
            ..When::default()
        };
        assert!(
            with_when(two_takes(), &silent).is_err(),
            "nothing left to choose"
        );
        let backwards = When {
            gap: Some([9.0, 2.0]),
            ..When::default()
        };
        assert!(with_when(two_takes(), &backwards).is_err());
    }

    #[test]
    fn a_resting_kind_reads_its_gap_variation_and_chances() {
        let yaml = "reference_spl: 60\ntakes:\n  - { file: a.mp3, chance: 3 }\n  - { file: b-song.mp3 }\nrule:\n  rests:\n    gap: [3, 8]\n    vary: { level: 2, pitch: 15 }\n";
        match kind(yaml).expect("a resting kind reads") {
            ResolvedSound::Sample { takes, rests } => {
                assert_eq!(takes[0].chance, 3.0);
                assert_eq!(
                    takes[1].name, "B Song",
                    "a take without a name is named after its file"
                );
                assert_eq!(
                    rests,
                    Some(Rests {
                        gap_seconds: (3.0, 8.0),
                        level_variation_db: 2.0,
                        pitch_variation_cents: 15.0,
                    })
                );
            }
            ResolvedSound::Synth { .. } => panic!("a recording, not a generator"),
        }
    }

    /// What a kind file can get wrong, each refused with a reason that says
    /// what to write instead. The one-file `path:` of before is one of them.
    #[test]
    fn a_kind_that_cannot_work_is_refused() {
        let two = "takes:\n  - { file: a.mp3 }\n  - { file: b.mp3 }\n";
        for (yaml, reason) in [
            (
                format!("reference_spl: 60\n{two}rule: loops\n"),
                "one take round and round",
            ),
            (
                "reference_spl: 60\ntakes: []\nrule: loops\n".to_string(),
                "at least one take",
            ),
            (
                format!("reference_spl: 60\n{two}rule: shuffles\n"),
                "is not a rule",
            ),
            (
                format!("reference_spl: 60\n{two}rule: {{ rests: {{ gap: [8, 3] }} }}\n"),
                "short to long",
            ),
            (
                "reference_spl: 60\ntakes:\n  - { file: a.mp3, chance: 0 }\nrule: loops\n"
                    .to_string(),
                "more than 0",
            ),
        ] {
            let refusal = kind(&yaml)
                .err()
                .unwrap_or_else(|| panic!("accepted:\n{yaml}"));
            assert!(refusal.contains(reason), "{refusal}");
        }
        assert!(kind("path: assets/campfire.mp3\nreference_spl: 68.0\n").is_err());
    }
}
