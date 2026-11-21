// VBAP gain lookup table: panning gains pre-computed per degree of azimuth.

use super::channel_gains::{ChannelGains, MAX_CHANNELS};
use super::layout::SpeakerLayout;
use super::vbap::angle_diff;
use crate::listener::Listener;
use crate::types::Vec3;

const LUT_SIZE: usize = 360;

/// Pre-computed VBAP gains at 1° azimuth resolution (Shukla 2019).
///
/// Eliminates per-source speaker-pair search + matrix inversion on the audio
/// thread. Build once per buffer (or when listener moves), then index by
/// azimuth angle. With the path architecture (7 VBAP evaluations per source),
/// this is ~7× fewer trig + linear-algebra ops per source.
///
/// 1° resolution matches Shukla's implementation on Bela Mini (ARM Cortex-A8),
/// where VBAP outperformed FOA for both ITD error (150 vs 202 μs) and ILD
/// error (3.18 vs 4.69 dB).
pub struct VbapLookup {
    entries: Vec<ChannelGains>,
    total_channels: usize,
    /// Listener state when this LUT was last computed.
    cached_position: Vec3,
    cached_yaw: f32,
}

impl VbapLookup {
    pub fn new(total_channels: usize) -> Self {
        Self {
            entries: vec![ChannelGains::silent(total_channels); LUT_SIZE],
            total_channels,
            cached_position: Vec3::new(f32::NAN, 0.0, 0.0), // force initial build
            cached_yaw: f32::NAN,
        }
    }

    /// Rebuild LUT if listener has moved. Returns true if rebuilt.
    pub fn update(&mut self, layout: &SpeakerLayout, listener: &Listener) -> bool {
        let pos_delta = (listener.position - self.cached_position).length();
        let yaw_delta = (listener.yaw - self.cached_yaw).abs();
        if pos_delta < 0.01 && yaw_delta < 0.001 {
            return false;
        }
        self.build(layout, listener);
        true
    }

    /// Look up pre-computed VBAP gains for a world-space direction vector.
    /// Transforms to listener-local frame, then linearly interpolates
    /// between the two nearest 1° LUT entries.
    pub fn lookup(&self, direction: Vec3) -> ChannelGains {
        // Transform world-space direction to listener-local frame
        let cos_y = self.cached_yaw.cos();
        let sin_y = self.cached_yaw.sin();
        let local_x = direction.x * cos_y + direction.y * sin_y;
        let local_y = -direction.x * sin_y + direction.y * cos_y;
        let azimuth = local_y.atan2(local_x);
        let deg = azimuth.to_degrees().rem_euclid(360.0);
        let idx = deg as usize;
        let frac = deg - idx as f32;

        let idx_a = idx % LUT_SIZE;
        let idx_b = (idx + 1) % LUT_SIZE;

        let a = &self.entries[idx_a];
        let b = &self.entries[idx_b];

        let mut result = ChannelGains::silent(self.total_channels);
        for ch in 0..self.total_channels {
            result.gains[ch] = a.gains[ch] * (1.0 - frac) + b.gains[ch] * frac;
        }
        result
    }

    fn build(&mut self, layout: &SpeakerLayout, listener: &Listener) {
        // Pre-compute speaker angles relative to listener (done once)
        let count = layout.speaker_count();
        let mut speaker_angles: [(f32, usize); MAX_CHANNELS] = [(0.0, 0); MAX_CHANNELS];
        let cos_y = listener.yaw.cos();
        let sin_y = listener.yaw.sin();

        for (i, speaker) in layout.speakers().iter().enumerate().take(count) {
            let sp = speaker.position - listener.position;
            let local_x = sp.x * cos_y + sp.y * sin_y;
            let local_y = -sp.x * sin_y + sp.y * cos_y;
            speaker_angles[i] = (local_y.atan2(local_x), i);
        }
        // Sort by angle
        for i in 1..count {
            let key = speaker_angles[i];
            let mut j = i;
            while j > 0 && speaker_angles[j - 1].0 > key.0 {
                speaker_angles[j] = speaker_angles[j - 1];
                j -= 1;
            }
            speaker_angles[j] = key;
        }

        // Pre-compute per-speaker distances for distance compensation
        let mut speaker_dists = [0.0f32; MAX_CHANNELS];
        for (i, dist) in speaker_dists.iter_mut().enumerate().take(count) {
            *dist = (layout.speakers()[i].position - listener.position)
                .length()
                .max(0.1);
        }
        let farthest = layout.farthest_speaker_distance(listener);

        // Build LUT: for each degree, find straddling pair and compute gains
        for deg in 0..LUT_SIZE {
            let azimuth = (deg as f32).to_radians();
            let sx = azimuth.cos();
            let sy = azimuth.sin();

            let mut best_a = 0usize;
            let mut best_b = 0usize;
            let mut best_ga = 0.0f32;
            let mut best_gb = 0.0f32;
            let mut found = false;

            for pair_idx in 0..count {
                let idx_a = pair_idx;
                let idx_b = (pair_idx + 1) % count;
                let (angle_a, si_a) = speaker_angles[idx_a];
                let (angle_b, si_b) = speaker_angles[idx_b];

                let (ax, ay) = (angle_a.cos(), angle_a.sin());
                let (bx, by) = (angle_b.cos(), angle_b.sin());

                let det = ax * by - bx * ay;
                if det.abs() < 1e-8 {
                    continue;
                }
                let inv_det = 1.0 / det;

                let ga = (by * sx - bx * sy) * inv_det;
                let gb = (-ay * sx + ax * sy) * inv_det;

                if ga >= -1e-6 && gb >= -1e-6 {
                    found = true;
                    best_a = si_a;
                    best_b = si_b;
                    best_ga = ga.max(0.0);
                    best_gb = gb.max(0.0);
                    break;
                }
            }

            if !found {
                // Fallback: nearest speaker
                let mut min_diff = f32::MAX;
                for &(angle, idx) in speaker_angles.iter().take(count) {
                    let diff = angle_diff(azimuth, angle).abs();
                    if diff < min_diff {
                        min_diff = diff;
                        best_a = idx;
                        best_ga = 1.0;
                        best_gb = 0.0;
                    }
                }
                best_b = best_a;
            }

            // Normalize for constant power
            let norm = (best_ga * best_ga + best_gb * best_gb).sqrt();
            if norm > 1e-8 {
                best_ga /= norm;
                best_gb /= norm;
            }

            // Distance compensation (same as compute_gains_vbap)
            best_ga *= speaker_dists[best_a] / farthest;
            if best_b != best_a {
                best_gb *= speaker_dists[best_b] / farthest;
            }

            let mut entry = ChannelGains::silent(layout.total_channels());
            entry.gains[layout.speakers()[best_a].channel] = best_ga;
            if best_b != best_a {
                entry.gains[layout.speakers()[best_b].channel] = best_gb;
            }
            self.entries[deg] = entry;
        }

        self.cached_position = listener.position;
        self.cached_yaw = listener.yaw;
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{arrival_power, room_layout, room_listener, world_direction};
    use super::*;

    #[test]
    fn vbap_lut_matches_live_panning() {
        let layout = room_layout();
        let listener = room_listener(); // yaw=π/2, so local != world frame

        let mut lut = VbapLookup::new(layout.total_channels());
        lut.update(&layout, &listener);

        // Test every 15° around the listener using world-space direction vectors
        let channels = layout.total_channels();
        for deg in (0..360).step_by(15) {
            let rad = (deg as f32).to_radians();
            let direction = Vec3::new(rad.cos(), rad.sin(), 0.0);

            let lut_gains = lut.lookup(direction);
            let live_gains = layout.compute_vbap_panning(&listener, direction);

            for ch in 0..channels {
                let diff = (lut_gains.gains[ch] - live_gains.gains[ch]).abs();
                assert!(
                    diff < 0.05,
                    "LUT vs live mismatch at {}°, ch{}: lut={:.4} live={:.4} (diff={:.4})",
                    deg,
                    ch,
                    lut_gains.gains[ch],
                    live_gains.gains[ch],
                    diff,
                );
            }
        }
    }

    // ── Hand-derived VBAP / layout values ─────────────────────────────

    #[test]
    fn vbap_lookup_matches_live_at_every_degree() {
        // LUT entries are exact VBAP at integer local azimuths; the lookup must
        // reproduce live panning there, and keep the arrival power at 1/13
        // between entries (linear interpolation 1° apart dips < 0.1 %).
        let layout = room_layout();
        let listener = room_listener();
        let mut lookup = VbapLookup::new(layout.total_channels());
        assert!(lookup.update(&layout, &listener));
        assert!(
            !lookup.update(&layout, &listener),
            "unchanged listener must not rebuild"
        );
        for degrees in 0..360 {
            let azimuth = (degrees as f32).to_radians();
            let direction = world_direction(&listener, azimuth);
            let from_lookup = lookup.lookup(direction);
            let live = layout.compute_vbap_panning(&listener, direction);
            for channel in 0..6 {
                assert!(
                    (from_lookup.gains[channel] - live.gains[channel]).abs() < 1e-3,
                    "{degrees}° ch{channel}: lookup {} live {}",
                    from_lookup.gains[channel],
                    live.gains[channel]
                );
            }
            let between = lookup.lookup(world_direction(&listener, azimuth + 0.5_f32.to_radians()));
            let between_power = arrival_power(&layout, &listener, &between);
            assert!((between_power * 13.0 - 1.0).abs() < 1e-2);
        }
    }
}
