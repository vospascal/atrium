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
    fn new(sample_rate: f32) -> Self {
        Self {
            filter: Biquad::lowpass(20000.0, sample_rate),
            current_cutoff: 20000.0,
            sample_rate,
        }
    }

    /// Update the filter cutoff based on distance, using ISO 9613-1 physics.
    fn update(&mut self, distance: f32, atmosphere: &AtmosphericParams) {
        let target = iso9613_cutoff(distance, atmosphere);

        // Only recalculate filter coefficients if cutoff changed significantly (>5%)
        if (target - self.current_cutoff).abs() / self.current_cutoff > 0.05 {
            self.filter = Biquad::lowpass(target, self.sample_rate);
            self.current_cutoff = target;
        }
    }

    #[inline]
    fn process(&mut self, sample: f32) -> f32 {
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
    let dist_params = distance_model.as_params();
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

        // Update air absorption filter cutoff for this source's distance
        state.air_absorption[src_idx].update(dist_to_listener, atmosphere);

        // Compute target gains for this buffer (once per source)
        // Uses MDAP when spread > 0 in VBAP mode.
        let target = layout.compute_gains_with_spread(
            listener,
            pos,
            source.orientation(),
            &source.directivity(),
            &dist_params,
            source.spread(),
        );

        // LFE target: omnidirectional distance-only at -6dB
        let lfe_target = if let Some(lfe) = lfe_channel {
            let lfe_dist = distance_gain_at_model(
                listener.position,
                pos,
                distance_model.ref_distance,
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
