// The river's slow motion: flow speed wandering over minutes and the two
// eddy scales, each a random smootherstep ramp on the sample clock.

use crate::synth::noise::Rng;

#[derive(Clone, Copy)]
pub(super) struct RandomRamp {
    from: f32,
    to: f32,
    elapsed: f32,
    pub(super) duration: f32,
}

impl RandomRamp {
    pub(super) fn new(initial: f32) -> Self {
        Self {
            from: initial,
            to: initial,
            elapsed: 0.0,
            duration: 0.0,
        }
    }

    #[inline]
    pub(super) fn smootherstep(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
    }

    pub(super) fn next(
        &mut self,
        rng: &mut Rng,
        dt: f32,
        target_min: f32,
        target_max: f32,
        time_min: f32,
        time_max: f32,
    ) -> f32 {
        if self.duration <= 0.0 || self.elapsed >= self.duration {
            self.from = self.to.clamp(target_min, target_max);
            self.to = target_min + (target_max - target_min) * rng.next_f32();
            self.duration = time_min + (time_max - time_min) * rng.next_f32();
            self.elapsed = 0.0;
        }
        let t = Self::smootherstep(self.elapsed / self.duration.max(dt));
        let value = self.from + (self.to - self.from) * t;
        self.elapsed += dt;
        value.clamp(target_min, target_max)
    }
}

#[derive(Clone, Copy)]
pub(super) struct FlowState {
    pub(super) speed_m_s: f32,
    pub(super) slow_eddy: f32,
    pub(super) fast_eddy: f32,
}

/// Long flow evolution plus two correlated-motion scales. The configured
/// eddy range controls the perceptually important 0.2-1 Hz motion; a derived
/// faster ramp adds surface detail without controlling the carrier rate.
pub(super) struct RiverDriver {
    rng: Rng,
    speed: RandomRamp,
    slow_eddy: RandomRamp,
    fast_eddy: RandomRamp,
    change_time_min: f32,
    change_time_max: f32,
    eddy_time_min: f32,
    eddy_time_max: f32,
}

impl RiverDriver {
    pub(super) fn new(seed: u64, initial_speed: f32) -> Self {
        Self {
            rng: Rng::new(seed),
            speed: RandomRamp::new(initial_speed),
            slow_eddy: RandomRamp::new(0.0),
            fast_eddy: RandomRamp::new(0.0),
            change_time_min: 15.0,
            change_time_max: 110.0,
            eddy_time_min: 0.35,
            eddy_time_max: 1.25,
        }
    }

    pub(super) fn set_change_time_range(&mut self, min_seconds: f32, max_seconds: f32) {
        let min_seconds = min_seconds.max(0.1);
        self.change_time_min = min_seconds.min(max_seconds.max(0.1));
        self.change_time_max = min_seconds.max(max_seconds.max(0.1));
    }

    pub(super) fn change_time_range(&self) -> (f32, f32) {
        (self.change_time_min, self.change_time_max)
    }

    pub(super) fn set_eddy_time_range(&mut self, min_seconds: f32, max_seconds: f32) {
        let min_seconds = min_seconds.max(0.05);
        self.eddy_time_min = min_seconds.min(max_seconds.max(0.05));
        self.eddy_time_max = min_seconds.max(max_seconds.max(0.05));
    }

    pub(super) fn eddy_time_range(&self) -> (f32, f32) {
        (self.eddy_time_min, self.eddy_time_max)
    }

    pub(super) fn next(&mut self, sample_rate: f32, min_speed: f32, max_speed: f32) -> FlowState {
        let dt = 1.0 / sample_rate.max(1.0);
        let speed_m_s = self.speed.next(
            &mut self.rng,
            dt,
            min_speed,
            max_speed,
            self.change_time_min,
            self.change_time_max,
        );
        let slow_eddy = self.slow_eddy.next(
            &mut self.rng,
            dt,
            -1.0,
            1.0,
            self.eddy_time_min,
            self.eddy_time_max,
        );
        let fast_min = (self.eddy_time_min * 0.16).clamp(0.04, 0.40);
        let fast_max = (self.eddy_time_max * 0.16).clamp(fast_min, 0.60);
        let fast_eddy = self
            .fast_eddy
            .next(&mut self.rng, dt, -1.0, 1.0, fast_min, fast_max);
        FlowState {
            speed_m_s,
            slow_eddy,
            fast_eddy,
        }
    }
}
