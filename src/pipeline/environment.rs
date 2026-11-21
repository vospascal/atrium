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
use crate::audio::turbulence::AirMotion;
use crate::pipeline::wall_material::WallMaterial;

/// Number of faces of the box.
pub const WALL_COUNT: usize = 6;

/// Index of the floor (−Z face). Faces are ordered −X, +X, −Y, +Y, −Z, +Z in
/// `Environment::wall_materials`, `Environment::image_sources` and
/// `PathContribution::wall_index`.
pub const FLOOR_WALL_INDEX: usize = 4;

/// An image closer than this to the receiver is not a usable reflection.
const NEAREST_IMAGE_METRES: f32 = 0.1;

/// A reflection that arrives less than this after the direct sound is part of
/// the direct sound.
const SHORTEST_REFLECTION_DELAY_SECONDS: f32 = 1e-6;

/// One first-order reflection, as the image source that produces it.
#[derive(Clone, Copy, Debug)]
pub struct WallReflection {
    /// The source mirrored across the face.
    pub image: Vec3,
    /// Length of the reflected path: image to receiver, in metres.
    pub distance: f32,
    /// How much later than the direct sound it arrives.
    pub delay_seconds: f32,
}

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
    /// The wind and the heat rising from the ground, which stir the air.
    pub air: AirMotion,
}

/// Index of the ceiling (+Z face).
pub const CEILING_WALL_INDEX: usize = 5;

impl Environment {
    /// Whether the place lies under the open sky: its ceiling reflects
    /// nothing. Only there do wind and sun stir the air a sound crosses.
    pub fn is_open_air(&self) -> bool {
        self.wall_materials[CEILING_WALL_INDEX].broadband_reflection_gain() <= 0.0
    }

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

    /// The first-order reflection off face `wall_index` on the way from
    /// `source` to `target`, or `None` when there is no separate reflection
    /// to render: the image is degenerate, or the bounce arrives together
    /// with the direct sound (a source or a receiver on the face itself is
    /// equally far from the other end directly and via its image).
    pub fn reflection_off(
        &self,
        wall_index: usize,
        source: Vec3,
        target: Vec3,
    ) -> Option<WallReflection> {
        let image = self.image_sources(source)[wall_index];
        let direct_distance = source.distance_to(target);
        let distance = image.distance_to(target);
        if distance < NEAREST_IMAGE_METRES || distance < direct_distance {
            return None;
        }
        let delay_seconds = (distance - direct_distance) / self.atmosphere.speed_of_sound();
        if delay_seconds < SHORTEST_REFLECTION_DELAY_SECONDS {
            return None;
        }
        Some(WallReflection {
            image,
            distance,
            delay_seconds,
        })
    }

    /// Whether the floor sends anything back.
    fn floor_reflects(&self) -> bool {
        self.wall_reflection_gain(FLOOR_WALL_INDEX) > 0.0
    }

    /// Whether the bounce off the floor between `source` and `target` is
    /// rendered as a reflection of its own. Where it is, the ISO 9613-2
    /// ground term, which describes that same bounce, must stay out of it.
    /// Where it is not (the floor absorbs, or one end stands on the floor so
    /// the bounce has no delay), the ground term is the only floor there is.
    pub fn floor_bounce_is_rendered(&self, source: Vec3, target: Vec3) -> bool {
        self.floor_reflects()
            && self
                .reflection_off(FLOOR_WALL_INDEX, source, target)
                .is_some()
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
            air: crate::audio::turbulence::AirMotion::STILL,
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

    /// Stone floor at z = 0, source and receiver 4 m apart.
    /// Both 1.2 m up: the floor image is at z = −1.2, √(4² + 2.4²) = 4.6648 m
    /// away, 0.6648 m longer than the direct 4 m → 0.6648 / 343.42 = 1.936 ms
    /// late. That bounce is rendered as a reflection.
    /// Receiver on the floor: image and source are equally far, no delay, no
    /// separate reflection, so the ground term must carry the floor.
    /// Same when the source is on the floor, and when the floor absorbs.
    #[test]
    fn floor_bounce_is_a_reflection_only_when_it_arrives_later() {
        let (min, max) = (Vec3::new(-10.0, -10.0, 0.0), Vec3::new(10.0, 10.0, 8.0));
        let stone = Environment::uniform(min, max, WallMaterial::stone());

        let raised_source = Vec3::new(-2.0, 0.0, 1.2);
        let raised_target = Vec3::new(2.0, 0.0, 1.2);
        let reflection = stone
            .reflection_off(FLOOR_WALL_INDEX, raised_source, raised_target)
            .expect("a raised pair has a delayed floor bounce");
        assert!(
            (reflection.distance - 4.6648).abs() < 1e-3,
            "{}",
            reflection.distance
        );
        assert!(
            (reflection.delay_seconds - 1.936e-3).abs() < 2e-6,
            "{}",
            reflection.delay_seconds
        );
        assert!(stone.floor_bounce_is_rendered(raised_source, raised_target));

        let target_on_floor = Vec3::new(2.0, 0.0, 0.0);
        let source_on_floor = Vec3::new(-2.0, 0.0, 0.0);
        assert!(!stone.floor_bounce_is_rendered(raised_source, target_on_floor));
        assert!(!stone.floor_bounce_is_rendered(source_on_floor, raised_target));
        assert!(!stone.floor_bounce_is_rendered(source_on_floor, target_on_floor));

        let open = Environment::anechoic(min, max);
        assert!(!open.floor_bounce_is_rendered(raised_source, raised_target));
    }
}
