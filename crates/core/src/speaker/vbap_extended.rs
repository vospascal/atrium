// Extended VBAP for a stereo pair (Gjørup et al.): polarity inversion places a
// phantom source beyond the physical speaker span.

use super::channel_gains::ChannelGains;
use super::layout::SpeakerLayout;
use super::vbap::angle_diff;
use crate::listener::Listener;
use crate::types::Vec3;

impl SpeakerLayout {
    /// Extended VBAP panning with stereo polarity inversion (Gjørup et al.).
    ///
    /// When a source falls outside the active speaker pair, standard VBAP
    /// snaps to the nearest speaker. Extended VBAP instead allows one gain
    /// to go negative (polarity inversion), creating a phantom source beyond
    /// the physical speaker span. Energy normalization `g/sqrt(g1²+g2²)` is
    /// preserved.
    ///
    /// **Stereo only.** With 3+ speakers forming a ring, sources are always
    /// inside some pair's span — extension never triggers. For 2 speakers,
    /// the extension is clamped to `max_extension` of the pair's angular span
    /// (paper validates up to 0.4, i.e., 40% beyond speaker span).
    ///
    /// # Parameters
    /// - `max_extension`: fraction of pair span to allow beyond speakers.
    ///   0.0 = standard VBAP, 0.4 = 40% extension (paper default).
    ///   Clamped to [0.0, 0.6] per paper's maximum evidence.
    pub fn compute_vbap_panning_extended(
        &self,
        listener: &Listener,
        direction: Vec3,
        max_extension: f32,
    ) -> ChannelGains {
        // Only meaningful for exactly 2 speakers (stereo).
        // For 3+ speakers, delegate to standard VBAP — source is always inside some pair.
        if self.count != 2 {
            return self.compute_vbap_panning(listener, direction);
        }

        let max_ext = max_extension.clamp(0.0, 0.6);
        let mut gains = ChannelGains::silent(self.total_channels);

        // Source azimuth in listener's local frame
        let source_azimuth = if direction.x * direction.x + direction.y * direction.y < 1e-10 {
            0.0
        } else {
            let cos_y = listener.yaw.cos();
            let sin_y = listener.yaw.sin();
            let local_x = direction.x * cos_y + direction.y * sin_y;
            let local_y = -direction.x * sin_y + direction.y * cos_y;
            local_y.atan2(local_x)
        };

        // Speaker azimuths
        let mut angles = [(0.0f32, 0usize); 2];
        for (i, speaker) in self.speakers.iter().enumerate().take(2) {
            let sp = speaker.position - listener.position;
            let cos_y = listener.yaw.cos();
            let sin_y = listener.yaw.sin();
            let local_x = sp.x * cos_y + sp.y * sin_y;
            let local_y = -sp.x * sin_y + sp.y * cos_y;
            angles[i] = (local_y.atan2(local_x), i);
        }
        // Sort so angles[0] < angles[1]
        if angles[0].0 > angles[1].0 {
            angles.swap(0, 1);
        }

        let (angle_a, si_a) = angles[0];
        let (angle_b, si_b) = angles[1];

        // Speaker pair span
        let pair_span = angle_b - angle_a;
        if pair_span.abs() < 1e-6 {
            // Degenerate: speakers at same angle
            gains.gains[self.speakers[si_a].channel] = 1.0;
            return gains;
        }

        // Solve g = L^{-1} * p (Gjørup Eq. 1)
        let (ax, ay) = (angle_a.cos(), angle_a.sin());
        let (bx, by) = (angle_b.cos(), angle_b.sin());
        let det = ax * by - bx * ay;
        if det.abs() < 1e-8 {
            gains.gains[self.speakers[si_a].channel] = 1.0;
            return gains;
        }
        // Limit the extension by angle, not by gain. The pair's arc is the
        // shorter arc between the speakers (where both VBAP gains are ≥ 0); its
        // centre bisects the two speaker vectors. The source may sit up to
        // `max_ext` of the arc width beyond either speaker: inside that the
        // exact (unclamped) Pulkki solution is used, and beyond it the panning
        // direction is held at the limit so the gains stop changing there.
        let arc_centre = (ay + by).atan2(ax + bx);
        let arc_half_width = 0.5 * (ax * bx + ay * by).clamp(-1.0, 1.0).acos();
        let offset_limit = arc_half_width * (1.0 + 2.0 * max_ext);
        let offset = angle_diff(source_azimuth, arc_centre).clamp(-offset_limit, offset_limit);
        let panned_azimuth = arc_centre + offset;

        let inv_det = 1.0 / det;
        let (sx, sy) = (panned_azimuth.cos(), panned_azimuth.sin());
        let mut ga = (by * sx - bx * sy) * inv_det;
        let mut gb = (-ay * sx + ax * sy) * inv_det;

        // Inside the pair both gains are non-negative; remove rounding noise
        // (this is also where a zero extension pins a source to the speaker).
        // Outside it, one gain is negative (polarity inversion) and already
        // bounded by the angular limit above.
        if ga >= -1e-6 && gb >= -1e-6 {
            ga = ga.max(0.0);
            gb = gb.max(0.0);
        }

        // Energy normalization (Gjørup Eq. 3): g / sqrt(ga² + gb²)
        let norm = (ga * ga + gb * gb).sqrt();
        if norm > 1e-8 {
            ga /= norm;
            gb /= norm;
        }

        gains.gains[self.speakers[si_a].channel] = ga;
        gains.gains[self.speakers[si_b].channel] = gb;

        gains
    }
}

#[cfg(test)]
mod tests {
    use super::super::layout::Speaker;
    use super::super::test_support::world_direction;
    use super::*;

    #[test]
    fn extended_vbap_inside_pair_matches_standard() {
        // Stereo speakers ahead-left and ahead-right. Source between them.
        let layout = SpeakerLayout::stereo(
            Vec3::new(-0.5, 1.0, 0.0), // left-forward
            Vec3::new(0.5, 1.0, 0.0),  // right-forward
        );
        let listener = Listener::new(Vec3::new(0.0, 0.0, 0.0), 0.0);

        // Source directly between speakers (slightly left of center).
        let dir = Vec3::new(-0.1, 1.0, 0.0);
        let standard = layout.compute_vbap_panning(&listener, dir);
        let extended = layout.compute_vbap_panning_extended(&listener, dir, 0.4);

        for ch in 0..2 {
            assert!(
                (standard.gains[ch] - extended.gains[ch]).abs() < 0.1,
                "ch{}: standard={} extended={} should be close",
                ch,
                standard.gains[ch],
                extended.gains[ch]
            );
        }
    }

    #[test]
    fn extended_vbap_at_speaker_gives_unity() {
        let layout = SpeakerLayout::stereo(Vec3::new(-1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0));
        let listener = Listener::new(Vec3::new(0.0, 0.0, 0.0), 0.0);

        // Direction exactly toward left speaker.
        let dir = Vec3::new(-1.0, 1.0, 0.0);
        let gains = layout.compute_vbap_panning_extended(&listener, dir, 0.4);

        // One gain should be ~1.0, other ~0.0.
        let max_g = gains.gains[0].abs().max(gains.gains[1].abs());
        assert!(
            max_g > 0.9,
            "at speaker, dominant gain should be near 1.0, got {}",
            max_g
        );
    }

    #[test]
    fn extended_vbap_beyond_speaker_has_negative_gain() {
        // Stereo: speakers at ±30° from forward (typical stereo).
        let layout = SpeakerLayout::stereo(
            Vec3::new(-0.5, 1.0, 0.0), // left-forward
            Vec3::new(0.5, 1.0, 0.0),  // right-forward
        );
        let listener = Listener::new(Vec3::new(0.0, 0.0, 0.0), 0.0);

        // Source well to the left (beyond left speaker span).
        let dir = Vec3::new(-1.0, 0.3, 0.0);
        let gains = layout.compute_vbap_panning_extended(&listener, dir, 0.4);

        // One gain should be negative (polarity inversion).
        let has_negative = gains.gains[0] < -0.01 || gains.gains[1] < -0.01;
        assert!(
            has_negative,
            "source beyond speaker should have a negative gain: L={} R={}",
            gains.gains[0], gains.gains[1]
        );
    }

    #[test]
    fn extended_vbap_energy_normalized() {
        let layout = SpeakerLayout::stereo(Vec3::new(-0.5, 1.0, 0.0), Vec3::new(0.5, 1.0, 0.0));
        let listener = Listener::new(Vec3::new(0.0, 0.0, 0.0), 0.0);

        // Test several directions, including ones beyond speaker span.
        let directions = [
            Vec3::new(1.0, 0.0, 0.0),  // ahead
            Vec3::new(-1.0, 0.3, 0.0), // far left
            Vec3::new(1.0, -0.5, 0.0), // far right
            Vec3::new(0.0, -1.0, 0.0), // behind
        ];

        for dir in &directions {
            let gains = layout.compute_vbap_panning_extended(&listener, *dir, 0.4);
            let energy = gains.gains[0] * gains.gains[0] + gains.gains[1] * gains.gains[1];
            assert!(
                (energy - 1.0).abs() < 0.15,
                "energy should be ≈ 1.0, got {} for dir {:?}",
                energy,
                dir
            );
        }
    }

    #[test]
    fn extended_vbap_zero_extension_matches_standard() {
        let layout = SpeakerLayout::stereo(Vec3::new(-0.5, 1.0, 0.0), Vec3::new(0.5, 1.0, 0.0));
        let listener = Listener::new(Vec3::new(0.0, 0.0, 0.0), 0.0);

        // Source beyond speaker span, but extension = 0 → should behave like standard.
        let dir = Vec3::new(-1.0, 0.3, 0.0);
        let gains = layout.compute_vbap_panning_extended(&listener, dir, 0.0);

        // With zero extension, no negative gains allowed.
        assert!(
            gains.gains[0] >= -0.01 && gains.gains[1] >= -0.01,
            "zero extension should not produce negative gains: L={} R={}",
            gains.gains[0],
            gains.gains[1]
        );
    }

    #[test]
    fn extended_vbap_3plus_speakers_delegates_to_standard() {
        // Quad layout (4 speakers) — extended should delegate to standard VBAP.
        let layout = SpeakerLayout::new(
            &[
                Speaker {
                    position: Vec3::new(-1.0, 1.0, 0.0),
                    channel: 0,
                },
                Speaker {
                    position: Vec3::new(1.0, 1.0, 0.0),
                    channel: 1,
                },
                Speaker {
                    position: Vec3::new(-1.0, -1.0, 0.0),
                    channel: 2,
                },
                Speaker {
                    position: Vec3::new(1.0, -1.0, 0.0),
                    channel: 3,
                },
            ],
            None,
            4,
        );
        let listener = Listener::new(Vec3::new(0.0, 0.0, 0.0), 0.0);
        let dir = Vec3::new(1.0, 0.0, 0.0);

        let standard = layout.compute_vbap_panning(&listener, dir);
        let extended = layout.compute_vbap_panning_extended(&listener, dir, 0.4);

        for ch in 0..4 {
            assert!(
                (standard.gains[ch] - extended.gains[ch]).abs() < 1e-6,
                "ch{}: 3+ speakers should delegate to standard VBAP",
                ch
            );
        }
    }

    // ── Hand-derived VBAP / layout values ─────────────────────────────

    /// The doc promises the extension is "clamped to `max_extension` of the
    /// pair's angular span". The old code clamped the *unnormalised* gain at
    /// −e/(1+e) — the tangent-law small-angle approximation of g_a/g_b, not a
    /// bound on the gain — so the limit engaged at 23.9 % extension, not 40 %.
    /// Stereo ±30° (span 60°), source at +48° (30 % beyond the left speaker,
    /// inside max_extension 0.4): exact VBAP gives
    /// g_right = −sin 18°/sin 60° = −0.356822, g_left = sin 78°/sin 60° = 1.129474,
    /// normalised (−0.301245, 0.953547). The old clamp at −0.285714 yielded
    /// (−0.245239, 0.969463).
    /// Experimental path, off by default (`extended_vbap: false`, no setter).
    #[test]
    fn extended_vbap_unclamped_inside_max_extension() {
        let half_span = 30.0_f32.to_radians();
        let layout = SpeakerLayout::stereo(
            Vec3::new(half_span.cos(), half_span.sin(), 0.0),
            Vec3::new(half_span.cos(), -half_span.sin(), 0.0),
        );
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let gains = layout.compute_vbap_panning_extended(
            &listener,
            world_direction(&listener, 48.0_f32.to_radians()),
            0.4,
        );
        assert!(
            (gains.gains[0] - 0.953_547).abs() < 1e-3,
            "L={}",
            gains.gains[0]
        );
        assert!(
            (gains.gains[1] + 0.301_245).abs() < 1e-3,
            "R={}",
            gains.gains[1]
        );
    }

    /// Beyond the allowed extension the gains are held at the limit. Stereo
    /// ±30°, max_extension 0.4 → limit 30° + 0.4·60° = 54°:
    /// g_left = sin 84°/sin 60° = 1.148377, g_right = −sin 24°/sin 60° = −0.469655,
    /// normalised (0.925584, −0.378543). Sources at 54°, 70° and 120° all give
    /// these gains; the mirror side gives them swapped.
    #[test]
    fn extended_vbap_holds_gains_at_extension_limit() {
        let half_span = 30.0_f32.to_radians();
        let layout = SpeakerLayout::stereo(
            Vec3::new(half_span.cos(), half_span.sin(), 0.0),
            Vec3::new(half_span.cos(), -half_span.sin(), 0.0),
        );
        let listener = Listener::new(Vec3::ZERO, 0.0);
        for degrees in [54.0_f32, 70.0, 120.0] {
            let left_side = layout.compute_vbap_panning_extended(
                &listener,
                world_direction(&listener, degrees.to_radians()),
                0.4,
            );
            assert!(
                (left_side.gains[0] - 0.925_584).abs() < 1e-3,
                "{degrees}°: L={}",
                left_side.gains[0]
            );
            assert!(
                (left_side.gains[1] + 0.378_543).abs() < 1e-3,
                "{degrees}°: R={}",
                left_side.gains[1]
            );
            let right_side = layout.compute_vbap_panning_extended(
                &listener,
                world_direction(&listener, (-degrees).to_radians()),
                0.4,
            );
            assert!((right_side.gains[0] - left_side.gains[1]).abs() < 1e-5);
            assert!((right_side.gains[1] - left_side.gains[0]).abs() < 1e-5);
        }
    }
}
