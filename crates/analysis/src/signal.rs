//! What is analysed, how its level is calibrated, and how its channels add
//! up to one level.

/// What is analysed: interleaved samples and what they are.
#[derive(Clone, Debug)]
pub struct Signal<'a> {
    pub interleaved: &'a [f32],
    pub channels: usize,
    pub sample_rate: u32,
    /// L, R, C, LFE, Ls, Rs, or as the sidecar names them. Empty, or shorter
    /// than `channels`, gives the usual names for the channel count.
    pub channel_names: Vec<String>,
}

/// How the levels are calibrated.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Settings {
    /// dB SPL of a signal whose RMS is 1.0 (0 dBFS as this crate counts it),
    /// when the file's calibration is known: the scene's spl_reference −
    /// 20·log10(target_rms), minus the gain `--normalise` applied. None for
    /// a recording, whose levels are then given in dBFS.
    pub full_scale_db_spl: Option<f64>,
}

impl Signal<'_> {
    /// Frames, whole ones only.
    pub(crate) fn frames(&self) -> usize {
        self.interleaved.len() / self.channels
    }

    pub(crate) fn seconds(&self) -> f64 {
        self.frames() as f64 / self.sample_rate as f64
    }

    /// One channel's samples.
    pub(crate) fn channel(&self, index: usize) -> Vec<f32> {
        self.interleaved
            .chunks_exact(self.channels)
            .map(|frame| frame[index])
            .collect()
    }

    /// Every channel's name: as given, or the usual one for the count.
    pub(crate) fn names(&self) -> Vec<String> {
        (0..self.channels)
            .map(|index| {
                self.channel_names
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| usual_name(self.channels, index))
            })
            .collect()
    }
}

/// The ears, or the L/R pair of a speaker layout.
pub(crate) fn pair_of(names: &[String]) -> Option<(usize, usize)> {
    if names.len() == 2 {
        return Some((0, 1));
    }
    let find = |wanted: &str| names.iter().position(|name| name == wanted);
    Some((find("L")?, find("R")?))
}

/// The names the engine gives its outputs: M for mono, L R for two, L R C
/// LFE Ls Rs for six; otherwise the channel's number from 1.
pub(crate) fn usual_name(channels: usize, index: usize) -> String {
    let names: &[&str] = match channels {
        1 => &["M"],
        2 => &["L", "R"],
        6 => &["L", "R", "C", "LFE", "Ls", "Rs"],
        _ => &[],
    };
    names
        .get(index)
        .map(|name| name.to_string())
        .unwrap_or_else(|| (index + 1).to_string())
}

/// How the channels of a file add up to its one overall level.
///
/// Two channels are taken as the two ears: the level of a sound is what each
/// ear hears, so the overall level is the mean of their powers (a source in
/// front reaches both ears at its full level, not half). Three or more are
/// loudspeakers: their sounds add at the listener, so the powers are summed
/// (a source panned between two speakers with power-preserving gains then
/// reads its own level).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Combination {
    Single,
    EarsMean,
    RoomSum,
}

impl Combination {
    pub(crate) fn for_channels(channels: usize) -> Self {
        match channels {
            1 => Self::Single,
            2 => Self::EarsMean,
            _ => Self::RoomSum,
        }
    }

    /// The combined power of per-channel powers.
    pub(crate) fn combine(self, powers: &[f64]) -> f64 {
        let sum: f64 = powers.iter().sum();
        match self {
            Self::Single | Self::RoomSum => sum,
            Self::EarsMean => sum / powers.len().max(1) as f64,
        }
    }

    /// The factor a sum of channel powers is scaled by.
    pub(crate) fn scale(self, channels: usize) -> f64 {
        match self {
            Self::Single | Self::RoomSum => 1.0,
            Self::EarsMean => 1.0 / channels.max(1) as f64,
        }
    }
}

/// The report's levels: dB SPL when the calibration is known, else dBFS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Calibration {
    /// Added to a level in dBFS to give the report's unit.
    pub(crate) offset_decibels: f64,
    pub(crate) known: bool,
}

impl Calibration {
    pub(crate) fn of(settings: &Settings) -> Self {
        match settings.full_scale_db_spl {
            Some(full_scale) => Self {
                offset_decibels: full_scale,
                known: true,
            },
            None => Self {
                offset_decibels: 0.0,
                known: false,
            },
        }
    }

    /// A power in full-scale units as a level in the report's unit.
    pub(crate) fn level(self, power: f64) -> f64 {
        crate::statistics::power_decibels(power) + self.offset_decibels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_ears_average_and_speakers_add() {
        assert!((Combination::for_channels(2).combine(&[0.2, 0.4]) - 0.3).abs() < 1e-12);
        assert!((Combination::for_channels(6).combine(&[0.1; 6]) - 0.6).abs() < 1e-12);
        assert_eq!(Combination::for_channels(1).combine(&[0.5]), 0.5);
    }

    #[test]
    fn names_fall_back_to_the_usual_ones() {
        let samples = [0.0f32; 12];
        let signal = Signal {
            interleaved: &samples,
            channels: 6,
            sample_rate: 48_000,
            channel_names: vec!["Left".into()],
        };
        assert_eq!(signal.names(), ["Left", "R", "C", "LFE", "Ls", "Rs"]);
        assert_eq!(signal.frames(), 2);
    }
}
