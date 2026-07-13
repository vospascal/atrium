//! Diffuse environmental-field renderer.
//!
//! Field emitters are not point sources made artificially wide. They bypass
//! distance attenuation, directivity, propagation paths, early reflections,
//! and the room-reverb send. A mono generator is phase-decorrelated into the
//! active spatial channels with constant total power, approximating mutually
//! incoherent arrivals around the listener.

use atrium_core::source::SoundSource;
use atrium_core::speaker::{SpeakerLayout, MAX_CHANNELS};

/// Representation expected by the stages following the source renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldEncoding {
    /// Write directly to physical/headphone output channels.
    Speaker,
    /// Write decorrelated W/Y/Z/X components for the Ambisonics decode stage.
    Ambisonics,
}

/// Energy-preserving Schroeder all-pass decorrelator.
struct AllPass {
    buffer: Vec<f32>,
    write: usize,
    feedback: f32,
}

impl AllPass {
    fn new(delay_samples: usize, feedback: f32) -> Self {
        Self {
            buffer: vec![0.0; delay_samples.max(1)],
            write: 0,
            feedback,
        }
    }

    #[inline(always)]
    fn process(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.write];
        let output = delayed - self.feedback * input;
        self.buffer[self.write] = input + self.feedback * output;
        self.write += 1;
        if self.write == self.buffer.len() {
            self.write = 0;
        }
        output
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write = 0;
    }
}

struct FieldSourceState {
    voices: [AllPass; MAX_CHANNELS],
}

impl FieldSourceState {
    fn new(sample_rate: f32) -> Self {
        // Mutually-prime-ish delays spanning roughly 7–31 ms. These are long
        // enough to reduce inter-channel coherence without reading as echoes for
        // a continuous stochastic texture. Alternating feedback signs further
        // diversify phase while every all-pass keeps the magnitude response flat.
        const DELAY_MS: [f32; MAX_CHANNELS] = [7.1, 8.9, 11.3, 13.7, 17.9, 21.1, 25.7, 31.1];
        Self {
            voices: std::array::from_fn(|channel| {
                let delay = (DELAY_MS[channel] * 0.001 * sample_rate).round() as usize;
                let feedback = if channel % 2 == 0 { 0.61 } else { -0.57 };
                AllPass::new(delay, feedback)
            }),
        }
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.reset();
        }
    }
}

/// Per-pipeline field state. Allocated when topology is prepared, never from
/// the per-sample render loop.
#[derive(Default)]
pub struct FieldRenderer {
    sources: Vec<FieldSourceState>,
    sample_rate: f32,
}

impl FieldRenderer {
    pub fn ensure_topology(&mut self, source_count: usize, sample_rate: f32) {
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.sources.clear();
        }
        while self.sources.len() < source_count {
            self.sources.push(FieldSourceState::new(sample_rate));
        }
    }

    pub fn reset(&mut self) {
        for source in &mut self.sources {
            source.reset();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_source(
        &mut self,
        source_index: usize,
        source: &mut dyn SoundSource,
        encoding: FieldEncoding,
        layout: &SpeakerLayout,
        sample_rate: f32,
        channels: usize,
        num_frames: usize,
        output: &mut [f32],
    ) {
        let Some(state) = self.sources.get_mut(source_index) else {
            return;
        };

        let mut target_channels = [usize::MAX; MAX_CHANNELS];
        let mut target_count = 0;
        match encoding {
            FieldEncoding::Ambisonics if channels >= 4 => {
                // ACN/SN3D channel order used by the pipeline: W, Y, Z, X.
                target_channels[..4].copy_from_slice(&[0, 1, 2, 3]);
                target_count = 4;
            }
            _ => {
                for speaker in layout.speakers() {
                    if target_count == MAX_CHANNELS {
                        break;
                    }
                    if Some(speaker.channel) == layout.lfe_channel() {
                        continue;
                    }
                    if speaker.channel < channels && layout.is_channel_active(speaker.channel) {
                        target_channels[target_count] = speaker.channel;
                        target_count += 1;
                    }
                }
                // Defensive fallback for a channel configuration without a
                // positional Speaker entry (for example a minimal headphone host).
                if target_count == 0 {
                    for channel in 0..channels.min(MAX_CHANNELS) {
                        if Some(channel) != layout.lfe_channel() {
                            target_channels[target_count] = channel;
                            target_count += 1;
                        }
                    }
                }
            }
        }

        if target_count == 0 {
            return;
        }
        let gain = 1.0 / (target_count as f32).sqrt();
        for frame in 0..num_frames {
            let mono = source.next_sample(sample_rate);
            let base = frame * channels;
            for voice_index in 0..target_count {
                let channel = target_channels[voice_index];
                let decorrelated = state.voices[voice_index].process(mono);
                output[base + channel] += decorrelated * gain;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allpass_decorrelators_preserve_energy_but_diverge_in_time() {
        let mut a = AllPass::new(17, 0.61);
        let mut b = AllPass::new(29, -0.57);
        let mut in_energy = 0.0;
        let mut a_energy = 0.0;
        let mut b_energy = 0.0;
        let mut difference = 0.0;
        for n in 0..20_000 {
            let input = ((n as f32 * 0.731).sin() + (n as f32 * 1.117).sin()) * 0.5;
            let av = a.process(input);
            let bv = b.process(input);
            if n > 200 {
                in_energy += input * input;
                a_energy += av * av;
                b_energy += bv * bv;
                difference += (av - bv).abs();
            }
        }
        assert!((a_energy / in_energy - 1.0).abs() < 0.03);
        assert!((b_energy / in_energy - 1.0).abs() < 0.03);
        assert!(
            difference > 100.0,
            "field voices should not remain coherent"
        );
    }
}
