//! Fixtures shared by the test modules of `speaker`.

use super::{ChannelGains, DistanceParams, SourceSpatial, SpeakerLayout};
use crate::directivity::DirectivityPattern;
use crate::listener::Listener;
use crate::panner::DistanceModelType;
use crate::types::Vec3;
use std::f32::consts::PI;

pub(super) fn omni(pos: Vec3, dir: Vec3) -> SourceSpatial<'static> {
    static OMNI: DirectivityPattern = DirectivityPattern::Omni;
    SourceSpatial {
        position: pos,
        orientation: dir,
        directivity: &OMNI,
    }
}

pub(super) fn default_distance() -> DistanceParams {
    DistanceParams {
        ref_distance: 1.0,
        max_distance: 20.0,
        rolloff: 1.0,
        model: DistanceModelType::Inverse,
    }
}

pub(super) fn vbap_5_1_layout() -> SpeakerLayout {
    // Room 6×4m, speakers on walls at standard ITU angles
    let layout = SpeakerLayout::surround_5_1(
        Vec3::new(1.0, 3.5, 0.0), // L: front-left
        Vec3::new(5.0, 3.5, 0.0), // R: front-right
        Vec3::new(3.0, 4.0, 0.0), // C: front-center
        Vec3::new(0.5, 0.5, 0.0), // Ls: rear-left
        Vec3::new(5.5, 0.5, 0.0), // Rs: rear-right
    );
    layout
}

// ── Real room layout L/R verification ──────────────────────────────
//
// Tests use the exact speaker positions from main.rs to verify
// that audio L/R mapping is correct for a listener at room center
// facing the front wall (+Y).
//
// Room layout (6×4m), audience perspective (facing front wall):
//   FL(ch0) ── C(ch2) ── FR(ch1)   (front wall, y=4)
//   │                     │
//   │    listener (3,2)   │         facing +Y → left = -X, right = +X
//   │                     │
//   RL(ch4) ────────── RR(ch5)      (rear wall, y=0)
//
// FL should be at low-x (listener's left), FR at high-x (listener's right).

pub(super) fn room_layout() -> SpeakerLayout {
    // 6×4m room. Audience faces front wall (+Y). Left = -X, Right = +X.
    SpeakerLayout::surround_5_1(
        Vec3::new(0.0, 4.0, 0.0), // FL (ch 0): front-left  (low x = left)
        Vec3::new(6.0, 4.0, 0.0), // FR (ch 1): front-right (high x = right)
        Vec3::new(3.0, 4.0, 0.0), // C  (ch 2): front-center
        Vec3::new(0.0, 0.0, 0.0), // RL (ch 4): rear-left
        Vec3::new(6.0, 0.0, 0.0), // RR (ch 5): rear-right
    )
}

/// Listener at room center facing front wall (+Y). yaw=π/2.
pub(super) fn room_listener() -> Listener {
    Listener::new(Vec3::new(3.0, 2.0, 0.0), PI / 2.0)
}

/// Quad ring of radius 2 around the origin at ±45° / ±135°, listener at the
/// origin facing +X (yaw 0, left = +Y). Equidistant, so distance
/// compensation is a no-op and gains are pure Pulkki VBAP.
pub(super) fn equidistant_quad() -> SpeakerLayout {
    let offset = 2.0_f32.sqrt();
    SpeakerLayout::quad(
        Vec3::new(offset, offset, 0.0),   // FL ch0 at +45°
        Vec3::new(offset, -offset, 0.0),  // FR ch1 at −45°
        Vec3::new(-offset, offset, 0.0),  // RL ch4 at +135°
        Vec3::new(-offset, -offset, 0.0), // RR ch5 at −135°
    )
}

/// Power reaching the listener: each speaker's gain falls off as one over
/// its distance, so this is Σ(gain / distance)².
pub(super) fn arrival_power(
    layout: &SpeakerLayout,
    listener: &Listener,
    gains: &ChannelGains,
) -> f32 {
    layout
        .speakers()
        .iter()
        .map(|speaker| {
            let distance = (speaker.position - listener.position).length();
            let arrival = gains.gains[speaker.channel] / distance;
            arrival * arrival
        })
        .sum()
}

pub(super) fn total_power(gains: &ChannelGains) -> f32 {
    gains.gains.iter().map(|gain| gain * gain).sum()
}

/// World direction for a local azimuth (0 = ahead, + = left) of a listener.
pub(super) fn world_direction(listener: &Listener, local_azimuth: f32) -> Vec3 {
    let world_angle = listener.yaw + local_azimuth;
    Vec3::new(world_angle.cos(), world_angle.sin(), 0.0)
}
