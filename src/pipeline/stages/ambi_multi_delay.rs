//! Ambisonics B-format decorrelation stage.
//!
//! Adds spatial width to the B-format signal by mixing in short-delayed,
//! Z-rotated copies. Operates before the AmbisonicsDecodeStage (in B-format
//! domain, channels 0–3). No feedback — pure delay taps only.
//!
//! Late reverb is handled separately by FdnReverbStage (added after decode,
//! same architecture as VBAP/HRTF/DBAP).
//!
//! ## Signal flow (per sample)
//!
//! ```text
//! 1. Read dry B-format from main buffer
//! 2. Read 4 delayed taps (one per line) from the shared B-format ring
//! 3. Per-tap: lowpass → Z-rotation (72° × line index)
//! 4. Write dry input to the ring (no feedback)
//! 5. Output: dry + depth/√N × Σ(rotated taps)
//! ```
//!
//! Every line delays the same dry signal, so one ring serves all four taps.
//! It is sized at init from the longest tap and the sample rate.
//!
//! ## Why short delays?
//!
//! Rudrich (2016) used 100–300ms delays for decorrelating an already-diffuse
//! reverb tail. When applied to the direct B-format signal (as we do), those
//! long delays create audible discrete echoes. Delays of 7–23ms stay below
//! the echo threshold (~50ms) while providing effective spatial decorrelation.
//!
//! No-ops unless `ambisonic_bus` says the bus carries B-format (the bilateral
//! bus is already decoded to ear signals by the renderer).

use atrium_core::ambisonics::{foa_rotate_z, BFormat};

use crate::pipeline::mix_stage::{MixContext, MixStage};
use crate::pipeline::renderers::ambisonics::{ambisonic_bus, AmbisonicBus};

/// A B-format frame of silence: the ring's initial content and the tap sum's
/// starting value.
const SILENT_FRAME: BFormat = BFormat {
    w: 0.0,
    y: 0.0,
    z: 0.0,
    x: 0.0,
};

/// Number of decorrelation delay lines.
const NUM_LINES: usize = 4;

/// Delay times in milliseconds. Mutually coprime to avoid modal patterns.
/// All below the 50ms echo threshold for imperceptible spatial widening.
const DELAY_MS: [f32; NUM_LINES] = [7.0, 11.0, 17.0, 23.0];

/// Per-line Z-rotation angles. Each line gets a different rotation to
/// distribute the delayed signal across different spatial directions.
/// Multiples of 72° (2π/5) — coprime with the 4 lines for maximal spread.
const ROTATE_ANGLES: [f32; NUM_LINES] = [
    72.0 * std::f32::consts::PI / 180.0,
    144.0 * std::f32::consts::PI / 180.0,
    216.0 * std::f32::consts::PI / 180.0,
    288.0 * std::f32::consts::PI / 180.0,
];

/// Decorrelation depth: controls the wet level of the delayed taps.
/// Each tap gets gain = DEPTH / √NUM_LINES. Total wet energy = DEPTH².
/// At 0.3: wet energy is 9% of dry (~-10.5 dB) — subtle spatial widening.
const DEPTH: f32 = 0.3;

/// One-pole lowpass filter for anti-aliasing the delayed taps.
#[derive(Clone, Copy)]
struct OnePole {
    coeff: f32,
    state: f32,
}

impl OnePole {
    fn new() -> Self {
        Self {
            coeff: 0.0,
            state: 0.0,
        }
    }

    fn set_lowpass(&mut self, cutoff_hz: f32, sample_rate: f32) {
        self.coeff = (-2.0 * std::f32::consts::PI * cutoff_hz / sample_rate).exp();
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        self.state = input * (1.0 - self.coeff) + self.state * self.coeff;
        self.state
    }

    fn reset(&mut self) {
        self.state = 0.0;
    }
}

/// B-format decorrelation stage (MixStage).
///
/// 4 independent delay lines with Z-rotation, no feedback.
/// Operates on channels 0–3 of the output buffer. No-op on a bilateral bus.
pub struct AmbiMultiDelayStage {
    /// Ring of dry B-format frames shared by all lines, sized at init to the
    /// next power of two above the longest tap (empty until then).
    ring: Vec<BFormat>,
    /// Bitmask for wrapping ring positions: `ring.len() - 1`.
    ring_mask: usize,
    /// Ring write position.
    write_pos: usize,
    /// Per-line delay lengths in samples.
    delay_samples: [usize; NUM_LINES],
    /// Per-line, per-channel lowpass filters.
    lp: [[OnePole; 4]; NUM_LINES],
    /// Per-tap gain: DEPTH / √NUM_LINES.
    tap_gain: f32,
    /// Whether init has been called.
    initialized: bool,
}

impl AmbiMultiDelayStage {
    pub fn new() -> Self {
        Self {
            ring: Vec::new(),
            ring_mask: 0,
            write_pos: 0,
            delay_samples: [0; NUM_LINES],
            lp: [[OnePole::new(); 4]; NUM_LINES],
            tap_gain: DEPTH / (NUM_LINES as f32).sqrt(),
            initialized: false,
        }
    }
}

impl Default for AmbiMultiDelayStage {
    fn default() -> Self {
        Self::new()
    }
}

impl MixStage for AmbiMultiDelayStage {
    fn init(&mut self, ctx: &MixContext) {
        for (delay_samples, &milliseconds) in self.delay_samples.iter_mut().zip(DELAY_MS.iter()) {
            *delay_samples = ((milliseconds / 1000.0) * ctx.sample_rate) as usize;
        }

        // The ring holds the longest tap at this sample rate: a power of two
        // above it, so the read never aliases the slot written this frame.
        let longest_delay = self.delay_samples.iter().copied().max().unwrap_or(0);
        let ring_size = (longest_delay + 1).next_power_of_two();
        if ring_size != self.ring.len() {
            self.ring = vec![SILENT_FRAME; ring_size];
            self.ring_mask = ring_size - 1;
            self.write_pos = 0;
        }

        for line in 0..NUM_LINES {
            for ch in 0..4 {
                self.lp[line][ch].set_lowpass(6000.0, ctx.sample_rate);
            }
        }

        self.initialized = true;
    }

    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        let carries_bformat = matches!(
            ambisonic_bus(ctx.layout, ctx.render_channels, ctx.channels),
            AmbisonicBus::BFormat(_)
        );
        if !carries_bformat || !self.initialized {
            return;
        }

        let num_frames = buffer.len() / ctx.channels;
        let gain = self.tap_gain;

        for frame in 0..num_frames {
            let base = frame * ctx.channels;

            // Read dry B-format from main buffer.
            let dry = BFormat {
                w: buffer[base],
                y: buffer[base + 1],
                z: buffer[base + 2],
                x: buffer[base + 3],
            };

            // Sum delayed, filtered, rotated taps.
            let mut wet = SILENT_FRAME;

            for line in 0..NUM_LINES {
                let read_pos =
                    (self.write_pos + self.ring.len() - self.delay_samples[line]) & self.ring_mask;
                let mut tap = self.ring[read_pos];

                // Lowpass for anti-aliasing.
                tap.w = self.lp[line][0].process(tap.w);
                tap.y = self.lp[line][1].process(tap.y);
                tap.z = self.lp[line][2].process(tap.z);
                tap.x = self.lp[line][3].process(tap.x);

                // Z-rotation for spatial decorrelation.
                let rotated = foa_rotate_z(&tap, ROTATE_ANGLES[line]);

                wet.w += rotated.w;
                wet.y += rotated.y;
                wet.z += rotated.z;
                wet.x += rotated.x;
            }

            // Write dry input to the ring (no feedback).
            self.ring[self.write_pos] = dry;
            self.write_pos = (self.write_pos + 1) & self.ring_mask;

            // Output: dry + scaled wet.
            buffer[base] = dry.w + wet.w * gain;
            buffer[base + 1] = dry.y + wet.y * gain;
            buffer[base + 2] = dry.z + wet.z * gain;
            buffer[base + 3] = dry.x + wet.x * gain;
        }
    }

    fn reset(&mut self) {
        self.ring.fill(SILENT_FRAME);
        self.write_pos = 0;
        for line in 0..NUM_LINES {
            for ch in 0..4 {
                self.lp[line][ch].reset();
            }
        }
    }

    fn name(&self) -> &str {
        "ambi_decorrelation"
    }
}

#[cfg(test)]
mod tests;
