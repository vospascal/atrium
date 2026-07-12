//! First-order reflections via image-source method (Allen & Berkley, 1979).
//!
//! `ReflectionCore` is the shared delay-buffer engine used by
//! WorldLockedRenderer (per-speaker reflections).

use atrium_core::types::Vec3;

const MAX_TAPS: usize = 6;

#[derive(Clone, Copy)]
struct ReflectionTap {
    delay_samples: usize,
    gain: f32,
}

/// Shared mono delay buffer + tapped readback for image-source reflections.
///
/// Buffer capacity is dynamically sized from the maximum first-order image-source
/// distance (via `room_acoustics::delay_buffer_capacity`), with a minimum of 4096
/// samples (~85ms at 48kHz). Power-of-2 sizing enables fast wrapping via bitmask.
pub(crate) struct ReflectionCore {
    buffer: Box<[f32]>,
    capacity: usize,
    mask: usize,
    write_pos: usize,
    taps: [ReflectionTap; MAX_TAPS],
    tap_count: usize,
    wall_reflectivity: f32,
}

impl ReflectionCore {
    /// Minimum capacity: 4096 samples (~85ms at 48kHz).
    const MIN_CAPACITY: usize = 4096;

    pub(crate) fn new(wall_reflectivity: f32, capacity: usize) -> Self {
        let capacity = capacity.max(Self::MIN_CAPACITY).next_power_of_two();
        Self {
            buffer: vec![0.0; capacity].into_boxed_slice(),
            capacity,
            mask: capacity - 1,
            write_pos: 0,
            taps: [ReflectionTap {
                delay_samples: 0,
                gain: 0.0,
            }; MAX_TAPS],
            tap_count: 0,
            wall_reflectivity,
        }
    }

    /// Compute taps from image sources (source mirrored across each wall)
    /// relative to a target (listener or speaker).
    ///
    /// `ref_distance` is the source's distance-model reference distance. Each
    /// tap's gain includes the reflection's *relative* distance attenuation
    /// `max(direct, ref) / image` — relative because the caller multiplies the
    /// summed direct+reflections signal by the direct path's distance gain, so
    /// per-tap gains only need the image-vs-direct ratio. For the inverse model
    /// (rolloff 1) the composition is exact: g_direct × ratio = ref / image.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn update(
        &mut self,
        room_min: Vec3,
        room_max: Vec3,
        source_pos: Vec3,
        target_pos: Vec3,
        sample_rate: f32,
        speed_of_sound: f32,
        ref_distance: f32,
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

        // √reflectivity: wall_reflectivity is energy-domain, amplitude = √energy.
        let amplitude_refl = self.wall_reflectivity.sqrt();

        for image in &images {
            let image_dist = image.distance_to(target_pos);
            if image_dist < 0.1 || image_dist < direct_dist {
                continue;
            }
            let delay_seconds = (image_dist - direct_dist) / speed_of_sound;
            let delay_samples = (delay_seconds * sample_rate) as usize;
            if delay_samples == 0 || delay_samples >= self.capacity {
                continue;
            }
            // Relative spherical-spreading attenuation for the longer reflected
            // path. Without this every tap arrived at full wall-gain amplitude,
            // and 6 near-unattenuated taps summed with the direct signal pushed
            // sustained tones into overload (+16 dB worst case).
            let distance_ratio = (direct_dist.max(ref_distance) / image_dist).min(1.0);
            self.taps[count] = ReflectionTap {
                delay_samples,
                gain: amplitude_refl * distance_ratio,
            };
            count += 1;
            if count >= MAX_TAPS {
                break;
            }
        }
        self.tap_count = count;
    }

    #[inline]
    pub(crate) fn process_sample(&mut self, input: f32) -> f32 {
        self.buffer[self.write_pos] = input;
        let mut wet = 0.0f32;
        for i in 0..self.tap_count {
            let tap = &self.taps[i];
            let read_pos = (self.write_pos + self.capacity - tap.delay_samples) & self.mask;
            wet += self.buffer[read_pos] * tap.gain;
        }
        self.write_pos = (self.write_pos + 1) & self.mask;
        wet
    }

    pub(crate) fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
        self.tap_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: f32 = 48000.0;
    const SPEED_OF_SOUND: f32 = 343.42;

    /// Feed a unit impulse and collect the tap outputs. Returns the sum of all
    /// echo amplitudes and the largest single echo.
    fn impulse_response_stats(core: &mut ReflectionCore, frames: usize) -> (f32, f32) {
        let mut sum = 0.0f32;
        let mut peak = 0.0f32;
        for n in 0..frames {
            let input = if n == 0 { 1.0 } else { 0.0 };
            let wet = core.process_sample(input);
            sum += wet.abs();
            peak = peak.max(wet.abs());
        }
        (sum, peak)
    }

    /// Each reflection tap must carry spherical-spreading attenuation for its
    /// longer path. Source 1 m from the target in a 10 m room: the nearest
    /// image is 9 m away, so its tap must arrive at ~1/9 of the wall gain —
    /// not at full wall gain (the old behavior that overloaded WorldLocked
    /// on sustained tones).
    #[test]
    fn taps_attenuate_with_image_distance() {
        // Energy reflectivity 0.81 → amplitude √0.81 = 0.9.
        let mut core = ReflectionCore::new(0.81, 65536);
        let room_min = Vec3::new(0.0, 0.0, 0.0);
        let room_max = Vec3::new(10.0, 10.0, 10.0);
        let source = Vec3::new(5.0, 5.0, 5.0);
        let target = Vec3::new(4.0, 5.0, 5.0);
        core.update(
            room_min,
            room_max,
            source,
            target,
            SAMPLE_RATE,
            SPEED_OF_SOUND,
            1.0,
        );

        // Expected tap gains (direct = 1 m, ref = 1 m → ratio = 1/image):
        //   -X wall: image (-5,5,5), 9 m  → 0.9/9      = 0.1000
        //   +X wall: image (15,5,5), 11 m → 0.9/11     = 0.0818
        //   ±Y, ±Z:  image dist √101 ≈ 10.05 m → 0.9/10.05 = 0.0896 (×4)
        let expected_sum = 0.9 / 9.0 + 0.9 / 11.0 + 4.0 * (0.9 / 101.0f32.sqrt());
        let expected_peak = 4.0 * (0.9 / 101.0f32.sqrt()); // 4 coincident ±Y/±Z taps

        let (sum, peak) = impulse_response_stats(&mut core, 4096);
        assert!(
            (sum - expected_sum).abs() < 0.01,
            "tap amplitude sum should be ~{expected_sum:.3} (was 5.4 before the fix), got {sum:.3}"
        );
        assert!(
            (peak - expected_peak).abs() < 0.01,
            "largest echo should be ~{expected_peak:.3}, got {peak:.3}"
        );
    }

    /// Sustained-tone overload guard: with 6 hard walls, the steady-state
    /// reflected energy added on top of the direct signal must stay well below
    /// the direct level for a source at conversational distance. Before the
    /// fix the wet sum reached 6 × 0.9 = 5.4× the direct amplitude.
    #[test]
    fn sustained_tone_reflection_sum_is_bounded() {
        let mut core = ReflectionCore::new(0.81, 65536);
        core.update(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 10.0, 10.0),
            Vec3::new(5.0, 5.0, 5.0),
            Vec3::new(3.0, 5.0, 5.0), // 2 m direct
            SAMPLE_RATE,
            SPEED_OF_SOUND,
            1.0,
        );

        // DC input at 1.0: steady-state wet output = coherent sum of tap gains.
        // Hand-computed: 0.9·(2/8 + 2/12 + 4·2/√104) ≈ 1.08 — the worst-case
        // coherent sum sits near the direct level (energy sum Σg² ≈ 0.20 of
        // direct). Before the fix this was 6 × 0.9 = 5.4 (+15 dB overload).
        let mut last = 0.0f32;
        for _ in 0..8192 {
            last = core.process_sample(1.0);
        }
        assert!(
            (last - 1.08).abs() < 0.02,
            "steady-state reflection sum should be ~1.08 (was 5.4 before the fix), got {last:.2}"
        );
    }

    /// A tap's distance ratio never exceeds 1 even when the source hugs a wall
    /// (image distance barely exceeds the direct distance). Uses an asymmetric
    /// room so all 6 image distances (and thus tap delays) are distinct — the
    /// impulse peak then measures the single largest tap.
    #[test]
    fn near_wall_tap_gain_capped_at_wall_gain() {
        let mut core = ReflectionCore::new(1.0, 65536); // perfect mirror
        core.update(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 12.0, 14.0),
            Vec3::new(0.2, 5.0, 6.0), // 0.2 m from the -X wall
            Vec3::new(9.0, 5.0, 6.0), // direct 8.8 m; -X image 9.2 m
            SAMPLE_RATE,
            SPEED_OF_SOUND,
            1.0,
        );
        let (_, peak) = impulse_response_stats(&mut core, 4096);
        // Largest single tap: -X image at 9.2 m → 8.8/9.2 ≈ 0.957.
        assert!(
            peak <= 1.0 + 1e-6,
            "no tap may exceed the wall's amplitude gain, got {peak:.3}"
        );
        assert!(
            (peak - 8.8 / 9.2).abs() < 0.01,
            "near-wall tap should be ~{:.3}, got {peak:.3}",
            8.8 / 9.2
        );
    }
}
