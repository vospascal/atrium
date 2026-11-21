// Multiple Direction Amplitude Panning (Pulkki 1999): source spread by
// averaging VBAP over a fan of phantom directions.

use super::channel_gains::ChannelGains;
use super::layout::SpeakerLayout;
use super::source::{DistanceParams, SourceSpatial};
use crate::listener::Listener;
use crate::types::Vec3;

impl SpeakerLayout {
    /// Compute per-channel gains using MDAP (Pulkki 1999).
    ///
    /// When `spread > 0`, evaluates VBAP at 7 phantom directions spread
    /// around the true source azimuth. The gain vectors are averaged,
    /// producing a wider image. When `spread == 0`, delegates to plain VBAP.
    pub fn compute_gains_mdap(
        &self,
        listener: &Listener,
        source: &SourceSpatial,
        distance: &DistanceParams,
        spread: f32,
    ) -> ChannelGains {
        if spread <= 0.0 || self.count < 2 {
            return self.compute_gains_vbap(listener, source, distance);
        }

        // Fan angle: spread=1.0 → π radians (180° total arc), spread=0.5 → 90°
        let fan = spread * std::f32::consts::PI;
        const N_PHANTOM: usize = 7;

        let mut acc = ChannelGains::silent(self.total_channels);

        // Source direction from listener
        let d = source.position - listener.position;
        let base_dist = d.length();
        if base_dist < 1e-6 {
            return self.compute_gains_vbap(listener, source, distance);
        }

        for i in 0..N_PHANTOM {
            // Spread phantom directions evenly across [-fan/2, +fan/2]
            let t = (i as f32 / (N_PHANTOM - 1) as f32) - 0.5; // [-0.5, 0.5]
            let angle_offset = t * fan;

            // Rotate source position around listener by angle_offset (2D, Z-up)
            let cos_a = angle_offset.cos();
            let sin_a = angle_offset.sin();
            let phantom_pos = Vec3::new(
                listener.position.x + d.x * cos_a - d.y * sin_a,
                listener.position.y + d.x * sin_a + d.y * cos_a,
                source.position.z,
            );

            let phantom = SourceSpatial {
                position: phantom_pos,
                ..*source
            };
            let phantom_gains = self.compute_gains_vbap(listener, &phantom, distance);

            for ch in 0..self.total_channels {
                acc.gains[ch] += phantom_gains.gains[ch];
            }
        }

        // Average and re-normalize for constant power
        let inv_n = 1.0 / N_PHANTOM as f32;
        let mut power = 0.0f32;
        for ch in 0..self.total_channels {
            acc.gains[ch] *= inv_n;
            power += acc.gains[ch] * acc.gains[ch];
        }

        // Re-normalize so total power matches a single VBAP evaluation
        if power > 1e-12 {
            let ref_gains = self.compute_gains_vbap(listener, source, distance);
            let ref_power: f32 = ref_gains.gains[..self.total_channels]
                .iter()
                .map(|g| g * g)
                .sum();
            let scale = (ref_power / power).sqrt();
            for ch in 0..self.total_channels {
                acc.gains[ch] *= scale;
            }
        }

        acc
    }

    /// Compute VBAP gains with MDAP (Multiple Direction Amplitude Panning) support.
    /// When spread > 0, pans to multiple directions for a wider image;
    /// when spread == 0, falls through to standard VBAP.
    pub fn compute_gains_with_spread(
        &self,
        listener: &Listener,
        source: &SourceSpatial,
        distance: &DistanceParams,
        spread: f32,
    ) -> ChannelGains {
        self.compute_gains_mdap(listener, source, distance, spread)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        default_distance, equidistant_quad, omni, total_power, vbap_5_1_layout,
    };
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn mdap_spread_activates_more_speakers() {
        let layout = vbap_5_1_layout();
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), PI / 2.0); // facing +Y
        let dist = default_distance();
        let src = omni(Vec3::new(3.0, 4.0, 0.0), Vec3::new(0.0, 1.0, 0.0));

        let point_gains = layout.compute_gains_mdap(&listener, &src, &dist, 0.0);
        let spread_gains = layout.compute_gains_mdap(&listener, &src, &dist, 0.5);

        // With spread, more speakers should have nonzero gain
        let point_active = point_gains.gains[..6]
            .iter()
            .filter(|g| g.abs() > 0.01)
            .count();
        let spread_active = spread_gains.gains[..6]
            .iter()
            .filter(|g| g.abs() > 0.01)
            .count();
        assert!(
            spread_active >= point_active,
            "spread should activate >= as many speakers: point={point_active} spread={spread_active}"
        );
    }

    #[test]
    fn mdap_preserves_power() {
        let layout = vbap_5_1_layout();
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), PI / 2.0);
        let dist = default_distance();
        let src = omni(Vec3::new(3.0, 4.0, 0.0), Vec3::new(0.0, 1.0, 0.0));

        let point_gains = layout.compute_gains_mdap(&listener, &src, &dist, 0.0);
        let spread_gains = layout.compute_gains_mdap(&listener, &src, &dist, 0.5);

        let point_power: f32 = point_gains.gains[..6].iter().map(|g| g * g).sum();
        let spread_power: f32 = spread_gains.gains[..6].iter().map(|g| g * g).sum();
        assert!(
            (point_power - spread_power).abs() < 0.05,
            "MDAP should preserve power: point={point_power} spread={spread_power}"
        );
    }

    // ── Hand-derived VBAP / layout values ─────────────────────────────

    #[test]
    fn mdap_spread_is_mirror_symmetric_for_centred_source() {
        // Phantoms at ±fan/2 are placed symmetrically, so a source dead ahead
        // stays centred: FL = FR, RL = RR, and power equals the point source.
        let layout = equidistant_quad();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let source = omni(Vec3::new(2.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0));
        let point = layout.compute_gains_mdap(&listener, &source, &default_distance(), 0.0);
        let spread = layout.compute_gains_mdap(&listener, &source, &default_distance(), 0.8);
        assert!((spread.gains[0] - spread.gains[1]).abs() < 1e-5);
        assert!((spread.gains[4] - spread.gains[5]).abs() < 1e-5);
        assert!(
            spread.gains[4] > 0.0,
            "spread 0.8 (±72°) must reach the rear pair"
        );
        assert!((total_power(&spread) - total_power(&point)).abs() < 1e-5);
    }
}
