//! Muting that fades: a sound silenced or let back in moves to its new
//! level over 10 ms instead of in one step, which is heard as a click. The
//! generator lab mutes the rest of the scene while it plays, and a click
//! there would be the first thing heard of every audition.
//!
//! A sound muted before it has played anything (built again in a voice the
//! listener silenced) is silent from its first sample.

/// How long muting and unmuting fade (s).
pub const MUTE_FADE_SECONDS: f32 = 0.010;

/// The level muting leaves a sound at, sample by sample.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MuteFade {
    muted: bool,
    gain: f32,
    started: bool,
}

impl MuteFade {
    /// Not muted.
    pub(crate) fn new() -> Self {
        Self {
            muted: false,
            gain: 1.0,
            started: false,
        }
    }

    pub(crate) fn is_muted(&self) -> bool {
        self.muted
    }

    /// Mute or unmute, from the next sample on; before the first sample, at
    /// once.
    pub(crate) fn set(&mut self, muted: bool) {
        self.muted = muted;
        if !self.started {
            self.gain = if muted { 0.0 } else { 1.0 };
        }
    }

    /// The gain for the next sample at `sample_rate`.
    #[inline]
    pub(crate) fn next_gain(&mut self, sample_rate: f32) -> f32 {
        self.started = true;
        let target = if self.muted { 0.0 } else { 1.0 };
        if self.gain != target {
            let step = 1.0 / (MUTE_FADE_SECONDS * sample_rate.max(1.0));
            self.gain = if target > self.gain {
                (self.gain + step).min(target)
            } else {
                (self.gain - step).max(target)
            };
        }
        self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Muted while playing, the level falls in 480 even steps at 48 kHz and
    /// rises the same way when unmuted; muted before playing, it is silent
    /// at once.
    #[test]
    fn muting_fades_over_ten_milliseconds() {
        let mut fade = MuteFade::new();
        assert_eq!(fade.next_gain(48_000.0), 1.0);
        fade.set(true);
        let falling: Vec<f32> = (0..480).map(|_| fade.next_gain(48_000.0)).collect();
        assert!((falling[0] - 479.0 / 480.0).abs() < 1e-6);
        assert!((falling[239] - 0.5).abs() < 1e-4);
        assert_eq!(falling[479], 0.0);
        assert_eq!(fade.next_gain(48_000.0), 0.0);
        fade.set(false);
        assert!((fade.next_gain(48_000.0) - 1.0 / 480.0).abs() < 1e-6);

        let mut fresh = MuteFade::new();
        fresh.set(true);
        assert_eq!(fresh.next_gain(48_000.0), 0.0);
    }
}
