//! Hierarchical wind: slow air-mass drift, individual gusts, and small eddies.
//!
//! This is deliberately the **same model** the audio engine's field-wind synth
//! uses (`src/synth/field_wind.rs`): three smooth random trajectories at
//! different time scales, mixed so that the slow weather occupies the range left
//! after reserving headroom for short gusts. Wind you can *hear* and wind you can
//! *see* should be one phenomenon, so keeping the model identical is what makes
//! it possible to later drive both from a single weather state instead of having
//! two unrelated wobbles that disagree with each other.
//!
//! It lives in `voxel-core` because it is pure deterministic math with no engine
//! dependency — the same reason the terrain generator does.
//!
//! Differences from the audio version, both deliberate:
//! * it advances by **elapsed seconds** rather than a sample count, because a
//!   frame loop has a variable step and no sample rate;
//! * it carries no DSP state, so it is cheap to keep one per world.

/// A small deterministic PRNG (xorshift64*), so a seed reproduces a wind history
/// exactly — the project's usual determinism rule.
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        // Never allow a zero state (xorshift would be stuck at zero forever).
        Self {
            state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        self.state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `0.0..1.0`.
    fn next_unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// Uniform in `-1.0..1.0`.
    fn next_bipolar(&mut self) -> f32 {
        self.next_unit() * 2.0 - 1.0
    }
}

/// A trajectory that wanders smoothly between random targets, each leg taking a
/// random time inside a configured range. Cosine easing between legs, so there
/// are no velocity discontinuities to see or hear.
struct SmoothRandom {
    rng: Rng,
    from: f32,
    to: f32,
    /// Seconds elapsed into the current leg, and the leg's total length.
    position_seconds: f32,
    length_seconds: f32,
    min_seconds: f32,
    max_seconds: f32,
}

impl SmoothRandom {
    fn new(seed: u64, min_seconds: f32, max_seconds: f32) -> Self {
        let mut rng = Rng::new(seed);
        // Start somewhere arbitrary in the process: starting every trajectory at
        // zero would make every world open with the same artificial calm.
        let initial = rng.next_bipolar();
        Self {
            rng,
            from: initial,
            to: initial,
            position_seconds: 0.0,
            length_seconds: 0.0,
            min_seconds,
            max_seconds,
        }
    }

    fn set_duration_range(&mut self, min_seconds: f32, max_seconds: f32) {
        let min_seconds = min_seconds.max(0.05);
        self.min_seconds = min_seconds.min(max_seconds.max(0.05));
        self.max_seconds = min_seconds.max(max_seconds);
    }

    /// Advance by `delta_seconds`. `rise_bias` biases *rising* legs toward the
    /// short end of the duration range and *falling* legs toward the long end
    /// (negative reverses it) — that asymmetry is what makes a gust arrive
    /// faster than it dies away, which is how real wind behaves.
    fn advance(&mut self, delta_seconds: f32, rise_bias: f32) -> f32 {
        if self.length_seconds <= 0.0 {
            self.from = self.to;
            self.to = self.rng.next_bipolar();
            let directional_bias = if self.to >= self.from {
                rise_bias
            } else {
                -rise_bias
            }
            .clamp(-1.0, 1.0);
            let raw = self.rng.next_unit();
            let shaped = if directional_bias >= 0.0 {
                raw.powf(1.0 + 2.0 * directional_bias)
            } else {
                1.0 - (1.0 - raw).powf(1.0 - 2.0 * directional_bias)
            };
            self.length_seconds =
                (self.min_seconds + shaped * (self.max_seconds - self.min_seconds)).max(0.05);
            self.position_seconds = 0.0;
        }
        let t = (self.position_seconds / self.length_seconds).clamp(0.0, 1.0);
        let smooth = 0.5 - 0.5 * (std::f32::consts::PI * t).cos();
        let value = self.from + (self.to - self.from) * smooth;
        self.position_seconds += delta_seconds.max(0.0);
        if self.position_seconds >= self.length_seconds {
            self.length_seconds = 0.0;
        }
        value
    }
}

/// How the three trajectories are shaped. Field names match the audio synth's
/// controls on purpose.
#[derive(Clone, Copy, Debug)]
pub struct WindShape {
    /// Speed (m/s) in a full lull.
    pub min_speed: f32,
    /// Speed (m/s) at the top of the strongest gust.
    pub max_speed: f32,
    /// Fraction of the speed range reserved for the short gust trajectory
    /// (`0` = only slow weather drift, `1` = gusts and genuine rests).
    pub gust_strength: f32,
    /// Asymmetry of the ramps: `> 0` makes gusts arrive faster than they fade.
    pub rise_bias: f32,
    /// Seconds per slow air-mass change.
    pub weather_seconds: (f32, f32),
    /// Seconds per gust.
    pub gust_seconds: (f32, f32),
    /// Seconds per small turbulent eddy.
    pub eddy_seconds: (f32, f32),
}

impl Default for WindShape {
    fn default() -> Self {
        // The audio field-wind defaults, so seen and heard wind agree.
        Self {
            min_speed: 1.0,
            max_speed: 12.0,
            gust_strength: 0.55,
            rise_bias: 0.35,
            weather_seconds: (18.0, 55.0),
            gust_seconds: (2.5, 11.0),
            eddy_seconds: (0.12, 0.80),
        }
    }
}

/// One frame of wind. `speed` is what the world should react to; the components
/// are exposed because different consumers want different scales (grass follows
/// gusts, a cloud layer wants the slow weather, foam wants the eddies).
#[derive(Clone, Copy, Debug, Default)]
pub struct WindFrame {
    /// Wind speed in m/s.
    pub speed: f32,
    /// Normalised `0..1` mix of weather + gust — `speed` before scaling.
    pub activity: f32,
    /// Slow air-mass state, `0..1`.
    pub weather: f32,
    /// Positive gust pressure, `0..1`.
    pub gust: f32,
    /// Small turbulence, `-1..1`.
    pub eddy: f32,
}

/// Hierarchical wind driver: slow weather, gusts, eddies.
pub struct WindDriver {
    weather: SmoothRandom,
    gust: SmoothRandom,
    eddy: SmoothRandom,
    shape: WindShape,
}

impl WindDriver {
    pub fn new(seed: u64, shape: WindShape) -> Self {
        let mut driver = Self {
            weather: SmoothRandom::new(seed.wrapping_add(101), 18.0, 55.0),
            gust: SmoothRandom::new(seed.wrapping_add(103), 2.5, 11.0),
            eddy: SmoothRandom::new(seed.wrapping_add(107), 0.12, 0.80),
            shape,
        };
        driver.set_shape(shape);
        driver
    }

    pub fn shape(&self) -> WindShape {
        self.shape
    }

    pub fn set_shape(&mut self, shape: WindShape) {
        self.shape = shape;
        self.weather
            .set_duration_range(shape.weather_seconds.0, shape.weather_seconds.1);
        self.gust
            .set_duration_range(shape.gust_seconds.0, shape.gust_seconds.1);
        self.eddy
            .set_duration_range(shape.eddy_seconds.0, shape.eddy_seconds.1);
    }

    /// Advance the wind by `delta_seconds`.
    pub fn advance(&mut self, delta_seconds: f32) -> WindFrame {
        let rise_bias = self.shape.rise_bias;
        let weather = 0.5 * (self.weather.advance(delta_seconds, rise_bias) + 1.0);
        let gust = self.gust.advance(delta_seconds, rise_bias);
        let eddy = self.eddy.advance(delta_seconds, 0.0);

        // The slow weather trajectory occupies the range left after reserving
        // headroom for short gusts, and only the POSITIVE half of the gust
        // trajectory counts — so its negative half becomes a genuine rest
        // instead of an unnatural downward push. Squaring sharpens the peaks.
        let gust_strength = self.shape.gust_strength.clamp(0.0, 1.0);
        let gust_pressure = gust.max(0.0) * gust.max(0.0);
        let activity =
            (weather * (1.0 - gust_strength) + gust_pressure * gust_strength).clamp(0.0, 1.0);
        let min_speed = self.shape.min_speed.max(0.0);
        let max_speed = self.shape.max_speed.max(min_speed);
        WindFrame {
            speed: min_speed + (max_speed - min_speed) * activity,
            activity,
            weather,
            gust: gust_pressure,
            eddy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(seed: u64, shape: WindShape, seconds: f32) -> Vec<WindFrame> {
        let mut driver = WindDriver::new(seed, shape);
        let step = 1.0 / 60.0;
        (0..(seconds / step) as usize)
            .map(|_| driver.advance(step))
            .collect()
    }

    #[test]
    fn same_seed_reproduces_the_same_wind() {
        let shape = WindShape::default();
        let first = run(7, shape, 30.0);
        let second = run(7, shape, 30.0);
        for (a, b) in first.iter().zip(&second) {
            assert_eq!(a.speed, b.speed);
        }
        let other = run(8, shape, 30.0);
        assert!(
            first
                .iter()
                .zip(&other)
                .any(|(a, b)| (a.speed - b.speed).abs() > 0.01),
            "a different seed should give a different wind history"
        );
    }

    #[test]
    fn speed_stays_inside_the_configured_range() {
        let shape = WindShape {
            min_speed: 2.0,
            max_speed: 9.0,
            ..WindShape::default()
        };
        for frame in run(3, shape, 400.0) {
            assert!(
                frame.speed >= 2.0 - 1e-4 && frame.speed <= 9.0 + 1e-4,
                "speed {} escaped 2..9",
                frame.speed
            );
            assert!((0.0..=1.0).contains(&frame.activity));
            assert!((0.0..=1.0).contains(&frame.gust));
        }
    }

    #[test]
    fn gusts_produce_both_peaks_and_real_lulls() {
        // With gusts dominating, the wind must actually rest between them —
        // that is the point of using only the positive half of the trajectory.
        let shape = WindShape {
            min_speed: 0.0,
            max_speed: 20.0,
            gust_strength: 1.0,
            ..WindShape::default()
        };
        let frames = run(11, shape, 600.0);
        let peak = frames.iter().map(|f| f.speed).fold(0.0_f32, f32::max);
        let calm = frames.iter().filter(|f| f.speed < 1.0).count();
        assert!(peak > 12.0, "expected strong gusts, peaked at {peak}");
        assert!(
            calm > frames.len() / 20,
            "expected genuine lulls, only {calm} calm frames of {}",
            frames.len()
        );
    }

    #[test]
    fn zero_gust_strength_leaves_only_slow_weather() {
        let shape = WindShape {
            gust_strength: 0.0,
            ..WindShape::default()
        };
        let frames = run(5, shape, 240.0);
        // Slow drift only: consecutive frames must barely differ.
        let biggest_step = frames
            .windows(2)
            .map(|pair| (pair[1].speed - pair[0].speed).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            biggest_step < 0.05,
            "slow weather should not jump; biggest step was {biggest_step}"
        );
    }

    #[test]
    fn a_variable_frame_step_matches_a_steady_one() {
        // The driver is advanced from a frame loop, so an uneven step must not
        // change where the wind ends up (only how finely it is sampled).
        let shape = WindShape::default();
        let mut steady = WindDriver::new(21, shape);
        let mut uneven = WindDriver::new(21, shape);
        let mut steady_time = 0.0;
        while steady_time < 20.0 {
            steady.advance(1.0 / 120.0);
            steady_time += 1.0 / 120.0;
        }
        let mut uneven_time = 0.0;
        let mut toggle = false;
        while uneven_time < 20.0 {
            let step = if toggle { 1.0 / 40.0 } else { 1.0 / 240.0 };
            toggle = !toggle;
            uneven.advance(step);
            uneven_time += step;
        }
        let steady_frame = steady.advance(0.0);
        let uneven_frame = uneven.advance(0.0);
        assert!(
            (steady_frame.speed - uneven_frame.speed).abs() < 2.0,
            "frame pacing should not change the wind much: {} vs {}",
            steady_frame.speed,
            uneven_frame.speed
        );
    }
}
