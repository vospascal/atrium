use std::ops::{Add, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn distance_to(self, other: Vec3) -> f32 {
        let d = self - other;
        (d.x * d.x + d.y * d.y + d.z * d.z).sqrt()
    }

    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    pub fn normalize(self) -> Vec3 {
        let len = self.length();
        if len < 1e-10 {
            Vec3::ZERO
        } else {
            self * (1.0 / len)
        }
    }

    pub fn dot(self, other: Vec3) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    /// Reflect this vector about a surface normal (assumes `normal` is unit length).
    /// Result: self - 2 * dot(self, normal) * normal
    pub fn reflect(self, normal: Vec3) -> Vec3 {
        self - normal * (2.0 * self.dot(normal))
    }

    pub fn cross(self, other: Vec3) -> Vec3 {
        Vec3::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, rhs: Vec3) -> Vec3 {
        Vec3::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, rhs: Vec3) -> Vec3 {
        Vec3::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

impl Neg for Vec3 {
    type Output = Vec3;
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_distance() {
        let a = Vec3::new(0.0, 0.0, 0.0);
        let b = Vec3::new(3.0, 4.0, 0.0);
        assert!((a.distance_to(b) - 5.0).abs() < 1e-5);
    }

    #[test]
    fn test_normalize() {
        let v = Vec3::new(3.0, 4.0, 0.0);
        let n = v.normalize();
        assert!((n.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn dot_perpendicular_is_zero() {
        let a = Vec3::new(1.0, 0.0, 0.0);
        let b = Vec3::new(0.0, 1.0, 0.0);
        assert!((a.dot(b)).abs() < 1e-6);
    }

    #[test]
    fn dot_parallel_unit_vectors_is_one() {
        let a = Vec3::new(1.0, 0.0, 0.0);
        assert!((a.dot(a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn dot_antiparallel_is_negative_one() {
        let a = Vec3::new(1.0, 0.0, 0.0);
        let b = Vec3::new(-1.0, 0.0, 0.0);
        assert!((a.dot(b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn reflect_45_degrees_off_wall() {
        let incoming = Vec3::new(1.0, -1.0, 0.0).normalize();
        let normal = Vec3::new(0.0, 1.0, 0.0);
        let reflected = incoming.reflect(normal);
        let expected = Vec3::new(1.0, 1.0, 0.0).normalize();
        assert!((reflected.x - expected.x).abs() < 1e-5);
        assert!((reflected.y - expected.y).abs() < 1e-5);
    }

    #[test]
    fn reflect_head_on() {
        let incoming = Vec3::new(-1.0, 0.0, 0.0);
        let normal = Vec3::new(1.0, 0.0, 0.0);
        let reflected = incoming.reflect(normal);
        assert!((reflected.x - 1.0).abs() < 1e-5);
        assert!(reflected.y.abs() < 1e-5);
    }

    #[test]
    fn cross_product_basis_vectors() {
        let x = Vec3::new(1.0, 0.0, 0.0);
        let y = Vec3::new(0.0, 1.0, 0.0);
        let z = x.cross(y);
        assert!((z.x).abs() < 1e-6);
        assert!((z.y).abs() < 1e-6);
        assert!((z.z - 1.0).abs() < 1e-6);
    }

    #[test]
    fn neg_reverses_direction() {
        let v = Vec3::new(1.0, -2.0, 3.0);
        let n = -v;
        assert!((n.x + 1.0).abs() < 1e-6);
        assert!((n.y - 2.0).abs() < 1e-6);
        assert!((n.z + 3.0).abs() < 1e-6);
    }
}
