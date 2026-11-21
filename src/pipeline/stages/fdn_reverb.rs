//! FDN (Feedback Delay Network) late reverb.
//!
//! Based on Jot & Chaigne (1991). 16 parallel delay lines coupled through a
//! 16×16 Hadamard mixing matrix, with one-pole lowpass damping per line for
//! frequency-dependent decay (highs fade faster than lows).
//!
//! Chain order: Direct (0–3ms) → EarlyReflections (3–50ms) → FdnReverb (50ms+)

mod config;
mod damping;
mod delay_lines;
mod feedback;

pub use config::FdnConfig;

use crate::audio::filters::Biquad;
use crate::pipeline::mix_stage::{MixContext, MixStage};
use crate::pipeline::room_acoustics;
use crate::pipeline::stages::sanitize_finite;
use config::{HF_DECAY_RATIO, MAX_OUT};
use damping::DampingFilter;
use delay_lines::{
    hermite_coefficients, DelayModulator, BASE_DELAYS, BASE_SAMPLE_RATE, MIN_BUF_SIZE,
    MOD_DEPTH_SAMPLES_48K, MOD_RATES_HZ, NUM_LINES,
};
use feedback::{feedback_soft_clamp, hadamard};

/// Fallback pre-delay when room geometry is degenerate (volume ≈ 0 or surface area ≈ 0).
const PRE_DELAY_FALLBACK_SECONDS: f32 = 0.020;
/// Shortest pre-delay the mean-free-path value is given (tiny rooms).
const PRE_DELAY_MINIMUM_SECONDS: f32 = 0.005;
/// Longest pre-delay, geometry-derived or explicit. The pre-delay ring is sized
/// at init to the pre-delay actually needed, so this only bounds the
/// allocation: the mean-free-path time reaches it at a mean free path of
/// ~86 m, far beyond any room (the 40×25×20 m cathedral is 50.6 ms).
const PRE_DELAY_MAXIMUM_SECONDS: f32 = 0.250;

/// Post-mix FDN late reverb stage.
///
/// Fully physics-based gain staging:
/// - **Send**: d²/(d²+d_c²) per source (bounded reverberant energy fraction)
/// - **Decay**: Sabine RT60 → Jot per-line damping gains, met at 500 Hz and 4 kHz
/// - **Output normalization**: derived from steady-state energy buildup of the
///   16-line feedback network (1/RMS over lines of the per-line white-noise
///   build-up), so the d/d_c send law directly controls perceived wet level
///   without an arbitrary wet knob.
///
/// High-frequency decay is derived from per-wall material absorption
/// coefficients via Sabine's equation at 500 Hz (low) and 4 kHz (high), and
/// each line's damping filter is solved to hit both at those frequencies, so
/// rooms with absorptive surfaces (carpet, acoustic tile) naturally produce
/// faster HF decay than hard-walled rooms.
pub struct FdnReverbStage {
    /// Delay line buffers — dynamically sized to accommodate delays at any
    /// sample rate. At 48 kHz the buffer is 4096 samples; at 96 kHz it grows
    /// to 8192 to preserve the coprime delay ratios.
    delay_buffers: Vec<Vec<f32>>,
    /// Current buffer size (always a power of 2).
    buffer_size: usize,
    /// Bitmask for wrapping write/read positions: `buffer_size - 1`.
    buffer_mask: usize,
    write_pos: usize,
    delays: [usize; NUM_LINES],
    damping: [DampingFilter; NUM_LINES],
    /// Pre-delay ring, sized at init to the next power of two above
    /// `pre_delay_samples` (see `set_pre_delay`).
    pre_delay_buffer: Vec<f32>,
    /// Bitmask for wrapping the pre-delay ring: `pre_delay_buffer.len() - 1`.
    pre_delay_mask: usize,
    pre_delay_write_pos: usize,
    pre_delay_samples: usize,
    /// Structural output normalization derived from the damping filters'
    /// steady-state energy build-up.
    /// Compensates for the FDN's steady-state energy accumulation so that
    /// the d/d_c send law directly controls the perceived wet level.
    output_normalization: f32,
    /// Per-line delay modulators. Slowly vary each line's read position
    /// to smear resonant modes, preventing metallic ringing.
    modulators: [DelayModulator; NUM_LINES],
    initialized: bool,
    /// False when the environment cannot sustain a reverberant field (no pair
    /// of facing reflective surfaces) and no RT60 was set explicitly. The
    /// stage then leaves the buffer untouched: an open field has no tail.
    reverberant: bool,
    measurement_mode: bool,
    // ── Configuration (resolved from the environment at build time) ──
    /// Explicit mid-band RT60 override (seconds); `None` = geometry-derived.
    rt60_override: Option<f32>,
    /// Explicit pre-delay override (milliseconds); `None` = mean-free-path.
    pre_delay_override_ms: Option<f32>,
    /// Per-channel wet trim (canonical 5.1 role order).
    channel_gains: [f32; MAX_OUT],
    /// Reverb-send high-pass cutoff (Hz); `0.0` disables it.
    hpf_cutoff_hz: f32,
    /// Reverb-send high-pass filter, built at init once the sample rate is known.
    send_hpf: Option<Biquad>,
    /// Overall wet-level multiplier for the tail.
    wet: f32,
}

/// Sabine RT60 band indices for FDN damping.
/// Band 2 = 500 Hz (low-frequency reference), band 5 = 4 kHz (HF decay).
const RT60_HIGH_BAND: usize = 5;

impl Default for FdnReverbStage {
    fn default() -> Self {
        Self::new()
    }
}

impl FdnReverbStage {
    /// Neutral reverb: physically-derived decay/pre-delay, uniform channel level,
    /// no send high-pass. Equivalent to `with_config(FdnConfig::default())`.
    pub fn new() -> Self {
        Self::with_config(FdnConfig::default())
    }

    /// Reverb configured from the environment (RT60/pre-delay overrides,
    /// per-channel voicing, reverb-send high-pass).
    pub fn with_config(config: FdnConfig) -> Self {
        Self {
            delay_buffers: vec![vec![0.0; MIN_BUF_SIZE]; NUM_LINES],
            buffer_size: MIN_BUF_SIZE,
            buffer_mask: MIN_BUF_SIZE - 1,
            write_pos: 0,
            delays: [0; NUM_LINES],
            damping: [DampingFilter::new(); NUM_LINES],
            pre_delay_buffer: vec![0.0; 1],
            pre_delay_mask: 0,
            pre_delay_write_pos: 0,
            pre_delay_samples: 0,
            output_normalization: 1.0,
            modulators: [DelayModulator::new(); NUM_LINES],
            initialized: false,
            reverberant: true,
            measurement_mode: false,
            rt60_override: config.rt60_seconds,
            pre_delay_override_ms: config.pre_delay_ms,
            channel_gains: config.channel_gains,
            hpf_cutoff_hz: config.hpf_cutoff_hz,
            send_hpf: None,
            wet: config.wet,
        }
    }

    /// Compute the minimum power-of-2 buffer size needed for the given sample rate.
    ///
    /// The oldest sample the modulated read touches is the longest line plus
    /// the full LFO depth plus the interpolator's two taps past the integer
    /// delay (see `hermite_coefficients`); the slot being written this sample
    /// must not alias it.
    fn required_buffer_size(sample_rate: f32) -> usize {
        let scale = sample_rate / BASE_SAMPLE_RATE;
        let longest_modulated_delay =
            ((BASE_DELAYS[NUM_LINES - 1] as f32 + MOD_DEPTH_SAMPLES_48K) * scale).ceil() as usize;
        let oldest_read = longest_modulated_delay + 2;
        (oldest_read + 1).next_power_of_two().max(MIN_BUF_SIZE)
    }

    fn compute_delays(&mut self, sample_rate: f32) {
        let needed = Self::required_buffer_size(sample_rate);
        if needed != self.buffer_size {
            self.buffer_size = needed;
            self.buffer_mask = needed - 1;
            self.delay_buffers = vec![vec![0.0; needed]; NUM_LINES];
            self.write_pos = 0;
        }
        let scale = sample_rate / BASE_SAMPLE_RATE;
        for (i, &base_delay) in BASE_DELAYS.iter().enumerate() {
            let scaled = ((base_delay as f32) * scale) as usize;
            self.delays[i] = scaled.clamp(1, self.buffer_size - 1);
        }

        // Initialize per-line delay modulators with different starting phases
        // so they don't all begin at the same point in their cycle.
        for (i, (modulator, &rate)) in self
            .modulators
            .iter_mut()
            .zip(MOD_RATES_HZ.iter())
            .enumerate()
        {
            let starting_phase = i as f32 / NUM_LINES as f32;
            modulator.init(rate, sample_rate, starting_phase);
        }
    }

    /// Compute pre-delay from mean free path time (average time between wall
    /// bounces in a diffuse field). Falls back to 20ms for degenerate rooms,
    /// and is held to at least `PRE_DELAY_MINIMUM_SECONDS`.
    ///
    /// Reference: Kuttruff, "Room Acoustics" (5th ed., 2009).
    fn compute_pre_delay(
        &mut self,
        sample_rate: f32,
        volume: f32,
        surface_area: f32,
        speed_of_sound: f32,
    ) {
        let pre_delay_seconds = if volume < 1e-6 || surface_area < 1e-6 {
            PRE_DELAY_FALLBACK_SECONDS
        } else {
            room_acoustics::mean_free_path_time(volume, surface_area, speed_of_sound)
        };
        self.set_pre_delay(
            pre_delay_seconds.max(PRE_DELAY_MINIMUM_SECONDS),
            sample_rate,
        );
    }

    /// Set the pre-delay, bounded to [0, `PRE_DELAY_MAXIMUM_SECONDS`], and size
    /// the ring to hold it at this sample rate (a power of two above the delay,
    /// so the read never aliases the write). Allocates when the size changes;
    /// called from `init` only.
    fn set_pre_delay(&mut self, seconds: f32, sample_rate: f32) {
        let seconds = seconds.clamp(0.0, PRE_DELAY_MAXIMUM_SECONDS);
        self.pre_delay_samples = (seconds * sample_rate) as usize;
        let required_size = (self.pre_delay_samples + 1).next_power_of_two();
        if required_size != self.pre_delay_buffer.len() {
            self.pre_delay_buffer = vec![0.0; required_size];
            self.pre_delay_mask = required_size - 1;
            self.pre_delay_write_pos = 0;
        }
    }

    /// Compute per-line one-pole damping filters for frequency-dependent decay,
    /// and derive structural output normalization from the loop gains.
    ///
    /// Each line's filter meets the Jot per-pass gain of `rt60_low` at 500 Hz
    /// and of `rt60_high` at 4 kHz, the frequencies the two RT60s describe
    /// (Sabine bands 2 and 5, or override and override × `HF_DECAY_RATIO`);
    /// see `DampingFilter::design` for the closed form and its edge cases.
    ///
    /// Output normalization divides out the network's steady-state energy
    /// build-up: per line the white-noise mean of 1/(1 − |H(e^{jω})|²) over
    /// frequency (`DampingFilter::white_noise_energy_buildup`, exact for the
    /// one-pole), RMS-averaged across lines (Hadamard is orthonormal, so lines
    /// don't add coherently).
    ///
    /// Reference: Jot, "Digital Delay Networks for Designing Artificial Reverberators",
    /// AES 90th Convention (1991), §3.2; extended in Jot's PhD thesis (1992), Ch. 4.
    fn compute_damping(&mut self, sample_rate: f32, rt60_low: f32, rt60_high: f32) {
        let mut sum_energy_buildup = 0.0f32;

        for (filter, &delay_samples) in self.damping.iter_mut().zip(self.delays.iter()) {
            *filter = DampingFilter::design(delay_samples, sample_rate, rt60_low, rt60_high);
            sum_energy_buildup += filter.white_noise_energy_buildup();
        }

        // Network-level gain: RMS across lines (Hadamard is orthonormal).
        let rms_gain = (sum_energy_buildup / NUM_LINES as f32).sqrt();
        let raw_norm = 1.0 / rms_gain;
        self.output_normalization = if self.measurement_mode {
            // Measurement mode: preserve linear gain, only prevent NaN/Inf.
            if raw_norm.is_finite() {
                raw_norm
            } else {
                1.0
            }
        } else {
            raw_norm.clamp(0.01, 1.0)
        };
    }

    #[allow(clippy::needless_range_loop)]
    #[inline(always)]
    fn process_fdn_sample(&mut self, mono_in: f32, out_channels: usize) -> [f32; MAX_OUT] {
        let mut output = [0.0f32; MAX_OUT];

        // Read from delay lines with modulated positions, smearing resonant
        // peaks. Cubic Hermite interpolation keeps the swept fractional read
        // nearly flat and never above unity gain (see `hermite_coefficients`).
        let mut taps = [0.0_f32; NUM_LINES];
        for i in 0..NUM_LINES {
            let mod_offset = self.modulators[i].next();
            let modulated_delay = self.delays[i] as f32 + mod_offset;
            // Taps span delays integer − 1 … integer + 2: the newest must be at
            // least 1 (the current slot is written below), the oldest at most
            // buffer_size − 1. `required_buffer_size` makes this a no-op guard.
            let clamped_delay = modulated_delay.clamp(2.0, (self.buffer_size - 3) as f32);
            let delay_integer = clamped_delay as usize;
            let coefficients = hermite_coefficients(clamped_delay - delay_integer as f32);

            let line = &self.delay_buffers[i];
            let newest = self.write_pos + self.buffer_size + 1 - delay_integer;
            taps[i] = coefficients[0] * line[newest & self.buffer_mask]
                + coefficients[1] * line[(newest - 1) & self.buffer_mask]
                + coefficients[2] * line[(newest - 2) & self.buffer_mask]
                + coefficients[3] * line[(newest - 3) & self.buffer_mask];
        }

        for i in 0..NUM_LINES {
            taps[i] = self.damping[i].process(taps[i]);
        }

        // Distribute the 16 delay line taps across output channels for a diffuse field.
        // Each channel accumulates taps via modular assignment (line i → channel i%N).
        // Normalization: divide each channel by √(taps_in_ch × ch_count).
        // This simultaneously:
        //  - Equalizes per-channel level (channels with more taps aren't louder)
        //  - Preserves total energy (total power across all channels = single-tap power)
        // Result: the reverb send level directly controls the perceived wet level.
        let ch_count = out_channels.clamp(1, MAX_OUT);
        let remainder = NUM_LINES % ch_count;
        for i in 0..NUM_LINES {
            output[i % ch_count] += taps[i];
        }
        for ch in 0..ch_count {
            let taps_in_ch = if ch < remainder {
                (NUM_LINES / ch_count + 1) as f32
            } else {
                (NUM_LINES / ch_count) as f32
            };
            output[ch] /= (taps_in_ch * ch_count as f32).sqrt();
        }

        hadamard(&mut taps);

        let input_scale = 1.0 / (NUM_LINES as f32).sqrt();
        let scaled_input = mono_in * input_scale;
        for i in 0..NUM_LINES {
            let v = taps[i] + scaled_input;
            self.delay_buffers[i][self.write_pos] = if self.measurement_mode {
                // Stability ceiling only: prevent runaway without soft clipping.
                v.clamp(-100.0, 100.0)
            } else {
                // Soft clamp: prevents runaway without introducing hard-clip
                // discontinuities that compound through the feedback loop.
                feedback_soft_clamp(v)
            };
        }

        self.write_pos = (self.write_pos + 1) & self.buffer_mask;
        output
    }
}

impl MixStage for FdnReverbStage {
    fn init(&mut self, ctx: &MixContext) {
        self.measurement_mode = ctx.measurement_mode;
        self.compute_delays(ctx.sample_rate);

        // An explicit RT60 is a designer's decision and always wins. Otherwise
        // the geometry decides: without facing reflective surfaces there is no
        // diffuse field for Sabine to describe.
        self.reverberant = self.rt60_override.is_some_and(|rt60| rt60 > 0.0)
            || room_acoustics::has_facing_reflective_surfaces(&ctx.environment.wall_materials);

        // Derive per-band RT60 from room geometry + wall materials via Sabine's equation.
        // RT60 at 500 Hz (band 2) sets the overall decay; RT60 at 4 kHz (band 5)
        // controls how much faster highs decay — driven by actual material absorption.
        let (volume, surface_area) =
            room_acoustics::room_geometry(ctx.environment.min, ctx.environment.max);
        let wall_areas =
            room_acoustics::wall_surface_areas(ctx.environment.min, ctx.environment.max);
        let rt60_low = room_acoustics::reverb_decay_seconds(ctx.environment, self.rt60_override);
        let rt60_high = room_acoustics::sabine_rt60_at_band(
            volume,
            &wall_areas,
            &ctx.environment.wall_materials,
            &ctx.environment.atmosphere,
            RT60_HIGH_BAND,
        );

        // Explicit RT60 override (e.g. "atrium tuned to 3.5 s") bypasses the
        // geometry/material Sabine decay. HF decays a bit faster for a bright
        // but non-sizzly tail. Guard against non-positive values.
        let rt60_high = match self.rt60_override {
            Some(rt60) if rt60 > 0.0 => rt60 * HF_DECAY_RATIO,
            _ => rt60_high,
        };

        // Pre-delay: an explicit override (e.g. 20 ms) wins; otherwise the mean
        // free path, the average time between wall bounces. Either sizes the
        // pre-delay ring for this sample rate.
        match self.pre_delay_override_ms {
            Some(milliseconds) => self.set_pre_delay(milliseconds / 1000.0, ctx.sample_rate),
            None => {
                let speed_of_sound = ctx.environment.atmosphere.speed_of_sound();
                self.compute_pre_delay(ctx.sample_rate, volume, surface_area, speed_of_sound);
            }
        }

        // Build the reverb-send high-pass now that the sample rate is known.
        self.send_hpf = (self.hpf_cutoff_hz > 0.0)
            .then(|| Biquad::highpass(self.hpf_cutoff_hz, ctx.sample_rate));

        self.compute_damping(ctx.sample_rate, rt60_low, rt60_high);
        self.initialized = true;
    }

    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        if !self.initialized || !self.reverberant {
            return;
        }

        let channels = ctx.channels;
        let render_channels = ctx.render_channels;
        let num_frames = buffer.len() / channels;

        for frame in 0..num_frames {
            let base = frame * channels;

            // Read mono input from the reverb send buffer (pre-weighted by d/d_c)
            // if available, otherwise fall back to mono-summing the main buffer.
            // Reverb send is a mono bus on channel 0 for the generic FDN path.
            let mono_in = if let Some(reverb_input) = ctx.reverb_input {
                reverb_input[base]
            } else {
                let mut mono_sum = 0.0f32;
                let mut active_count = 0u32;
                for ch in 0..render_channels {
                    if ctx.layout.is_channel_active(ch) {
                        mono_sum += buffer[base + ch];
                        active_count += 1;
                    }
                }
                if active_count == 0 {
                    continue;
                }
                mono_sum / active_count as f32
            };

            // High-pass the reverb send so the tail stays clear of low-mid mud
            // (and, with little sub-bass left, keeps reverb out of the LFE).
            let mono_in = match self.send_hpf.as_mut() {
                Some(hpf) => hpf.process(mono_in),
                None => mono_in,
            };

            // Pre-delay
            self.pre_delay_buffer[self.pre_delay_write_pos] = mono_in;
            let read_pos = (self.pre_delay_write_pos + self.pre_delay_buffer.len()
                - self.pre_delay_samples)
                & self.pre_delay_mask;
            let delayed_in = self.pre_delay_buffer[read_pos];
            self.pre_delay_write_pos = (self.pre_delay_write_pos + 1) & self.pre_delay_mask;

            // FDN → only active channels within render range
            let wet = self.process_fdn_sample(delayed_in, render_channels);

            for ch in 0..render_channels {
                if ctx.layout.is_channel_active(ch) {
                    // Per-channel voicing: surrounds run wetter than fronts, the
                    // centre stays near-dry (canonical 5.1 role order); `wet` sets
                    // the overall tail prominence.
                    let wet_ch =
                        wet[ch] * self.output_normalization * self.channel_gains[ch] * self.wet;
                    let mixed = buffer[base + ch] + wet_ch;
                    // Linear output — MasterGainStage is the single point of
                    // output limiting. sanitize_finite prevents NaN/Inf propagation
                    // and enforces a ±100.0 stability ceiling for runaway FDN.
                    buffer[base + ch] = sanitize_finite(mixed);
                }
            }
        }
    }

    fn reset(&mut self) {
        for line in &mut self.delay_buffers {
            line.fill(0.0);
        }
        self.pre_delay_buffer.fill(0.0);
        self.write_pos = 0;
        self.pre_delay_write_pos = 0;
        for d in &mut self.damping {
            d.state = 0.0;
        }
        if let Some(hpf) = self.send_hpf.as_mut() {
            hpf.reset();
        }
    }

    fn name(&self) -> &str {
        "fdn_reverb"
    }
}

// ── Test-only diagnostic accessors ──────────────────────────────────────────
#[cfg(test)]
impl FdnReverbStage {
    /// Diagnostic snapshot of FDN internal state for pipeline analysis.
    pub fn diagnostics(&self) -> FdnDiagnostics {
        let mut loop_gains = [0.0f32; NUM_LINES];
        for i in 0..NUM_LINES {
            // Effective loop gain per line: k / (1 - p) approximation at DC
            let k = self.damping[i].k;
            let p = self.damping[i].p;
            loop_gains[i] = if (1.0 - p).abs() > f32::EPSILON {
                k / (1.0 - p)
            } else {
                k
            };
        }
        FdnDiagnostics {
            output_normalization: self.output_normalization,
            pre_delay_samples: self.pre_delay_samples,
            delays: self.delays,
            loop_gains,
            buffer_size: self.buffer_size,
            initialized: self.initialized,
        }
    }
}

#[cfg(test)]
#[derive(Debug)]
pub struct FdnDiagnostics {
    pub output_normalization: f32,
    pub pre_delay_samples: usize,
    pub delays: [usize; NUM_LINES],
    pub loop_gains: [f32; NUM_LINES],
    pub buffer_size: usize,
    pub initialized: bool,
}

#[cfg(test)]
mod tests;
