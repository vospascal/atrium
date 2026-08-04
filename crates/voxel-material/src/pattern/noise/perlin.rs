//! Perlin gradient noise.

use super::hash::{hash_u32, quintic};

/// One of the 12 edge-midpoint gradients, chosen by hash. Mirrors
/// `pattern_gradient`; shared by [`perlin_noise`] and [`simplex_noise`].
///
/// The `as u32` casts are the same two's-complement reinterpretation
/// [`hash_cell`] documents, and `%` on `u32` is the same operation in both
/// languages, so the chosen index agrees exactly.
pub(crate) fn gradient(cell: [i32; 3], salt: u32) -> [f32; 3] {
    let mixed = (cell[0] as u32).wrapping_mul(0x27d4_eb2d)
        ^ (cell[1] as u32).wrapping_mul(0x9e37_79b9)
        ^ (cell[2] as u32).wrapping_mul(0x85eb_ca6b)
        ^ salt.wrapping_mul(0xc2b2_ae35);
    let index = hash_u32(mixed) % 12;
    let axis = index / 4;
    let first = if index & 1 != 0 { -1.0 } else { 1.0 };
    let second = if index & 2 != 0 { -1.0 } else { 1.0 };
    match axis {
        0 => [first, second, 0.0],
        1 => [first, 0.0, second],
        _ => [0.0, first, second],
    }
}

/// Perlin gradient noise on the cubic lattice. Mirrors `pattern_perlin_noise`.
pub(crate) fn perlin_noise(point: [f32; 3], salt: u32) -> f32 {
    let base = [point[0].floor(), point[1].floor(), point[2].floor()];
    let cell = [base[0] as i32, base[1] as i32, base[2] as i32];
    let local = [point[0] - base[0], point[1] - base[1], point[2] - base[2]];
    let fade = [quintic(local[0]), quintic(local[1]), quintic(local[2])];
    let mut accumulated = 0.0;
    for corner in 0..8 {
        let offset = [corner & 1, (corner >> 1) & 1, (corner >> 2) & 1];
        let mut weight = 1.0;
        for axis in 0..3 {
            weight *= if offset[axis] == 1 {
                fade[axis]
            } else {
                1.0 - fade[axis]
            };
        }
        let corner_cell = [
            cell[0] + offset[0],
            cell[1] + offset[1],
            cell[2] + offset[2],
        ];
        let corner_gradient = gradient(corner_cell, salt);
        let mut dot = 0.0;
        for axis in 0..3 {
            dot += corner_gradient[axis] * (local[axis] - offset[axis] as f32);
        }
        accumulated += weight * dot;
    }
    (0.5 + 0.5 * accumulated).clamp(0.0, 1.0)
}
