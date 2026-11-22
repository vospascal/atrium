//! A lab session as a file, so it can be reopened: the question, the
//! reference and its passage, each candidate's seed, cue and settings, the
//! trims, the notes pinned while listening and the verdict.
//!
//! It is `lab/<sound>/session.json`, rewritten on every save. Its fields
//! follow the round log's (`docs/generator-lab-plan.md`, "The log"), so a
//! later log line can be made from it. A missing field is read as absent, so
//! later fields never make an older file unreadable.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::mix::{Fader, Layer, LayerMix};
use super::settings::CandidateSettings;

/// The layout of the file; a file of another is refused.
pub const SESSION_VERSION: u32 = 1;

/// Where `sound`'s session is kept, relative to the working directory.
pub fn session_path(sound: &str) -> PathBuf {
    Path::new("lab").join(sound).join("session.json")
}

/// One lab session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub session_version: u32,
    /// The sound tuned, as its preset's file is named: `soft-wind`.
    pub sound: String,
    /// When it was saved, UTC.
    pub saved: String,
    /// What this round listens for.
    pub question: String,
    pub reference: Option<ReferencePassage>,
    pub candidates: SessionCandidates,
    /// Whether the passage repeats, and whether the generator runs on over
    /// the rest of the recording instead of the marked gust.
    #[serde(rename = "loop")]
    pub looping: bool,
    pub run_freely: bool,
    /// The trims a match set (dB), by candidate name; a candidate not here
    /// was not matched.
    pub trims_db: BTreeMap<String, f32>,
    /// The candidates whose trim was held through an edit since its match.
    pub rematch_needed: Vec<String>,
    pub hold_level_match: bool,
    /// How the candidates were heard: in place, alone.
    pub mode: String,
    pub notes: Vec<SessionNote>,
    /// In the listener's own words.
    pub verdict: String,
}

/// The reference recording and the passage of it the loop plays.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReferencePassage {
    pub file: String,
    /// The passage's name, such as "Gust 1".
    pub passage: String,
    /// Where it starts and ends in the recording (s).
    pub from: f64,
    pub to: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionCandidates {
    pub previous: SessionCandidate,
    pub working: SessionCandidate,
}

/// A generator candidate: its run and its settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionCandidate {
    pub seed: u64,
    /// Where the passage starts in the seeded run (s).
    pub cue: f64,
    /// Settings that differ from the preset, by name.
    pub settings: BTreeMap<String, f32>,
    /// The mixer's strips that differ from the generator's own, by layer.
    pub mix: BTreeMap<String, Fader>,
    /// For the working candidate: what differs from the previous one, as
    /// `[previous, working]`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub changed: BTreeMap<String, [f32; 2]>,
}

impl SessionCandidate {
    /// `settings` at `seed` and `cue`.
    pub fn of(settings: &CandidateSettings, seed: u64, cue: f64) -> Self {
        Self {
            seed,
            cue,
            settings: settings.values.clone(),
            mix: Layer::ALL
                .into_iter()
                .filter(|layer| settings.mix.fader(*layer) != Fader::default())
                .map(|layer| (layer.name().to_owned(), settings.mix.fader(layer)))
                .collect(),
            changed: BTreeMap::new(),
        }
    }

    /// Its settings as the lab works with them. A layer the lab does not
    /// know is passed over.
    pub fn settings(&self) -> CandidateSettings {
        let mut mix = LayerMix::default();
        for (name, fader) in &self.mix {
            if let Some(layer) = Layer::named(name) {
                mix.set(layer, *fader);
            }
        }
        CandidateSettings {
            values: self.settings.clone(),
            mix,
        }
    }
}

/// An observation pinned to the moment it was heard.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionNote {
    /// Into the passage (s).
    pub at: f64,
    /// The candidate heard: `reference`, `previous` or `working`.
    pub heard: String,
    /// The same moment in what was heard: the recording's time for the
    /// reference, the seeded run's for a generator (s).
    pub moment: f64,
    pub text: String,
}

impl Session {
    /// Write it to `path`, making its folder if need be; `saved` is set to now.
    pub fn save(&mut self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        self.session_version = SESSION_VERSION;
        self.saved = utc_now();
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text + "\n")?;
        Ok(())
    }

    /// Read the session at `path`.
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let session: Self =
            serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        if session.session_version != SESSION_VERSION {
            return Err(format!(
                "{}: a session of layout {}, and this lab reads {SESSION_VERSION}",
                path.display(),
                session.session_version
            )
            .into());
        }
        Ok(session)
    }
}

/// Now, UTC, as `2026-11-22T10:42:00Z`.
fn utc_now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (days, of_day) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60
    )
}

/// The date `days` after 1970-01-01, in the proleptic Gregorian calendar
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_count_from_the_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(59), (1970, 3, 1));
        assert_eq!(civil_from_days(20_779), (2026, 11, 22));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }
}
