// Room geometry and spatial structure.
//
// References for future expansion:
//   - Ray casting: raytraced-audio (https://github.com/whoStoleMyCoffee/raytraced-audio)
//     uses persistent rays that bounce off geometry, sensing room properties emergently
//   - Physics-based propagation: audionimbus/Steam Audio for full scene geometry support
//   - Zone system: rooms become zones with blend regions (see idea.md §acoustic-zones)
//   - Materials: per-surface absorption coefficients, frequency-dependent reflection
//
// See REFERENCES.md for full list.

use super::types::Vec3;

/// Trait for room geometry.
/// Future: add cast_ray() for ray-traced reflections, surface_material_at() for materials.
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
