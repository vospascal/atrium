// Early reflections via the image-source method (Allen & Berkley, 1979).
//
// For an axis-aligned BoxRoom, each wall produces one first-order reflection.
// The reflection arrives delayed (round-trip to wall / speed of sound) and
// attenuated (wall absorption × inverse distance). These short delays (5-18ms)
// give the brain cues about room size and shape.
//
// Implementation: tapped delay line on the mixed stereo signal. Each tap
// corresponds to one wall reflection. Circular buffer with power-of-2 size
// for efficient bitmask wrapping. No allocations in the audio callback.

use crate::processors::AudioProcessor;
use crate::spatial::listener::Listener;
use crate::world::types::Vec3;

/// Maximum number of reflection taps (6 walls of a box room).
const MAX_TAPS: usize = 6;

/// Buffer size: 4096 samples = ~85ms at 48kHz. Power of 2 for bitmask wrapping.
const BUFFER_SIZE: usize = 4096;
const BUFFER_MASK: usize = BUFFER_SIZE - 1;

/// Speed of sound in air at ~20°C, meters per second.
const SPEED_OF_SOUND: f32 = 343.0;

/// A single early reflection tap representing one wall reflection.
#[derive(Clone, Copy, Debug)]
struct ReflectionTap {
    delay_samples: usize,
    gain: f32,
}

/// Early reflections processor using the image-source method on the mixed signal.
/// Supports any channel count: one circular delay buffer per output channel.
pub struct EarlyReflections {
    /// Per-channel circular delay buffers. Lazy-initialized on first process() call.
    buffers: Vec<Box<[f32; BUFFER_SIZE]>>,
    write_pos: usize,
    taps: [ReflectionTap; MAX_TAPS],
    tap_count: usize,
    initialized: bool,
    wet_gain: f32,
    wall_absorption: f32,
}

impl EarlyReflections {
    /// Create an uninitialized early reflections processor.
    ///
    /// - `wet_gain`: how much reflection to mix in (0.0–1.0, typical 0.3–0.5)
    /// - `wall_absorption`: wall reflectivity (0.0 = absorbs all, 1.0 = perfect mirror, typical 0.85–0.95 for plaster)
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
        }
    }

    /// Compute reflection taps from room bounds and listener position.
    fn compute_taps(
        &mut self,
        room_min: Vec3,
        room_max: Vec3,
        listener: &Listener,
        sample_rate: f32,
    ) {
        let pos = listener.position;
        let mut count = 0;

        // 6 walls of the box room: distance from listener to each wall
        let wall_distances = [
            pos.x - room_min.x, // x-min wall
            room_max.x - pos.x, // x-max wall
            pos.y - room_min.y, // y-min wall
            room_max.y - pos.y, // y-max wall
            pos.z - room_min.z, // floor
            room_max.z - pos.z, // ceiling
        ];

        for &dist in &wall_distances {
            // Skip walls the listener is very close to (<0.1m).
            // Sub-millisecond reflections fuse perceptually with the direct signal.
            if dist < 0.1 {
                continue;
            }

            let round_trip = 2.0 * dist;
            let delay_seconds = round_trip / SPEED_OF_SOUND;
            let delay_samples = (delay_seconds * sample_rate) as usize;

            if delay_samples == 0 || delay_samples >= BUFFER_SIZE {
                continue;
            }

            // Gain: wall absorption × inverse distance attenuation
            let distance_atten = 1.0 / round_trip;

            self.taps[count] = ReflectionTap {
                delay_samples,
                gain: self.wall_absorption * distance_atten,
            };
            count += 1;
        }

        self.tap_count = count;
        self.initialized = true;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-source reflections (image-source method, Allen & Berkley 1979)
// ─────────────────────────────────────────────────────────────────────────────

/// Per-source first-order reflections using the image-source method.
///
/// Unlike the post-mix `EarlyReflections` processor, this struct lives in the
/// mixer and operates on the mono source signal BEFORE panning. Each source gets
/// its own delay buffer with taps computed from the source's image positions
/// relative to the listener.
///
/// Image-source principle: for each wall, mirror the source position across the
/// wall plane. The image→listener distance gives the reflection delay; gain is
/// `wall_absorption / image_distance`.
pub struct SourceReflections {
    /// Circular delay buffer (mono, one per source).
    buffer: Box<[f32; BUFFER_SIZE]>,
    write_pos: usize,
    taps: [ReflectionTap; MAX_TAPS],
    tap_count: usize,
    wet_gain: f32,
    wall_absorption: f32,
}

impl SourceReflections {
    pub fn new(wet_gain: f32, wall_absorption: f32) -> Self {
        Self {
            buffer: Box::new([0.0; BUFFER_SIZE]),
            write_pos: 0,
            taps: [ReflectionTap { delay_samples: 0, gain: 0.0 }; MAX_TAPS],
            tap_count: 0,
            wet_gain,
            wall_absorption,
        }
    }

    /// Recompute reflection taps from room geometry, source position, and listener.
    ///
    /// For a box room with 6 walls, each wall generates one image source:
    ///   image_pos = source mirrored across the wall plane
    ///   delay = image_pos.distance_to(listener) / speed_of_sound
    ///   gain = wall_absorption / image_pos.distance_to(listener)
    pub fn update(
        &mut self,
        room_min: Vec3,
        room_max: Vec3,
        source_pos: Vec3,
        listener_pos: Vec3,
        sample_rate: f32,
    ) {
        let mut count = 0;

        // 6 image sources: mirror source across each wall of the box
        let images = [
            Vec3::new(2.0 * room_min.x - source_pos.x, source_pos.y, source_pos.z), // x-min
            Vec3::new(2.0 * room_max.x - source_pos.x, source_pos.y, source_pos.z), // x-max
            Vec3::new(source_pos.x, 2.0 * room_min.y - source_pos.y, source_pos.z), // y-min
            Vec3::new(source_pos.x, 2.0 * room_max.y - source_pos.y, source_pos.z), // y-max
            Vec3::new(source_pos.x, source_pos.y, 2.0 * room_min.z - source_pos.z), // floor
            Vec3::new(source_pos.x, source_pos.y, 2.0 * room_max.z - source_pos.z), // ceiling
        ];

        let direct_dist = source_pos.distance_to(listener_pos);

        for image in &images {
            let image_dist = image.distance_to(listener_pos);

            // Skip if image is closer than direct (shouldn't happen but guard)
            // or if image distance is tiny (source at wall)
            if image_dist < 0.1 || image_dist < direct_dist {
                continue;
            }

            // Delay relative to direct sound: (image_path - direct_path) / c
            let delay_seconds = (image_dist - direct_dist) / SPEED_OF_SOUND;
            let delay_samples = (delay_seconds * sample_rate) as usize;

            if delay_samples == 0 || delay_samples >= BUFFER_SIZE {
                continue;
            }

            // Gain: wall absorption × 1/image_distance (amplitude, not energy)
            let distance_atten = 1.0 / image_dist;

            self.taps[count] = ReflectionTap {
                delay_samples,
                gain: self.wall_absorption * distance_atten,
            };
            count += 1;
            if count >= MAX_TAPS {
                break;
            }
        }

        self.tap_count = count;
    }

    /// Process one mono sample: write to delay buffer, return wet reflection sum.
    /// The caller adds `wet * wet_gain` to the output.
    #[inline]
    pub fn process_sample(&mut self, input: f32) -> f32 {
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

    /// Get the wet_gain and wall_absorption parameters.
    pub fn params(&self) -> (f32, f32) {
        (self.wet_gain, self.wall_absorption)
    }

    /// Clear the delay buffer.
    pub fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Post-mix early reflections processor (legacy — listener-only)
// ─────────────────────────────────────────────────────────────────────────────

impl AudioProcessor for EarlyReflections {
    fn init(
        &mut self,
        room_min: Vec3,
        room_max: Vec3,
        listener: &Listener,
        sample_rate: f32,
    ) {
        self.compute_taps(room_min, room_max, listener, sample_rate);
    }

    fn process(&mut self, buffer: &mut [f32], channels: usize, _sample_rate: f32) {
        if !self.initialized || self.tap_count == 0 {
            return;
        }

        // Lazy-init per-channel delay buffers (only allocates on first call)
        while self.buffers.len() < channels {
            self.buffers.push(Box::new([0.0; BUFFER_SIZE]));
        }

        let num_frames = buffer.len() / channels;

        for frame in 0..num_frames {
            let base = frame * channels;

            // Write dry signal into per-channel circular delay buffers
            for ch in 0..channels {
                self.buffers[ch][self.write_pos] = buffer[base + ch];
            }

            // Sum delayed taps per channel
            for ch in 0..channels {
                let mut wet = 0.0f32;
                for i in 0..self.tap_count {
                    let tap = &self.taps[i];
                    let read_pos =
                        (self.write_pos + BUFFER_SIZE - tap.delay_samples) & BUFFER_MASK;
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
        "EarlyReflections"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_computation_centered_listener() {
        let mut er = EarlyReflections::new(1.0, 0.9);
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);
        er.compute_taps(Vec3::ZERO, Vec3::new(6.0, 4.0, 3.0), &listener, 48000.0);

        // Floor (z=0) skipped (distance < 0.1m), 5 taps active
        assert_eq!(er.tap_count, 5);

        for i in 0..er.tap_count {
            assert!(er.taps[i].delay_samples > 0);
            assert!(er.taps[i].delay_samples < BUFFER_SIZE);
            assert!(er.taps[i].gain > 0.0);
            assert!(er.taps[i].gain < 1.0);
        }
    }

    #[test]
    fn all_six_taps_when_listener_at_center_height() {
        let mut er = EarlyReflections::new(1.0, 0.9);
        let listener = Listener::new(Vec3::new(3.0, 2.0, 1.5), 0.0);
        er.compute_taps(Vec3::ZERO, Vec3::new(6.0, 4.0, 3.0), &listener, 48000.0);

        // All 6 walls > 0.1m away → 6 taps
        assert_eq!(er.tap_count, 6);
    }

    #[test]
    fn impulse_produces_delayed_copies() {
        let mut er = EarlyReflections::new(1.0, 1.0);
        let listener = Listener::new(Vec3::new(3.0, 2.0, 1.5), 0.0);
        er.compute_taps(Vec3::ZERO, Vec3::new(6.0, 4.0, 3.0), &listener, 48000.0);

        let channels = 2;
        let total_frames = 1024;
        let mut buffer = vec![0.0f32; total_frames * channels];
        buffer[0] = 1.0; // L impulse
        buffer[1] = 1.0; // R impulse

        er.process(&mut buffer, channels, 48000.0);

        // Dry signal at frame 0 should be preserved (plus reflections from frame 0 = 0)
        assert!(buffer[0] > 0.9, "dry signal: {}", buffer[0]);

        // Frames 1 through shortest-delay-1 should be silence.
        // Shortest delay: 1.5m (floor/ceiling) → round_trip=3m → 3/343*48000 ≈ 419 samples
        for frame in 1..419 {
            let l = buffer[frame * channels];
            assert!(
                l.abs() < 1e-6,
                "unexpected signal at frame {}: {}",
                frame,
                l
            );
        }

        // At the shortest delay, there should be a reflection
        let reflection_frame = 419;
        let l = buffer[reflection_frame * channels];
        assert!(
            l.abs() > 0.01,
            "expected reflection at frame {}: {}",
            reflection_frame,
            l
        );
    }

    #[test]
    fn silence_in_silence_out() {
        let mut er = EarlyReflections::new(0.5, 0.9);
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);
        er.compute_taps(Vec3::ZERO, Vec3::new(6.0, 4.0, 3.0), &listener, 48000.0);

        let mut buffer = vec![0.0f32; 512 * 2];
        er.process(&mut buffer, 2, 48000.0);

        for &sample in &buffer {
            assert_eq!(sample, 0.0);
        }
    }

    #[test]
    fn uninitialized_is_passthrough() {
        let mut er = EarlyReflections::new(0.5, 0.9);
        // Don't call compute_taps — initialized is false

        let mut buffer = vec![0.5f32; 128 * 2];
        let original = buffer.clone();
        er.process(&mut buffer, 2, 48000.0);

        assert_eq!(buffer, original);
    }

    #[test]
    fn name_returns_expected() {
        let er = EarlyReflections::new(0.5, 0.9);
        assert_eq!(er.name(), "EarlyReflections");
    }

    // ── SourceReflections (per-source image-source method) ──────────────

    #[test]
    fn source_reflections_compute_taps() {
        let mut sr = SourceReflections::new(1.0, 0.9);
        let room_min = Vec3::ZERO;
        let room_max = Vec3::new(6.0, 4.0, 3.0);
        let source = Vec3::new(2.0, 2.0, 1.5);
        let listener = Vec3::new(4.0, 2.0, 1.5);
        sr.update(room_min, room_max, source, listener, 48000.0);

        // Source at (2,2,1.5), listener at (4,2,1.5) — all 6 images should
        // produce valid taps (source is well inside the room)
        assert!(sr.tap_count >= 4, "expected at least 4 taps, got {}", sr.tap_count);

        for i in 0..sr.tap_count {
            assert!(sr.taps[i].delay_samples > 0);
            assert!(sr.taps[i].gain > 0.0);
        }
    }

    #[test]
    fn source_reflections_impulse() {
        let mut sr = SourceReflections::new(1.0, 1.0);
        let room_min = Vec3::ZERO;
        let room_max = Vec3::new(6.0, 4.0, 3.0);
        let source = Vec3::new(3.0, 2.0, 1.5);
        let listener = Vec3::new(3.0, 2.0, 1.5); // co-located
        sr.update(room_min, room_max, source, listener, 48000.0);

        // Feed an impulse
        let wet0 = sr.process_sample(1.0);
        // First sample: no delay tap can fire yet (all delays > 0)
        assert!(wet0.abs() < 1e-6, "no reflection at t=0: got {wet0}");

        // Feed silence until shortest tap fires
        let mut found_reflection = false;
        for _ in 1..2048 {
            let wet = sr.process_sample(0.0);
            if wet.abs() > 0.001 {
                found_reflection = true;
                break;
            }
        }
        assert!(found_reflection, "should find at least one reflection");
    }
}
