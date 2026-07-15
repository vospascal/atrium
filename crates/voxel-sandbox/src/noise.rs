//! Deterministic hash-based value noise.
//!
//! Hand-rolled (no `noise` crate dependency) so terrain generation is fully
//! deterministic per seed and trivially portable to the audio side later
//! (e.g. deriving reverb zones from the same heightmap).

/// Integer hash of a 3D lattice point. Deterministic per seed.
pub fn hash_3d(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut hash = seed.wrapping_mul(0x9E37_79B9)
        ^ (x as u32).wrapping_mul(0x85EB_CA6B)
        ^ (y as u32).wrapping_mul(0xC2B2_AE35)
        ^ (z as u32).wrapping_mul(0x27D4_EB2F);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2C1B_3C6D);
    hash ^= hash >> 12;
    hash = hash.wrapping_mul(0x297A_2D39);
    hash ^= hash >> 15;
    hash
}

/// Map a hash to a uniform value in `[0, 1)`.
pub fn hash_to_unit(hash: u32) -> f32 {
    (hash >> 8) as f32 / 16_777_216.0
}

fn lattice_value(x: i32, z: i32, seed: u32) -> f32 {
    hash_to_unit(hash_3d(x, 0, z, seed))
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Smoothly interpolated 2D value noise in `[0, 1)`.
pub fn value_noise_2d(x: f32, z: f32, seed: u32) -> f32 {
    let x_floor = x.floor();
    let z_floor = z.floor();
    let x0 = x_floor as i32;
    let z0 = z_floor as i32;
    let tx = smooth(x - x_floor);
    let tz = smooth(z - z_floor);

    let value_00 = lattice_value(x0, z0, seed);
    let value_10 = lattice_value(x0 + 1, z0, seed);
    let value_01 = lattice_value(x0, z0 + 1, seed);
    let value_11 = lattice_value(x0 + 1, z0 + 1, seed);

    let bottom = value_00 + (value_10 - value_00) * tx;
    let top = value_01 + (value_11 - value_01) * tx;
    bottom + (top - bottom) * tz
}

/// Fractal Brownian motion: `octaves` layers of value noise, each at double
/// frequency and half amplitude. Normalized to `[0, 1)`.
pub fn fractal_noise_2d(x: f32, z: f32, octaves: u32, seed: u32) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut normalization = 0.0;
    for octave in 0..octaves {
        total +=
            value_noise_2d(x * frequency, z * frequency, seed.wrapping_add(octave)) * amplitude;
        normalization += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    total / normalization
}

/// Hermite smoothstep between two edges (edges may be in either order).
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_deterministic_and_in_range() {
        for sample_index in 0..1000 {
            let x = sample_index as f32 * 0.173;
            let z = sample_index as f32 * 0.291;
            let value = fractal_noise_2d(x, z, 5, 42);
            assert!((0.0..1.0).contains(&value), "noise out of range: {value}");
            assert_eq!(value, fractal_noise_2d(x, z, 5, 42));
        }
    }
}
