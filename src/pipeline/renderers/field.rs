//! Diffuse environmental-field renderer.
//!
//! Field emitters are not point sources made artificially wide. They bypass
//! distance attenuation, directivity, propagation paths, early reflections,
//! and the room-reverb send. For loudspeakers and the ambisonic bus a mono
//! generator is phase-decorrelated into the active spatial channels with
//! constant total power, approximating mutually incoherent arrivals around
//! the listener.
//!
//! Any sound can be ambient, so the decorrelation must leave an attack (a
//! croak, a drop of rain) heard once: each channel gets a short all-pass
//! cascade (`decorrelator`) that spends over 90 % of an attack's energy in
//! its first 10 ms.
//!
//! On headphones the field is not decorrelated here. It joins one mono bus,
//! and the headphone renderer gives each ear what the measured head hears of
//! a diffuse field (`renderers::binaural`).

use atrium_core::source::SoundSource;
use atrium_core::speaker::{SpeakerLayout, MAX_CHANNELS};

mod decorrelator;

use decorrelator::Decorrelator;

/// Representation expected by the stages following the source renderer.
/// Chosen by `Renderer::field_encoding`, never from the channel count here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldEncoding {
    /// Write directly to the active physical/headphone output channels.
    Speaker,
    /// Write decorrelated W/Y/Z/X components to channels 0–3 for the
    /// Ambisonics decode stage. Only valid on a B-format bus
    /// (`AmbisonicBus::BFormat`), which guarantees at least four channels.
    Ambisonics,
    /// Add the mono signal, undecorrelated, to the ambient bus that the
    /// renderer then hears through two ears (`Renderer::render_ambient_bus`).
    Ears,
}

struct FieldSourceState {
    voices: [Decorrelator; MAX_CHANNELS],
}

impl FieldSourceState {
    fn new(sample_rate: f32) -> Self {
        Self {
            voices: std::array::from_fn(|channel| Decorrelator::new(channel, sample_rate)),
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

    /// `heard` is how much of the field the last buffer let through and how
    /// much this one does, each 0 to 1: 1 and 1 unless a probe hides ambient
    /// sound.
    #[allow(clippy::too_many_arguments)]
    pub fn render_source(
        &mut self,
        source_index: usize,
        source: &mut dyn SoundSource,
        encoding: FieldEncoding,
        layout: &SpeakerLayout,
        sample_rate: f32,
        channels: usize,
        render_channels: usize,
        num_frames: usize,
        heard: (f32, f32),
        output: &mut [f32],
    ) {
        let Some(state) = self.sources.get_mut(source_index) else {
            return;
        };
        // How much of the field reaches the output glides from the last
        // buffer's share to this one's, so a probe fades it and never cuts.
        let (was_heard, now_heard) = heard;
        let glide = (now_heard - was_heard) / num_frames.max(1) as f32;

        let mut target_channels = [usize::MAX; MAX_CHANNELS];
        let mut target_count = 0;
        match encoding {
            FieldEncoding::Ambisonics => {
                debug_assert!(
                    channels >= 4,
                    "a B-format bus needs four channels, got {channels}"
                );
                // ACN/SN3D channel order used by the pipeline: W, Y, Z, X.
                target_channels[..4].copy_from_slice(&[0, 1, 2, 3]);
                target_count = 4;
            }
            FieldEncoding::Speaker => {
                for speaker in layout.speakers() {
                    if target_count == MAX_CHANNELS {
                        break;
                    }
                    if Some(speaker.channel) == layout.lfe_channel() {
                        continue;
                    }
                    if speaker.channel < render_channels
                        && layout.is_channel_active(speaker.channel)
                    {
                        target_channels[target_count] = speaker.channel;
                        target_count += 1;
                    }
                }
                // Defensive fallback for a channel configuration without a
                // positional Speaker entry (for example a minimal headphone host).
                if target_count == 0 {
                    for channel in 0..render_channels.min(MAX_CHANNELS) {
                        if Some(channel) != layout.lfe_channel() {
                            target_channels[target_count] = channel;
                            target_count += 1;
                        }
                    }
                }
            }
            FieldEncoding::Ears => {
                // One bus for every ambient sound: the two ears' filters
                // that follow make it diffuse, and they are linear, so each
                // source needs nothing of its own.
                for frame in 0..num_frames {
                    let level = was_heard + glide * frame as f32;
                    output[frame * channels] += source.next_sample(sample_rate) * level;
                }
                return;
            }
        }

        if target_count == 0 {
            return;
        }
        let gain = 1.0 / (target_count as f32).sqrt();
        for frame in 0..num_frames {
            let mono = source.next_sample(sample_rate);
            let base = frame * channels;
            let level = gain * (was_heard + glide * frame as f32);
            for voice_index in 0..target_count {
                let channel = target_channels[voice_index];
                let decorrelated = state.voices[voice_index].process(mono);
                output[base + channel] += decorrelated * level;
            }
        }
    }
}

#[cfg(test)]
mod tests;
