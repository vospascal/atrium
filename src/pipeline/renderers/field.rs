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
//! An ambient sound may come from an arc of directions instead of all round
//! (`ambient_arc`). On speakers each speaker's copy then carries the share
//! of the arc's power that panning over the arc would give it
//! (`speaker_shares`); on the ambisonic bus three copies are mixed into the
//! arc's B-format covariance (`bformat_arc`). All round takes the path it
//! always took, and a sound glides between the two over one callback.
//!
//! On headphones the field is not decorrelated here. The headphone renderer
//! gives each ear what the measured head hears of it, all round or from its
//! arc (`renderers::binaural`); this only hands it the sound.

use atrium_core::source::SoundSource;
use atrium_core::speaker::{SpeakerLayout, MAX_CHANNELS};
use atrium_core::types::Vec3;

mod ambient_arc;
mod bformat_arc;
mod decorrelator;
mod speaker_shares;

pub use ambient_arc::AmbientArc;
use bformat_arc::bformat_mix;
use decorrelator::Decorrelator;
use speaker_shares::speaker_shares;

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
    /// Hand the mono signal, undecorrelated, to the renderer, which hears it
    /// through two ears (`Renderer::hear_ambient`).
    Ears,
}

/// How each output (a speaker, or W, Y, Z, X) is made of the decorrelated
/// voices: `[output][voice]`.
type Mix = [[f32; MAX_CHANNELS]; MAX_CHANNELS];

/// What an arc's mix was worked out from: the arc, and the speakers'
/// headings from the receiver (or, on the ambisonic bus, the arc's middle
/// relative to where the listener faces). It is worked out again only when
/// one of them changes.
#[derive(Clone, Copy, PartialEq)]
struct MixInputs {
    width: f32,
    centre: f32,
    headings: [f32; MAX_CHANNELS],
    outputs: usize,
}

struct FieldSourceState {
    voices: [Decorrelator; MAX_CHANNELS],
    /// The mix the last callback ended on while the sound is, or is gliding
    /// from, an arc; `None` while it is steadily all round.
    mix: Option<Mix>,
    /// The last arc mix and what it was worked out from.
    arc_mix: Option<(MixInputs, Mix)>,
    /// Whether anything was rendered since it was made or reset: until
    /// then there is nothing to glide from, and an arc starts as itself.
    rendered: bool,
}

impl FieldSourceState {
    fn new(sample_rate: f32) -> Self {
        Self {
            voices: std::array::from_fn(|channel| Decorrelator::new(channel, sample_rate)),
            mix: None,
            arc_mix: None,
            rendered: false,
        }
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.reset();
        }
        self.mix = None;
        self.rendered = false;
    }

    /// The mix of an arc, worked out again only when what it depends on
    /// changed.
    fn arc_mix(&mut self, inputs: MixInputs, encoding: FieldEncoding) -> Mix {
        if let Some((cached, mix)) = self.arc_mix {
            if cached == inputs {
                return mix;
            }
        }
        let mut mix = [[0.0f32; MAX_CHANNELS]; MAX_CHANNELS];
        let arc = AmbientArc {
            width: inputs.width,
            centre: inputs.centre,
        };
        match encoding {
            FieldEncoding::Ambisonics => {
                for (output, row) in bformat_mix(arc.width, arc.centre).iter().enumerate() {
                    mix[output][..4].copy_from_slice(row);
                }
            }
            FieldEncoding::Speaker | FieldEncoding::Ears => {
                let shares = speaker_shares(arc, &inputs.headings[..inputs.outputs]);
                for output in 0..inputs.outputs {
                    mix[output][output] = shares[output].sqrt();
                }
            }
        }
        self.arc_mix = Some((inputs, mix));
        mix
    }
}

/// The mix of the field all round: each of `outputs` voices to its own
/// output at `gain`, as the field has always been rendered.
fn all_round_mix(outputs: usize, gain: f32) -> Mix {
    let mut mix = [[0.0f32; MAX_CHANNELS]; MAX_CHANNELS];
    for (output, row) in mix.iter_mut().enumerate().take(outputs) {
        row[output] = gain;
    }
    mix
}

/// Where one callback of an ambient sound comes from: all round, or `arc`,
/// heard at `receiver` by a listener facing `listener_yaw`.
#[derive(Clone, Copy)]
pub struct FieldPlacement {
    pub arc: Option<AmbientArc>,
    pub receiver: Vec3,
    pub listener_yaw: f32,
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
    /// sound. `placement` says whether it comes from all round or an arc; on
    /// headphones (`FieldEncoding::Ears`) the sound is written as it is to a
    /// one-channel `output`, and the renderer places it.
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
        placement: FieldPlacement,
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
        let mut target_headings = [0.0f32; MAX_CHANNELS];
        let mut target_count = 0;
        // Whether the targets are speakers in known places, round which an
        // arc can be shared.
        let mut placed = encoding == FieldEncoding::Ambisonics;
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
                        let across = speaker.position - placement.receiver;
                        target_channels[target_count] = speaker.channel;
                        target_headings[target_count] = across.y.atan2(across.x);
                        target_count += 1;
                        placed = true;
                    }
                }
                // Defensive fallback for a channel configuration without a
                // positional Speaker entry (for example a minimal headphone
                // host). Without speakers' places an arc has nothing to be
                // shared over, and it is heard all round.
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
                // The renderer places it: on the all-round bus, or through
                // its arc's filters.
                for frame in 0..num_frames {
                    let level = was_heard + glide * frame as f32;
                    output[frame * channels] = source.next_sample(sample_rate) * level;
                }
                return;
            }
        }

        if target_count == 0 {
            return;
        }
        let gain = 1.0 / (target_count as f32).sqrt();
        let arc = placement.arc.filter(|_| placed);
        if arc.is_some() || state.mix.is_some() {
            let all_round = all_round_mix(target_count, gain);
            let wanted = match arc {
                Some(arc) => {
                    let (centre, headings) = if encoding == FieldEncoding::Ambisonics {
                        (arc.centre - placement.listener_yaw, [0.0; MAX_CHANNELS])
                    } else {
                        (arc.centre, target_headings)
                    };
                    state.arc_mix(
                        MixInputs {
                            width: arc.width,
                            centre,
                            headings,
                            outputs: target_count,
                        },
                        encoding,
                    )
                }
                None => all_round,
            };
            let from = match state.mix {
                Some(mix) => mix,
                None if !state.rendered => wanted,
                None => all_round,
            };
            state.mix = arc.map(|_| wanted);
            state.rendered = true;
            let inverse_frames = 1.0 / num_frames.max(1) as f32;
            let mut decorrelated = [0.0f32; MAX_CHANNELS];
            for frame in 0..num_frames {
                let mono = source.next_sample(sample_rate);
                let base = frame * channels;
                let level = was_heard + glide * frame as f32;
                let progress = frame as f32 * inverse_frames;
                for (voice, value) in decorrelated.iter_mut().enumerate().take(target_count) {
                    *value = state.voices[voice].process(mono);
                }
                for (target, (from_row, wanted_row)) in
                    from.iter().zip(&wanted).enumerate().take(target_count)
                {
                    let mut sum = 0.0;
                    for voice in 0..target_count {
                        let start = from_row[voice];
                        sum +=
                            (start + (wanted_row[voice] - start) * progress) * decorrelated[voice];
                    }
                    output[base + target_channels[target]] += sum * level;
                }
            }
            return;
        }
        state.rendered = true;
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
