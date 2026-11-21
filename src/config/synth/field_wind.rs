//! The `field_wind` synth: its YAML parameters and how they configure
//! `crate::synth::field_wind::FieldWindSource`.

use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;
use serde::Deserialize;

use crate::synth::field_wind::FieldWindSource;
use crate::synth::wind_behaviour::{WindBehaviour, WindEvent, WindMove};

/// What the wind does now and then, as a preset writes it:
///
/// ```yaml
/// behaviour:
///   rest: [40, 120]          # steady wind between two moves (s)
///   repertoire:
///     - { move: gust,  chance: 3, strength: [0.3, 0.8], seconds: [3, 8] }
///     - { move: swell, chance: 2, strength: [0.2, 0.5], seconds: [10, 25] }
///     - { move: lull,  chance: 1, strength: [0.3, 0.6], seconds: [8, 20] }
/// ```
#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub(in crate::config) struct BehaviourParams {
    pub rest: [f32; 2],
    pub repertoire: Vec<MoveParams>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub(in crate::config) struct MoveParams {
    #[serde(rename = "move")]
    pub kind: MoveKind,
    #[serde(default = "one")]
    pub chance: f32,
    pub strength: [f32; 2],
    pub seconds: [f32; 2],
}

#[derive(Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "lowercase")]
pub(in crate::config) enum MoveKind {
    Gust,
    Swell,
    Lull,
}

fn one() -> f32 {
    1.0
}

impl BehaviourParams {
    fn to_behaviour(&self) -> WindBehaviour {
        WindBehaviour {
            rest: (self.rest[0], self.rest[1]),
            events: self
                .repertoire
                .iter()
                .map(|entry| WindEvent {
                    kind: match entry.kind {
                        MoveKind::Gust => WindMove::Gust,
                        MoveKind::Swell => WindMove::Swell,
                        MoveKind::Lull => WindMove::Lull,
                    },
                    chance: entry.chance,
                    strength: (entry.strength[0], entry.strength[1]),
                    seconds: (entry.seconds[0], entry.seconds[1]),
                })
                .collect(),
        }
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
pub(in crate::config) struct FieldWindSynthParams {
    pub min_speed: Option<f32>,
    pub max_speed: Option<f32>,
    pub change_time_min: Option<f32>,
    pub change_time_max: Option<f32>,
    pub gust_duration_min: Option<f32>,
    pub gust_duration_max: Option<f32>,
    pub turbulence_time_min: Option<f32>,
    pub turbulence_time_max: Option<f32>,
    pub gust_strength: Option<f32>,
    /// What the wind does now and then; without it, it gusts about half the time.
    pub behaviour: Option<BehaviourParams>,
    pub rise_bias: Option<f32>,
    pub gust_brightness: Option<f32>,
    pub turbulence_brightness: Option<f32>,
    pub low_gain: Option<f32>,
    pub body_gain: Option<f32>,
    pub mid_gain: Option<f32>,
    pub presence_gain: Option<f32>,
    pub air_gain: Option<f32>,
    pub turbulence_depth: Option<f32>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

impl FieldWindSynthParams {
    /// Construct the generator at `position`. `default_seed` applies when
    /// the params don't pin one.
    pub(super) fn build_generator(
        &self,
        position: Vec3,
        default_seed: u64,
    ) -> Box<dyn SoundSource> {
        let mut generator = FieldWindSource::new(
            position,
            self.min_speed.unwrap_or(1.0),
            self.max_speed.unwrap_or(8.0),
            self.seed.unwrap_or(default_seed),
        );
        let (default_min, default_max) = generator.change_time_range();
        generator.set_change_time_range(
            self.change_time_min.unwrap_or(default_min),
            self.change_time_max.unwrap_or(default_max),
        );
        let (default_min, default_max) = generator.gust_duration_range();
        generator.set_gust_duration_range(
            self.gust_duration_min.unwrap_or(default_min),
            self.gust_duration_max.unwrap_or(default_max),
        );
        let (default_min, default_max) = generator.turbulence_time_range();
        generator.set_turbulence_time_range(
            self.turbulence_time_min.unwrap_or(default_min),
            self.turbulence_time_max.unwrap_or(default_max),
        );
        if let Some(value) = self.gust_strength {
            generator.gust_strength = value.clamp(0.0, 1.0);
        }
        if let Some(behaviour) = &self.behaviour {
            generator.set_behaviour(behaviour.to_behaviour());
        }
        if let Some(value) = self.rise_bias {
            generator.rise_bias = value.clamp(-1.0, 1.0);
        }
        if let Some(value) = self.gust_brightness {
            generator.gust_brightness = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.turbulence_brightness {
            generator.turbulence_brightness = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.low_gain {
            generator.low_gain = value;
        }
        if let Some(value) = self.body_gain {
            generator.body_gain = value;
        }
        if let Some(value) = self.mid_gain {
            generator.mid_gain = value;
        }
        if let Some(value) = self.presence_gain {
            generator.presence_gain = value;
        }
        if let Some(value) = self.air_gain {
            generator.air_gain = value;
        }
        if let Some(value) = self.turbulence_depth {
            generator.turbulence_depth = value;
        }
        if let Some(value) = self.master_gain {
            generator.master_gain = value;
        }
        Box::new(generator)
    }
}
