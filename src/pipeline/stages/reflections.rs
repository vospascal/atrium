//! First-order reflections via image-source method (Allen & Berkley, 1979).
//!
//! `ReflectionCore` is the shared delay-buffer engine used by
//! WorldLockedRenderer (per-speaker reflections).

use crate::pipeline::environment::{Environment, FLOOR_WALL_INDEX};
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
/// distance (via `room_acoustics::delay_capacity_for_seconds`), with a minimum of 4096
/// samples (~85ms at 48kHz). Power-of-2 sizing enables fast wrapping via bitmask.
pub(crate) struct ReflectionCore {
    buffer: Box<[f32]>,
    capacity: usize,
    mask: usize,
    write_pos: usize,
    taps: [ReflectionTap; MAX_TAPS],
    tap_count: usize,
    /// Whether the last `update` produced an audible tap for the floor image.
    floor_tap_active: bool,
}

impl ReflectionCore {
    /// Minimum capacity: 4096 samples (~85ms at 48kHz).
    const MIN_CAPACITY: usize = 4096;

    pub(crate) fn new(capacity: usize) -> Self {
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
            floor_tap_active: false,
        }
    }

    /// True when the floor bounce is being rendered as its own delayed tap.
    ///
    /// It is not when the floor reflects nothing, or when the bounce arrives
    /// with the direct sound: a target standing on the floor is equally far
    /// from the source and from its floor image, so there is no delay to tap.
    pub(crate) fn has_floor_tap(&self) -> bool {
        self.floor_tap_active
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
    pub(crate) fn update(
        &mut self,
        environment: &Environment,
        source_pos: Vec3,
        target_pos: Vec3,
        sample_rate: f32,
        speed_of_sound: f32,
        ref_distance: f32,
    ) {
        let images = environment.image_sources(source_pos);

        let direct_dist = source_pos.distance_to(target_pos);
        let mut count = 0;

        self.floor_tap_active = false;

        for (wall_index, image) in images.iter().enumerate() {
            // Amplitude gain of this face, from its material.
            let amplitude_refl = environment.wall_reflection_gain(wall_index);
            if amplitude_refl <= 0.0 {
                // Nothing comes back from this face: no tap to read.
                continue;
            }
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
            if wall_index == FLOOR_WALL_INDEX {
                self.floor_tap_active = true;
            }
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
        self.floor_tap_active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::wall_material::WallMaterial;

    const SAMPLE_RATE: f32 = 48000.0;
    const SPEED_OF_SOUND: f32 = 343.42;

    /// A box whose six faces all have energy reflectivity `reflectivity`.
    fn room(min: Vec3, max: Vec3, reflectivity: f32) -> Environment {
        Environment::uniform(min, max, WallMaterial::with_reflectivity(reflectivity))
    }

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
    /// The floor follows its own material, not the walls'. Room 10 × 10 × 4,
    /// source (3, 5, 1), target (7, 5, 1): direct 4 m, floor image at
    /// (3, 5, −1) → √(4² + 2²) = √20 = 4.4721 m.
    /// Stone floor (reflectivity 0.966): tap gain √0.966 × 4/√20 = 0.87909,
    /// 0.4721 m late = 0.4721 / 343.42 × 48000 = 65.99 → 65 samples.
    /// Open floor: no floor tap at all, even though the walls still reflect.
    #[test]
    fn floor_tap_follows_the_floor_material() {
        use crate::pipeline::environment::FLOOR_WALL_INDEX;
        let floor_tap = |floor: WallMaterial| {
            let mut environment = room(Vec3::ZERO, Vec3::new(10.0, 10.0, 4.0), 0.81);
            environment.wall_materials[FLOOR_WALL_INDEX] = floor;
            let mut core = ReflectionCore::new(65536);
            core.update(
                &environment,
                Vec3::new(3.0, 5.0, 1.0),
                Vec3::new(7.0, 5.0, 1.0),
                48000.0,
                343.42,
                1.0,
            );
            let tap = core.taps[..core.tap_count]
                .iter()
                .find(|tap| tap.delay_samples == 65)
                .map(|tap| tap.gain);
            (core.has_floor_tap(), core.tap_count, tap)
        };

        let (has_floor_tap, tap_count, gain) = floor_tap(WallMaterial::stone());
        assert!(has_floor_tap);
        assert_eq!(tap_count, 6);
        let gain = gain.expect("the floor tap is 65 samples late");
        assert!((gain - 0.87909).abs() < 1e-4, "floor tap gain {gain}");

        let (has_floor_tap, tap_count, gain) = floor_tap(WallMaterial::open());
        assert!(!has_floor_tap, "an open floor sends nothing back");
        assert_eq!(tap_count, 5, "an absorbing face gets no tap");
        assert_eq!(gain, None);
    }

    #[test]
    fn taps_attenuate_with_image_distance() {
        // Energy reflectivity 0.81 → amplitude √0.81 = 0.9.
        let mut core = ReflectionCore::new(65536);
        let environment = room(Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 10.0, 10.0), 0.81);
        let source = Vec3::new(5.0, 5.0, 5.0);
        let target = Vec3::new(4.0, 5.0, 5.0);
        core.update(
            &environment,
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
        let mut core = ReflectionCore::new(65536);
        let environment = room(Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 10.0, 10.0), 0.81);
        core.update(
            &environment,
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
        let mut core = ReflectionCore::new(65536);
        let environment = room(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 12.0, 14.0),
            1.0, // perfect mirror
        );
        core.update(
            &environment,
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

    /// Unit impulse in, full impulse response out.
    fn impulse_response(core: &mut ReflectionCore, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|n| core.process_sample(if n == 0 { 1.0 } else { 0.0 }))
            .collect()
    }

    /// Image-source timing (Allen & Berkley): each tap is delayed by the path
    /// difference (image − direct)/c, because the direct path's own delay is
    /// applied upstream. Source (5,5,5), target (4,5,5) in a 10 m cube, c =
    /// 343.42 m/s, 48 kHz, perfect mirrors (√R = 1), ref 1 m:
    ///   −X image (−5,5,5): 9 m   → 8 m extra   → 1118.2 → sample 1118, gain 1/9
    ///   ±Y, ±Z images:     √101 m → 9.05 m extra → 1264.9 → sample 1264, gain 4/√101
    ///   +X image (15,5,5): 11 m  → 10 m extra  → 1397.7 → sample 1397, gain 1/11
    /// Delays truncate to whole samples (≤ 1 sample = 21 µs early).
    #[test]
    fn tap_delays_equal_path_difference_over_speed_of_sound() {
        let mut core = ReflectionCore::new(65536);
        let environment = room(Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 10.0, 10.0), 1.0);
        core.update(
            &environment,
            Vec3::new(5.0, 5.0, 5.0),
            Vec3::new(4.0, 5.0, 5.0),
            SAMPLE_RATE,
            SPEED_OF_SOUND,
            1.0,
        );
        let response = impulse_response(&mut core, 2048);
        let expected = [
            (1118, 1.0 / 9.0),
            (1264, 4.0 / 101.0f32.sqrt()),
            (1397, 1.0 / 11.0),
        ];
        for (sample, gain) in expected {
            assert!(
                (response[sample] - gain).abs() < 1e-5,
                "sample {sample}: expected echo {gain:.5}, got {:.5}",
                response[sample]
            );
        }
        let echo_count = response.iter().filter(|x| x.abs() > 1e-9).count();
        assert_eq!(echo_count, 3, "exactly three distinct arrival times");
    }

    /// Wall reflectivity is energy-domain: the tap amplitude carries √R, so
    /// halving the reflected energy lowers every echo by 1/√2 (−3.01 dB).
    #[test]
    fn tap_amplitude_is_square_root_of_energy_reflectivity() {
        let echo_energy = |reflectivity: f32| -> f32 {
            let mut core = ReflectionCore::new(65536);
            let environment = room(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(10.0, 10.0, 10.0),
                reflectivity,
            );
            core.update(
                &environment,
                Vec3::new(5.0, 5.0, 5.0),
                Vec3::new(4.0, 5.0, 5.0),
                SAMPLE_RATE,
                SPEED_OF_SOUND,
                1.0,
            );
            impulse_response(&mut core, 2048)
                .iter()
                .map(|x| x * x)
                .sum()
        };
        let ratio = echo_energy(0.5) / echo_energy(1.0);
        assert!(
            (ratio - 0.5).abs() < 1e-5,
            "echo energy must scale linearly with energy reflectivity, got {ratio}"
        );
    }

    /// Inside the reference distance the direct path is held at unity gain, so
    /// the relative tap gain is ref/image (the absolute inverse-law reflection
    /// level), not direct/image. Source 0.5 m from the target, ref 1 m, R = 0.64:
    /// −X image at 9.5 m → gain 0.8 × 1/9.5 = 0.08421, 9.0 m extra → sample 1257.
    #[test]
    fn tap_gain_uses_reference_distance_inside_it() {
        let mut core = ReflectionCore::new(65536);
        let environment = room(Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 10.0, 10.0), 0.64);
        core.update(
            &environment,
            Vec3::new(5.0, 5.0, 5.0),
            Vec3::new(4.5, 5.0, 5.0),
            SAMPLE_RATE,
            SPEED_OF_SOUND,
            1.0,
        );
        let response = impulse_response(&mut core, 2048);
        let expected_gain = 0.8 / 9.5;
        assert!(
            (response[1257] - expected_gain).abs() < 1e-5,
            "−X echo should be {expected_gain:.5}, got {:.5}",
            response[1257]
        );
    }
}
