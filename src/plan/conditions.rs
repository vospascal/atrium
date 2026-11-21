//! The place's conditions: a few slow values every sound can follow.

use atrium_core::commands::SynthParam;

/// A condition of the place, from 0 to 1. Changes in a place have a cause
/// because sounds follow these instead of changing each by itself: the birds
/// fall silent because the rain comes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Condition {
    Wind,
    Rain,
    Storm,
    Daylight,
}

impl Condition {
    pub const ALL: [Condition; 4] = [
        Condition::Wind,
        Condition::Rain,
        Condition::Storm,
        Condition::Daylight,
    ];

    /// The name a file uses.
    pub fn name(self) -> &'static str {
        match self {
            Condition::Wind => "wind",
            Condition::Rain => "rain",
            Condition::Storm => "storm",
            Condition::Daylight => "daylight",
        }
    }

    /// The condition a file names, if there is one.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|condition| condition.name() == name)
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Every condition's value at one moment.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Conditions([f32; Condition::ALL.len()]);

impl Conditions {
    pub fn get(&self, condition: Condition) -> f32 {
        self.0[condition.index()]
    }

    pub fn set(&mut self, condition: Condition, value: f32) {
        self.0[condition.index()] = value.clamp(0.0, 1.0);
    }
}

/// What follows a condition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// How often a sound with takes sounds, 0 (never) to 1 (as its rests
    /// say). Several links multiply.
    Activity,
    /// How much louder or softer a sound with takes is (dB). Several add up.
    Level,
    /// One of a generator's own settings, such as rain intensity.
    Setting(SynthParam),
}

/// A setting tied to a condition: `from` when the condition is 0, `to` when
/// it is 1, a straight line between.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Follow {
    pub condition: Condition,
    pub target: Target,
    pub from: f32,
    pub to: f32,
}

impl Follow {
    /// A link as a file writes it, with the names checked against what the
    /// sound has: `takes` to follow with activity or level (a recording, or
    /// thunder), `settings` to follow with a generator's own settings.
    pub fn named(
        condition: &str,
        setting: &str,
        from: f32,
        to: f32,
        takes: bool,
        settings: bool,
    ) -> Result<Self, String> {
        let condition = Condition::named(condition).ok_or_else(|| {
            format!("`{condition}` is not a condition; use wind, rain, storm or daylight")
        })?;
        let target = match setting {
            "activity" | "level" if !takes => {
                return Err(format!(
                    "this sound has no takes, so it cannot follow with `{setting}`; follow with one of its own settings, such as `master_gain`"
                ))
            }
            "activity" => Target::Activity,
            "level" => Target::Level,
            other => Target::Setting(SynthParam::named(other).ok_or_else(|| {
                format!("`{other}` is not a setting; use activity, level or a generator's setting such as intensity")
            })?),
        };
        if matches!(target, Target::Setting(_)) && !settings {
            return Err(format!(
                "`{setting}` is a generator's setting, and this sound is a recording; follow with activity or level"
            ));
        }
        if !(from.is_finite() && to.is_finite()) {
            return Err("`from` and `to` must be numbers".into());
        }
        Ok(Self {
            condition,
            target,
            from,
            to,
        })
    }

    /// The setting's value under `conditions`.
    pub fn value(&self, conditions: &Conditions) -> f32 {
        let along = conditions.get(self.condition);
        self.from + (self.to - self.from) * along
    }
}

/// How often a sound with takes sounds under `conditions`, 0 to 1.
pub fn activity(follows: &[Follow], conditions: &Conditions) -> f32 {
    follows
        .iter()
        .filter(|follow| follow.target == Target::Activity)
        .map(|follow| follow.value(conditions).clamp(0.0, 1.0))
        .product()
}

/// How many dB louder or softer a sound with takes is under `conditions`.
pub fn level_db(follows: &[Follow], conditions: &Conditions) -> f32 {
    follows
        .iter()
        .filter(|follow| follow.target == Target::Level)
        .map(|follow| follow.value(conditions))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn under(pairs: &[(Condition, f32)]) -> Conditions {
        let mut conditions = Conditions::default();
        for (condition, value) in pairs {
            conditions.set(*condition, *value);
        }
        conditions
    }

    /// A bird that stops in the rain and sings more by day: both links
    /// together, and each by itself.
    #[test]
    fn activity_follows_every_link_together() {
        let follows = [
            Follow::named("rain", "activity", 1.0, 0.0, true, false).expect("rain"),
            Follow::named("daylight", "activity", 0.2, 1.0, true, false).expect("daylight"),
        ];
        assert!((activity(&follows, &under(&[(Condition::Daylight, 1.0)])) - 1.0).abs() < 1e-6);
        assert!(
            activity(
                &follows,
                &under(&[(Condition::Daylight, 1.0), (Condition::Rain, 1.0)])
            ) < 1e-6
        );
        assert!(
            (activity(&follows, &under(&[])) - 0.2).abs() < 1e-6,
            "at night, a little"
        );
        assert!(
            (activity(&[], &under(&[])) - 1.0).abs() < 1e-6,
            "no links: as its rests say"
        );
    }

    #[test]
    fn level_links_add_up_in_db() {
        let follows = [
            Follow::named("wind", "level", 0.0, -6.0, true, false).expect("wind"),
            Follow::named("rain", "level", 0.0, -6.0, true, false).expect("rain"),
        ];
        let level = level_db(
            &follows,
            &under(&[(Condition::Wind, 0.5), (Condition::Rain, 1.0)]),
        );
        assert!((level + 9.0).abs() < 1e-5);
    }

    /// A file's names are checked, and each kind of sound only follows with
    /// what it has.
    #[test]
    fn a_link_must_name_what_the_sound_has() {
        assert!(Follow::named("fog", "activity", 0.0, 1.0, true, false).is_err());
        assert!(Follow::named("rain", "loudness", 0.0, 1.0, true, false).is_err());
        assert!(
            Follow::named("rain", "activity", 0.0, 1.0, false, true).is_err(),
            "a generator has no takes"
        );
        assert!(
            Follow::named("rain", "intensity", 0.0, 1.0, true, false).is_err(),
            "a recording has no intensity"
        );
        let rain =
            Follow::named("rain", "intensity", 0.0, 1.0, false, true).expect("rain makes rain");
        assert!(
            Follow::named("storm", "activity", 0.0, 1.0, true, true).is_ok(),
            "thunder has both"
        );
        assert_eq!(rain.target, Target::Setting(SynthParam::Intensity));
    }
}
