// Room geometry and spatial structure.

use crate::types::Vec3;

/// Trait for room geometry.
pub trait Room: Send {
    fn bounds(&self) -> (Vec3, Vec3);
    fn contains(&self, point: Vec3) -> bool;
}

/// A simple axis-aligned box room.
pub struct BoxRoom {
    pub min: Vec3,
    pub max: Vec3,
}

impl BoxRoom {
    pub fn new(width: f32, depth: f32, height: f32) -> Self {
        Self {
            min: Vec3::ZERO,
            max: Vec3::new(width, depth, height),
        }
    }
}

impl Room for BoxRoom {
    fn bounds(&self) -> (Vec3, Vec3) {
        (self.min, self.max)
    }

    fn contains(&self, point: Vec3) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
            && point.z >= self.min.z
            && point.z <= self.max.z
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_point_inside() {
        let room = BoxRoom::new(6.0, 4.0, 3.0);
        assert!(room.contains(Vec3::new(3.0, 2.0, 1.5)));
    }

    #[test]
    fn rejects_point_outside() {
        let room = BoxRoom::new(6.0, 4.0, 3.0);
        assert!(!room.contains(Vec3::new(7.0, 2.0, 1.5)));
    }

    #[test]
    fn bounds_correct() {
        let room = BoxRoom::new(6.0, 4.0, 3.0);
        let (min, max) = room.bounds();
        assert_eq!(min, Vec3::ZERO);
        assert_eq!(max, Vec3::new(6.0, 4.0, 3.0));
    }
}
