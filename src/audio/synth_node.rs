//! Scene wrapper for procedural synth generators.
//!
//! The generators in `crate::synth` are pure DSP — they produce raw mono
//! samples and know nothing about SPL calibration, directivity, spread, or
//! muting. `SynthNode` wraps one and gives it the same scene-facing surface
//! as the sample-playing `TestNode`, so the pipeline treats both identically.

use crate::audio::mute_fade::MuteFade;
use crate::audio::signal_meter::SignalMeter;
use atrium_core::directivity::DirectivityPattern;
use atrium_core::layers::{LayerMix, LayerSetting, MOST_LAYERS};
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
    /// The arc it arrives from when ambient, in degrees (360 all round).
    arc_degrees: f32,
    pub ref_dist: f32,
    position: Vec3,
    /// The heading it was turned to (radians); +X until it is turned.
    facing: Option<f32>,
    muted: MuteFade,
    /// What the scene made of it, in place of the generator's own kind.
    emitter_kind: Option<atrium_core::source::EmitterKind>,
    signal_meter: Option<SignalMeter>,
    /// A second voice sounding at the same moment from another place: the
    /// right ear's tone of a binaural beat. Whatever the scene sets on this
    /// node is set on it too, so the two stay one sound.
    second_voice: Option<Box<SynthNode>>,
    /// The mix of its layers. With a second voice the two voices are its two
    /// layers: this voice is layer 0 and the second layer 1, which is set to
    /// what this mix leaves of it.
    layer_mix: LayerMix,
}

impl SynthNode {
    pub fn new(generator: Box<dyn SoundSource>, position: Vec3) -> Self {
        Self {
            generator,
            amplitude: 0.5,
            unit_amplitude: 0.0,
            pattern: DirectivityPattern::OMNI,
            spread: 0.0,
            arc_degrees: atrium_core::source::ALL_ROUND_DEGREES,
            ref_dist: 1.0,
            position,
            facing: None,
            muted: MuteFade::new(),
            emitter_kind: None,
            signal_meter: None,
            second_voice: None,
            layer_mix: LayerMix::untouched(),
        }
    }

    /// Give it `generator` as a second voice, calibrated, placed and shaped
    /// as this node is now. Set it up first: what is set afterwards reaches
    /// both voices.
    pub(crate) fn add_second_voice(&mut self, generator: Box<dyn SoundSource>) {
        let mut voice = SynthNode::new(generator, self.position);
        voice.amplitude = self.amplitude;
        voice.unit_amplitude = self.unit_amplitude;
        voice.pattern = self.pattern;
        voice.spread = self.spread;
        voice.arc_degrees = self.arc_degrees;
        voice.ref_dist = self.ref_dist;
        voice.facing = self.facing;
        voice.signal_meter = self.signal_meter.as_ref().map(SignalMeter::fresh);
        self.second_voice = Some(Box::new(voice));
    }

    /// Put every layer's weight where its setting asks at once, as for a
    /// sound built with its layers already set: no glide from 0 dB.
    pub(crate) fn settle_layers(&mut self) {
        self.layer_mix.settle();
        self.also_second_voice(SynthNode::settle_layers);
    }

    /// Do `change` to the second voice, if it has one.
    fn also_second_voice(&mut self, change: impl FnOnce(&mut SynthNode)) {
        if let Some(voice) = &mut self.second_voice {
            change(voice);
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
        self.layer_mix.advance(sample_rate);
        let sample = self.generator.next_mixed(sample_rate, &mut self.layer_mix);
        let unmuted = self.muted.next_gain(sample_rate);
        if unmuted == 0.0 {
            return 0.0;
        }
        let output = sample * self.amplitude * unmuted;
        if let Some(meter) = &mut self.signal_meter {
            meter.push(output, sample_rate);
        }
        output
    }

    /// The meter's reading, with each layer's RMS: a second voice's from its
    /// own meter, a generator's layers as their shares of the power the mix
    /// summed.
    fn signal(&self) -> Option<SourceSignal> {
        let mut signal = self.signal_meter.as_ref().map(SignalMeter::signal)?;
        let count = self.layer_count().min(MOST_LAYERS);
        signal.layer_count = count as u8;
        signal.layer_rms = [0.0; MOST_LAYERS];
        if let Some(voice) = &self.second_voice {
            signal.layer_rms[0] = signal.relative_rms;
            signal.layer_rms[1] = voice.signal().map_or(0.0, |voice| voice.relative_rms);
        } else {
            let powers = self.layer_mix.powers();
            let total: f32 = powers[..count].iter().sum();
            for (rms, power) in signal.layer_rms.iter_mut().zip(&powers[..count]) {
                *rms = if total > 0.0 {
                    signal.relative_rms * (power / total).sqrt()
                } else {
                    0.0
                };
            }
        }
        Some(signal)
    }

    /// The generator's layers, or with a second voice the two voices.
    fn layer_count(&self) -> usize {
        if self.second_voice.is_some() {
            2
        } else {
            self.generator.layer_count()
        }
    }

    fn set_layer(&mut self, layer: usize, setting: LayerSetting) {
        if layer >= self.layer_count() {
            return;
        }
        self.layer_mix.set(layer, setting);
        let heard = self.layer_mix.heard(1);
        if let Some(voice) = &mut self.second_voice {
            voice.set_layer(0, heard);
        }
    }

    /// The generator's own: a generator that keeps appointments (thunder)
    /// rests between them; one that never rests is always on take 0.
    fn sounding_take(&self) -> Option<u8> {
        self.generator.sounding_take()
    }

    fn set_clock(&mut self, sample: u64) {
        self.generator.set_clock(sample);
        self.also_second_voice(|voice| voice.set_clock(sample));
    }

    fn schedule(&mut self, utterance: atrium_core::source::Utterance) {
        self.generator.schedule(utterance);
        self.also_second_voice(|voice| voice.schedule(utterance));
    }

    fn cancel(&mut self) {
        self.generator.cancel();
        self.also_second_voice(SynthNode::cancel);
    }

    fn position(&self) -> Vec3 {
        // Anchor plus whatever the generator has flown since (zero for
        // stationary generators).
        self.position + self.generator.motion_offset()
    }

    fn beside_head(&self) -> Option<Vec3> {
        self.generator.beside_head()
    }

    fn second_voice(&mut self) -> Option<&mut dyn SoundSource> {
        self.second_voice
            .as_deref_mut()
            .map(|voice| voice as &mut dyn SoundSource)
    }

    fn emitter_kind(&self) -> atrium_core::source::EmitterKind {
        self.emitter_kind
            .unwrap_or_else(|| self.generator.emitter_kind())
    }

    /// A sound held beside the head is a point there: it cannot be made
    /// ambient, and stays what it is.
    fn set_emitter_kind(&mut self, kind: atrium_core::source::EmitterKind) {
        if self.beside_head().is_some() {
            return;
        }
        self.emitter_kind = Some(kind);
    }

    fn tick(&mut self, dt: f32) {
        self.generator.tick(dt);
        self.also_second_voice(|voice| voice.tick(dt));
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
        self.muted.is_muted()
    }

    fn set_muted(&mut self, muted: bool) {
        if muted && !self.muted.is_muted() {
            if let Some(meter) = &mut self.signal_meter {
                meter.reset();
            }
        }
        self.muted.set(muted);
        self.also_second_voice(|voice| voice.set_muted(muted));
    }

    fn set_position(&mut self, position: Vec3) {
        self.position = position;
        self.also_second_voice(|voice| voice.set_position(position));
    }

    fn spread(&self) -> f32 {
        self.spread
    }

    fn set_spread(&mut self, spread: f32) {
        self.spread = spread;
        self.also_second_voice(|voice| voice.set_spread(spread));
    }

    fn arc_degrees(&self) -> f32 {
        self.arc_degrees
    }

    fn set_arc_degrees(&mut self, degrees: f32) {
        self.arc_degrees = degrees;
        self.also_second_voice(|voice| voice.set_arc_degrees(degrees));
    }

    fn set_reference_spl(&mut self, spl: f32) -> f32 {
        if self.unit_amplitude > 0.0 {
            self.amplitude = self.unit_amplitude * 10.0_f32.powf(spl / 20.0);
        }
        self.also_second_voice(|voice| {
            voice.set_reference_spl(spl);
        });
        self.amplitude
    }

    fn set_directivity(&mut self, pattern: DirectivityPattern) {
        self.pattern = pattern;
        self.also_second_voice(|voice| voice.set_directivity(pattern));
    }

    fn set_facing(&mut self, yaw: f32) {
        self.facing = Some(yaw);
        self.also_second_voice(|voice| voice.set_facing(yaw));
    }

    fn set_synth_param(&mut self, param: atrium_core::commands::SynthParam, value: f32) {
        self.generator.set_synth_param(param, value);
        self.also_second_voice(|voice| voice.set_synth_param(param, value));
    }

    fn ref_distance(&self) -> f32 {
        self.ref_dist
    }
}

#[cfg(test)]
mod layer_checks;

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
        // It fades back in over 10 ms, 480 samples.
        for step in 1..=600 {
            let fade = (step as f32 / 480.0).min(1.0);
            let expected = raw.next_sample(48_000.0) * fade;
            assert!((node.next_sample(48_000.0) - expected).abs() <= 1e-5);
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
        assert_eq!(node.arc_degrees(), 360.0);
        node.set_arc_degrees(90.0);
        assert_eq!(node.arc_degrees(), 90.0);
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

    /// A node with a second voice: the right ear's tone of a binaural beat.
    /// What the scene sets on the node (mute, level, settings) reaches both
    /// voices; both are held beside the head; neither can be made ambient.
    #[test]
    fn a_second_voice_follows_everything_set_on_the_node() {
        use crate::synth::binaural_beat::{BinauralBeatSettings, BinauralToneSource, Ear};
        let settings = BinauralBeatSettings::default();
        let mut node = SynthNode::new(
            Box::new(BinauralToneSource::new(Ear::Left, settings)),
            Vec3::ZERO,
        );
        node.unit_amplitude = 0.001;
        node.set_reference_spl(40.0);
        node.add_second_voice(Box::new(BinauralToneSource::new(Ear::Right, settings)));
        let second_amplitude = |node: &SynthNode| node.second_voice.as_ref().unwrap().amplitude;
        assert_eq!(second_amplitude(&node), node.amplitude);
        node.set_reference_spl(60.0);
        assert_eq!(second_amplitude(&node), node.amplitude);

        let left_ear = node.beside_head().expect("beside the head");
        let right_ear = node.second_voice().unwrap().beside_head().unwrap();
        assert_eq!(right_ear, Vec3::new(0.0, -left_ear.y, 0.0));

        node.set_emitter_kind(EmitterKind::Field);
        assert_eq!(node.emitter_kind(), EmitterKind::Object);

        // A setting reaches the second voice: it plays the right ear's tone
        // with the new beat, at the node's amplitude.
        node.set_synth_param(SynthParam::Beat, 30.0);
        let amplitude = node.amplitude;
        let mut reference = BinauralToneSource::new(Ear::Right, settings);
        reference.set_synth_param(SynthParam::Beat, 30.0);
        let voice = node.second_voice().unwrap();
        for _ in 0..4_800 {
            assert_eq!(
                voice.next_sample(48_000.0),
                reference.next_sample(48_000.0) * amplitude
            );
        }
        // Muting the node mutes both voices.
        node.set_muted(true);
        let voice = node.second_voice().unwrap();
        assert!(voice.is_muted());
        for _ in 0..4_800 {
            voice.next_sample(48_000.0);
        }
        assert_eq!(voice.next_sample(48_000.0), 0.0);
        // A sound with one voice has no second.
        assert!(river_node(1).second_voice().is_none());
    }
}
