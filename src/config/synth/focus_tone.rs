//! The `focus_tone` synth: its YAML parameters and how they configure
//! `crate::synth::focus_tone::FocusToneSource`.

use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;
use serde::Deserialize;

use crate::synth::focus_tone::{line_count, FocusToneSettings, FocusToneSource};

#[derive(Deserialize, Clone, Debug, Default)]
pub(in crate::config) struct FocusToneSynthParams {
    pub pitch: Option<f32>,
    pub floor: Option<f32>,
    pub rotation_period: Option<f32>,
    pub rotation_depth: Option<f32>,
    pub master_gain: Option<f32>,
}

impl FocusToneSynthParams {
    /// Construct the generator at `position`. It has no randomness, so the
    /// seed every generator is offered goes unused.
    fn settings(&self) -> FocusToneSettings {
        let defaults = FocusToneSettings::default();
        FocusToneSettings {
            pitch_hz: self.pitch.unwrap_or(defaults.pitch_hz),
            floor_hz: self.floor.unwrap_or(defaults.floor_hz),
            rotation_period_seconds: self
                .rotation_period
                .unwrap_or(defaults.rotation_period_seconds),
            rotation_depth_db: self.rotation_depth.unwrap_or(defaults.rotation_depth_db),
            master_gain: self.master_gain.unwrap_or(defaults.master_gain),
        }
    }

    pub(super) fn build_generator(
        &self,
        position: Vec3,
        _default_seed: u64,
    ) -> Box<dyn SoundSource> {
        Box::new(FocusToneSource::new(position, self.settings()))
    }

    /// One layer per line, keyed `pitch`, `octave-1`, `octave-2` and
    /// `octave-3` and named by its frequency ("264 Hz"), as the stack is at
    /// these settings.
    pub(super) fn layers(&self) -> Vec<(String, String)> {
        let settings = self.settings().clamped();
        (0..line_count(settings.pitch_hz, settings.floor_hz))
            .map(|line| {
                let key = match line {
                    0 => "pitch".to_string(),
                    octave => format!("octave-{octave}"),
                };
                let frequency = settings.pitch_hz as f64 / (1u32 << line) as f64;
                (key, super::hertz(frequency))
            })
            .collect()
    }
}
