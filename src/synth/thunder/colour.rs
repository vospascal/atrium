use std::f32::consts::TAU;

use super::kinds::Kind;
use super::roar::MOMENT_SECONDS;
use crate::audio::filters::Biquad;
use crate::synth::noise::Rng;

/// Below this a clap thins out (Hz).
const LOW_CUT_HZ: f32 = 90.0;
/// Where a flash's boom can sit (Hz), and how much it lifts (dB) and how broad it is.
const BOOM_HZ: (f32, f32) = (170.0, 230.0);
const BOOM_DB: f32 = 4.0;
const BOOM_Q: f32 = 1.0;
/// How far the boom drifts either way, as a share of its pitch, and how fast (Hz).
const BOOM_DRIFT: f32 = 0.15;
const BOOM_DRIFT_HZ: (f32, f32) = (0.15, 0.4);

/// The colour every clap of thunder has, measured on a recording a few
/// kilometres off: little below about 90 Hz, a boom around 200 Hz where most of
/// its energy sits, and a cliff above which almost nothing is left. The boom's
/// pitch differs from flash to flash and drifts slowly through each.
pub(super) struct Colour {
    low_cut: Biquad,
    boom: Biquad,
    cliff_first: Biquad,
    cliff_second: Biquad,
    boom_hz: f32,
    drift_rate: f32,
    drift_phase: f32,
    cliff_hz: f32,
    /// The output rate the filters are set for; 0 until a clap starts.
    sample_rate: f32,
    /// When the boom was last moved (s into the clap).
    moved_at: f32,
}

impl Colour {
    pub(super) fn new() -> Self {
        Self {
            low_cut: Biquad::unity(),
            boom: Biquad::unity(),
            cliff_first: Biquad::unity(),
            cliff_second: Biquad::unity(),
            boom_hz: BOOM_HZ.0,
            drift_rate: 0.0,
            drift_phase: 0.0,
            cliff_hz: 1_000.0,
            sample_rate: 0.0,
            moved_at: 0.0,
        }
    }

    pub(super) fn start(&mut self, kind: &Kind, random: &mut Rng) {
        self.boom_hz = BOOM_HZ.0 + (BOOM_HZ.1 - BOOM_HZ.0) * random.next_f32();
        self.drift_rate = BOOM_DRIFT_HZ.0 + (BOOM_DRIFT_HZ.1 - BOOM_DRIFT_HZ.0) * random.next_f32();
        self.drift_phase = random.next_f32() * TAU;
        self.cliff_hz = kind.cliff_hz;
        // Set afresh on the first sample, when the output rate is known.
        self.sample_rate = 0.0;
        self.moved_at = f32::NEG_INFINITY;
        self.low_cut.reset();
        self.boom.reset();
        self.cliff_first.reset();
        self.cliff_second.reset();
    }

    /// `roar` is thinned below the boom; `pulses` are not, since they carry
    /// the infrasonic swell for the LFE channel.
    pub(super) fn process(
        &mut self,
        roar: f32,
        pulses: f32,
        elapsed: f32,
        sample_rate: f32,
    ) -> f32 {
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.low_cut = Biquad::highpass(LOW_CUT_HZ, sample_rate);
            self.cliff_first = Biquad::lowpass(self.cliff_hz, sample_rate);
            self.cliff_second = Biquad::lowpass(self.cliff_hz, sample_rate);
            self.moved_at = f32::NEG_INFINITY;
        }
        if elapsed - self.moved_at >= MOMENT_SECONDS {
            let drift =
                1.0 + BOOM_DRIFT * (TAU * self.drift_rate * elapsed + self.drift_phase).sin();
            self.boom
                .set_peak(self.boom_hz * drift, BOOM_DB, BOOM_Q, sample_rate);
            self.moved_at = elapsed;
        }
        let shaped = self.boom.process(self.low_cut.process(roar) + pulses);
        self.cliff_second.process(self.cliff_first.process(shaped))
    }
}
