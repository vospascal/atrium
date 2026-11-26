//! How a scene's sound resolves against the preset it names, and the preset
//! formats in `sources/*.yaml` (sample file or procedural synth).

use atrium_world::{Entity, EntityId, Follow, When};
use serde::Deserialize;

use super::description::title_from_path;
use crate::plan::{LevelSpread, Rests};
use atrium_core::directivity::SILENT_BEHIND_DB;

/// The width of a sound that goes out all round (degrees).
pub(super) const ALL_ROUND: f32 = 360.0;

use atrium_core::source::{ALL_ROUND_DEGREES, NARROWEST_ARC_DEGREES};

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
    /// How it follows the place's conditions: the scene's links, else the
    /// preset's.
    pub(super) follows: Vec<Follow>,
    pub(super) spread: f32,
    /// Whether the scene makes it ambient or a point, in place of what it is
    /// by itself.
    pub(super) ambient: Option<bool>,
    /// The arc it arrives from when ambient (degrees, 360 all round).
    pub(super) arc: f32,
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
    if let (Some(SourceDef::Synth(synth)), Some(path)) = (&def, &entity.sound.preset) {
        synth
            .spec
            .check()
            .map_err(|error| format!("{path}: {error}"))?;
    }

    // Split the def into its sound identity + intrinsic defaults.
    let def_follows = match &def {
        Some(SourceDef::Sample(sample)) => sample.follows.clone(),
        Some(SourceDef::Synth(synth)) => synth.follows.clone(),
        None => Vec::new(),
    };
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

    let arc = entity.shape.arc.unwrap_or(ALL_ROUND_DEGREES);
    if !(NARROWEST_ARC_DEGREES..=ALL_ROUND_DEGREES).contains(&arc) {
        return Err(format!(
            "`shape.arc` is {arc}: an ambient sound arrives from {NARROWEST_ARC_DEGREES} to {ALL_ROUND_DEGREES} degrees"
        )
        .into());
    }

    Ok(ResolvedSource {
        sound: with_when(sound, &entity.when)?,
        reference_spl: entity.sound.level.or(def_spl).unwrap_or(70.0),
        follows: entity.follows.clone().unwrap_or(def_follows),
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
        ambient: entity.shape.ambient,
        arc,
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
    let rests = match when.bout {
        None => rests,
        Some(bout) => {
            let bout = checked_bout(bout)?;
            Some(Rests {
                bout,
                ..rests.unwrap_or(Rests::plain(0.0, 0.0))
            })
        }
    };
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
                let kind = rests.unwrap_or(Rests::plain(0.0, 0.0));
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
    /// A few sentences a library shows with it: what it is and where it
    /// comes from.
    #[serde(default)]
    pub about: Option<String>,
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
    /// How it follows the place's conditions, unless a scene says otherwise.
    #[serde(default)]
    pub follows: Vec<Follow>,
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
    /// How many times one take is uttered running, fewest and most, before
    /// another is chosen. Left out, a new take every time.
    #[serde(default)]
    pub bout: Option<[u32; 2]>,
    /// How loud one of the kind is against another, and one utterance
    /// against the next, as measured. Left out, all are as loud.
    #[serde(default)]
    pub level_spread: Option<LevelSpreadDef>,
}

/// A measured spread of level, as written:
/// `level_spread: { individual: 3.8, utterance: 2.5, range: [-15.4, 12.3] }`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LevelSpreadDef {
    /// Standard deviation of one of the kind against another (dB).
    pub individual: f32,
    /// Standard deviation of one utterance against the same one's others (dB).
    pub utterance: f32,
    /// Softest and loudest utterance measured, from the kind's mean (dB).
    pub range: [f32; 2],
}

impl LevelSpreadDef {
    /// The spread as written, checked: no negative deviation, and a range
    /// that holds the mean.
    fn checked(self) -> Result<LevelSpread, Box<dyn std::error::Error>> {
        let [softest, loudest] = self.range;
        if !(self.individual >= 0.0 && self.utterance >= 0.0) {
            return Err(format!(
                "`level_spread`'s `individual: {}` and `utterance: {}` must be 0 or more",
                self.individual, self.utterance
            )
            .into());
        }
        if !(softest <= 0.0 && loudest >= 0.0) {
            return Err(format!(
                "`level_spread`'s `range: [{softest}, {loudest}]` must run from at most 0 to at least 0 dB"
            )
            .into());
        }
        Ok(LevelSpread {
            individual_db: self.individual,
            utterance_db: self.utterance,
            range_db: (softest, loudest),
        })
    }
}

/// A bout as written, checked: at least once, and from few to many.
fn checked_bout([fewest, most]: [u32; 2]) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    if fewest >= 1 && most >= fewest {
        Ok((fewest, most))
    } else {
        Err(
            format!("`bout: [{fewest}, {most}]` must run from at least 1 to as many or more")
                .into(),
        )
    }
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
                    bout: rests.bout.map(checked_bout).transpose()?.unwrap_or((1, 1)),
                    level_spread: rests
                        .level_spread
                        .map(LevelSpreadDef::checked)
                        .transpose()?
                        .unwrap_or(LevelSpread::NONE),
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
    /// A few sentences a library shows with it: what it is and where it
    /// comes from.
    #[serde(default)]
    pub about: Option<String>,
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
    /// How it follows the place's conditions, unless a scene says otherwise.
    #[serde(default)]
    pub follows: Vec<Follow>,
}

/// SPL value in dB at 1 meter (IEC 61672 measurement distance).
/// Always a numeric value — no presets, no magic strings, just real-world dB.
type SplValue = f32;

pub(super) fn resolve_spl(db: f32) -> SoundProfile {
    SoundProfile { reference_spl: db }
}

#[cfg(test)]
mod tests;
