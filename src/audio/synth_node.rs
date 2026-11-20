//! Scene wrapper for procedural synth generators.
//!
//! The generators in `crate::synth` are pure DSP — they produce raw mono
//! samples and know nothing about SPL calibration, directivity, spread, or
//! muting. `SynthNode` wraps one and gives it the same scene-facing surface
//! as the sample-playing `TestNode`, so the pipeline treats both identically.

use crate::audio::signal_meter::SignalMeter;
use atrium_core::directivity::DirectivityPattern;
use atrium_core::source::SoundSource;
use atrium_core::telemetry::SourceSignal;
use atrium_core::types::Vec3;

/// A procedural sound source placed in the scene: a raw generator plus the
/// SPL/directivity/spread state every scene source carries.
pub struct SynthNode {
    generator: Box<dyn SoundSource>,
    /// Playback amplitude derived from reference SPL and preview-render RMS.
    pub amplitude: f32,
    /// Base amplitude at 0 dB SPL: `amplitude = unit_amplitude · 10^(spl/20)`.
    /// Set by the scene builder so a live SPL edit can rescale amplitude
    /// without re-rendering the preview.
    pub unit_amplitude: f32,
    pub pattern: DirectivityPattern,
    pub spread: f32,
    pub ref_dist: f32,
    position: Vec3,
    /// The heading it was turned to (radians); +X until it is turned.
    facing: Option<f32>,
    muted: bool,
    signal_meter: Option<SignalMeter>,
}

impl SynthNode {
    pub fn new(generator: Box<dyn SoundSource>, position: Vec3) -> Self {
        Self {
            generator,
            amplitude: 0.5,
            unit_amplitude: 0.0,
            pattern: DirectivityPattern::OMNI,
            spread: 0.0,
            ref_dist: 1.0,
            position,
            facing: None,
            muted: false,
            signal_meter: None,
        }
    }

    pub(crate) fn meter_signal(&mut self, rms: f32) {
        self.signal_meter = Some(SignalMeter::new(rms * self.amplitude));
    }
}

impl SoundSource for SynthNode {
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        // Always advance the generator so envelopes/gust cycles keep evolving
        // while muted — unmuting resumes mid-weather instead of restarting.
        let sample = self.generator.next_sample(sample_rate);
        if self.muted {
            return 0.0;
        }
        let output = sample * self.amplitude;
        if let Some(meter) = &mut self.signal_meter {
            meter.push(output, sample_rate);
        }
        output
    }

    fn signal(&self) -> Option<SourceSignal> {
        self.signal_meter.as_ref().map(SignalMeter::signal)
    }

    fn position(&self) -> Vec3 {
        // Anchor plus whatever the generator has flown since (zero for
        // stationary generators).
        self.position + self.generator.motion_offset()
    }

    fn emitter_kind(&self) -> atrium_core::source::EmitterKind {
        self.generator.emitter_kind()
    }

    fn tick(&mut self, dt: f32) {
        self.generator.tick(dt);
    }

    fn directivity(&self) -> DirectivityPattern {
        self.pattern
    }

    fn orientation(&self) -> Vec3 {
        match self.facing {
            Some(yaw) => Vec3::new(yaw.cos(), yaw.sin(), 0.0),
            None => Vec3::new(1.0, 0.0, 0.0),
        }
    }

    fn is_muted(&self) -> bool {
        self.muted
    }

    fn set_muted(&mut self, muted: bool) {
        if muted && !self.muted {
            if let Some(meter) = &mut self.signal_meter {
                meter.reset();
            }
        }
        self.muted = muted;
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

    fn set_synth_param(&mut self, param: atrium_core::commands::SynthParam, value: f32) {
        self.generator.set_synth_param(param, value);
    }

    fn ref_distance(&self) -> f32 {
        self.ref_dist
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::field_wind::FieldWindSource;

    fn field_wind_node() -> SynthNode {
        let generator = Box::new(FieldWindSource::new(Vec3::ZERO, 1.0, 8.0, 42));
        SynthNode::new(generator, Vec3::new(1.0, 2.0, 3.0))
    }

    #[test]
    fn muted_node_outputs_silence_but_keeps_evolving() {
        let mut node = field_wind_node();
        node.amplitude = 1.0;
        node.set_muted(true);
        let energy: f32 = (0..4800).map(|_| node.next_sample(48000.0).powi(2)).sum();
        assert_eq!(energy, 0.0, "muted synth node must be silent");

        node.set_muted(false);
        let energy: f32 = (0..4800).map(|_| node.next_sample(48000.0).powi(2)).sum();
        assert!(energy > 0.0, "unmuted synth node must produce audio");
    }

    #[test]
    fn spl_edit_rescales_amplitude_by_20db_per_decade() {
        let mut node = field_wind_node();
        node.unit_amplitude = 0.001;
        let a40 = node.set_reference_spl(40.0);
        let a60 = node.set_reference_spl(60.0);
        // +20 dB SPL must be exactly 10× the amplitude.
        assert!(
            (a60 / a40 - 10.0).abs() < 1e-4,
            "expected 10× amplitude for +20 dB, got {}",
            a60 / a40
        );
    }

    use crate::synth::river::RiverSource;
    use crate::synth::wisp::WispSource;
    use atrium_core::commands::SynthParam;
    use atrium_core::source::EmitterKind;

    fn river_node(seed: u64) -> SynthNode {
        SynthNode::new(
            Box::new(RiverSource::new(Vec3::ZERO, 0.8, 0.8, seed)),
            Vec3::ZERO,
        )
    }

    /// Output = generator sample x amplitude, sample for sample.
    #[test]
    fn output_is_the_generator_scaled_by_the_amplitude() {
        let mut node = river_node(5);
        node.amplitude = 0.25;
        let mut raw = RiverSource::new(Vec3::ZERO, 0.8, 0.8, 5);
        for _ in 0..2_000 {
            let expected = raw.next_sample(48_000.0) * 0.25;
            assert_eq!(node.next_sample(48_000.0), expected);
        }
    }

    /// The generator keeps advancing while muted: the first sample after
    /// unmuting is the generator's sample N, not its sample 0.
    #[test]
    fn muting_does_not_pause_the_generator() {
        let mut node = river_node(6);
        node.amplitude = 1.0;
        node.set_muted(true);
        assert!(node.is_muted());
        let mut raw = RiverSource::new(Vec3::ZERO, 0.8, 0.8, 6);
        for _ in 0..5_000 {
            assert_eq!(node.next_sample(48_000.0), 0.0);
            raw.next_sample(48_000.0);
        }
        node.set_muted(false);
        assert!(!node.is_muted());
        for _ in 0..100 {
            assert_eq!(node.next_sample(48_000.0), raw.next_sample(48_000.0));
        }
    }

    /// amplitude = unit_amplitude x 10^(spl / 20): 0 dB is the unit, +6.02 dB
    /// doubles, -20 dB divides by ten; the new amplitude is returned and
    /// stored. With no unit calibration the amplitude is left alone.
    #[test]
    fn reference_spl_follows_the_decibel_law() {
        let mut node = river_node(1);
        node.unit_amplitude = 0.002;
        assert!((node.set_reference_spl(0.0) - 0.002).abs() < 1e-9);
        let doubled = node.set_reference_spl(20.0 * 2.0_f32.log10());
        assert!((doubled - 0.004).abs() < 1e-7);
        assert!((node.set_reference_spl(-20.0) - 0.0002).abs() < 1e-8);
        let returned = node.set_reference_spl(-20.0);
        assert_eq!(node.amplitude, returned);

        let mut uncalibrated = river_node(1);
        uncalibrated.amplitude = 0.7;
        assert_eq!(uncalibrated.set_reference_spl(80.0), 0.7);
        assert_eq!(uncalibrated.amplitude, 0.7);
    }

    /// Position = anchor + whatever the generator has flown; a stationary
    /// generator reports exactly its anchor, and `set_position` moves it.
    #[test]
    fn position_is_the_anchor_plus_the_generators_motion() {
        let mut still = river_node(1);
        still.set_position(Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(still.position(), Vec3::new(1.0, 2.0, 3.0));

        let mut flying = SynthNode::new(
            Box::new(WispSource::new(Vec3::ZERO, 3)),
            Vec3::new(10.0, 0.0, 0.0),
        );
        let start = flying.position();
        for _ in 0..300 {
            flying.tick(0.01);
        }
        let moved = flying.position() - start;
        assert!(
            moved.length() > 0.1,
            "tick was not forwarded to the generator"
        );
        flying.set_position(Vec3::new(-5.0, 0.0, 0.0));
        // The wisp's own offset survives re-anchoring.
        let offset = flying.position() - Vec3::new(-5.0, 0.0, 0.0);
        assert!((offset - (start - Vec3::new(10.0, 0.0, 0.0) + moved)).length() < 1e-4);
    }

    #[test]
    fn scene_facing_state_is_stored_and_the_emitter_kind_is_forwarded() {
        let mut node = river_node(1);
        assert_eq!(node.emitter_kind(), EmitterKind::Object);
        assert_eq!(node.spread(), 0.0);
        node.set_spread(0.4);
        assert_eq!(node.spread(), 0.4);
        assert_eq!(node.ref_distance(), 1.0);
        node.ref_dist = 2.5;
        assert_eq!(node.ref_distance(), 2.5);
        node.set_directivity(DirectivityPattern::cardioid());
        assert!(matches!(node.directivity(), DirectivityPattern::Polar { alpha } if alpha == 0.5));
        assert!(node.is_active());
        assert_eq!(field_wind_node().emitter_kind(), EmitterKind::Field);
    }

    #[test]
    fn synth_params_reach_the_generator() {
        let mut node = river_node(1);
        node.amplitude = 1.0;
        node.set_synth_param(SynthParam::MasterGain, 0.0);
        let energy: f32 = (0..2_000).map(|_| node.next_sample(48_000.0).powi(2)).sum();
        assert_eq!(energy, 0.0, "master gain 0 should silence the river");
    }
}
