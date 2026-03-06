use crate::audio::atmosphere::{iso9613_cutoff, AtmosphericParams};
use crate::spatial::panner::distance_gain_at_model;
use crate::spatial::source::SoundSource;
use atrium_core::listener::Listener;
use atrium_core::panner::DistanceModelType;
use atrium_core::speaker::{DistanceParams, SpeakerLayout, MAX_CHANNELS};

/// Distance model parameters for attenuation.
pub struct DistanceModel {
    pub ref_distance: f32,
    pub max_distance: f32,
    pub rolloff: f32,
    pub model: DistanceModelType,
}

impl Default for DistanceModel {
    fn default() -> Self {
        Self {
            ref_distance: 0.3,
            max_distance: 20.0,
            rolloff: 1.0,
            model: DistanceModelType::Inverse,
        }
    }
}

impl DistanceModel {
    /// Convert to core DistanceParams for the speaker gain computation.
    pub fn as_params(&self) -> DistanceParams {
        DistanceParams {
            ref_distance: self.ref_distance,
            max_distance: self.max_distance,
            rolloff: self.rolloff,
            model: self.model,
        }
    }
}

/// 2nd-order IIR biquad filter (Direct Form I).
/// Used for LFE low-pass crossover.
pub struct Biquad {
    b0: f32, b1: f32, b2: f32,
    a1: f32, a2: f32,
    x1: f32, x2: f32,
    y1: f32, y2: f32,
}

impl Biquad {
    /// Create a 2nd-order Butterworth low-pass filter.
    pub fn lowpass(cutoff_hz: f32, sample_rate: f32) -> Self {
        let omega = 2.0 * std::f32::consts::PI * cutoff_hz / sample_rate;
        let cos_w = omega.cos();
        let sin_w = omega.sin();
        let alpha = sin_w / (2.0 * std::f32::consts::FRAC_1_SQRT_2); // Q = 1/√2 (Butterworth)

        let b0 = (1.0 - cos_w) / 2.0;
        let b1 = 1.0 - cos_w;
        let b2 = b0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w;
        let a2 = 1.0 - alpha;

        Self {
            b0: b0 / a0, b1: b1 / a0, b2: b2 / a0,
            a1: a1 / a0, a2: a2 / a0,
            x1: 0.0, x2: 0.0,
            y1: 0.0, y2: 0.0,
        }
    }

    /// Process a single sample through the filter.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
              - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Per-channel delay lines for speaker distance compensation.
/// Nearer speakers get delayed so all channels arrive at the listener simultaneously.
pub struct DelayCompensation {
    /// Per-channel circular buffers. Outer len = num channels.
    buffers: Vec<Vec<f32>>,
    /// Write position per channel (wraps around buffer length).
    write_pos: Vec<usize>,
    /// Per-channel delay in fractional samples.
    delays: Vec<f32>,
    /// Buffer capacity (samples). Must be power of 2 for fast wrapping.
    capacity: usize,
}

impl DelayCompensation {
    /// Create delay compensation for `num_channels` channels.
    /// Buffer size of 1024 covers ~23ms at 44.1kHz (rooms up to ~8m diameter).
    pub fn new(num_channels: usize) -> Self {
        let capacity = 1024;
        Self {
            buffers: vec![vec![0.0; capacity]; num_channels],
            write_pos: vec![0; num_channels],
            delays: vec![0.0; num_channels],
            capacity,
        }
    }

    /// Recompute delays based on speaker distances from listener.
    /// Nearer speakers get MORE delay (aligned to the farthest speaker).
    pub fn update_delays(&mut self, layout: &SpeakerLayout, listener: &Listener, sample_rate: f32) {
        const SPEED_OF_SOUND: f32 = 343.0; // m/s

        // Find max speaker distance
        let mut d_max = 0.0f32;
        for i in 0..layout.speaker_count() {
            if let Some(speaker) = layout.speaker_by_index(i) {
                let d = (speaker.position - listener.position).length();
                d_max = d_max.max(d);
            }
        }

        // Compute delay per channel: (d_max - d_ch) / speed_of_sound * sample_rate
        for i in 0..layout.speaker_count() {
            if let Some(speaker) = layout.speaker_by_index(i) {
                let d = (speaker.position - listener.position).length();
                let delay_seconds = (d_max - d) / SPEED_OF_SOUND;
                let delay_samples = delay_seconds * sample_rate;
                if speaker.channel < self.delays.len() {
                    self.delays[speaker.channel] = delay_samples;
                }
            }
        }
    }

    /// Process one sample for a given channel: write to buffer, read delayed.
    /// Uses linear interpolation for fractional-sample accuracy.
    #[inline]
    pub fn process(&mut self, ch: usize, input: f32) -> f32 {
        let cap = self.capacity;
        let mask = cap - 1; // capacity is power of 2

        // Write input to circular buffer
        let wp = self.write_pos[ch];
        self.buffers[ch][wp] = input;
        self.write_pos[ch] = (wp + 1) & mask;

        // Read with fractional delay
        let delay = self.delays[ch];
        if delay < 0.5 {
            return input; // no meaningful delay
        }
        let delay_int = delay as usize;
        let frac = delay - delay_int as f32;

        // Read positions (backwards from write position)
        let idx0 = (wp + cap - delay_int) & mask;
        let idx1 = (wp + cap - delay_int - 1) & mask;

        // Linear interpolation between two samples
        let s0 = self.buffers[ch][idx0];
        let s1 = self.buffers[ch][idx1];
        s0 + (s1 - s0) * frac
    }
}

/// Per-source air absorption filter (ISO 9613-1).
///
/// Models frequency-dependent atmospheric absorption: air absorbs high frequencies
/// more than lows. The filter cutoff is derived from the ISO 9613-1 standard based
/// on temperature, humidity, pressure, and distance.
pub struct AirAbsorption {
    filter: Biquad,
    /// Current cutoff to avoid recalculating when distance hasn't changed much.
    current_cutoff: f32,
    sample_rate: f32,
}

impl AirAbsorption {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            filter: Biquad::lowpass(20000.0, sample_rate),
            current_cutoff: 20000.0,
            sample_rate,
        }
    }

    /// Update the filter cutoff based on distance, using ISO 9613-1 physics.
    pub fn update(&mut self, distance: f32, atmosphere: &AtmosphericParams) {
        let target = iso9613_cutoff(distance, atmosphere);

        // Only recalculate filter coefficients if cutoff changed significantly (>5%)
        if (target - self.current_cutoff).abs() / self.current_cutoff > 0.05 {
            self.filter = Biquad::lowpass(target, self.sample_rate);
            self.current_cutoff = target;
        }
    }

    #[inline]
    pub fn process(&mut self, sample: f32) -> f32 {
        self.filter.process(sample)
    }
}

/// Persistent state for the mixer. Lives on the audio thread across callbacks.
/// Holds per-source previous gains for smooth interpolation and LFE filter.
pub struct MixerState {
    /// Previous per-channel gains for each source. Indexed [source_idx][channel].
    prev_gains: Vec<[f32; MAX_CHANNELS]>,
    /// Low-pass filter for LFE channel (~120Hz Butterworth).
    /// None until sample_rate is known.
    lfe_filter: Option<Biquad>,
    /// Per-speaker delay compensation for time-of-arrival alignment.
    delay_comp: DelayCompensation,
    /// Per-source air absorption filters (high freq drops with distance).
    air_absorption: Vec<AirAbsorption>,
}

impl MixerState {
    pub fn new(num_sources: usize) -> Self {
        Self {
            prev_gains: vec![[0.0; MAX_CHANNELS]; num_sources],
            lfe_filter: None,
            delay_comp: DelayCompensation::new(MAX_CHANNELS),
            air_absorption: Vec::new(), // initialized once sample_rate is known
        }
    }

    /// Initialize the LFE low-pass filter once sample rate is known.
    pub fn init_lfe_filter(&mut self, sample_rate: f32) {
        self.lfe_filter = Some(Biquad::lowpass(120.0, sample_rate));
    }

    /// Initialize air absorption filters once sample rate is known.
    fn ensure_air_absorption(&mut self, num_sources: usize, sample_rate: f32) {
        while self.air_absorption.len() < num_sources {
            self.air_absorption.push(AirAbsorption::new(sample_rate));
        }
    }

    /// Grow the gains vector if new sources were added.
    fn ensure_capacity(&mut self, num_sources: usize) {
        while self.prev_gains.len() < num_sources {
            self.prev_gains.push([0.0; MAX_CHANNELS]);
        }
    }
}

/// Mix all active sources into an interleaved multichannel output buffer.
///
/// Loop order: sources outer, frames inner. This lets us compute target gains
/// once per source per buffer and ramp smoothly from prev → target per sample.
pub fn mix_sources(
    sources: &mut [Box<dyn SoundSource>],
    listener: &Listener,
    output: &mut [f32],
    channels: usize,
    sample_rate: f32,
    master_gain: f32,
    distance_model: &DistanceModel,
    layout: &SpeakerLayout,
    state: &mut MixerState,
    atmosphere: &AtmosphericParams,
) {
    let num_frames = output.len() / channels;
    let base_dist_params = distance_model.as_params();
    let lfe_channel = layout.lfe_channel();
    let inv_frames = 1.0 / num_frames as f32;

    state.ensure_capacity(sources.len());
    state.ensure_air_absorption(sources.len(), sample_rate);

    // Zero output buffer (sources accumulate into it)
    for sample in output.iter_mut() {
        *sample = 0.0;
    }

    for (src_idx, source) in sources.iter_mut().enumerate() {
        if !source.is_active() {
            continue;
        }

        let pos = source.position();
        let dist_to_listener = listener.position.distance_to(pos);

        // Per-source distance params: override ref_distance from source's SPL-derived value
        let src_ref_dist = source.ref_distance();
        let src_dist_params = DistanceParams {
            ref_distance: src_ref_dist,
            ..base_dist_params
        };

        // Update air absorption filter cutoff for this source's distance
        state.air_absorption[src_idx].update(dist_to_listener, atmosphere);

        // Compute target gains for this buffer (once per source)
        // Uses MDAP when spread > 0 in VBAP mode.
        let target = layout.compute_gains_with_spread(
            listener,
            pos,
            source.orientation(),
            &source.directivity(),
            &src_dist_params,
            source.spread(),
        );

        // LFE target: omnidirectional distance-only at -6dB
        let lfe_target = if let Some(lfe) = lfe_channel {
            let lfe_dist = distance_gain_at_model(
                listener.position,
                pos,
                src_ref_dist,
                distance_model.max_distance,
                distance_model.rolloff,
                distance_model.model,
            );
            (lfe, lfe_dist * 0.5)
        } else {
            (0, 0.0)
        };

        let prev = &state.prev_gains[src_idx];

        // Ramp gains per sample: linear interpolation from prev → target
        for frame in 0..num_frames {
            let t = frame as f32 * inv_frames;
            let raw_mono = source.next_sample(sample_rate);
            // Apply air absorption: high frequencies attenuated with distance
            let mono = state.air_absorption[src_idx].process(raw_mono);
            let base = frame * channels;

            for ch in 0..channels {
                let gain = prev[ch] + (target.gains[ch] - prev[ch]) * t;
                output[base + ch] += mono * gain;
            }

            // LFE: smooth ramp too
            if lfe_channel.is_some() {
                let (lfe, lfe_tgt) = lfe_target;
                let lfe_prev = prev[lfe];
                // LFE prev already includes the spatial gain + LFE contribution.
                // We ramp only the LFE-specific part. Since spatial gains are
                // already ramped above, we add the LFE delta here.
                let lfe_gain = lfe_prev + (lfe_tgt - lfe_prev) * t;
                output[base + lfe] += mono * lfe_gain;
            }
        }

        // Store target as prev for next buffer
        let mut new_prev = target.gains;
        if let Some(lfe) = lfe_channel {
            new_prev[lfe] = lfe_target.1;
        }
        state.prev_gains[src_idx] = new_prev;
    }

    // Initialize LFE filter on first call (sample_rate now known)
    if state.lfe_filter.is_none() && lfe_channel.is_some() {
        state.init_lfe_filter(sample_rate);
    }

    // Apply LFE low-pass crossover filter (~120Hz Butterworth)
    if let (Some(lfe), Some(ref mut filter)) = (lfe_channel, &mut state.lfe_filter) {
        for frame in 0..num_frames {
            let idx = frame * channels + lfe;
            output[idx] = filter.process(output[idx]);
        }
    }

    // Update speaker delay compensation (recalculates when listener moves)
    state.delay_comp.update_delays(layout, listener, sample_rate);

    // Apply per-speaker delay compensation (time-of-arrival alignment)
    for frame in 0..num_frames {
        let base = frame * channels;
        for ch in 0..channels {
            output[base + ch] = state.delay_comp.process(ch, output[base + ch]);
        }
    }

    // Final pass: apply master gain and clamp
    for sample in output.iter_mut() {
        *sample = (*sample * master_gain).clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::binaural::BinauralMixer;
    use crate::audio::decode::AudioBuffer;
    use crate::spatial::sound_profile::SoundProfile;
    use crate::spatial::source::TestNode;
    use crate::world::types::Vec3;
    use atrium_core::listener::Listener;
    use atrium_core::speaker::{RenderMode, SpeakerLayout};
    use std::sync::Arc;

    const SR: f32 = 48000.0;
    const FRAMES: usize = 1024;

    // ── Helpers ─────────────────────────────────────────────────────────────

    /// 1 kHz sine tone, 1 second. Passes air absorption and isn't LFE-filtered.
    fn sine_buffer() -> Arc<AudioBuffer> {
        let n = SR as usize;
        let samples: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / SR).sin())
            .collect();
        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt();
        Arc::new(AudioBuffer { samples, sample_rate: SR as u32, rms })
    }

    fn make_source(buf: &Arc<AudioBuffer>, spl: f32, pos: Vec3, ceiling: f32) -> Box<dyn SoundSource> {
        let profile = SoundProfile { reference_spl: spl };
        let amplitude = profile.amplitude(buf.rms, 0.1, ceiling);
        let mut node = TestNode::new(Arc::clone(buf), pos, 0.0, 0.0);
        node.amplitude = amplitude;
        Box::new(node)
    }

    /// Listener with omnidirectional hearing (no cone attenuation).
    fn omni_listener(pos: Vec3, yaw: f32) -> Listener {
        let mut l = Listener::new(pos, yaw);
        l.hearing_cone.pattern = atrium_core::directivity::DirectivityPattern::Omni;
        l
    }

    fn default_distance() -> DistanceModel {
        DistanceModel { ref_distance: 1.0, max_distance: 100.0, rolloff: 1.0, model: DistanceModelType::Inverse }
    }

    fn to_db(linear: f32) -> f32 {
        20.0 * linear.max(1e-10).log10()
    }

    // ── Layout factories ────────────────────────────────────────────────────

    /// Listener at origin facing +x. Speakers placed symmetrically.
    /// In this frame: +x = forward, +y = left, -y = right.

    fn layout_stereo() -> SpeakerLayout {
        let mut l = SpeakerLayout::stereo(
            Vec3::new(-10.0, 10.0, 0.0),
            Vec3::new(10.0, 10.0, 0.0),
        );
        l.mode = RenderMode::Stereo;
        l
    }

    fn layout_vbap_5_1() -> SpeakerLayout {
        // Listener at origin facing +x. +x = forward, +y = left.
        // ITU 5.1: FL(0) FR(1) C(2) LFE(3) RL(4) RR(5)
        let mut l = SpeakerLayout::surround_5_1(
            Vec3::new(10.0, 5.0, 0.0),   // FL — front-left  (~27° left)
            Vec3::new(10.0, -5.0, 0.0),  // FR — front-right (~27° right)
            Vec3::new(10.0, 0.0, 0.0),   // C  — center      (0°)
            Vec3::new(-10.0, 5.0, 0.0),  // RL — rear-left   (~153° left)
            Vec3::new(-10.0, -5.0, 0.0), // RR — rear-right  (~153° right)
        );
        l.mode = RenderMode::Vbap;
        l
    }

    fn layout_quad() -> SpeakerLayout {
        // Listener at origin facing +x.
        let mut l = SpeakerLayout::quad(
            Vec3::new(10.0, 5.0, 0.0),   // FL
            Vec3::new(10.0, -5.0, 0.0),  // FR
            Vec3::new(-10.0, 5.0, 0.0),  // RL
            Vec3::new(-10.0, -5.0, 0.0), // RR
        );
        l.mode = RenderMode::Quad;
        l
    }

    fn layout_mono() -> SpeakerLayout {
        let mut l = SpeakerLayout::stereo(
            Vec3::new(-10.0, 10.0, 0.0),
            Vec3::new(10.0, 10.0, 0.0),
        );
        l.mode = RenderMode::Mono;
        l
    }

    // ── Generic render helper (any channel count) ───────────────────────────

    /// Render two buffers and return the stable second one.
    fn render(
        sources: &mut [Box<dyn SoundSource>],
        listener: &Listener,
        layout: &SpeakerLayout,
        dist: &DistanceModel,
    ) -> Vec<f32> {
        let ch = layout.total_channels();
        let atmo = AtmosphericParams::default();
        let mut state = MixerState::new(sources.len());
        let mut out = vec![0.0; FRAMES * ch];
        // First buffer: ramp from 0 → target (discard)
        mix_sources(sources, listener, &mut out, ch, SR, 1.0, dist, layout, &mut state, &atmo);
        // Second buffer: stable gains
        out.fill(0.0);
        mix_sources(sources, listener, &mut out, ch, SR, 1.0, dist, layout, &mut state, &atmo);
        out
    }

    /// RMS of one channel from interleaved multichannel buffer.
    fn ch_rms(buf: &[f32], channels: usize, ch: usize) -> f32 {
        let sum: f32 = buf.iter().skip(ch).step_by(channels).map(|s| s * s).sum();
        let n = buf.len() / channels;
        (sum / n as f32).sqrt()
    }

    /// Total RMS across all channels.
    fn total_rms(buf: &[f32], channels: usize) -> f32 {
        (0..channels).map(|ch| ch_rms(buf, channels, ch)).sum()
    }

    // ── Binaural render helper ──────────────────────────────────────────────

    fn render_binaural(
        sources: &mut [Box<dyn SoundSource>],
        listener: &Listener,
        dist: &DistanceModel,
    ) -> Vec<f32> {
        let ch = 2;
        let atmo = AtmosphericParams::default();
        let mut mixer = BinauralMixer::new("assets/hrtf/default.sofa", SR, sources.len())
            .expect("failed to load SOFA file");
        let mut out = vec![0.0; FRAMES * ch];
        // Run several passes: gain ramp settles + HRTF convolver fully transitions
        // (HRTF updates every 4 calls, FFT overlap needs a few blocks to flush)
        for _ in 0..8 {
            out.fill(0.0);
            mixer.mix(sources, listener, &mut out, ch, SR, 1.0, dist, &atmo);
        }
        out
    }

    // ── Unit tests: SoundProfile amplitude ──────────────────────────────────

    #[test]
    fn amplitude_at_ceiling_equals_target_rms() {
        let buf = sine_buffer();
        let amp = SoundProfile { reference_spl: 100.0 }.amplitude(buf.rms, 0.1, 100.0);
        let expected = 0.1 / buf.rms;
        assert!((amp - expected).abs() < 1e-6, "expected {expected}, got {amp}");
    }

    #[test]
    fn amplitude_scales_by_spl_difference() {
        let buf = sine_buffer();
        let loud = SoundProfile { reference_spl: 100.0 }.amplitude(buf.rms, 0.1, 100.0);
        let quiet = SoundProfile { reference_spl: 45.0 }.amplitude(buf.rms, 0.1, 100.0);
        let db_diff = to_db(quiet / loud);
        assert!((db_diff - (-55.0)).abs() < 0.1, "expected -55 dB, got {db_diff:.1} dB");
    }

    #[test]
    fn amplitude_independent_of_other_sources() {
        let buf = sine_buffer();
        let a = SoundProfile { reference_spl: 45.0 }.amplitude(buf.rms, 0.1, 100.0);
        let b = SoundProfile { reference_spl: 45.0 }.amplitude(buf.rms, 0.1, 100.0);
        assert!((a - b).abs() < 1e-10);
    }

    #[test]
    fn lowering_ceiling_boosts_quiet_sources() {
        let buf = sine_buffer();
        let at_100 = SoundProfile { reference_spl: 45.0 }.amplitude(buf.rms, 0.1, 100.0);
        let at_80 = SoundProfile { reference_spl: 45.0 }.amplitude(buf.rms, 0.1, 80.0);
        let db_diff = to_db(at_80 / at_100);
        assert!((db_diff - 20.0).abs() < 0.1, "expected +20 dB, got {db_diff:.1} dB");
    }

    #[test]
    fn sources_above_ceiling_are_capped() {
        // Djembe (100 dB) with ceiling at 60 should be capped to same gain
        // as a 60 dB source (both get spl_gain = 1.0)
        let buf = sine_buffer();
        let rms_correction = 0.1 / buf.rms;
        let loud = SoundProfile { reference_spl: 100.0 }.amplitude(buf.rms, 0.1, 60.0);
        let at_ceiling = SoundProfile { reference_spl: 60.0 }.amplitude(buf.rms, 0.1, 60.0);
        // Both should equal rms_correction (spl_gain capped at 1.0)
        assert!((loud - rms_correction).abs() < 1e-6, "100 dB should be capped: {loud}");
        assert!((at_ceiling - rms_correction).abs() < 1e-6, "60 dB at ceiling: {at_ceiling}");
        // Quiet source below ceiling still scales down
        let quiet = SoundProfile { reference_spl: 45.0 }.amplitude(buf.rms, 0.1, 60.0);
        assert!(quiet < loud, "45 dB should be quieter than capped 100 dB");
        let expected_db = -15.0; // 45 - 60
        let actual_db = to_db(quiet / loud);
        assert!((actual_db - expected_db).abs() < 0.5, "expected {expected_db} dB, got {actual_db:.1} dB");
    }

    // ── Unit tests: SoundProfile ref_distance ────────────────────────────

    #[test]
    fn ref_distance_scales_with_spl() {
        let djembe = SoundProfile { reference_spl: 100.0 }.ref_distance(1.0, 40.0);
        let campfire = SoundProfile { reference_spl: 45.0 }.ref_distance(1.0, 40.0);
        let cat = SoundProfile { reference_spl: 25.0 }.ref_distance(1.0, 40.0);
        // Louder sources should have larger ref_distance
        assert!(djembe > campfire, "djembe should project further than campfire");
        assert!(campfire > cat, "campfire should project further than cat");
        // Formula: global_ref * (spl / spl_reference)
        assert!((djembe - 2.5).abs() < 0.01, "djembe ref_dist: {djembe}");
        assert!((campfire - 1.125).abs() < 0.01, "campfire ref_dist: {campfire}");
        assert!((cat - 0.625).abs() < 0.01, "cat ref_dist: {cat}");
    }

    #[test]
    fn ref_distance_scales_with_global_ref() {
        let a = SoundProfile { reference_spl: 60.0 }.ref_distance(1.0, 60.0);
        let b = SoundProfile { reference_spl: 60.0 }.ref_distance(0.5, 60.0);
        assert!((a - 1.0).abs() < 1e-6, "at spl=reference, should equal global_ref");
        assert!((b - 0.5).abs() < 1e-6, "should scale with global_ref");
    }

    // ════════════════════════════════════════════════════════════════════════
    // STEREO
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn stereo_distance_6db_per_doubling() {
        let buf = sine_buffer();
        let layout = layout_stereo();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);

        let mut s1 = vec![make_source(&buf, 100.0, Vec3::new(1.0, 0.0, 0.0), 100.0)];
        let mut s2 = vec![make_source(&buf, 100.0, Vec3::new(2.0, 0.0, 0.0), 100.0)];
        let r1 = total_rms(&render(&mut s1, &listener, &layout, &dist), 2);
        let r2 = total_rms(&render(&mut s2, &listener, &layout, &dist), 2);
        let db = to_db(r2 / r1);
        assert!((db - (-6.0)).abs() < 1.0, "stereo: expected -6 dB, got {db:.1}");
    }

    #[test]
    fn stereo_spl_difference_preserved() {
        let buf = sine_buffer();
        let layout = layout_stereo();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);
        let pos = Vec3::new(2.0, 0.0, 0.0);

        let mut loud = vec![make_source(&buf, 100.0, pos, 100.0)];
        let mut quiet = vec![make_source(&buf, 45.0, pos, 100.0)];
        let rl = ch_rms(&render(&mut loud, &listener, &layout, &dist), 2, 0);
        let rq = ch_rms(&render(&mut quiet, &listener, &layout, &dist), 2, 0);
        let db = to_db(rq / rl);
        assert!((db - (-55.0)).abs() < 1.5, "stereo: expected -55 dB, got {db:.1}");
    }

    #[test]
    fn stereo_left_right_panning() {
        let buf = sine_buffer();
        let layout = layout_stereo();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);

        // Source left (+y)
        let mut sl = vec![make_source(&buf, 100.0, Vec3::new(1.0, 2.0, 0.0), 100.0)];
        let ol = render(&mut sl, &listener, &layout, &dist);
        assert!(ch_rms(&ol, 2, 0) > ch_rms(&ol, 2, 1) * 2.0, "stereo: left source should be louder in L");

        // Source right (-y)
        let mut sr = vec![make_source(&buf, 100.0, Vec3::new(1.0, -2.0, 0.0), 100.0)];
        let or = render(&mut sr, &listener, &layout, &dist);
        assert!(ch_rms(&or, 2, 1) > ch_rms(&or, 2, 0) * 2.0, "stereo: right source should be louder in R");
    }

    #[test]
    fn stereo_center_equal() {
        let buf = sine_buffer();
        let layout = layout_stereo();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);

        let mut s = vec![make_source(&buf, 100.0, Vec3::new(2.0, 0.0, 0.0), 100.0)];
        let out = render(&mut s, &listener, &layout, &dist);
        let diff = to_db(ch_rms(&out, 2, 0) / ch_rms(&out, 2, 1)).abs();
        assert!(diff < 1.0, "stereo: center L/R diff {diff:.1} dB");
    }

    // ════════════════════════════════════════════════════════════════════════
    // VBAP 5.1
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn vbap_5_1_distance_6db() {
        let buf = sine_buffer();
        let layout = layout_vbap_5_1();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);

        let mut s1 = vec![make_source(&buf, 100.0, Vec3::new(1.0, 0.0, 0.0), 100.0)];
        let mut s2 = vec![make_source(&buf, 100.0, Vec3::new(2.0, 0.0, 0.0), 100.0)];
        let r1 = total_rms(&render(&mut s1, &listener, &layout, &dist), 6);
        let r2 = total_rms(&render(&mut s2, &listener, &layout, &dist), 6);
        let db = to_db(r2 / r1);
        assert!((db - (-6.0)).abs() < 1.5, "5.1: expected -6 dB, got {db:.1}");
    }

    #[test]
    fn vbap_5_1_spl_difference() {
        let buf = sine_buffer();
        let layout = layout_vbap_5_1();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);
        let pos = Vec3::new(2.0, 0.0, 0.0);

        let mut loud = vec![make_source(&buf, 100.0, pos, 100.0)];
        let mut quiet = vec![make_source(&buf, 45.0, pos, 100.0)];
        let rl = total_rms(&render(&mut loud, &listener, &layout, &dist), 6);
        let rq = total_rms(&render(&mut quiet, &listener, &layout, &dist), 6);
        let db = to_db(rq / rl);
        assert!((db - (-55.0)).abs() < 2.0, "5.1: expected -55 dB, got {db:.1}");
    }

    #[test]
    fn vbap_5_1_front_left_panning() {
        let buf = sine_buffer();
        let layout = layout_vbap_5_1();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);
        // Source at front-left: should activate FL(0) more than FR(1) and rears
        let mut s = vec![make_source(&buf, 100.0, Vec3::new(2.0, 3.0, 0.0), 100.0)];
        let out = render(&mut s, &listener, &layout, &dist);
        let fl = ch_rms(&out, 6, 0);
        let fr = ch_rms(&out, 6, 1);
        let rl = ch_rms(&out, 6, 4);
        let rr = ch_rms(&out, 6, 5);
        assert!(fl > fr, "5.1: FL ({fl:.4}) > FR ({fr:.4}) for front-left source");
        assert!(fl > rl, "5.1: FL ({fl:.4}) > RL ({rl:.4}) for front-left source");
        assert!(fl > rr, "5.1: FL ({fl:.4}) > RR ({rr:.4}) for front-left source");
    }

    #[test]
    fn vbap_5_1_rear_right_panning() {
        let buf = sine_buffer();
        let layout = layout_vbap_5_1();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);
        // Source behind and to the right
        let mut s = vec![make_source(&buf, 100.0, Vec3::new(-2.0, -3.0, 0.0), 100.0)];
        let out = render(&mut s, &listener, &layout, &dist);
        let fl = ch_rms(&out, 6, 0);
        let rr = ch_rms(&out, 6, 5);
        assert!(rr > fl, "5.1: RR ({rr:.4}) > FL ({fl:.4}) for rear-right source");
    }

    // ════════════════════════════════════════════════════════════════════════
    // QUAD 4.0
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn quad_distance_6db() {
        let buf = sine_buffer();
        let layout = layout_quad();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);

        let mut s1 = vec![make_source(&buf, 100.0, Vec3::new(1.0, 0.0, 0.0), 100.0)];
        let mut s2 = vec![make_source(&buf, 100.0, Vec3::new(2.0, 0.0, 0.0), 100.0)];
        let r1 = total_rms(&render(&mut s1, &listener, &layout, &dist), 4);
        let r2 = total_rms(&render(&mut s2, &listener, &layout, &dist), 4);
        let db = to_db(r2 / r1);
        assert!((db - (-6.0)).abs() < 1.5, "quad: expected -6 dB, got {db:.1}");
    }

    #[test]
    fn quad_spl_difference() {
        let buf = sine_buffer();
        let layout = layout_quad();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);
        let pos = Vec3::new(2.0, 0.0, 0.0);

        let mut loud = vec![make_source(&buf, 100.0, pos, 100.0)];
        let mut quiet = vec![make_source(&buf, 45.0, pos, 100.0)];
        let rl = total_rms(&render(&mut loud, &listener, &layout, &dist), 4);
        let rq = total_rms(&render(&mut quiet, &listener, &layout, &dist), 4);
        let db = to_db(rq / rl);
        assert!((db - (-55.0)).abs() < 2.0, "quad: expected -55 dB, got {db:.1}");
    }

    #[test]
    fn quad_front_left_panning() {
        let buf = sine_buffer();
        let layout = layout_quad();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);
        // Source front-left
        let mut s = vec![make_source(&buf, 100.0, Vec3::new(2.0, 3.0, 0.0), 100.0)];
        let out = render(&mut s, &listener, &layout, &dist);
        let fl = ch_rms(&out, 4, 0); // ch 0
        let fr = ch_rms(&out, 4, 1); // ch 1
        let rl = ch_rms(&out, 4, 2); // ch 2
        let rr = ch_rms(&out, 4, 3); // ch 3
        assert!(fl > fr, "quad: FL > FR for front-left source");
        assert!(fl > rl, "quad: FL > RL for front-left source");
        assert!(fl > rr, "quad: FL > RR for front-left source");
    }

    #[test]
    fn quad_rear_right_panning() {
        let buf = sine_buffer();
        let layout = layout_quad();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);
        // Source behind-right
        let mut s = vec![make_source(&buf, 100.0, Vec3::new(-2.0, -3.0, 0.0), 100.0)];
        let out = render(&mut s, &listener, &layout, &dist);
        let fl = ch_rms(&out, 4, 0);
        let rr = ch_rms(&out, 4, 3);
        assert!(rr > fl, "quad: RR ({rr:.4}) > FL ({fl:.4}) for rear-right source");
    }

    // ════════════════════════════════════════════════════════════════════════
    // MONO
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn mono_distance_6db() {
        let buf = sine_buffer();
        let layout = layout_mono();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);

        let mut s1 = vec![make_source(&buf, 100.0, Vec3::new(1.0, 0.0, 0.0), 100.0)];
        let mut s2 = vec![make_source(&buf, 100.0, Vec3::new(2.0, 0.0, 0.0), 100.0)];
        let r1 = total_rms(&render(&mut s1, &listener, &layout, &dist), 2);
        let r2 = total_rms(&render(&mut s2, &listener, &layout, &dist), 2);
        let db = to_db(r2 / r1);
        assert!((db - (-6.0)).abs() < 1.5, "mono: expected -6 dB, got {db:.1}");
    }

    #[test]
    fn mono_spl_difference() {
        let buf = sine_buffer();
        let layout = layout_mono();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);
        let pos = Vec3::new(2.0, 0.0, 0.0);

        let mut loud = vec![make_source(&buf, 100.0, pos, 100.0)];
        let mut quiet = vec![make_source(&buf, 45.0, pos, 100.0)];
        let rl = total_rms(&render(&mut loud, &listener, &layout, &dist), 2);
        let rq = total_rms(&render(&mut quiet, &listener, &layout, &dist), 2);
        let db = to_db(rq / rl);
        assert!((db - (-55.0)).abs() < 2.0, "mono: expected -55 dB, got {db:.1}");
    }

    #[test]
    fn mono_equal_channels_regardless_of_position() {
        let buf = sine_buffer();
        let layout = layout_mono();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);
        // Source hard left — mono should still be equal in both channels
        let mut s = vec![make_source(&buf, 100.0, Vec3::new(0.0, 3.0, 0.0), 100.0)];
        let out = render(&mut s, &listener, &layout, &dist);
        let l = ch_rms(&out, 2, 0);
        let r = ch_rms(&out, 2, 1);
        let diff = to_db(l / r).abs();
        assert!(diff < 0.5, "mono: L/R diff should be ~0 dB, got {diff:.1} dB");
    }

    // ════════════════════════════════════════════════════════════════════════
    // BINAURAL (HRTF)
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn binaural_distance_6db() {
        let buf = sine_buffer();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);

        let mut s1 = vec![make_source(&buf, 100.0, Vec3::new(1.0, 0.0, 0.0), 100.0)];
        let mut s2 = vec![make_source(&buf, 100.0, Vec3::new(2.0, 0.0, 0.0), 100.0)];
        let r1 = total_rms(&render_binaural(&mut s1, &listener, &dist), 2);
        let r2 = total_rms(&render_binaural(&mut s2, &listener, &dist), 2);
        let db = to_db(r2 / r1);
        assert!((db - (-6.0)).abs() < 2.0, "binaural: expected -6 dB, got {db:.1}");
    }

    #[test]
    fn binaural_spl_difference() {
        let buf = sine_buffer();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);
        let pos = Vec3::new(2.0, 0.0, 0.0);

        let mut loud = vec![make_source(&buf, 100.0, pos, 100.0)];
        let mut quiet = vec![make_source(&buf, 45.0, pos, 100.0)];
        let rl = total_rms(&render_binaural(&mut loud, &listener, &dist), 2);
        let rq = total_rms(&render_binaural(&mut quiet, &listener, &dist), 2);
        let db = to_db(rq / rl);
        assert!((db - (-55.0)).abs() < 2.0, "binaural: expected -55 dB, got {db:.1}");
    }

    #[test]
    fn binaural_left_source_louder_in_left_ear() {
        let buf = sine_buffer();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);
        // Source to the left (+y = left when facing +x)
        let mut s = vec![make_source(&buf, 100.0, Vec3::new(0.0, 2.0, 0.0), 100.0)];
        let out = render_binaural(&mut s, &listener, &dist);
        let l = ch_rms(&out, 2, 0);
        let r = ch_rms(&out, 2, 1);
        assert!(l > r, "binaural: left ear ({l:.4}) > right ear ({r:.4}) for left source");
    }

    #[test]
    fn binaural_right_source_louder_in_right_ear() {
        let buf = sine_buffer();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);
        // Source to the right (-y)
        let mut s = vec![make_source(&buf, 100.0, Vec3::new(0.0, -2.0, 0.0), 100.0)];
        let out = render_binaural(&mut s, &listener, &dist);
        let l = ch_rms(&out, 2, 0);
        let r = ch_rms(&out, 2, 1);
        assert!(r > l, "binaural: right ear ({r:.4}) > left ear ({l:.4}) for right source");
    }

    #[test]
    fn binaural_center_roughly_equal() {
        let buf = sine_buffer();
        let dist = default_distance();
        let listener = omni_listener(Vec3::ZERO, 0.0);
        // Source directly ahead
        let mut s = vec![make_source(&buf, 100.0, Vec3::new(2.0, 0.0, 0.0), 100.0)];
        let out = render_binaural(&mut s, &listener, &dist);
        let l = ch_rms(&out, 2, 0);
        let r = ch_rms(&out, 2, 1);
        let diff = to_db(l / r).abs();
        // HRTF may not be perfectly symmetric, allow 3 dB
        assert!(diff < 3.0, "binaural: center L/R diff {diff:.1} dB (expected < 3)");
    }
}
