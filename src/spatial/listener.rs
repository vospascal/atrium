use crate::world::types::Vec3;

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
}
