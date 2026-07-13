//! Scene wrapper for procedural synth generators.
//!
//! The generators in `crate::synth` are pure DSP — they produce raw mono
//! samples and know nothing about SPL calibration, directivity, spread, or
//! muting. `SynthNode` wraps one and gives it the same scene-facing surface
//! as the sample-playing `TestNode`, so the pipeline treats both identically.

use crate::world::types::Vec3;
use atrium_core::directivity::DirectivityPattern;
use atrium_core::source::SoundSource;

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
    muted: bool,
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
            muted: false,
        }
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
        sample * self.amplitude
    }

    fn position(&self) -> Vec3 {
        self.position
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

    fn is_muted(&self) -> bool {
        self.muted
    }

    fn set_muted(&mut self, muted: bool) {
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
}
