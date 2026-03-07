//! Post-mix early reflections via image-source method (Allen & Berkley, 1979).
//!
//! Tapped delay line on the mixed multichannel signal. Each tap corresponds
//! to one wall reflection of the box room. Delays are 5–18ms, giving the
//! brain cues about room size and shape.
//!
//! This is the POST-MIX version (listener-relative). The per-source version
//! lives in `ReflectionsStage` (used by SourceStages / PathStages).

use crate::pipeline::mix_stage::{MixContext, MixStage};

use atrium_core::types::Vec3;

const MAX_TAPS: usize = 6;
const BUFFER_SIZE: usize = 4096;
const BUFFER_MASK: usize = BUFFER_SIZE - 1;
use crate::audio::atmosphere::SPEED_OF_SOUND;

#[derive(Clone, Copy)]
struct ReflectionTap {
    delay_samples: usize,
    gain: f32,
}

/// Minimum listener movement (meters) before recomputing taps.
const TAP_UPDATE_THRESHOLD: f32 = 0.1;

/// Post-mix early reflections stage.
pub struct EarlyReflectionsStage {
    buffers: Vec<Box<[f32; BUFFER_SIZE]>>,
    write_pos: usize,
    taps: [ReflectionTap; MAX_TAPS],
    tap_count: usize,
    initialized: bool,
    wet_gain: f32,
    wall_absorption: f32,
    last_listener_pos: Vec3,
}

impl EarlyReflectionsStage {
    pub fn new(wet_gain: f32, wall_absorption: f32) -> Self {
        Self {
            buffers: Vec::new(),
            write_pos: 0,
            taps: [ReflectionTap {
                delay_samples: 0,
                gain: 0.0,
            }; MAX_TAPS],
            tap_count: 0,
            initialized: false,
            wet_gain,
            wall_absorption,
            last_listener_pos: Vec3::ZERO,
        }
    }

    fn compute_taps(
        &mut self,
        room_min: Vec3,
        room_max: Vec3,
        listener_pos: Vec3,
        sample_rate: f32,
    ) {
        let pos = listener_pos;
        let mut count = 0;

        let wall_distances = [
            pos.x - room_min.x,
            room_max.x - pos.x,
            pos.y - room_min.y,
            room_max.y - pos.y,
            pos.z - room_min.z,
            room_max.z - pos.z,
        ];

        for &dist in &wall_distances {
            if dist < 0.1 {
                continue;
            }
            let round_trip = 2.0 * dist;
            let delay_samples = (round_trip / SPEED_OF_SOUND * sample_rate) as usize;
            if delay_samples == 0 || delay_samples >= BUFFER_SIZE {
                continue;
            }
            let distance_atten = (1.0 / round_trip).min(1.0);
            self.taps[count] = ReflectionTap {
                delay_samples,
                gain: self.wall_absorption * distance_atten,
            };
            count += 1;
        }

        self.tap_count = count;
        self.last_listener_pos = listener_pos;
        self.initialized = true;
    }
}

impl MixStage for EarlyReflectionsStage {
    fn init(&mut self, ctx: &MixContext) {
        self.compute_taps(
            ctx.room_min,
            ctx.room_max,
            ctx.listener.position,
            ctx.sample_rate,
        );
    }

    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        // Recompute taps when listener moves beyond threshold
        if self.initialized
            && ctx.listener.position.distance_to(self.last_listener_pos) > TAP_UPDATE_THRESHOLD
        {
            self.compute_taps(
                ctx.room_min,
                ctx.room_max,
                ctx.listener.position,
                ctx.sample_rate,
            );
        }

        if !self.initialized || self.tap_count == 0 {
            return;
        }

        let channels = ctx.channels;

        // Lazy-init per-channel delay buffers
        while self.buffers.len() < channels {
            self.buffers.push(Box::new([0.0; BUFFER_SIZE]));
        }

        let num_frames = buffer.len() / channels;
        for frame in 0..num_frames {
            let base = frame * channels;

            for ch in 0..channels {
                self.buffers[ch][self.write_pos] = buffer[base + ch];
            }

            for ch in 0..channels {
                let mut wet = 0.0f32;
                for i in 0..self.tap_count {
                    let tap = &self.taps[i];
                    let read_pos = (self.write_pos + BUFFER_SIZE - tap.delay_samples) & BUFFER_MASK;
                    wet += self.buffers[ch][read_pos] * tap.gain;
                }
                buffer[base + ch] = (buffer[base + ch] + wet * self.wet_gain).clamp(-1.0, 1.0);
            }

            self.write_pos = (self.write_pos + 1) & BUFFER_MASK;
        }
    }

    fn reset(&mut self) {
        for buf in &mut self.buffers {
            buf.fill(0.0);
        }
        self.write_pos = 0;
    }

    fn name(&self) -> &str {
        "early_reflections"
    }
}
