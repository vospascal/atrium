//! The soft wind's settings as the lab knows them: which change only how the
//! wind sounds and which change when its gusts come, and a candidate's own
//! values for them.
//!
//! The soft wind draws its gust driver and each noise layer from streams of
//! their own (`crate::synth::soft_wind`), so a level or a colour setting
//! leaves every gust where it was and can be applied on top of a state kept
//! at the cue. A timing setting makes different gusts: the run from the seed
//! to the cue is made again, and the gust marked in the old run no longer
//! lines up.

use std::collections::BTreeMap;

use atrium_core::commands::SynthParam;
use atrium_core::source::SoundSource;

use super::mix::{layer_gains, Layer, LayerMix};
use crate::synth::soft_wind::SoftWindSource;

/// What changing a setting does to the gusts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingKind {
    /// How loud or how bright the wind is: the same gusts at the same
    /// samples, heard at once.
    LevelOrColour,
    /// When gusts come and how long they last: different gusts.
    Timing,
}

/// One setting the lab can change, as the lab draws and explains it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Setting {
    pub param: SynthParam,
    pub label: &'static str,
    pub kind: SettingKind,
    pub low: f32,
    pub high: f32,
    /// Whether the Behaviour panel shows it.
    pub in_behaviour: bool,
    /// One line on what it does.
    pub description: &'static str,
}

impl Setting {
    /// The name a file uses.
    pub fn name(&self) -> &'static str {
        self.param.name()
    }

    /// Its value in `generator`.
    pub fn value_in(&self, generator: &SoftWindSource) -> f32 {
        match self.param {
            SynthParam::GustBrightness => generator.gust_brightness,
            SynthParam::GustDurationMin => generator.gust_duration_range().0,
            SynthParam::GustDurationMax => generator.gust_duration_range().1,
            SynthParam::TurbulenceTimeMin => generator.turbulence_time_range().0,
            SynthParam::TurbulenceTimeMax => generator.turbulence_time_range().1,
            // A share of the preset's own rest: as written.
            _ => 1.0,
        }
    }
}

/// The soft wind's settings the lab changes outside its mixer. The Behaviour
/// panel shows those marked for it; the others are what the plan names as
/// timing (gust duration, the rest between moves, turbulence time).
pub const SETTINGS: [Setting; 6] = [
    Setting {
        param: SynthParam::GustBrightness,
        label: "Gust brightness",
        kind: SettingKind::LevelOrColour,
        low: 0.0,
        high: 1.0,
        in_behaviour: true,
        description: "How much the wind brightens during a gust; the gusts stay where they are",
    },
    Setting {
        param: SynthParam::MoveRest,
        label: "Rest between moves",
        kind: SettingKind::Timing,
        low: 0.1,
        high: 4.0,
        in_behaviour: false,
        description: "Steady wind between two gusts, swells or lulls, as a share of the preset's; makes different gusts",
    },
    Setting {
        param: SynthParam::GustDurationMin,
        label: "Gust duration, shortest",
        kind: SettingKind::Timing,
        low: 0.05,
        high: 10.0,
        in_behaviour: false,
        description: "The shortest push of the gust trajectory (s); makes different gusts",
    },
    Setting {
        param: SynthParam::GustDurationMax,
        label: "Gust duration, longest",
        kind: SettingKind::Timing,
        low: 0.05,
        high: 10.0,
        in_behaviour: false,
        description: "The longest push of the gust trajectory (s); makes different gusts",
    },
    Setting {
        param: SynthParam::TurbulenceTimeMin,
        label: "Turbulence time, shortest",
        kind: SettingKind::Timing,
        low: 0.02,
        high: 2.0,
        in_behaviour: false,
        description: "The quickest ruffle of the eddies (s); makes different ruffles",
    },
    Setting {
        param: SynthParam::TurbulenceTimeMax,
        label: "Turbulence time, longest",
        kind: SettingKind::Timing,
        low: 0.02,
        high: 2.0,
        in_behaviour: false,
        description: "The slowest ruffle of the eddies (s); makes different ruffles",
    },
];

/// The setting a file names, if the lab changes it.
pub fn setting_named(name: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|setting| setting.name() == name)
}

/// The settings applied on top of a state kept at the cue: the five layer
/// gains, then the level and colour settings. Applying them never changes
/// which samples the run makes, only how loud and bright they are.
pub(crate) const LIVE: [SynthParam; 6] = [
    SynthParam::LowGain,
    SynthParam::BodyGain,
    SynthParam::MidGain,
    SynthParam::PresenceGain,
    SynthParam::AirGain,
    SynthParam::GustBrightness,
];

/// One candidate generator's settings: values that differ from the preset,
/// and its mixer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CandidateSettings {
    /// By setting name; a setting not here keeps the preset's value.
    pub values: BTreeMap<String, f32>,
    pub mix: LayerMix,
}

impl CandidateSettings {
    /// `setting`'s value: this candidate's own, else the preset's as
    /// `preset` has it.
    pub fn value(&self, setting: &Setting, preset: &SoftWindSource) -> f32 {
        self.values
            .get(setting.name())
            .copied()
            .unwrap_or_else(|| setting.value_in(preset))
    }

    /// Set up `generator`, fresh from the preset, as this candidate.
    pub fn apply(&self, generator: &mut SoftWindSource) {
        let live = self.live_values(generator);
        for (name, value) in &self.values {
            if let Some(setting) = setting_named(name) {
                generator.set_synth_param(setting.param, *value);
            }
        }
        apply_live(generator, &live);
    }

    /// The values of [`LIVE`] for this candidate, given `preset`, the
    /// generator as the preset makes it.
    pub fn live_values(&self, preset: &SoftWindSource) -> [f32; LIVE.len()] {
        let gains = self.mix.gains(layer_gains(preset));
        let mut values = [0.0; LIVE.len()];
        values[..Layer::ALL.len()].copy_from_slice(&gains);
        for (value, param) in values.iter_mut().zip(LIVE).skip(Layer::ALL.len()) {
            *value = setting_named(param.name())
                .map(|setting| self.value(setting, preset))
                .unwrap_or_default();
        }
        values
    }

    /// Whether this candidate's gusts come where `other`'s do: every timing
    /// setting the same.
    pub fn lines_up_with(&self, other: &Self, preset: &SoftWindSource) -> bool {
        SETTINGS
            .iter()
            .filter(|setting| setting.kind == SettingKind::Timing)
            .all(|setting| self.value(setting, preset) == other.value(setting, preset))
    }

    /// What differs from `previous`, as `[previous, this]`: the settings by
    /// name, the layers as `<layer>_db`, `<layer>_solo` and `<layer>_mute`
    /// (1 for on).
    pub fn changed_from(
        &self,
        previous: &Self,
        preset: &SoftWindSource,
    ) -> BTreeMap<String, [f32; 2]> {
        let mut changed = BTreeMap::new();
        for setting in &SETTINGS {
            let (was, now) = (previous.value(setting, preset), self.value(setting, preset));
            if was != now {
                changed.insert(setting.name().to_owned(), [was, now]);
            }
        }
        for layer in Layer::ALL {
            let (was, now) = (previous.mix.fader(layer), self.mix.fader(layer));
            let pairs = [
                ("db", was.level_db, now.level_db),
                ("solo", was.solo as u8 as f32, now.solo as u8 as f32),
                ("mute", was.mute as u8 as f32, now.mute as u8 as f32),
            ];
            for (what, was, now) in pairs {
                if was != now {
                    changed.insert(format!("{}_{what}", layer.name()), [was, now]);
                }
            }
        }
        changed
    }
}

/// Apply the values of [`LIVE`] to `generator`. Allocation-free: the audio
/// thread calls it after every wrap.
#[inline]
pub(crate) fn apply_live(generator: &mut SoftWindSource, values: &[f32; LIVE.len()]) {
    for (param, value) in LIVE.iter().zip(values) {
        generator.set_synth_param(*param, *value);
    }
}
