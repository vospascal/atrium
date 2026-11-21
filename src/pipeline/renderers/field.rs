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
/// Chosen by `Renderer::field_encoding`, never from the channel count here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldEncoding {
    /// Write directly to the active physical/headphone output channels.
    Speaker,
    /// Write decorrelated W/Y/Z/X components to channels 0–3 for the
    /// Ambisonics decode stage. Only valid on a B-format bus
    /// (`AmbisonicBus::BFormat`), which guarantees at least four channels.
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
        }

        if target_count == 0 {
            return;
        }
        let gain = 1.0 / (target_count as f32).sqrt();
        // How much of the field reaches the output glides from the last
        // buffer's share to this one's, so a probe fades it and never cuts.
        let (was_heard, now_heard) = heard;
        let glide = (now_heard - was_heard) / num_frames.max(1) as f32;
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

    // ── Audit 2026-11: hand-derived values ────────────────────────────────

    use atrium_core::types::Vec3;

    /// Unit impulse at frame 0, silence afterwards.
    struct ImpulseSource {
        fired: bool,
    }
    impl SoundSource for ImpulseSource {
        fn next_sample(&mut self, _sample_rate: f32) -> f32 {
            if self.fired {
                0.0
            } else {
                self.fired = true;
                1.0
            }
        }
        fn position(&self) -> Vec3 {
            Vec3::ZERO
        }
        fn tick(&mut self, _dt: f32) {}
    }

    fn render_impulse(
        encoding: FieldEncoding,
        layout: &SpeakerLayout,
        channels: usize,
        frames: usize,
    ) -> Vec<f32> {
        let mut renderer = FieldRenderer::default();
        renderer.ensure_topology(1, 48000.0);
        let mut source = ImpulseSource { fired: false };
        let mut output = vec![0.0f32; frames * channels];
        renderer.render_source(
            0,
            &mut source,
            encoding,
            layout,
            48000.0,
            channels,
            channels,
            frames,
            (1.0, 1.0),
            &mut output,
        );
        output
    }

    fn channel_energy(output: &[f32], channels: usize, channel: usize) -> f32 {
        output
            .chunks_exact(channels)
            .map(|frame| frame[channel] * frame[channel])
            .sum()
    }

    #[test]
    fn allpass_impulse_response_is_hand_derived_and_lossless() {
        // H(z) = (−g + z^−M)/(1 − g·z^−M): h[0] = −g, h[M] = 1 − g²,
        // h[2M] = g(1 − g²), … and Σh² = g² + (1 − g²)²/(1 − g²) = 1.
        let gain = 0.61;
        let delay = 5;
        let mut allpass = AllPass::new(delay, gain);
        let response: Vec<f32> = (0..2000)
            .map(|n| allpass.process(if n == 0 { 1.0 } else { 0.0 }))
            .collect();
        assert!((response[0] + gain).abs() < 1e-7);
        assert!((response[delay] - (1.0 - gain * gain)).abs() < 1e-6);
        assert!((response[2 * delay] - gain * (1.0 - gain * gain)).abs() < 1e-6);
        for (index, value) in response.iter().enumerate() {
            if index % delay != 0 {
                assert_eq!(*value, 0.0, "only multiples of M carry energy");
            }
        }
        let energy: f32 = response.iter().map(|value| value * value).sum();
        assert!((energy - 1.0).abs() < 1e-5, "all-pass energy {energy}");
    }

    #[test]
    fn voice_delays_round_to_expected_samples_at_48k() {
        // round(ms × 48): 7.1 → 341, 8.9 → 427, 11.3 → 542, 13.7 → 658,
        // 17.9 → 859, 21.1 → 1013, 25.7 → 1234, 31.1 → 1493.
        let state = FieldSourceState::new(48000.0);
        let lengths: Vec<usize> = state
            .voices
            .iter()
            .map(|voice| voice.buffer.len())
            .collect();
        assert_eq!(lengths, vec![341, 427, 542, 658, 859, 1013, 1234, 1493]);
        for (index, voice) in state.voices.iter().enumerate() {
            let expected = if index % 2 == 0 { 0.61 } else { -0.57 };
            assert_eq!(voice.feedback, expected);
        }
    }

    #[test]
    fn ambisonics_field_puts_one_quarter_power_in_each_of_w_y_z_x() {
        // Four voices at gain 1/√4 = ½: each channel's impulse response is
        // ½·h_allpass, so its energy is ¼ and the four sum to 1. First samples
        // are ½·(−g): W = −0.305, Y = +0.285, Z = −0.305, X = +0.285.
        // Note: a diffuse field encoded with this engine's foa_encode would have
        // E[W²] = ½ and E[Y²] = E[X²] = a²/3 = ⅙ per unit power; and every
        // production decoder drops Z, so ¼ of the field power never reaches a
        // speaker (−1.25 dB).
        let layout = SpeakerLayout::new(&[], None, 6);
        let channels = 6;
        let output = render_impulse(FieldEncoding::Ambisonics, &layout, channels, 40_000);
        for (channel, first) in [(0, -0.305), (1, 0.285), (2, -0.305), (3, 0.285)] {
            assert!(
                (output[channel] - first).abs() < 1e-6,
                "ch{channel} first sample"
            );
            let energy = channel_energy(&output, channels, channel);
            assert!((energy - 0.25).abs() < 1e-4, "ch{channel} energy {energy}");
        }
        for channel in 4..channels {
            assert_eq!(channel_energy(&output, channels, channel), 0.0);
        }
    }

    #[test]
    fn speaker_field_skips_lfe_and_splits_power_equally_over_5_speakers() {
        // 5.1: five spatial speakers, LFE (3) excluded; gain 1/√5 → each speaker
        // carries ⅕ of the power, and the total is the source power.
        let layout = SpeakerLayout::surround_5_1(
            Vec3::new(-1.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
        );
        let channels = 6;
        let output = render_impulse(FieldEncoding::Speaker, &layout, channels, 40_000);
        let mut total = 0.0;
        for channel in [0, 1, 2, 4, 5] {
            let energy = channel_energy(&output, channels, channel);
            assert!((energy - 0.2).abs() < 1e-4, "ch{channel} energy {energy}");
            total += energy;
        }
        assert_eq!(
            channel_energy(&output, channels, 3),
            0.0,
            "LFE stays silent"
        );
        assert!((total - 1.0).abs() < 5e-4, "total field power {total}");
    }
}
