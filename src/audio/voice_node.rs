//! A voice with takes: it utters the take it is told to, when it is told to.
//!
//! What to utter and when is decided ahead by the session's plan
//! (`crate::plan`), not here. The voice only keeps appointments: it holds a
//! few upcoming utterances and starts each when the clock reaches it. A take
//! is one recording played from start to end; between takes the voice is
//! silent. Everything here is fixed-size, so nothing allocates on the audio
//! thread: the takes are decoded beforehand.

use std::sync::Arc;

use atrium_core::directivity::DirectivityPattern;
use atrium_core::source::EmitterKind;
use atrium_core::source::{SoundSource, Utterance};
use atrium_core::telemetry::SourceSignal;
use atrium_core::types::Vec3;

use crate::audio::decode::AudioBuffer;
use crate::audio::mute_fade::MuteFade;
use crate::audio::signal_meter::SignalMeter;

/// Seconds over which a take fades in and out, so its edges never click.
const EDGE_FADE_SECONDS: f32 = 0.005;
/// How many appointments a voice holds. The plan hands over less than half a
/// second at a time, which is a few utterances at most.
const APPOINTMENTS: usize = 8;

enum State {
    Silent,
    /// Uttering `take` from `position` (in the take's own samples), having
    /// started at `start`.
    Sounding {
        take: usize,
        position: f64,
        start: f64,
        gain: f32,
        rate: f64,
    },
}

/// A source that utters its takes when the plan says.
pub struct VoiceNode {
    takes: Vec<Arc<AudioBuffer>>,
    state: State,
    /// Upcoming utterances, earliest first; `None` is a free place.
    appointments: [Option<Utterance>; APPOINTMENTS],
    /// The session's clock, in output samples.
    clock: u64,
    position: Vec3,
    pub amplitude: f32,
    /// Base amplitude at 0 dB SPL: `amplitude = unit_amplitude · 10^(spl/20)`.
    pub unit_amplitude: f32,
    pub pattern: DirectivityPattern,
    pub spread: f32,
    /// The arc it arrives from when ambient, in degrees (360 all round).
    arc_degrees: f32,
    pub ref_dist: f32,
    facing: Option<f32>,
    muted: MuteFade,
    emitter_kind: EmitterKind,
    signal_meter: Option<SignalMeter>,
}

impl VoiceNode {
    /// A silent voice at `position` that can utter `takes`.
    pub fn new(takes: Vec<Arc<AudioBuffer>>, position: Vec3) -> Self {
        Self {
            takes,
            state: State::Silent,
            appointments: [None; APPOINTMENTS],
            clock: 0,
            position,
            amplitude: 0.5,
            unit_amplitude: 0.0,
            pattern: DirectivityPattern::OMNI,
            spread: 0.0,
            arc_degrees: atrium_core::source::ALL_ROUND_DEGREES,
            ref_dist: 1.0,
            facing: None,
            muted: MuteFade::new(),
            emitter_kind: EmitterKind::Object,
            signal_meter: None,
        }
    }

    pub(crate) fn meter_signal(&mut self, rms: f32) {
        self.signal_meter = Some(SignalMeter::new(rms * self.amplitude));
    }

    /// Start the earliest appointment if it is due and the voice is free.
    fn keep_appointment(&mut self) {
        if !matches!(self.state, State::Silent) {
            return;
        }
        let Some(utterance) = self.appointments[0].filter(|due| due.at_sample <= self.clock) else {
            return;
        };
        self.appointments.rotate_left(1);
        self.appointments[APPOINTMENTS - 1] = None;
        if let Some(audio) = self.takes.get(utterance.take as usize) {
            // Started partway, the take is that far along at its own rate.
            let skipped = utterance.skip_seconds.max(0.0) as f64
                * audio.sample_rate as f64
                * utterance.rate as f64;
            self.state = State::Sounding {
                take: utterance.take as usize,
                position: skipped,
                start: skipped,
                gain: utterance.gain,
                rate: utterance.rate as f64,
            };
        }
    }
}

impl SoundSource for VoiceNode {
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        self.keep_appointment();
        self.clock += 1;
        let sample = match &mut self.state {
            State::Silent => 0.0,
            State::Sounding {
                take,
                position,
                start,
                gain,
                rate,
            } => {
                let audio = &self.takes[*take];
                let length = audio.samples.len();
                let index = *position as usize;
                if index + 1 >= length {
                    self.state = State::Silent;
                    0.0
                } else {
                    let fraction = (*position - index as f64) as f32;
                    let value = audio.samples[index]
                        + (audio.samples[index + 1] - audio.samples[index]) * fraction;
                    // Fade the first and last few milliseconds.
                    let edge = (EDGE_FADE_SECONDS * audio.sample_rate as f32).max(1.0);
                    // Fade in from wherever it started, so a take begun partway does not click.
                    let from_start = (*position - *start) as f32 / edge;
                    let from_end = (length as f64 - 1.0 - *position) as f32 / edge;
                    let fade = from_start.min(from_end).clamp(0.0, 1.0);
                    let out = value * *gain * fade;
                    *position += audio.sample_rate as f64 / sample_rate as f64 * *rate;
                    out
                }
            }
        };

        let unmuted = self.muted.next_gain(sample_rate);
        if unmuted == 0.0 {
            // It keeps going while muted, so it comes back where it would be.
            return 0.0;
        }
        let output = sample * self.amplitude * unmuted;
        if let Some(meter) = &mut self.signal_meter {
            meter.push(output, sample_rate);
        }
        output
    }

    fn set_clock(&mut self, sample: u64) {
        self.clock = sample;
    }

    /// Kept in time order. With every place taken, the latest appointment
    /// gives way to an earlier one.
    fn schedule(&mut self, utterance: Utterance) {
        let at = self
            .appointments
            .iter()
            .position(|slot| slot.is_none_or(|held| held.at_sample > utterance.at_sample));
        if let Some(at) = at {
            self.appointments[at..].rotate_right(1);
            self.appointments[at] = Some(utterance);
        }
    }

    fn cancel(&mut self) {
        self.appointments = [None; APPOINTMENTS];
        self.state = State::Silent;
    }

    fn signal(&self) -> Option<SourceSignal> {
        self.signal_meter.as_ref().map(SignalMeter::signal)
    }

    fn sounding_take(&self) -> Option<u8> {
        match self.state {
            State::Sounding { take, .. } => Some(take as u8),
            State::Silent => None,
        }
    }

    fn position(&self) -> Vec3 {
        self.position
    }

    fn emitter_kind(&self) -> EmitterKind {
        self.emitter_kind
    }

    fn set_emitter_kind(&mut self, kind: EmitterKind) {
        self.emitter_kind = kind;
    }

    fn tick(&mut self, _dt: f32) {}

    fn orientation(&self) -> Vec3 {
        match self.facing {
            Some(yaw) => Vec3::new(yaw.cos(), yaw.sin(), 0.0),
            None => Vec3::new(1.0, 0.0, 0.0),
        }
    }

    fn directivity(&self) -> DirectivityPattern {
        self.pattern
    }

    fn is_muted(&self) -> bool {
        self.muted.is_muted()
    }

    fn set_muted(&mut self, muted: bool) {
        self.muted.set(muted);
    }

    fn set_position(&mut self, position: Vec3) {
        self.position = position;
    }

    fn spread(&self) -> f32 {
        self.spread
    }

    fn set_spread(&mut self, spread: f32) {
        self.spread = spread;
    }

    fn arc_degrees(&self) -> f32 {
        self.arc_degrees
    }

    fn set_arc_degrees(&mut self, degrees: f32) {
        self.arc_degrees = degrees;
    }

    fn set_reference_spl(&mut self, spl: f32) -> f32 {
        if self.unit_amplitude > 0.0 {
            self.amplitude = self.unit_amplitude * 10.0_f32.powf(spl / 20.0);
        }
        self.amplitude
    }

    fn set_directivity(&mut self, pattern: DirectivityPattern) {
        self.pattern = pattern;
    }

    fn set_facing(&mut self, yaw: f32) {
        self.facing = Some(yaw);
    }

    fn ref_distance(&self) -> f32 {
        self.ref_dist
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::spectral_profile::SpectralProfile;

    const RATE: u32 = 1_000;

    /// A take of `length` samples that all hold `value`, so the take being
    /// uttered can be read from the output.
    fn take(value: f32, length: usize) -> Arc<AudioBuffer> {
        Arc::new(AudioBuffer {
            samples: vec![value; length],
            sample_rate: RATE,
            rms: value.abs(),
            spectral_profile: SpectralProfile::default(),
        })
    }

    fn voice() -> VoiceNode {
        let mut node = VoiceNode::new(vec![take(0.1, 100), take(0.2, 100)], Vec3::ZERO);
        node.amplitude = 1.0;
        node
    }

    fn at(sample: u64, take: u8) -> Utterance {
        Utterance {
            take,
            at_sample: sample,
            gain: 1.0,
            rate: 1.0,
            skip_seconds: 0.0,
        }
    }

    /// It is silent until an appointment is due, then utters that take to
    /// its end, and is silent again.
    #[test]
    fn it_utters_a_take_when_the_clock_reaches_it() {
        let mut node = voice();
        node.schedule(at(50, 1));
        let output: Vec<f32> = (0..300).map(|_| node.next_sample(RATE as f32)).collect();
        assert!(
            output[..50].iter().all(|sample| *sample == 0.0),
            "silent before"
        );
        assert!((output[100] - 0.2).abs() < 1e-6, "take 1 in the middle");
        assert!(
            output[160..].iter().all(|sample| *sample == 0.0),
            "silent after"
        );
        assert_eq!(node.sounding_take(), None);
    }

    /// Appointments are kept in time order whatever order they came in, and
    /// one that falls during a take waits for it to end.
    #[test]
    fn appointments_are_kept_in_time_order() {
        let mut node = voice();
        node.schedule(at(300, 1));
        node.schedule(at(0, 0));
        node.schedule(at(50, 1));
        let mut uttered = Vec::new();
        let mut before = None;
        for _ in 0..500 {
            node.next_sample(RATE as f32);
            let now = node.sounding_take();
            if now != before {
                uttered.extend(now);
                before = now;
            }
        }
        assert_eq!(uttered, [0, 1, 1]);
    }

    /// The clock is the session's: a voice added late, told the time, keeps
    /// an appointment made for then and treats a past one as due now.
    #[test]
    fn a_voice_follows_the_sessions_clock() {
        let mut node = voice();
        node.set_clock(10_000);
        node.schedule(at(10_020, 0));
        for _ in 0..19 {
            node.next_sample(RATE as f32);
        }
        assert_eq!(node.sounding_take(), None);
        node.next_sample(RATE as f32);
        node.next_sample(RATE as f32);
        assert_eq!(node.sounding_take(), Some(0));

        let mut late = voice();
        late.set_clock(10_000);
        late.schedule(at(0, 1));
        late.next_sample(RATE as f32);
        assert_eq!(
            late.sounding_take(),
            Some(1),
            "a moment already past is kept at once"
        );
    }

    /// A take moved into partway starts there, and cancelling stops it and
    /// forgets what was to come.
    #[test]
    fn a_take_starts_partway_and_cancelling_forgets_it() {
        let mut node = voice();
        node.schedule(Utterance {
            skip_seconds: 0.08,
            ..at(0, 1)
        });
        node.schedule(at(500, 0));
        let heard: Vec<f32> = (0..40).map(|_| node.next_sample(RATE as f32)).collect();
        assert!(heard[0] < 0.2 * 0.5, "it fades in where it starts");
        assert!(
            heard[6..10]
                .iter()
                .all(|sample| (*sample - 0.2).abs() < 1e-6),
            "from 80 of its 100 samples"
        );
        assert!(
            heard[25..].iter().all(|sample| *sample == 0.0),
            "so it ends 20 samples later"
        );
        node.schedule(at(0, 0));
        node.next_sample(RATE as f32);
        node.cancel();
        assert_eq!(node.sounding_take(), None);
        for _ in 0..1000 {
            node.next_sample(RATE as f32);
        }
        assert_eq!(
            node.sounding_take(),
            None,
            "the appointment at 500 was forgotten"
        );
    }

    /// A muted voice is silent, and keeps its appointments so it comes back
    /// where it would have been.
    #[test]
    fn a_muted_voice_is_silent_and_keeps_going() {
        let mut node = voice();
        node.schedule(at(0, 0));
        node.set_muted(true);
        for _ in 0..50 {
            assert_eq!(node.next_sample(RATE as f32), 0.0);
        }
        assert_eq!(node.sounding_take(), Some(0));
    }

    /// The level and pitch an utterance was given are heard: twice the gain
    /// is twice as loud, and twice the rate ends in half the time.
    #[test]
    fn an_utterance_is_as_loud_and_as_fast_as_it_was_given() {
        let mut node = voice();
        node.schedule(Utterance {
            take: 0,
            at_sample: 0,
            gain: 2.0,
            rate: 2.0,
            skip_seconds: 0.0,
        });
        let output: Vec<f32> = (0..200).map(|_| node.next_sample(RATE as f32)).collect();
        assert!((output[25] - 0.2).abs() < 1e-6);
        assert!(
            output[55..].iter().all(|sample| *sample == 0.0),
            "100 samples at twice the rate"
        );
    }
}
