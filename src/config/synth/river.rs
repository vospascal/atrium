//! The `river` synth: its YAML parameters and how they configure
//! `crate::synth::river::RiverSource`.

use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;
use serde::Deserialize;

use crate::synth::river::RiverSource;

#[derive(Deserialize, Clone, Debug, Default)]
pub(in crate::config) struct RiverSynthParams {
    pub min_flow_speed: Option<f32>,
    pub max_flow_speed: Option<f32>,
    pub change_time_min: Option<f32>,
    pub change_time_max: Option<f32>,
    pub eddy_time_min: Option<f32>,
    pub eddy_time_max: Option<f32>,
    pub eddy_depth: Option<f32>,
    pub body_gain: Option<f32>,
    pub current_gain: Option<f32>,
    pub bubble_activity: Option<f32>,
    pub splash_rate: Option<f32>,
    pub splash_gain: Option<f32>,
    pub spray_gain: Option<f32>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

impl RiverSynthParams {
    /// Construct the generator at `position`. `default_seed` applies when
    /// the params don't pin one.
    pub(super) fn build_generator(
        &self,
        position: Vec3,
        default_seed: u64,
    ) -> Box<dyn SoundSource> {
        let mut generator = RiverSource::new(
            position,
            self.min_flow_speed.unwrap_or(0.3),
            self.max_flow_speed.unwrap_or(1.2),
            self.seed.unwrap_or(default_seed),
        );
        let (default_min, default_max) = generator.change_time_range();
        generator.set_change_time_range(
            self.change_time_min.unwrap_or(default_min),
            self.change_time_max.unwrap_or(default_max),
        );
        let (default_min, default_max) = generator.eddy_time_range();
        generator.set_eddy_time_range(
            self.eddy_time_min.unwrap_or(default_min),
            self.eddy_time_max.unwrap_or(default_max),
        );
        if let Some(value) = self.eddy_depth {
            generator.eddy_depth = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.body_gain {
            generator.body_gain = value.max(0.0);
        }
        if let Some(value) = self.current_gain {
            generator.current_gain = value.max(0.0);
        }
        if let Some(value) = self.bubble_activity {
            generator.bubble_activity = value.max(0.0);
        }
        if let Some(value) = self.splash_rate {
            generator.splash_rate = value.max(0.0);
        }
        if let Some(value) = self.splash_gain {
            generator.splash_gain = value.max(0.0);
        }
        if let Some(value) = self.spray_gain {
            generator.spray_gain = value.max(0.0);
        }
        if let Some(value) = self.master_gain {
            generator.master_gain = value.clamp(0.0, 2.0);
        }
        Box::new(generator)
    }
}

/// The keys of the layers the generator hands over, in its order.
pub(super) const LAYERS: &[&str] = &["body", "current", "spray", "bubbles-and-splashes"];
