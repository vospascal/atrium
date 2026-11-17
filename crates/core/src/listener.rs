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
}
