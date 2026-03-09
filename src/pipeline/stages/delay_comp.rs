//! Speaker delay compensation.
//!
//! Aligns time-of-arrival across speakers by delaying nearer channels.
//! Two modes:
//!
//! - **Static calibration** (WorldLocked): fixed delays from installation
//!   measurement. Does NOT track listener position.
//! - **Listener-relative** (VBAP): recomputes per-speaker delays every buffer
//!   based on current listener position.
//!
//! Uses per-channel circular delay buffers with power-of-2 capacity for
//! branch-free bitmask wrapping. Linear interpolation for fractional-sample
//! accuracy.

use atrium_core::speaker::MAX_CHANNELS;

use crate::pipeline::mix_stage::{MixContext, MixStage};

/// Delay compensation mode.
#[derive(Clone, Copy, Debug)]
enum Mode {
    /// Fixed delays — set once at init, never updated.
    Static,
    /// Recomputed every buffer from listener position.
    ListenerRelative,
}

/// Post-mix delay compensation stage.
pub struct DelayCompStage {
    mode: Mode,
    buffers: Vec<Vec<f32>>,
    write_pos: Vec<usize>,
    delays: Vec<f32>,
    capacity: usize,
}

impl DelayCompStage {
    /// WorldLocked mode: fixed delays based on installation calibration.
    pub fn static_calibration() -> Self {
        Self::new_inner(Mode::Static)
    }

    /// VBAP mode: delays track listener position each buffer.
    pub fn listener_relative() -> Self {
        Self::new_inner(Mode::ListenerRelative)
    }

    fn new_inner(mode: Mode) -> Self {
        Self {
            mode,
            buffers: Vec::new(),
            write_pos: Vec::new(),
            delays: vec![0.0; MAX_CHANNELS],
            capacity: 0,
        }
    }

    /// Compute per-speaker delays so all channels arrive simultaneously.
    /// Nearer speakers get MORE delay (aligned to the farthest).
    fn compute_delays(&mut self, ctx: &MixContext) {
        let layout = ctx.layout;
        let target_pos = match self.mode {
            // For static: use room center as reference point
            Mode::Static => (ctx.room_min + ctx.room_max) * 0.5,
            // For listener-relative: use actual listener position
            Mode::ListenerRelative => ctx.listener.position,
        };

        let mut d_max = 0.0f32;
        for i in 0..layout.speaker_count() {
            if let Some(speaker) = layout.speaker_by_index(i) {
                let d = (speaker.position - target_pos).length();
                d_max = d_max.max(d);
            }
        }

        for i in 0..layout.speaker_count() {
            if let Some(speaker) = layout.speaker_by_index(i) {
                let d = (speaker.position - target_pos).length();
                let delay_seconds = (d_max - d) / ctx.atmosphere.speed_of_sound();
                let delay_samples = delay_seconds * ctx.sample_rate;
                if speaker.channel < self.delays.len() {
                    self.delays[speaker.channel] = delay_samples;
                }
            }
        }
    }

    /// Ensure buffers are large enough for the room geometry.
    fn ensure_capacity(&mut self, ctx: &MixContext) {
        let room_diag = (ctx.room_max - ctx.room_min).length();
        let max_delay_samples =
            (room_diag / ctx.atmosphere.speed_of_sound() * ctx.sample_rate).ceil() as usize;
        let needed = max_delay_samples.next_power_of_two().max(1024);

        if needed > self.capacity || self.buffers.len() < ctx.channels {
            self.capacity = needed;
            self.buffers = vec![vec![0.0; needed]; ctx.channels];
            self.write_pos = vec![0; ctx.channels];
        }
    }

    #[inline]
    fn process_channel(&mut self, ch: usize, input: f32) -> f32 {
        let cap = self.capacity;
        let mask = cap - 1;

        let wp = self.write_pos[ch];
        self.buffers[ch][wp] = input;
        self.write_pos[ch] = (wp + 1) & mask;

        let delay = self.delays[ch];
        if delay < 0.5 {
            return input;
        }
        let delay_clamped = delay.min((cap - 2) as f32);
        let delay_int = delay_clamped as usize;
        let frac = delay_clamped - delay_int as f32;

        let idx0 = (wp + cap - delay_int) & mask;
        let idx1 = (wp + cap - delay_int - 1) & mask;

        let s0 = self.buffers[ch][idx0];
        let s1 = self.buffers[ch][idx1];
        s0 + (s1 - s0) * frac
    }
}

impl MixStage for DelayCompStage {
    fn init(&mut self, ctx: &MixContext) {
        self.ensure_capacity(ctx);
        // Static mode: compute delays once at init
        if matches!(self.mode, Mode::Static) {
            self.compute_delays(ctx);
        }
    }

    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        if self.capacity == 0 {
            return;
        }

        // Listener-relative: update delays every buffer
        if matches!(self.mode, Mode::ListenerRelative) {
            self.compute_delays(ctx);
        }

        let channels = ctx.channels;
        let num_frames = buffer.len() / channels;
        for frame in 0..num_frames {
            let base = frame * channels;
            for ch in 0..channels {
                buffer[base + ch] = self.process_channel(ch, buffer[base + ch]);
            }
        }
    }

    fn reset(&mut self) {
        for buf in &mut self.buffers {
            buf.fill(0.0);
        }
        for wp in &mut self.write_pos {
            *wp = 0;
        }
    }

    fn name(&self) -> &str {
        "delay_comp"
    }
}
