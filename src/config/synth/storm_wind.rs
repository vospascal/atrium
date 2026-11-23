//! The `storm_wind` synth: its YAML parameters and how they configure
//! `crate::synth::storm_wind::StormWindSource`.

use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;
use serde::Deserialize;

use super::wind_behaviour::BehaviourParams;

use crate::synth::storm_wind::StormWindSource;

#[derive(Deserialize, Clone, Debug, Default)]
pub(in crate::config) struct StormWindSynthParams {
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
    pub turbulence_depth: Option<f32>,
    pub gust_brightness: Option<f32>,
    pub turbulence_brightness: Option<f32>,
    pub debris_level: Option<f32>,
    pub structure_level: Option<f32>,
    pub pressure_gain: Option<f32>,
    pub roar_gain: Option<f32>,
    pub shear_gain: Option<f32>,
    pub tear_gain: Option<f32>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

impl StormWindSynthParams {
    /// Construct the generator at `position`. `default_seed` applies when
    /// the params don't pin one.
    pub(super) fn build_generator(
        &self,
        position: Vec3,
        default_seed: u64,
    ) -> Box<dyn SoundSource> {
        let mut generator = StormWindSource::new(
            position,
            self.min_speed.unwrap_or(8.0),
            self.max_speed.unwrap_or(18.0),
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
        if let Some(behaviour) = &self.behaviour {
            generator.set_behaviour(behaviour.to_behaviour());
        }
        if let Some(value) = self.gust_strength {
            generator.gust_strength = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.rise_bias {
            generator.rise_bias = value.clamp(-1.0, 1.0);
        }
        if let Some(value) = self.turbulence_depth {
            generator.turbulence_depth = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.gust_brightness {
            generator.gust_brightness = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.turbulence_brightness {
            generator.turbulence_brightness = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.debris_level {
            generator.debris_level = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.structure_level {
            generator.structure_level = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.pressure_gain {
            generator.pressure_gain = value.max(0.0);
        }
        if let Some(value) = self.roar_gain {
            generator.roar_gain = value.max(0.0);
        }
        if let Some(value) = self.shear_gain {
            generator.shear_gain = value.max(0.0);
        }
        if let Some(value) = self.tear_gain {
            generator.tear_gain = value.max(0.0);
        }
        if let Some(value) = self.master_gain {
            generator.master_gain = value.clamp(0.0, 2.0);
        }
        Box::new(generator)
    }
}

/// The keys of the layers the generator hands over, in its order.
pub(super) const LAYERS: &[&str] = &[
    "pressure",
    "roar",
    "shear",
    "tearing",
    "debris",
    "structure",
];
