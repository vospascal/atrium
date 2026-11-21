//! Fixtures shared by the test modules of `ambisonics`.

use crate::listener::Listener;
use crate::speaker::{ChannelGains, Speaker};
use std::f32::consts::FRAC_PI_2;

pub(super) fn test_5_1_speakers() -> Vec<Speaker> {
    use crate::types::Vec3;
    vec![
        Speaker {
            position: Vec3::new(0.0, 4.0, 0.0),
            channel: 0,
        }, // FL
        Speaker {
            position: Vec3::new(6.0, 4.0, 0.0),
            channel: 1,
        }, // FR
        Speaker {
            position: Vec3::new(3.0, 4.0, 0.0),
            channel: 2,
        }, // C
        Speaker {
            position: Vec3::new(3.0, 2.0, 0.0),
            channel: 3,
        }, // LFE (at listener)
        Speaker {
            position: Vec3::new(0.0, 0.0, 0.0),
            channel: 4,
        }, // RL
        Speaker {
            position: Vec3::new(6.0, 0.0, 0.0),
            channel: 5,
        }, // RR
    ]
}

pub(super) fn test_listener() -> Listener {
    use crate::types::Vec3;
    Listener::new(Vec3::new(3.0, 2.0, 0.0), FRAC_PI_2) // facing +Y
}

pub(super) fn assert_close(actual: f32, expected: f32, tolerance: f32, label: &str) {
    assert!(
        (actual - expected).abs() < tolerance,
        "{label}: got {actual}, expected {expected} (±{tolerance})"
    );
}

pub(super) fn gains_for(decoded: &ChannelGains, channels: &[usize]) -> Vec<f32> {
    channels
        .iter()
        .map(|&channel| decoded.gains[channel])
        .collect()
}

pub(super) fn speaker_at(x: f32, y: f32, z: f32, channel: usize) -> Speaker {
    use crate::types::Vec3;
    Speaker {
        position: Vec3::new(x, y, z),
        channel,
    }
}

pub(super) fn origin_listener() -> Listener {
    Listener::new(crate::types::Vec3::ZERO, 0.0)
}
