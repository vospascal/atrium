//! The `noise_colour` synth: its YAML parameters and how they configure
//! `crate::synth::noise_colour::NoiseColourSource`.

use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;
use serde::Deserialize;

use crate::synth::noise_colour::{
    NoiseColour, NoiseColourSettings, NoiseColourSource, ORANGE_LAYERS,
};

#[derive(Deserialize, Clone, Debug, Default)]
pub(in crate::config) struct NoiseColourSynthParams {
    /// white, pink, brown, red, blue, violet, grey, green or orange; white
    /// when unset.
    pub colour: Option<NoiseColour>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

impl NoiseColourSynthParams {
    fn settings(&self) -> NoiseColourSettings {
        let defaults = NoiseColourSettings::default();
        NoiseColourSettings {
            colour: self.colour.unwrap_or(defaults.colour),
            master_gain: self.master_gain.unwrap_or(defaults.master_gain),
        }
    }

    /// Construct the generator at `position`. `default_seed` applies when
    /// the params don't pin one.
    pub(super) fn build_generator(
        &self,
        position: Vec3,
        default_seed: u64,
    ) -> Box<dyn SoundSource> {
        Box::new(NoiseColourSource::new(
            position,
            self.settings(),
            self.seed.unwrap_or(default_seed),
        ))
    }

    /// Orange's two layers, deep and bright; every other colour is one
    /// stream and names none.
    pub(super) fn layers(&self) -> Vec<(String, String)> {
        match self.settings().colour {
            NoiseColour::Orange => ORANGE_LAYERS
                .iter()
                .map(|key| (key.to_string(), key.to_string()))
                .collect(),
            _ => Vec::new(),
        }
    }
}
