//! The `binaural_beat` synth: its YAML parameters and how they configure the
//! two ears' `crate::synth::binaural_beat::BinauralToneSource`s.

use atrium_core::source::SoundSource;
use serde::Deserialize;

use crate::synth::binaural_beat::{BinauralBeatSettings, BinauralToneSource, Ear};

#[derive(Deserialize, Clone, Debug, Default)]
pub(in crate::config) struct BinauralBeatSynthParams {
    /// The left ear's tone, the carrier (Hz).
    pub pitch: Option<f32>,
    /// The beat (Hz); a session's at its start.
    pub beat: Option<f32>,
    /// A session's beat at its end (Hz); left out, the beat is fixed.
    pub end_beat: Option<f32>,
    /// How long it plays, fades included (s).
    pub duration: Option<f32>,
    pub master_gain: Option<f32>,
}

impl BinauralBeatSynthParams {
    fn settings(&self) -> BinauralBeatSettings {
        let defaults = BinauralBeatSettings::default();
        BinauralBeatSettings {
            carrier_hz: self.pitch.unwrap_or(defaults.carrier_hz),
            beat_hz: self.beat.unwrap_or(defaults.beat_hz),
            end_beat_hz: self.end_beat,
            duration_seconds: self.duration.unwrap_or(defaults.duration_seconds),
            master_gain: self.master_gain.unwrap_or(defaults.master_gain),
        }
    }

    /// The left ear's tone: the sound itself. It has no randomness, so the
    /// seed every generator is offered goes unused, and it is held beside the
    /// head, so the place it is offered goes unused too.
    pub(super) fn build_generator(&self) -> Box<dyn SoundSource> {
        Box::new(BinauralToneSource::new(Ear::Left, self.settings()))
    }

    /// The right ear's tone: the sound's second voice.
    pub(super) fn build_second_voice(&self) -> Box<dyn SoundSource> {
        Box::new(BinauralToneSource::new(Ear::Right, self.settings()))
    }

    /// Its two voices as its two layers, keyed `left` and `right` and named
    /// by the ear and the tone it starts on: "left ear 108 Hz", "right ear
    /// 109.618 Hz".
    pub(super) fn layers(&self) -> Vec<(String, String)> {
        let settings = self.settings().clamped();
        [(Ear::Left, "left"), (Ear::Right, "right")]
            .into_iter()
            .map(|(ear, key)| {
                let frequency = settings.ear_frequency(ear, 0.0);
                (
                    key.to_string(),
                    format!("{key} ear {}", super::hertz(frequency)),
                )
            })
            .collect()
    }
}
