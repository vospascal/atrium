//! Per-source first-order reflections via image-source method (Allen & Berkley, 1979).
//!
//! SourceStage version: source→listener (VBAP / HRTF).
//! PathStage version: source→speaker (WorldLocked).

use crate::pipeline::path_stage::{PathContext, PathStage};
use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

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

/// Shared mono delay buffer + tapped readback for image-source reflections.
struct ReflectionCore {
    buffer: Box<[f32; BUFFER_SIZE]>,
    write_pos: usize,
    taps: [ReflectionTap; MAX_TAPS],
    tap_count: usize,
    wet_gain: f32,
    wall_absorption: f32,
}

impl ReflectionCore {
    fn new(wet_gain: f32, wall_absorption: f32) -> Self {
        Self {
            buffer: Box::new([0.0; BUFFER_SIZE]),
            write_pos: 0,
            taps: [ReflectionTap {
                delay_samples: 0,
                gain: 0.0,
            }; MAX_TAPS],
            tap_count: 0,
            wet_gain,
            wall_absorption,
        }
    }

    /// Compute taps from image sources (source mirrored across each wall)
    /// relative to a target (listener or speaker).
    fn update(
        &mut self,
        room_min: Vec3,
        room_max: Vec3,
        source_pos: Vec3,
        target_pos: Vec3,
        sample_rate: f32,
    ) {
        let images = [
            Vec3::new(2.0 * room_min.x - source_pos.x, source_pos.y, source_pos.z),
            Vec3::new(2.0 * room_max.x - source_pos.x, source_pos.y, source_pos.z),
            Vec3::new(source_pos.x, 2.0 * room_min.y - source_pos.y, source_pos.z),
            Vec3::new(source_pos.x, 2.0 * room_max.y - source_pos.y, source_pos.z),
            Vec3::new(source_pos.x, source_pos.y, 2.0 * room_min.z - source_pos.z),
            Vec3::new(source_pos.x, source_pos.y, 2.0 * room_max.z - source_pos.z),
        ];

        let direct_dist = source_pos.distance_to(target_pos);
        let mut count = 0;

        for image in &images {
            let image_dist = image.distance_to(target_pos);
            if image_dist < 0.1 || image_dist < direct_dist {
                continue;
            }
            let delay_seconds = (image_dist - direct_dist) / SPEED_OF_SOUND;
            let delay_samples = (delay_seconds * sample_rate) as usize;
            if delay_samples == 0 || delay_samples >= BUFFER_SIZE {
                continue;
            }
            self.taps[count] = ReflectionTap {
                delay_samples,
                gain: (self.wall_absorption / image_dist).min(1.0),
            };
            count += 1;
            if count >= MAX_TAPS {
                break;
            }
        }
        self.tap_count = count;
    }

    #[inline]
    fn process_sample(&mut self, input: f32) -> f32 {
        self.buffer[self.write_pos] = input;
        let mut wet = 0.0f32;
        for i in 0..self.tap_count {
            let tap = &self.taps[i];
            let read_pos = (self.write_pos + BUFFER_SIZE - tap.delay_samples) & BUFFER_MASK;
            wet += self.buffer[read_pos] * tap.gain;
        }
        self.write_pos = (self.write_pos + 1) & BUFFER_MASK;
        wet * self.wet_gain
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SourceStage: per-source, listener-relative
// ─────────────────────────────────────────────────────────────────────────────

/// Per-source reflections. Image sources relative to listener.
pub struct ReflectionsStage {
    core: ReflectionCore,
}

impl ReflectionsStage {
    pub fn new(wet_gain: f32, wall_absorption: f32) -> Self {
        Self {
            core: ReflectionCore::new(wet_gain, wall_absorption),
        }
    }
}

impl SourceStage for ReflectionsStage {
    fn process(&mut self, ctx: &SourceContext, _output: &mut SourceOutput) {
        self.core.update(
            ctx.room_min,
            ctx.room_max,
            ctx.source_pos,
            ctx.listener.position,
            ctx.sample_rate,
        );
    }

    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        // Return direct + wet reflection
        sample + self.core.process_sample(sample)
    }

    fn name(&self) -> &str {
        "reflections"
    }

    fn reset(&mut self) {
        self.core.reset();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PathStage: per source × speaker, world-locked
// ─────────────────────────────────────────────────────────────────────────────

/// Per-path reflections. Image sources relative to speaker (target).
pub struct ReflectionsPath {
    core: ReflectionCore,
}

impl ReflectionsPath {
    pub fn new(wet_gain: f32, wall_absorption: f32) -> Self {
        Self {
            core: ReflectionCore::new(wet_gain, wall_absorption),
        }
    }
}

impl PathStage for ReflectionsPath {
    fn update(&mut self, ctx: &PathContext) {
        self.core.update(
            ctx.room_min,
            ctx.room_max,
            ctx.source_pos,
            ctx.target_pos,
            ctx.sample_rate,
        );
    }

    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        sample + self.core.process_sample(sample)
    }

    fn name(&self) -> &str {
        "reflections_path"
    }

    fn reset(&mut self) {
        self.core.reset();
    }
}
