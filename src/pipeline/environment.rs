//! The acoustic environment: the box the scene lives in, what its six faces
//! are made of, the ground and the air.
//!
//! One value owns all of it, and every pipeline reads the same one. Before,
//! bounds, materials, ground and atmosphere travelled as separate fields, and
//! two renderers could disagree about the same room: the path pipelines took
//! the floor from the per-wall materials while WorldLocked took it from a
//! single reflectivity number.

use atrium_core::types::Vec3;

use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::propagation::GroundProperties;
use crate::pipeline::wall_material::WallMaterial;

/// Number of faces of the box.
pub const WALL_COUNT: usize = 6;

/// Index of the floor (−Z face). Faces are ordered −X, +X, −Y, +Y, −Z, +Z in
/// `Environment::wall_materials`, `Environment::image_sources` and
/// `PathContribution::wall_index`.
pub const FLOOR_WALL_INDEX: usize = 4;

/// The virtual acoustic space.
#[derive(Clone, Debug)]
pub struct Environment {
    /// Corner of the box with the smallest coordinates.
    pub min: Vec3,
    /// Corner of the box with the largest coordinates.
    pub max: Vec3,
    /// Surface material of each face, in the order −X, +X, −Y, +Y, −Z, +Z.
    pub wall_materials: [WallMaterial; WALL_COUNT],
    /// Ground properties for the ISO 9613-2 ground effect.
    pub ground: GroundProperties,
    /// Air temperature, humidity and pressure.
    pub atmosphere: AtmosphericParams,
}

impl Environment {
    /// Middle of the box.
    pub fn centre(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// First-order image sources: `source` mirrored across each face, in face
    /// order. The reflection off face `i` reaches any receiver as if it came
    /// straight from image `i`.
    pub fn image_sources(&self, source: Vec3) -> [Vec3; WALL_COUNT] {
        [
            Vec3::new(2.0 * self.min.x - source.x, source.y, source.z),
            Vec3::new(2.0 * self.max.x - source.x, source.y, source.z),
            Vec3::new(source.x, 2.0 * self.min.y - source.y, source.z),
            Vec3::new(source.x, 2.0 * self.max.y - source.y, source.z),
            Vec3::new(source.x, source.y, 2.0 * self.min.z - source.z),
            Vec3::new(source.x, source.y, 2.0 * self.max.z - source.z),
        ]
    }

    /// Broadband amplitude gain of the reflection off face `wall_index`.
    pub fn wall_reflection_gain(&self, wall_index: usize) -> f32 {
        self.wall_materials[wall_index].broadband_reflection_gain()
    }

    /// Whether the floor sends anything back. A floor that does is rendered
    /// as its own reflection; the ISO 9613-2 ground term describes that same
    /// bounce and only applies where the floor has no image.
    pub fn floor_reflects(&self) -> bool {
        self.wall_reflection_gain(FLOOR_WALL_INDEX) > 0.0
    }

    /// A box with no reflecting face, hard ground and default air.
    #[cfg(test)]
    pub(crate) fn anechoic(min: Vec3, max: Vec3) -> Self {
        Self {
            min,
            max,
            wall_materials: std::array::from_fn(|_| WallMaterial::open()),
            ground: GroundProperties::hard(),
            atmosphere: AtmosphericParams::default(),
        }
    }

    /// A box whose six faces are all made of `material`, hard ground and
    /// default air.
    #[cfg(test)]
    pub(crate) fn uniform(min: Vec3, max: Vec3, material: WallMaterial) -> Self {
        Self {
            wall_materials: std::array::from_fn(|_| material.clone()),
            ..Self::anechoic(min, max)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Box (−6, −5, 0) … (6, 5, 4), source at (3, 2, 1). Mirroring across a
    /// face at coordinate c maps x to 2c − x:
    ///   −X: 2·(−6) − 3 = −15    +X: 2·6 − 3 = 9
    ///   −Y: 2·(−5) − 2 = −12    +Y: 2·5 − 2 = 8
    ///   −Z: 2·0 − 1 = −1        +Z: 2·4 − 1 = 7
    #[test]
    fn image_sources_mirror_the_source_across_each_face() {
        let environment =
            Environment::anechoic(Vec3::new(-6.0, -5.0, 0.0), Vec3::new(6.0, 5.0, 4.0));
        let images = environment.image_sources(Vec3::new(3.0, 2.0, 1.0));
        let expected = [
            Vec3::new(-15.0, 2.0, 1.0),
            Vec3::new(9.0, 2.0, 1.0),
            Vec3::new(3.0, -12.0, 1.0),
            Vec3::new(3.0, 8.0, 1.0),
            Vec3::new(3.0, 2.0, -1.0),
            Vec3::new(3.0, 2.0, 7.0),
        ];
        for (image, expected) in images.iter().zip(expected) {
            assert!(image.distance_to(expected) < 1e-6, "{image:?}");
        }
        assert!(environment.centre().distance_to(Vec3::new(0.0, 0.0, 2.0)) < 1e-6);
    }

    /// Each image is as far behind its face as the source is in front of it,
    /// so a point on the face is equally far from both.
    #[test]
    fn a_point_on_a_face_is_equidistant_from_source_and_image() {
        let environment =
            Environment::anechoic(Vec3::new(-6.0, -5.0, 0.0), Vec3::new(6.0, 5.0, 4.0));
        let source = Vec3::new(3.0, 2.0, 1.0);
        let images = environment.image_sources(source);
        let on_the_floor = Vec3::new(-2.0, 4.0, 0.0);
        let to_source = on_the_floor.distance_to(source);
        let to_image = on_the_floor.distance_to(images[FLOOR_WALL_INDEX]);
        assert!((to_source - to_image).abs() < 1e-6);
    }

    /// Stone: α = 0.02, 0.03, 0.03, 0.04, 0.05 over 250 Hz–4 kHz, mean 0.034,
    /// so reflectivity 0.966 and amplitude gain √0.966 = 0.98285. An open face
    /// absorbs everything.
    #[test]
    fn the_floor_reflects_when_its_material_does() {
        let bounds = (Vec3::ZERO, Vec3::new(4.0, 4.0, 3.0));
        let stone = Environment::uniform(bounds.0, bounds.1, WallMaterial::stone());
        assert!((stone.wall_reflection_gain(FLOOR_WALL_INDEX) - 0.98285).abs() < 1e-4);
        assert!(stone.floor_reflects());
        assert!(!Environment::anechoic(bounds.0, bounds.1).floor_reflects());
    }
}
