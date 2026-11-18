//! The `rain_v2` synth: its YAML parameters and how they configure
//! `crate::synth::rain_v2::RainSourceV2`.

use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;
use serde::Deserialize;

use crate::synth::rain_v2::RainSourceV2;

#[derive(Deserialize, Clone, Debug, Default)]
pub(in crate::config) struct RainV2SynthParams {
    pub intensity: Option<f32>,
    pub drip_rate: Option<f32>,
    pub impact_gain: Option<f32>,
    pub bubble_gain: Option<f32>,
    pub bed_gain: Option<f32>,
    pub texture_gain: Option<f32>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

impl RainV2SynthParams {
    /// Construct the generator at `position`. `default_seed` applies when
    /// the params don't pin one.
    pub(super) fn build_generator(
        &self,
        position: Vec3,
        default_seed: u64,
    ) -> Box<dyn SoundSource> {
        let mut generator = RainSourceV2::new(
            position,
            self.intensity.unwrap_or(0.5),
            self.seed.unwrap_or(default_seed),
        );
        if let Some(value) = self.drip_rate {
            generator.drip_rate = value;
        }
        if let Some(value) = self.impact_gain {
            generator.impact_gain = value;
        }
        if let Some(value) = self.bubble_gain {
            generator.bubble_gain = value;
        }
        if let Some(value) = self.bed_gain {
            generator.bed_gain = value;
        }
        if let Some(value) = self.texture_gain {
            generator.texture_gain = value;
        }
        if let Some(value) = self.master_gain {
            generator.master_gain = value;
        }
        Box::new(generator)
    }
}
