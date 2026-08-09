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

/// Integer hash of a 3D lattice point using the complete persisted world seed.
///
/// The legacy [`hash_3d`] API remains for the fixed-size fixture world. New
/// generation must use this variant so seeds that differ only in their upper
/// 32 bits do not collapse to the same world.
pub fn hash_3d_u64(x: i32, y: i32, z: i32, seed: u64) -> u64 {
    let mut hash = seed
        ^ (x as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as i64 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ (z as i64 as u64).wrapping_mul(0x94D0_49BB_1331_11EB);
    hash ^= hash >> 30;
    hash = hash.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    hash ^= hash >> 27;
    hash = hash.wrapping_mul(0x94D0_49BB_1331_11EB);
    hash ^ (hash >> 31)
}

/// Map a hash to a uniform value in `[0, 1)`.
pub fn hash_to_unit(hash: u32) -> f32 {
    (hash >> 8) as f32 / 16_777_216.0
}

/// Map a 64-bit hash to a uniform value in `[0, 1)`.
pub fn hash_to_unit_u64(hash: u64) -> f32 {
    (hash >> 40) as f32 / 16_777_216.0
}

/// A uniform `[0, 1)` value from one 21-bit window of an already-avalanched
/// hash. Three windows fit in a `u64`, which is what lets a single hash supply a
/// 3D feature point or several independent per-voxel decisions.
pub fn hash_window_to_unit(hash: u64, shift: u32) -> f32 {
    const WINDOW_MASK: u64 = (1 << 21) - 1;
    ((hash >> shift) & WINDOW_MASK) as f32 / (WINDOW_MASK + 1) as f32
}

/// A deterministic `[0, 1)` roll for one world coordinate and purpose.
///
/// The scatter passes need "does a mushroom stand here" thousands of times per
/// chunk, and each roll must depend on the world coordinate alone so generation
/// order cannot change the answer. `salt` separates independent decisions at the
/// same coordinate.
pub fn coordinate_roll(x: i32, y: i32, z: i32, seed: u64, salt: u64) -> f32 {
    hash_to_unit_u64(hash_3d_u64(x, y, z, seed ^ salt))
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

fn lattice_value_3d(x: i32, y: i32, z: i32, seed: u64) -> f32 {
    hash_to_unit_u64(hash_3d_u64(x, y, z, seed))
}

/// Smoothly interpolated 3D value noise in `[0, 1)`.
pub fn value_noise_3d(x: f32, y: f32, z: f32, seed: u64) -> f32 {
    let x_floor = x.floor();
    let y_floor = y.floor();
    let z_floor = z.floor();
    let x0 = x_floor as i32;
    let y0 = y_floor as i32;
    let z0 = z_floor as i32;
    let tx = smooth(x - x_floor);
    let ty = smooth(y - y_floor);
    let tz = smooth(z - z_floor);

    let value = |dx: i32, dy: i32, dz: i32| lattice_value_3d(x0 + dx, y0 + dy, z0 + dz, seed);
    let x00 = value(0, 0, 0) + (value(1, 0, 0) - value(0, 0, 0)) * tx;
    let x10 = value(0, 1, 0) + (value(1, 1, 0) - value(0, 1, 0)) * tx;
    let x01 = value(0, 0, 1) + (value(1, 0, 1) - value(0, 0, 1)) * tx;
    let x11 = value(0, 1, 1) + (value(1, 1, 1) - value(0, 1, 1)) * tx;
    let y0 = x00 + (x10 - x00) * ty;
    let y1 = x01 + (x11 - x01) * ty;
    y0 + (y1 - y0) * tz
}

/// Cellular/Worley-style 3D noise in `[0, 1]`.
///
/// Each lattice cell contains one deterministic feature point. The returned
/// value is the distance to the nearest point, which can be combined with an
/// inverted Billow field to form rounded chambers with organic boundaries.
pub fn worley_noise_3d(x: f32, y: f32, z: f32, seed: u64) -> f32 {
    let cell_x = x.floor() as i32;
    let cell_y = y.floor() as i32;
    let cell_z = z.floor() as i32;
    let mut nearest_squared = f32::MAX;
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let lattice_x = cell_x + dx;
                let lattice_y = cell_y + dy;
                let lattice_z = cell_z + dz;
                // One avalanche hash per cell, sliced into three feature
                // coordinates. `hash_3d_u64` already mixes every input bit
                // across the whole word, so three independent 21-bit windows are
                // as good as three separate hashes and cost a third as much —
                // this runs 27 times per sample in the per-voxel cave loop.
                let hash = hash_3d_u64(lattice_x, lattice_y, lattice_z, seed);
                let feature_x = hash_window_to_unit(hash, 0);
                let feature_y = hash_window_to_unit(hash, 21);
                let feature_z = hash_window_to_unit(hash, 42);
                let offset_x = lattice_x as f32 + feature_x - x;
                let offset_y = lattice_y as f32 + feature_y - y;
                let offset_z = lattice_z as f32 + feature_z - z;
                nearest_squared = nearest_squared
                    .min(offset_x * offset_x + offset_y * offset_y + offset_z * offset_z);
            }
        }
    }
    nearest_squared.sqrt().min(1.0)
}

/// Billow noise built from 3D value noise. Absolute-value folding turns the
/// usual ridges into rounded bumps useful for uneven floors and ceilings. The
/// inversion makes the bump centres low and their surrounding ridges high.
pub fn billow_noise_3d(x: f32, y: f32, z: f32, octaves: u32, seed: u64) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut normalization = 0.0;
    for octave in 0..octaves {
        let value = value_noise_3d(
            x * frequency,
            y * frequency,
            z * frequency,
            seed.wrapping_add(u64::from(octave).wrapping_mul(0x9E37_79B9_7F4A_7C15)),
        );
        total += (1.0 - (value * 2.0 - 1.0).abs()) * amplitude;
        normalization += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    if normalization == 0.0 {
        0.0
    } else {
        total / normalization
    }
}

/// Fractal 2D value noise using the complete persisted world seed.
pub fn fractal_noise_2d_u64(x: f32, z: f32, octaves: u32, seed: u64) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut normalization = 0.0;
    for octave in 0..octaves {
        total += value_noise_3d(
            x * frequency,
            0.0,
            z * frequency,
            seed.wrapping_add(u64::from(octave)),
        ) * amplitude;
        normalization += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    // Zero octaves normalises to zero; returning 0.0 keeps the contract's
    // `[0, 1)` range instead of handing callers a NaN that an `as i32` cast
    // would silently turn into a plausible-looking surface height.
    if normalization == 0.0 {
        0.0
    } else {
        total / normalization
    }
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

    #[test]
    fn full_seed_noise_is_deterministic_and_uses_upper_bits() {
        let a = value_noise_3d(1.25, -2.5, 8.75, 7);
        let b = value_noise_3d(1.25, -2.5, 8.75, 7);
        let upper_bit_variant = value_noise_3d(1.25, -2.5, 8.75, 7 | (1_u64 << 48));
        assert_eq!(a, b);
        assert_ne!(a, upper_bit_variant);
        assert!((0.0..1.0).contains(&a));
        assert!((0.0..1.0).contains(&upper_bit_variant));
    }

    #[test]
    fn full_seed_fractal_noise_is_deterministic() {
        let value = fractal_noise_2d_u64(12.5, -3.25, 4, 42);
        assert_eq!(value, fractal_noise_2d_u64(12.5, -3.25, 4, 42));
        assert!((0.0..1.0).contains(&value));
    }

    #[test]
    fn volumetric_noises_are_deterministic_and_bounded() {
        let worley = worley_noise_3d(1.25, -2.5, 8.75, 42);
        let billow = billow_noise_3d(1.25, -2.5, 8.75, 4, 42);
        assert_eq!(worley, worley_noise_3d(1.25, -2.5, 8.75, 42));
        assert_eq!(billow, billow_noise_3d(1.25, -2.5, 8.75, 4, 42));
        assert!((0.0..=1.0).contains(&worley));
        assert!((0.0..=1.0).contains(&billow));
    }
}
