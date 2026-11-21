//! The `thunder` synth: its YAML parameters and how they configure
//! `crate::synth::thunder::ThunderSource`.

use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;
use serde::Deserialize;

use crate::plan::{Rests, Rhythm, TakeRhythm};
use crate::synth::thunder::{
    clap_seconds, ThunderSource, PRESET_FAR, PRESET_MASSIVE, PRESET_MID, PRESET_NEAR,
};

#[derive(Deserialize, Clone, Debug, Default)]
pub(in crate::config) struct ThunderSynthParams {
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
    /// Shortest and longest rest between two claps (s), at full storm. A
    /// storm that is weaker strikes less often: thunder follows `storm`.
    pub gap: Option<[f32; 2]>,
}

/// Rest between claps when a scene gives none (s).
const DEFAULT_GAP: [f32; 2] = [8.0, 40.0];

impl ThunderSynthParams {
    /// Construct the generator at `position`. `default_seed` applies when
    /// the params don't pin one.
    ///
    /// Thunder is silent until the session's plan tells it to strike
    /// (`SoundSource::schedule`); it keeps those appointments on the
    /// session's clock.
    pub(super) fn build_generator(
        &self,
        position: Vec3,
        default_seed: u64,
    ) -> Box<dyn SoundSource> {
        let mut generator = ThunderSource::new(position, self.seed.unwrap_or(default_seed));
        if let Some(value) = self.master_gain {
            generator.master_gain = value;
        }
        Box::new(generator)
    }

    /// Thunder as the plan sees it: its distances are its takes, each a clap
    /// as long as that distance rolls, and it rests between claps. Middle and
    /// far claps are the common ones; a massive one is rare. Every clap is a
    /// little louder or softer and higher or lower than the last.
    pub(super) fn rhythm(&self) -> Rhythm {
        let [shortest, longest] = self.gap.unwrap_or(DEFAULT_GAP);
        let take = |preset, chance| TakeRhythm {
            seconds: clap_seconds(preset),
            chance,
        };
        Rhythm {
            takes: vec![
                take(PRESET_NEAR, 2.0),
                take(PRESET_MID, 3.0),
                take(PRESET_FAR, 3.0),
                take(PRESET_MASSIVE, 1.0),
            ],
            rests: Rests {
                level_variation_db: 3.0,
                pitch_variation_cents: 150.0,
                ..Rests::plain(shortest, longest)
            },
        }
    }
}
