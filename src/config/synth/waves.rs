//! The `waves` synth: its YAML parameters and how they configure
//! `crate::synth::wave::WaveSource`.

use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;
use serde::Deserialize;

use crate::synth::wave::WaveSource;

#[derive(Deserialize, Clone, Debug, Default)]
pub(in crate::config) struct WaveSynthParams {
    pub period: Option<f32>,
    pub crash_prob: Option<f32>,
    pub roar_level: Option<f32>,
    pub hiss_level: Option<f32>,
    pub crash_gain: Option<f32>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

impl WaveSynthParams {
    /// Construct the generator at `position`. `default_seed` applies when
    /// the params don't pin one.
    pub(super) fn build_generator(
        &self,
        position: Vec3,
        default_seed: u64,
    ) -> Box<dyn SoundSource> {
        let mut generator = WaveSource::new(
            position,
            self.period.unwrap_or(6.0),
            self.crash_prob.unwrap_or(0.25),
            self.seed.unwrap_or(default_seed),
        );
        if let Some(value) = self.roar_level {
            generator.roar_level = value;
        }
        if let Some(value) = self.hiss_level {
            generator.hiss_level = value;
        }
        if let Some(value) = self.crash_gain {
            generator.crash_gain = value;
        }
        if let Some(value) = self.master_gain {
            generator.master_gain = value;
        }
        Box::new(generator)
    }
}
