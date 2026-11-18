//! The `wisp` synth: its YAML parameters and how they configure
//! `crate::synth::wisp::WispSource`.

use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;
use serde::Deserialize;

use crate::synth::wisp::WispSource;

/// A small flying creature with its own flight; see `crate::synth::wisp`.
#[derive(Deserialize, Clone, Debug, Default)]
pub(in crate::config) struct WispSynthParams {
    /// Horizontal radius of its territory around the source position (m).
    pub range: Option<f32>,
    pub min_height: Option<f32>,
    pub max_height: Option<f32>,
    /// Top speed in a dart (m/s).
    pub max_speed: Option<f32>,
    pub hover_time_min: Option<f32>,
    pub hover_time_max: Option<f32>,
    pub wing_gain: Option<f32>,
    pub hum_gain: Option<f32>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

impl WispSynthParams {
    /// Construct the generator at `position`. `default_seed` applies when
    /// the params don't pin one.
    pub(super) fn build_generator(
        &self,
        position: Vec3,
        default_seed: u64,
    ) -> Box<dyn SoundSource> {
        let mut generator = WispSource::new(position, self.seed.unwrap_or(default_seed));
        if let Some(value) = self.range {
            generator.range = value.max(0.0);
        }
        if let Some(value) = self.min_height {
            generator.min_height = value.max(0.0);
        }
        if let Some(value) = self.max_height {
            generator.max_height = value.max(generator.min_height);
        }
        if let Some(value) = self.max_speed {
            generator.max_speed = value.max(0.1);
        }
        if let Some(value) = self.hover_time_min {
            generator.hover_time_min = value.max(0.0);
        }
        if let Some(value) = self.hover_time_max {
            generator.hover_time_max = value.max(generator.hover_time_min);
        }
        if let Some(value) = self.wing_gain {
            generator.wing_gain = value.max(0.0);
        }
        if let Some(value) = self.hum_gain {
            generator.hum_gain = value.max(0.0);
        }
        if let Some(value) = self.master_gain {
            generator.master_gain = value.clamp(0.0, 2.0);
        }
        Box::new(generator)
    }
}
