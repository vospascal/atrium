//! Biome classification over environmental fields.
//!
//! A biome describes how an environment is interpreted. It does not own
//! material internals or generate terrain directly; it selects reusable
//! surface, feature, presentation, audio, and animation profiles.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::environment::{EnvironmentChannel, EnvironmentContext};

macro_rules! semantic_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

semantic_id!(BiomeId);
semantic_id!(SurfaceProfileId);
semantic_id!(MaterialPaletteId);
semantic_id!(FeatureSetId);
semantic_id!(ModifierId);
semantic_id!(AudioProfileId);
semantic_id!(AnimationProfileId);
semantic_id!(MaterialRole);
semantic_id!(FeatureId);
semantic_id!(RuleId);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldRange {
    pub min: f32,
    pub max: f32,
    /// Distance outside the interval over which membership fades to zero.
    pub falloff: f32,
}

impl FieldRange {
    pub fn validate(self) -> bool {
        self.min.is_finite()
            && self.max.is_finite()
            && self.falloff.is_finite()
            && self.min <= self.max
            && self.falloff >= 0.0
    }

    pub fn distance(self, value: f32) -> f32 {
        if value < self.min {
            self.min - value
        } else if value > self.max {
            value - self.max
        } else {
            0.0
        }
    }

    pub(crate) fn membership(self, value: f32) -> f32 {
        let distance = self.distance(value);
        if distance == 0.0 {
            1.0
        } else if self.falloff <= f32::EPSILON {
            0.0
        } else {
            (1.0 - distance / self.falloff).clamp(0.0, 1.0)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldConstraint {
    pub channel: EnvironmentChannel,
    pub range: FieldRange,
    /// Relative importance in nearest-biome fallback and blend shaping.
    pub weight: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BiomeSelector {
    pub constraints: Vec<FieldConstraint>,
    pub priority: i32,
}

impl BiomeSelector {
    fn evaluate(&self, context: &EnvironmentContext) -> SelectorEvaluation {
        if self.constraints.is_empty() {
            return SelectorEvaluation {
                membership: 1.0,
                distance: 0.0,
            };
        }
        let mut membership = 1.0_f32;
        let mut weighted_distance = 0.0;
        let mut total_weight = 0.0;
        for constraint in &self.constraints {
            let weight = constraint.weight.max(0.0);
            let value = context.field(&constraint.channel);
            membership *= constraint
                .range
                .membership(value)
                .powf(weight.max(f32::EPSILON));
            weighted_distance += constraint.range.distance(value) * weight;
            total_weight += weight;
        }
        SelectorEvaluation {
            membership,
            distance: if total_weight > 0.0 {
                weighted_distance / total_weight
            } else {
                0.0
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct SelectorEvaluation {
    membership: f32,
    distance: f32,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BiomeTrait {
    Terrestrial,
    Aquatic,
    Underground,
    Frozen,
    Arid,
    Humid,
    Forested,
    Rocky,
    Custom(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BiomeDefinition {
    pub id: BiomeId,
    pub name: String,
    pub selector: BiomeSelector,
    pub traits: BTreeSet<BiomeTrait>,
    pub surface_profile: SurfaceProfileId,
    pub material_palette: MaterialPaletteId,
    pub feature_sets: Vec<FeatureSetId>,
    pub audio_profile: Option<AudioProfileId>,
    pub animation_profile: Option<AnimationProfileId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BiomeInfluence {
    pub biome: BiomeId,
    pub weight: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BiomeSample {
    pub primary: BiomeId,
    pub influences: Vec<BiomeInfluence>,
}

impl BiomeSample {
    pub fn weight(&self, biome: &BiomeId) -> f32 {
        self.influences
            .iter()
            .find(|influence| &influence.biome == biome)
            .map(|influence| influence.weight)
            .unwrap_or(0.0)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BiomeRegistry {
    pub biomes: Vec<BiomeDefinition>,
}

impl BiomeRegistry {
    pub fn validate(&self) -> Result<(), BiomeError> {
        if self.biomes.is_empty() {
            return Err(BiomeError::NoBiomes);
        }
        let mut ids = BTreeSet::new();
        for biome in &self.biomes {
            if biome.id.0.trim().is_empty() || !ids.insert(biome.id.clone()) {
                return Err(BiomeError::DuplicateOrEmptyId(biome.id.clone()));
            }
            if biome.selector.constraints.is_empty() {
                continue;
            }
            for constraint in &biome.selector.constraints {
                if !constraint.range.validate()
                    || !constraint.weight.is_finite()
                    || constraint.weight <= 0.0
                {
                    return Err(BiomeError::InvalidConstraint(biome.id.clone()));
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, id: &BiomeId) -> Option<&BiomeDefinition> {
        self.biomes.iter().find(|biome| &biome.id == id)
    }

    pub fn resolve(
        &self,
        context: &EnvironmentContext,
        max_influences: usize,
    ) -> Result<BiomeSample, BiomeError> {
        self.validate()?;
        Ok(self.resolve_validated(context, max_influences))
    }

    /// Resolve a registry that has already passed [`Self::validate`]. World
    /// profile compilation uses this path so procedural sampling does not
    /// rebuild validation sets for every voxel or surface point.
    pub(crate) fn resolve_validated(
        &self,
        context: &EnvironmentContext,
        max_influences: usize,
    ) -> BiomeSample {
        let mut candidates: Vec<_> = self
            .biomes
            .iter()
            .map(|biome| (biome, biome.selector.evaluate(context)))
            .collect();
        let has_membership = candidates
            .iter()
            .any(|(_, evaluation)| evaluation.membership > 0.0);
        candidates.sort_by(|(left, left_evaluation), (right, right_evaluation)| {
            let ordering = if has_membership {
                right_evaluation
                    .membership
                    .partial_cmp(&left_evaluation.membership)
                    .unwrap_or(Ordering::Equal)
            } else {
                left_evaluation
                    .distance
                    .partial_cmp(&right_evaluation.distance)
                    .unwrap_or(Ordering::Equal)
            };
            ordering
                .then_with(|| right.selector.priority.cmp(&left.selector.priority))
                .then_with(|| left.id.cmp(&right.id))
        });
        let count = max_influences.max(1).min(candidates.len());
        let mut raw: Vec<_> = candidates
            .into_iter()
            .take(count)
            .map(|(biome, evaluation)| {
                let weight = if has_membership {
                    evaluation.membership
                } else {
                    1.0 / (1.0 + evaluation.distance)
                };
                (biome.id.clone(), weight)
            })
            .collect();
        let total = raw.iter().map(|(_, weight)| *weight).sum::<f32>();
        if total <= f32::EPSILON {
            raw[0].1 = 1.0;
        }
        let total = raw.iter().map(|(_, weight)| *weight).sum::<f32>();
        let primary = raw[0].0.clone();
        BiomeSample {
            primary,
            influences: raw
                .into_iter()
                .map(|(biome, weight)| BiomeInfluence {
                    biome,
                    weight: weight / total,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BiomeError {
    NoBiomes,
    DuplicateOrEmptyId(BiomeId),
    InvalidConstraint(BiomeId),
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::environment::{GeneratedEnvironment, RuntimeEnvironmentState};

    fn biome(id: &str, temperature: FieldRange) -> BiomeDefinition {
        BiomeDefinition {
            id: BiomeId::new(id),
            name: id.to_string(),
            selector: BiomeSelector {
                constraints: vec![FieldConstraint {
                    channel: EnvironmentChannel::Temperature,
                    range: temperature,
                    weight: 1.0,
                }],
                priority: 0,
            },
            traits: BTreeSet::new(),
            surface_profile: SurfaceProfileId::new("ground"),
            material_palette: MaterialPaletteId::new("default"),
            feature_sets: Vec::new(),
            audio_profile: None,
            animation_profile: None,
        }
    }

    fn context(temperature: f32) -> EnvironmentContext {
        EnvironmentContext::compose(
            &GeneratedEnvironment {
                world_seed: 7,
                position: [0.0; 3],
                normal: [0.0, 1.0, 0.0],
                fields: BTreeMap::from([(EnvironmentChannel::Temperature, temperature)]),
            },
            &RuntimeEnvironmentState::default(),
            &BTreeMap::new(),
        )
        .unwrap()
    }

    #[test]
    fn overlapping_parameter_ranges_produce_normalized_biome_influences() {
        let registry = BiomeRegistry {
            biomes: vec![
                biome(
                    "tundra",
                    FieldRange {
                        min: -1.0,
                        max: 0.0,
                        falloff: 0.4,
                    },
                ),
                biome(
                    "plains",
                    FieldRange {
                        min: 0.0,
                        max: 1.0,
                        falloff: 0.4,
                    },
                ),
            ],
        };
        let sample = registry.resolve(&context(0.0), 2).unwrap();
        assert_eq!(sample.influences.len(), 2);
        assert!(
            (sample
                .influences
                .iter()
                .map(|item| item.weight)
                .sum::<f32>()
                - 1.0)
                .abs()
                < 0.0001
        );
        assert_eq!(sample.primary, BiomeId::new("plains"));
    }

    #[test]
    fn nearest_interval_is_used_when_no_falloff_contains_the_sample() {
        let registry = BiomeRegistry {
            biomes: vec![
                biome(
                    "cold",
                    FieldRange {
                        min: -1.0,
                        max: -0.5,
                        falloff: 0.0,
                    },
                ),
                biome(
                    "warm",
                    FieldRange {
                        min: 0.5,
                        max: 1.0,
                        falloff: 0.0,
                    },
                ),
            ],
        };
        assert_eq!(
            registry.resolve(&context(0.3), 1).unwrap().primary,
            BiomeId::new("warm")
        );
    }

    #[test]
    fn duplicate_ids_are_rejected_before_resolution() {
        let duplicate = biome(
            "same",
            FieldRange {
                min: 0.0,
                max: 1.0,
                falloff: 0.0,
            },
        );
        let registry = BiomeRegistry {
            biomes: vec![duplicate.clone(), duplicate],
        };
        assert!(matches!(
            registry.validate(),
            Err(BiomeError::DuplicateOrEmptyId(_))
        ));
    }
}
