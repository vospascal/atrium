//! Renderer-independent environmental inputs shared by world generation,
//! biome selection, surface composition, materials, audio, and animation.
//!
//! Generated fields describe stable facts about the world. Runtime state adds
//! mutable facts such as season and weather. Consumers receive the merged
//! [`EnvironmentContext`] and never depend on a concrete biome identifier.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentChannel {
    Temperature,
    Humidity,
    Continentalness,
    Erosion,
    Weirdness,
    Elevation,
    Depth,
    Slope,
    Exposure,
    WaterTable,
    SoilMoisture,
    Fertility,
    Geology,
    SnowAccumulation,
    Wetness,
    Wind,
    #[serde(untagged)]
    Custom(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpatialGranularity {
    Global,
    PerRegion,
    PerChunk,
    PerVoxel,
    PerSample,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateFrequency {
    CompileTime,
    WorldEvent,
    SimulationTick,
    PerFrame,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvaluationClass {
    pub spatial: SpatialGranularity,
    pub update: UpdateFrequency,
}

impl EvaluationClass {
    pub(crate) const STATIC_WORLD: Self = Self {
        spatial: SpatialGranularity::PerVoxel,
        update: UpdateFrequency::CompileTime,
    };
    pub const CHUNK_EVENT: Self = Self {
        spatial: SpatialGranularity::PerChunk,
        update: UpdateFrequency::WorldEvent,
    };
    pub(crate) const FRAME_UNIFORM: Self = Self {
        spatial: SpatialGranularity::Global,
        update: UpdateFrequency::PerFrame,
    };

    pub(crate) fn combines(self, other: Self) -> Self {
        Self {
            spatial: self.spatial.max(other.spatial),
            update: self.update.max(other.update),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherKind {
    Clear,
    Rain,
    Snow,
    Storm,
    Fog,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeEnvironmentState {
    pub season: Season,
    /// Normalized progress through the current season.
    pub season_phase: f32,
    /// Normalized time of day, where 0 and 1 are midnight.
    pub day_phase: f32,
    pub weather: WeatherKind,
    pub weather_intensity: f32,
    pub temperature_offset: f32,
    pub humidity_offset: f32,
}

impl Default for RuntimeEnvironmentState {
    fn default() -> Self {
        Self {
            season: Season::Summer,
            season_phase: 0.0,
            day_phase: 0.5,
            weather: WeatherKind::Clear,
            weather_intensity: 0.0,
            temperature_offset: 0.0,
            humidity_offset: 0.0,
        }
    }
}

impl RuntimeEnvironmentState {
    pub fn validate(&self) -> Result<(), EnvironmentError> {
        for (name, value) in [
            ("season_phase", self.season_phase),
            ("day_phase", self.day_phase),
            ("weather_intensity", self.weather_intensity),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(EnvironmentError::InvalidUnitValue(name));
            }
        }
        if !self.temperature_offset.is_finite() || !self.humidity_offset.is_finite() {
            return Err(EnvironmentError::NonFiniteOffset);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeneratedEnvironment {
    pub world_seed: u64,
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub fields: BTreeMap<EnvironmentChannel, f32>,
}

impl GeneratedEnvironment {
    pub fn validate(&self) -> Result<(), EnvironmentError> {
        if !self.position.iter().all(|value| value.is_finite())
            || !self.normal.iter().all(|value| value.is_finite())
            || !self.fields.values().all(|value| value.is_finite())
        {
            return Err(EnvironmentError::NonFiniteGeneratedValue);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentContext {
    pub world_seed: u64,
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub season: Season,
    pub season_phase: f32,
    pub day_phase: f32,
    pub weather: WeatherKind,
    pub weather_intensity: f32,
    fields: BTreeMap<EnvironmentChannel, f32>,
}

impl EnvironmentContext {
    pub fn compose(
        generated: &GeneratedEnvironment,
        runtime: &RuntimeEnvironmentState,
        local_overrides: &BTreeMap<EnvironmentChannel, f32>,
    ) -> Result<Self, EnvironmentError> {
        generated.validate()?;
        runtime.validate()?;
        if !local_overrides.values().all(|value| value.is_finite()) {
            return Err(EnvironmentError::NonFiniteGeneratedValue);
        }
        let mut fields = generated.fields.clone();
        *fields.entry(EnvironmentChannel::Temperature).or_default() += runtime.temperature_offset;
        *fields.entry(EnvironmentChannel::Humidity).or_default() += runtime.humidity_offset;
        for (channel, value) in local_overrides {
            fields.insert(channel.clone(), *value);
        }
        Ok(Self {
            world_seed: generated.world_seed,
            position: generated.position,
            normal: generated.normal,
            season: runtime.season,
            season_phase: runtime.season_phase,
            day_phase: runtime.day_phase,
            weather: runtime.weather,
            weather_intensity: runtime.weather_intensity,
            fields,
        })
    }

    pub fn field(&self, channel: &EnvironmentChannel) -> f32 {
        self.fields.get(channel).copied().unwrap_or(0.0)
    }

    pub fn fields(&self) -> &BTreeMap<EnvironmentChannel, f32> {
        &self.fields
    }

    /// Stable random value for procedural decisions. It is independent of
    /// evaluation order and therefore safe across chunk boundaries and threads.
    pub(crate) fn stable_random(&self, salt: u64) -> f32 {
        let coordinates = self.position.map(|value| value.floor() as i64 as u64);
        let mut hash = self.world_seed ^ salt.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        for coordinate in coordinates {
            hash ^= coordinate
                .wrapping_add(0x9e37_79b9_7f4a_7c15)
                .wrapping_add(hash << 6)
                .wrapping_add(hash >> 2);
            hash = splitmix64(hash);
        }
        // The upper 24 bits are exactly representable as f32. Dividing by
        // 2^24 (rather than 2^24 - 1) keeps the contract half-open: [0, 1).
        // Probability-one decisions can therefore never fail at the top edge.
        ((hash >> 40) as u32 as f32) / (1_u32 << 24) as f32
    }
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentError {
    InvalidUnitValue(&'static str),
    NonFiniteOffset,
    NonFiniteGeneratedValue,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generated() -> GeneratedEnvironment {
        GeneratedEnvironment {
            world_seed: 42,
            position: [12.0, 70.0, -8.0],
            normal: [0.0, 1.0, 0.0],
            fields: BTreeMap::from([
                (EnvironmentChannel::Temperature, 0.25),
                (EnvironmentChannel::Humidity, 0.4),
            ]),
        }
    }

    #[test]
    fn runtime_and_local_environment_are_composed_without_mutating_base_fields() {
        let base = generated();
        let runtime = RuntimeEnvironmentState {
            season: Season::Winter,
            temperature_offset: -0.3,
            humidity_offset: 0.1,
            ..RuntimeEnvironmentState::default()
        };
        let local = BTreeMap::from([(EnvironmentChannel::Wetness, 0.8)]);
        let context = EnvironmentContext::compose(&base, &runtime, &local).unwrap();
        assert_eq!(context.season, Season::Winter);
        assert!((context.field(&EnvironmentChannel::Temperature) + 0.05).abs() < 0.0001);
        assert_eq!(context.field(&EnvironmentChannel::Humidity), 0.5);
        assert_eq!(context.field(&EnvironmentChannel::Wetness), 0.8);
        assert_eq!(base.fields[&EnvironmentChannel::Temperature], 0.25);
    }

    #[test]
    fn stable_random_is_order_independent_and_position_sensitive() {
        let runtime = RuntimeEnvironmentState::default();
        let first = EnvironmentContext::compose(&generated(), &runtime, &BTreeMap::new()).unwrap();
        let mut moved = generated();
        moved.position[0] += 1.0;
        let second = EnvironmentContext::compose(&moved, &runtime, &BTreeMap::new()).unwrap();
        assert_eq!(first.stable_random(9), first.stable_random(9));
        assert_ne!(first.stable_random(9), second.stable_random(9));
    }

    #[test]
    fn stable_random_is_always_half_open() {
        let runtime = RuntimeEnvironmentState::default();
        for seed in 0..10_000_u64 {
            let context = EnvironmentContext::compose(
                &GeneratedEnvironment {
                    world_seed: seed,
                    position: [seed as f32, 0.0, -(seed as f32)],
                    normal: [0.0, 1.0, 0.0],
                    fields: BTreeMap::new(),
                },
                &runtime,
                &BTreeMap::new(),
            )
            .unwrap();
            let value = context.stable_random(seed.rotate_left(17));
            assert!((0.0..1.0).contains(&value), "{value} for seed {seed}");
        }
    }

    #[test]
    fn evaluation_classes_combine_to_the_most_expensive_dependencies() {
        assert_eq!(
            EvaluationClass::STATIC_WORLD.combines(EvaluationClass::FRAME_UNIFORM),
            EvaluationClass {
                spatial: SpatialGranularity::PerVoxel,
                update: UpdateFrequency::PerFrame,
            }
        );
    }
}
