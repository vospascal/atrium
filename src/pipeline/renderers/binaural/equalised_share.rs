//! How much of the headphone-equalised render is heard: all of it, none of
//! it, or, in the callback where the probe's switch changes, a glide from
//! one to the other.
//!
//! A cut would click. Like every other probe switch it glides over one
//! callback, and the renderer weighs both renders sample by sample with the
//! same shares, so what the ears hear moves smoothly from one to the other.

/// The equalised render's share of what is heard during one callback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum EqualisedShare {
    /// Only the measured responses as they are: the render before the
    /// equalisation, sample for sample.
    None,
    /// Only the equalised render.
    All,
    /// From one share to the other across the callback.
    Gliding { from: f32, to: f32 },
}

impl EqualisedShare {
    /// The share for a callback that starts where the last one ended,
    /// `previous` (0 or 1, or `None` straight after a reset, when nothing
    /// was heard yet to glide from), and heads for whether the probe wants
    /// the equalisation now.
    pub(super) fn between(previous: Option<f32>, equalised: bool) -> Self {
        let target = if equalised { 1.0 } else { 0.0 };
        match previous.unwrap_or(target) {
            from if from == target && equalised => Self::All,
            from if from == target => Self::None,
            from => Self::Gliding { from, to: target },
        }
    }

    /// The equalised share at `progress` through the callback (0 at its
    /// first sample), as every gain in the renderer ramps.
    pub(super) fn at(&self, progress: f32) -> f32 {
        match *self {
            Self::None => 0.0,
            Self::All => 1.0,
            Self::Gliding { from, to } => from + (to - from) * progress,
        }
    }

    /// Where the callback ends: what the next one glides from.
    pub(super) fn end(&self) -> f32 {
        match *self {
            Self::None => 0.0,
            Self::All => 1.0,
            Self::Gliding { to, .. } => to,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_settled_switch_does_not_glide_and_a_changed_one_does() {
        assert_eq!(EqualisedShare::between(None, true), EqualisedShare::All);
        assert_eq!(EqualisedShare::between(None, false), EqualisedShare::None);
        assert_eq!(
            EqualisedShare::between(Some(1.0), true),
            EqualisedShare::All
        );
        assert_eq!(
            EqualisedShare::between(Some(0.0), false),
            EqualisedShare::None
        );
        let switching_off = EqualisedShare::between(Some(1.0), false);
        assert_eq!(
            switching_off,
            EqualisedShare::Gliding { from: 1.0, to: 0.0 }
        );
        assert_eq!(switching_off.at(0.0), 1.0);
        assert_eq!(switching_off.at(0.25), 0.75);
        assert_eq!(switching_off.end(), 0.0);
        assert_eq!(EqualisedShare::between(Some(0.0), true).at(0.5), 0.5);
    }
}
