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

use super::ray::{Ray, RayHit};
use super::types::Vec3;

/// Trait for room geometry.
pub trait Room: Send {
    fn bounds(&self) -> (Vec3, Vec3);
    fn contains(&self, point: Vec3) -> bool;

    /// Cast a ray against the room geometry.
    /// Returns the nearest intersection (hit point, inward normal, distance).
    /// The normal always faces the incoming ray: dot(direction, normal) < 0.
    fn cast_ray(&self, ray: &Ray) -> Option<RayHit>;
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

    /// AABB slab-method ray intersection.
    ///
    /// For rays inside the box (the common case — sources are inside the room),
    /// we find the first positive t where the ray exits through a wall.
    /// The returned normal points inward (toward room interior) so that
    /// direction.reflect(normal) gives the correct bounce direction.
    fn cast_ray(&self, ray: &Ray) -> Option<RayHit> {
        let origin = [ray.origin.x, ray.origin.y, ray.origin.z];
        let dir = [ray.direction.x, ray.direction.y, ray.direction.z];
        let min = [self.min.x, self.min.y, self.min.z];
        let max = [self.max.x, self.max.y, self.max.z];

        let epsilon = 1e-4;
        let mut best_t = f32::INFINITY;
        let mut best_axis: usize = 0;
        let mut best_normal_sign: f32 = -1.0;

        // For each axis, find where the ray hits the min and max planes.
        // For a ray inside the box, we want the nearest positive-t wall hit.
        for axis in 0..3 {
            if dir[axis].abs() < 1e-10 {
                continue; // parallel to this slab
            }

            let inv_d = 1.0 / dir[axis];

            // t where ray hits the min-side plane
            let t_min = (min[axis] - origin[axis]) * inv_d;
            // t where ray hits the max-side plane
            let t_max = (max[axis] - origin[axis]) * inv_d;

            // We want the positive t (wall ahead of the ray).
            // For a ray inside the box going in +dir[axis], t_max is positive.
            // For a ray going in -dir[axis], t_min is positive.
            for &(t, sign) in &[(t_min, 1.0_f32), (t_max, -1.0_f32)] {
                if t > epsilon && t < best_t {
                    // Verify the hit point is actually on the box face
                    // (within the bounds on the other two axes)
                    let mut on_face = true;
                    for other in 0..3 {
                        if other == axis {
                            continue;
                        }
                        let p = origin[other] + dir[other] * t;
                        if p < min[other] - epsilon || p > max[other] + epsilon {
                            on_face = false;
                            break;
                        }
                    }
                    if on_face {
                        best_t = t;
                        best_axis = axis;
                        // Normal points inward (opposite to ray direction on this axis)
                        best_normal_sign = sign;
                    }
                }
            }
        }

        if best_t == f32::INFINITY {
            return None;
        }

        let hit_point = ray.origin + ray.direction * best_t;
        let mut normal = Vec3::ZERO;
        match best_axis {
            0 => normal.x = best_normal_sign,
            1 => normal.y = best_normal_sign,
            2 => normal.z = best_normal_sign,
            _ => unreachable!(),
        }

        Some(RayHit {
            point: hit_point,
            normal,
            distance: best_t,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room_6x4x3() -> BoxRoom {
        BoxRoom::new(6.0, 4.0, 3.0)
    }

    #[test]
    fn ray_from_center_hits_plus_x_wall() {
        let room = room_6x4x3();
        let ray = Ray {
            origin: Vec3::new(3.0, 2.0, 1.5),
            direction: Vec3::new(1.0, 0.0, 0.0),
        };
        let hit = room.cast_ray(&ray).expect("should hit");
        assert!((hit.point.x - 6.0).abs() < 1e-3, "hit.x = {}", hit.point.x);
        assert!((hit.distance - 3.0).abs() < 1e-3, "dist = {}", hit.distance);
        // Normal should point inward (-X)
        assert!(hit.normal.x < 0.0, "normal.x = {}", hit.normal.x);
    }

    #[test]
    fn ray_from_center_hits_minus_x_wall() {
        let room = room_6x4x3();
        let ray = Ray {
            origin: Vec3::new(3.0, 2.0, 1.5),
            direction: Vec3::new(-1.0, 0.0, 0.0),
        };
        let hit = room.cast_ray(&ray).expect("should hit");
        assert!(hit.point.x.abs() < 1e-3, "hit.x = {}", hit.point.x);
        assert!((hit.distance - 3.0).abs() < 1e-3);
        // Normal should point inward (+X)
        assert!(hit.normal.x > 0.0);
    }

    #[test]
    fn ray_hits_each_of_six_walls() {
        let room = room_6x4x3();
        let center = Vec3::new(3.0, 2.0, 1.5);
        let directions = [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, -1.0),
        ];
        for dir in &directions {
            let ray = Ray {
                origin: center,
                direction: *dir,
            };
            let hit = room.cast_ray(&ray);
            assert!(hit.is_some(), "missed wall for dir {:?}", dir);
            let hit = hit.unwrap();
            assert!(hit.distance > 0.0, "negative distance for dir {:?}", dir);
        }
    }

    #[test]
    fn hit_normal_faces_incoming_ray() {
        let room = room_6x4x3();
        let center = Vec3::new(3.0, 2.0, 1.5);
        let directions = [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, -1.0),
        ];
        for dir in &directions {
            let ray = Ray {
                origin: center,
                direction: *dir,
            };
            let hit = room.cast_ray(&ray).unwrap();
            let dot = ray.direction.dot(hit.normal);
            assert!(
                dot < 0.0,
                "normal doesn't face ray for dir {:?}: dot={}",
                dir,
                dot
            );
        }
    }

    #[test]
    fn ray_at_angle_hits_correct_distance() {
        let room = room_6x4x3();
        // 45° in XY plane from center — should hit the +X wall at (6, 2+d, 1.5)
        let ray = Ray {
            origin: Vec3::new(3.0, 2.0, 1.5),
            direction: Vec3::new(1.0, 1.0, 0.0).normalize(),
        };
        let hit = room.cast_ray(&ray).expect("should hit");
        // The +Y wall is 2m away, +X wall is 3m away.
        // At 45°, we hit +Y wall first (at t = 2/sin(45°) = 2*sqrt(2) ≈ 2.83)
        // +X wall: t = 3/cos(45°) = 3*sqrt(2) ≈ 4.24
        assert!(hit.distance < 3.0, "dist = {}", hit.distance);
        assert!(
            (hit.point.y - 4.0).abs() < 1e-2,
            "hit.y = {}",
            hit.point.y
        );
    }

    #[test]
    fn reflect_direction_is_correct() {
        let room = room_6x4x3();
        let ray = Ray {
            origin: Vec3::new(3.0, 2.0, 1.5),
            direction: Vec3::new(1.0, 0.0, 0.0),
        };
        let hit = room.cast_ray(&ray).unwrap();
        let reflected = ray.direction.reflect(hit.normal);
        // Hitting +X wall with +X direction, normal is -X → reflect to -X
        assert!(reflected.x < -0.9, "reflected.x = {}", reflected.x);
    }

    #[test]
    fn ray_near_wall_still_hits() {
        let room = room_6x4x3();
        // Ray starting very close to +X wall, going toward it
        let ray = Ray {
            origin: Vec3::new(5.99, 2.0, 1.5),
            direction: Vec3::new(1.0, 0.0, 0.0),
        };
        let hit = room.cast_ray(&ray);
        assert!(hit.is_some(), "should hit even when close");
        assert!(hit.unwrap().distance < 0.02);
    }
}
