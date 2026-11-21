//! The `canopy_wind` synth: its YAML parameters and how they configure
//! `crate::synth::canopy_wind::CanopyWindSource`.

use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;
use serde::Deserialize;

use super::wind_behaviour::BehaviourParams;

use crate::synth::canopy_wind::CanopyWindSource;

#[derive(Deserialize, Clone, Debug, Default)]
pub(in crate::config) struct CanopyWindSynthParams {
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
    pub foliage_density: Option<f32>,
    pub leaf_dryness: Option<f32>,
    pub branch_level: Option<f32>,
    pub body_gain: Option<f32>,
    pub rustle_gain: Option<f32>,
    pub contact_gain: Option<f32>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

impl CanopyWindSynthParams {
    /// Construct the generator at `position`. `default_seed` applies when
    /// the params don't pin one.
    pub(super) fn build_generator(
        &self,
        position: Vec3,
        default_seed: u64,
    ) -> Box<dyn SoundSource> {
        let mut generator = CanopyWindSource::new(
            position,
            self.min_speed.unwrap_or(1.5),
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
        if let Some(value) = self.foliage_density {
            generator.foliage_density = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.leaf_dryness {
            generator.leaf_dryness = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.branch_level {
            generator.branch_level = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.body_gain {
            generator.body_gain = value.max(0.0);
        }
        if let Some(value) = self.rustle_gain {
            generator.rustle_gain = value.max(0.0);
        }
        if let Some(value) = self.contact_gain {
            generator.contact_gain = value.max(0.0);
        }
        if let Some(value) = self.master_gain {
            generator.master_gain = value.clamp(0.0, 2.0);
        }
        Box::new(generator)
    }
}
