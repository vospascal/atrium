//! The `focus_tone` synth: its YAML parameters and how they configure
//! `crate::synth::focus_tone::FocusToneSource`.

use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;
use serde::Deserialize;

use crate::synth::focus_tone::{FocusToneSettings, FocusToneSource};

#[derive(Deserialize, Clone, Debug, Default)]
pub(in crate::config) struct FocusToneSynthParams {
    pub pitch: Option<f32>,
    pub brightness: Option<f32>,
    pub fundamental_level: Option<f32>,
    pub overtone_level: Option<f32>,
    pub shimmer_rate_min: Option<f32>,
    pub shimmer_rate_max: Option<f32>,
    pub shimmer_depth: Option<f32>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

impl FocusToneSynthParams {
    /// Construct the generator at `position`. `default_seed` applies when
    /// the params don't pin one.
    pub(super) fn build_generator(
        &self,
        position: Vec3,
        default_seed: u64,
    ) -> Box<dyn SoundSource> {
        let defaults = FocusToneSettings::default();
        let settings = FocusToneSettings {
            pitch_hz: self.pitch.unwrap_or(defaults.pitch_hz),
            brightness: self.brightness.unwrap_or(defaults.brightness),
            fundamental_level: self.fundamental_level.unwrap_or(defaults.fundamental_level),
            overtone_level: self.overtone_level.unwrap_or(defaults.overtone_level),
            shimmer_rate_min_hz: self
                .shimmer_rate_min
                .unwrap_or(defaults.shimmer_rate_min_hz),
            shimmer_rate_max_hz: self
                .shimmer_rate_max
                .unwrap_or(defaults.shimmer_rate_max_hz),
            shimmer_depth: self.shimmer_depth.unwrap_or(defaults.shimmer_depth),
            master_gain: self.master_gain.unwrap_or(defaults.master_gain),
        };
        Box::new(FocusToneSource::new(
            position,
            settings,
            self.seed.unwrap_or(default_seed),
        ))
    }
}
