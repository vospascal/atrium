use crate::types::Vec3;

#[derive(Clone, Copy, Debug)]
pub struct Listener {
    pub position: Vec3,
    /// Yaw in radians. 0 = facing +X, π/2 = facing +Y.
    pub yaw: f32,
}

impl Listener {
    pub fn new(position: Vec3, yaw: f32) -> Self {
        Self { position, yaw }
    }

    /// Unit vector of the listener's forward direction (derived from yaw).
    pub fn forward(&self) -> Vec3 {
        Vec3::new(self.yaw.cos(), self.yaw.sin(), 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn forward_at_zero_yaw_is_plus_x() {
        let l = Listener::new(Vec3::ZERO, 0.0);
        let f = l.forward();
        assert!((f.x - 1.0).abs() < 1e-6);
        assert!(f.y.abs() < 1e-6);
    }

    #[test]
    fn forward_at_90_degrees_is_plus_y() {
        let l = Listener::new(Vec3::ZERO, FRAC_PI_2);
        let f = l.forward();
        assert!(f.x.abs() < 1e-5);
        assert!((f.y - 1.0).abs() < 1e-5);
    }

    /// Yaw is a counter-clockwise rotation about +Z (z-up, right-handed):
    /// forward is unit length and horizontal for every yaw, π faces −X, and
    /// yaw wraps with period 2π.
    #[test]
    fn forward_is_horizontal_unit_vector_rotating_counter_clockwise() {
        for step in -16..=16 {
            let yaw = step as f32 * std::f32::consts::FRAC_PI_8;
            let forward = Listener::new(Vec3::ZERO, yaw).forward();
            assert!((forward.length() - 1.0).abs() < 1e-6);
            assert_eq!(forward.z, 0.0);
            let wrapped = Listener::new(Vec3::ZERO, yaw + std::f32::consts::TAU).forward();
            assert!((forward - wrapped).length() < 1e-5);
        }
        let behind = Listener::new(Vec3::ZERO, std::f32::consts::PI).forward();
        assert!((behind.x + 1.0).abs() < 1e-6 && behind.y.abs() < 1e-6);
        // Counter-clockwise: +X × forward(π/2) = +Z.
        let left = Listener::new(Vec3::ZERO, FRAC_PI_2).forward();
        assert!((Vec3::new(1.0, 0.0, 0.0).cross(left).z - 1.0).abs() < 1e-5);
    }

    /// A listener facing a source receives it on-axis with any directional
    /// reception pattern; turned away by π it gets the rear gain.
    #[test]
    fn forward_feeds_directivity_on_axis() {
        use crate::directivity::{directivity_gain, DirectivityPattern};
        let listener = Listener::new(Vec3::new(2.0, 3.0, 1.7), 0.6);
        let source = listener.position + listener.forward() * 5.0;
        let cardioid = DirectivityPattern::cardioid();
        let on_axis = directivity_gain(listener.position, listener.forward(), source, &cardioid);
        assert!((on_axis - 1.0).abs() < 1e-5, "{on_axis}");
        let turned = Listener::new(listener.position, 0.6 + std::f32::consts::PI);
        let behind = directivity_gain(turned.position, turned.forward(), source, &cardioid);
        assert!(behind < 1e-5, "{behind}");
    }
}
