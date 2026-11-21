// 2D Vector Base Amplitude Panning (Pulkki 1997): the speaker pair straddling
// the source direction, constant-power gains, and per-speaker distance
// compensation.

use super::channel_gains::{ChannelGains, MAX_CHANNELS};
use super::layout::SpeakerLayout;
use super::source::{DistanceParams, SourceSpatial};
use crate::directivity::directivity_gain;
use crate::listener::Listener;
use crate::panner::distance_gain_at_model;
use crate::types::Vec3;

impl SpeakerLayout {
    /// Closest a speaker is ever treated as being to the listener, so that a
    /// listener standing on a speaker does not silence it.
    const MIN_SPEAKER_DISTANCE: f32 = 0.1;

    /// Distance from the listener to the farthest speaker in the layout.
    ///
    /// This is the reference for distance compensation: the farthest speaker
    /// plays at full gain and every closer one is turned down to match it.
    pub(super) fn farthest_speaker_distance(&self, listener: &Listener) -> f32 {
        self.speakers()
            .iter()
            .map(|speaker| (speaker.position - listener.position).length())
            .fold(Self::MIN_SPEAKER_DISTANCE, f32::max)
    }

    /// Gain that makes one speaker arrive at the listener as loud as the
    /// farthest speaker would: `distance / farthest`.
    ///
    /// The reference is the farthest speaker of the whole layout, not of the
    /// active pair, and the pair is deliberately not renormalised afterwards.
    /// Renormalising per pair cancels the compensation between pairs: with
    /// the centre speaker at 2 m and the others at 3.6 m, a source straight
    /// ahead arrived 5.1 dB louder than the same source at the side.
    fn arrival_compensation(
        &self,
        speaker_index: usize,
        listener: &Listener,
        farthest: f32,
    ) -> f32 {
        let distance = (self.speakers[speaker_index].position - listener.position)
            .length()
            .max(Self::MIN_SPEAKER_DISTANCE);
        distance / farthest
    }

    /// Compute per-channel gains using 2D VBAP.
    ///
    /// Finds the speaker pair straddling the source direction from the listener,
    /// then computes normalized gains. Distance from listener to source is applied
    /// as overall attenuation. Source directivity is applied along listener direction.
    pub fn compute_gains_vbap(
        &self,
        listener: &Listener,
        source: &SourceSpatial,
        distance: &DistanceParams,
    ) -> ChannelGains {
        let source_pos = source.position;
        let source_orientation = source.orientation;
        let source_directivity = source.directivity;
        let mut gains = ChannelGains::silent(self.total_channels);

        if self.count == 0 {
            return gains;
        }

        // Source azimuth in listener's local frame
        let d = source_pos - listener.position;
        let source_azimuth = if d.x * d.x + d.y * d.y < 1e-10 {
            0.0 // source at listener → treat as ahead
        } else {
            let cos_y = listener.yaw.cos();
            let sin_y = listener.yaw.sin();
            let local_x = d.x * cos_y + d.y * sin_y; // forward
            let local_y = -d.x * sin_y + d.y * cos_y; // left
            local_y.atan2(local_x)
        };

        // Compute speaker azimuths relative to listener and sort by angle
        let mut speaker_angles: [(f32, usize); MAX_CHANNELS] = [(0.0, 0); MAX_CHANNELS];
        for (i, speaker) in self.speakers.iter().enumerate().take(self.count) {
            let sp = speaker.position - listener.position;
            let cos_y = listener.yaw.cos();
            let sin_y = listener.yaw.sin();
            let local_x = sp.x * cos_y + sp.y * sin_y;
            let local_y = -sp.x * sin_y + sp.y * cos_y;
            speaker_angles[i] = (local_y.atan2(local_x), i);
        }
        // Sort by angle (insertion sort, N ≤ 8)
        for i in 1..self.count {
            let key = speaker_angles[i];
            let mut j = i;
            while j > 0 && speaker_angles[j - 1].0 > key.0 {
                speaker_angles[j] = speaker_angles[j - 1];
                j -= 1;
            }
            speaker_angles[j] = key;
        }

        // Find the speaker pair that straddles the source azimuth
        let mut best_a = 0usize;
        let mut best_b = 0usize;
        let mut best_ga = 0.0f32;
        let mut best_gb = 0.0f32;
        let mut found = false;

        for pair_idx in 0..self.count {
            let idx_a = pair_idx;
            let idx_b = (pair_idx + 1) % self.count;
            let (angle_a, si_a) = speaker_angles[idx_a];
            let (angle_b, si_b) = speaker_angles[idx_b];

            // Speaker direction unit vectors
            let (ax, ay) = (angle_a.cos(), angle_a.sin());
            let (bx, by) = (angle_b.cos(), angle_b.sin());

            // 2x2 inverse of [a, b] matrix
            let det = ax * by - bx * ay;
            if det.abs() < 1e-8 {
                continue; // collinear speakers
            }
            let inv_det = 1.0 / det;

            // Source direction unit vector
            let (sx, sy) = (source_azimuth.cos(), source_azimuth.sin());

            // g = inv([a,b]) * s
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
            // Fallback: assign to nearest speaker
            let mut min_diff = f32::MAX;
            for &(angle, idx) in speaker_angles.iter().take(self.count) {
                let diff = angle_diff(source_azimuth, angle).abs();
                if diff < min_diff {
                    min_diff = diff;
                    best_a = idx;
                    best_ga = 1.0;
                    best_gb = 0.0;
                }
            }
            best_b = best_a; // single speaker
        }

        // Normalize for constant power: g /= sqrt(ga² + gb²)
        let norm = (best_ga * best_ga + best_gb * best_gb).sqrt();
        if norm > 1e-8 {
            best_ga /= norm;
            best_gb /= norm;
        }

        // Per-speaker distance compensation: attenuate closer speakers so
        // every direction arrives at the listener at the same level.
        let farthest = self.farthest_speaker_distance(listener);
        best_ga *= self.arrival_compensation(best_a, listener, farthest);
        if best_b != best_a {
            best_gb *= self.arrival_compensation(best_b, listener, farthest);
        }

        // Distance attenuation from listener to source
        let dist = distance_gain_at_model(
            listener.position,
            source_pos,
            distance.ref_distance,
            distance.max_distance,
            distance.rolloff,
            distance.model,
        );

        // Source directivity toward listener
        let dir = directivity_gain(
            source_pos,
            source_orientation,
            listener.position,
            source_directivity,
        );

        let attenuation = dist * dir;
        gains.gains[self.speakers[best_a].channel] += best_ga * attenuation;
        if best_b != best_a {
            gains.gains[self.speakers[best_b].channel] += best_gb * attenuation;
        }

        gains
    }

    /// Compute angular VBAP panning gains only (no distance attenuation,
    /// no directivity, no hearing gain). Used by per-path rendering where
    /// each path carries its own gain factor.
    pub fn compute_vbap_panning(&self, listener: &Listener, direction: Vec3) -> ChannelGains {
        let mut gains = ChannelGains::silent(self.total_channels);

        if self.count == 0 {
            return gains;
        }

        // Source azimuth in listener's local frame from direction vector
        let source_azimuth = if direction.x * direction.x + direction.y * direction.y < 1e-10 {
            0.0
        } else {
            let cos_y = listener.yaw.cos();
            let sin_y = listener.yaw.sin();
            let local_x = direction.x * cos_y + direction.y * sin_y;
            let local_y = -direction.x * sin_y + direction.y * cos_y;
            local_y.atan2(local_x)
        };

        // Compute speaker azimuths relative to listener and sort by angle
        let mut speaker_angles: [(f32, usize); MAX_CHANNELS] = [(0.0, 0); MAX_CHANNELS];
        for (i, speaker) in self.speakers.iter().enumerate().take(self.count) {
            let sp = speaker.position - listener.position;
            let cos_y = listener.yaw.cos();
            let sin_y = listener.yaw.sin();
            let local_x = sp.x * cos_y + sp.y * sin_y;
            let local_y = -sp.x * sin_y + sp.y * cos_y;
            speaker_angles[i] = (local_y.atan2(local_x), i);
        }
        for i in 1..self.count {
            let key = speaker_angles[i];
            let mut j = i;
            while j > 0 && speaker_angles[j - 1].0 > key.0 {
                speaker_angles[j] = speaker_angles[j - 1];
                j -= 1;
            }
            speaker_angles[j] = key;
        }

        // Find the speaker pair that straddles the source azimuth
        let mut best_a = 0usize;
        let mut best_b = 0usize;
        let mut best_ga = 0.0f32;
        let mut best_gb = 0.0f32;
        let mut found = false;

        for pair_idx in 0..self.count {
            let idx_a = pair_idx;
            let idx_b = (pair_idx + 1) % self.count;
            let (angle_a, si_a) = speaker_angles[idx_a];
            let (angle_b, si_b) = speaker_angles[idx_b];

            let (ax, ay) = (angle_a.cos(), angle_a.sin());
            let (bx, by) = (angle_b.cos(), angle_b.sin());

            let det = ax * by - bx * ay;
            if det.abs() < 1e-8 {
                continue;
            }
            let inv_det = 1.0 / det;

            let (sx, sy) = (source_azimuth.cos(), source_azimuth.sin());

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
            let mut min_diff = f32::MAX;
            for &(angle, idx) in speaker_angles.iter().take(self.count) {
                let diff = angle_diff(source_azimuth, angle).abs();
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

        // Per-speaker distance compensation (same as compute_gains_vbap)
        let farthest = self.farthest_speaker_distance(listener);
        best_ga *= self.arrival_compensation(best_a, listener, farthest);
        if best_b != best_a {
            best_gb *= self.arrival_compensation(best_b, listener, farthest);
        }

        gains.gains[self.speakers[best_a].channel] += best_ga;
        if best_b != best_a {
            gains.gains[self.speakers[best_b].channel] += best_gb;
        }

        gains
    }
}

/// Shortest signed angular difference, normalized to [-π, π].
pub(super) fn angle_diff(a: f32, b: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let d = a - b;
    (d + PI).rem_euclid(TAU) - PI
}

#[cfg(test)]
mod tests;
