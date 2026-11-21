// The weather driver every wind shares: slow air-mass changes, gusts and
// eddies, each a smooth random trajectory on the sample clock.

use std::f32::consts::PI;

use crate::synth::noise::Rng;
use crate::synth::wind_behaviour::{WindBehaviour, WindMoves};

/// Smooth random trajectory with segment durations specified in seconds.
/// Cosine interpolation gives zero slope at target changes and avoids audible
/// control discontinuities.
pub(super) struct SmoothRandom {
    rng: Rng,
    pub(super) from: f32,
    pub(super) to: f32,
    pub(super) position: usize,
    pub(super) length: usize,
    min_seconds: f32,
    max_seconds: f32,
}

impl SmoothRandom {
    pub(super) fn new(seed: u64, min_seconds: f32, max_seconds: f32) -> Self {
        let mut rng = Rng::new(seed);
        // Begin at an arbitrary point in the weather process. Starting every
        // source at zero would create a seed-independent artificial fade and
        // make short control-side loudness previews unrepresentative.
        let initial = rng.next_bipolar();
        Self {
            rng,
            from: initial,
            to: initial,
            position: 0,
            length: 0,
            min_seconds,
            max_seconds,
        }
    }

    pub(super) fn set_duration_range(&mut self, min_seconds: f32, max_seconds: f32) {
        let min_seconds = min_seconds.max(0.05);
        self.min_seconds = min_seconds.min(max_seconds.max(0.05));
        self.max_seconds = min_seconds.max(max_seconds);
    }

    pub(super) fn duration_range(&self) -> (f32, f32) {
        (self.min_seconds, self.max_seconds)
    }

    #[inline(always)]
    pub(super) fn next(&mut self, sample_rate: f32) -> f32 {
        self.next_shaped(sample_rate, 0.0)
    }

    /// Advance the trajectory, optionally biasing rising segments toward the
    /// short end and falling segments toward the long end of the configured
    /// duration range. Negative bias reverses that relationship.
    #[inline(always)]
    pub(super) fn next_shaped(&mut self, sample_rate: f32, rise_bias: f32) -> f32 {
        if self.length == 0 {
            self.from = self.to;
            self.to = self.rng.next_bipolar();
            let directional_bias = if self.to >= self.from {
                rise_bias
            } else {
                -rise_bias
            }
            .clamp(-1.0, 1.0);
            let raw = self.rng.next_f32();
            let shaped = if directional_bias >= 0.0 {
                raw.powf(1.0 + 2.0 * directional_bias)
            } else {
                1.0 - (1.0 - raw).powf(1.0 - 2.0 * directional_bias)
            };
            let seconds = self.min_seconds + shaped * (self.max_seconds - self.min_seconds);
            self.length = (seconds * sample_rate).round().max(1.0) as usize;
            self.position = 0;
        }
        let t = self.position as f32 / self.length as f32;
        let smooth = 0.5 - 0.5 * (PI * t).cos();
        let value = self.from + (self.to - self.from) * smooth;
        self.position += 1;
        if self.position >= self.length {
            self.length = 0;
        }
        value
    }
}

/// Hierarchical weather state: slow air-mass changes, individual gusts, and
/// small turbulent eddies. This replaces a repeating ADSR-like gust cycle.
pub(crate) struct WindDriver {
    weather: SmoothRandom,
    gust: SmoothRandom,
    eddy: SmoothRandom,
    /// What the wind does now and then, when it has a repertoire. Without
    /// one the gust trajectory above pushes the wind about half the time,
    /// which suits a storm.
    moves: Option<WindMoves>,
    seed: u64,
}

impl WindDriver {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            weather: SmoothRandom::new(seed.wrapping_add(101), 18.0, 55.0),
            gust: SmoothRandom::new(seed.wrapping_add(103), 2.5, 11.0),
            eddy: SmoothRandom::new(seed.wrapping_add(107), 0.12, 0.80),
            moves: None,
            seed,
        }
    }

    /// Let the wind follow `behaviour`: steady, with a gust, swell or lull
    /// now and then.
    pub(crate) fn set_behaviour(&mut self, behaviour: WindBehaviour) {
        self.moves = Some(WindMoves::new(behaviour, self.seed.wrapping_add(109)));
    }

    /// Rest `share` as long as the repertoire says between two moves. A wind
    /// without a repertoire has no rests to change.
    pub(crate) fn set_move_rest(&mut self, share: f32) {
        if let Some(moves) = &mut self.moves {
            moves.set_rest_share(share);
        }
    }

    /// Make the repertoire's moves `share` as strong as it says.
    pub(crate) fn set_move_strength(&mut self, share: f32) {
        if let Some(moves) = &mut self.moves {
            moves.set_strength_share(share);
        }
    }

    pub(crate) fn set_change_time_range(&mut self, min_seconds: f32, max_seconds: f32) {
        self.weather.set_duration_range(min_seconds, max_seconds);
    }

    pub(crate) fn change_time_range(&self) -> (f32, f32) {
        self.weather.duration_range()
    }

    pub(crate) fn set_gust_duration_range(&mut self, min_seconds: f32, max_seconds: f32) {
        self.gust.set_duration_range(min_seconds, max_seconds);
    }

    pub(crate) fn gust_duration_range(&self) -> (f32, f32) {
        self.gust.duration_range()
    }

    pub(crate) fn set_turbulence_time_range(&mut self, min_seconds: f32, max_seconds: f32) {
        self.eddy.set_duration_range(min_seconds, max_seconds);
    }

    pub(crate) fn turbulence_time_range(&self) -> (f32, f32) {
        self.eddy.duration_range()
    }

    #[inline(always)]
    pub(crate) fn next(
        &mut self,
        sample_rate: f32,
        min_speed: f32,
        max_speed: f32,
        gust_strength: f32,
        rise_bias: f32,
    ) -> WindFrame {
        let weather = 0.5 * (self.weather.next_shaped(sample_rate, rise_bias) + 1.0);
        let eddy = self.eddy.next(sample_rate);

        // The slow weather trajectory occupies the range left after reserving
        // headroom for gusts. A lull takes the steady wind down for a while.
        let gust_strength = gust_strength.clamp(0.0, 1.0);
        let (gust_pressure, lull) = match &mut self.moves {
            Some(moves) => {
                let push = moves.next(sample_rate);
                (push.max(0.0), (-push).max(0.0))
            }
            // Negative halves of the gust trajectory become genuine rests
            // rather than downward oscillations.
            None => {
                let gust = self.gust.next_shaped(sample_rate, rise_bias);
                (gust.max(0.0).powi(2), 0.0)
            }
        };
        let steady = weather * (1.0 - gust_strength) * (1.0 - lull);
        let activity = (steady + gust_pressure * gust_strength).clamp(0.0, 1.0);
        let min_speed = min_speed.clamp(0.0, 25.0);
        let max_speed = max_speed.clamp(min_speed, 25.0);
        let speed = min_speed + (max_speed - min_speed) * activity;
        WindFrame {
            speed,
            activity,
            weather,
            gust: gust_pressure,
            eddy,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct WindFrame {
    pub(crate) speed: f32,
    pub(crate) activity: f32,
    /// Slow normalized air-mass state before gusts are mixed in.
    pub(crate) weather: f32,
    /// Positive normalized gust pressure before it is mixed into `activity`.
    pub(crate) gust: f32,
    pub(crate) eddy: f32,
}
