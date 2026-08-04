//! Value noise: hash the eight cell corners and interpolate.

use super::hash::{ease, hash_cell};
use crate::pattern::MAX_NOISE_OCTAVES;

/// Value noise on the lattice: hash the eight corners, ease-interpolate.
///
/// Value rather than gradient (Perlin) noise: it needs one hash per corner
/// instead of a hash plus a dot product, has no zero-at-the-lattice artefact to
/// work around, and at the periods this stage uses — a few centimetres to a metre
/// — the difference in character is invisible against the voxel grid it sits on.
pub(crate) fn value_noise(point: [f32; 3], salt: u32) -> f32 {
    let base = [point[0].floor(), point[1].floor(), point[2].floor()];
    let cell = [base[0] as i32, base[1] as i32, base[2] as i32];
    let fraction = [
        ease(point[0] - base[0]),
        ease(point[1] - base[1]),
        ease(point[2] - base[2]),
    ];
    let mut accumulated = 0.0;
    for corner in 0..8 {
        let offset = [corner & 1, (corner >> 1) & 1, (corner >> 2) & 1];
        let weight = (0..3)
            .map(|axis| {
                if offset[axis] == 1 {
                    fraction[axis]
                } else {
                    1.0 - fraction[axis]
                }
            })
            .product::<f32>();
        let corner_cell = [
            cell[0] + offset[0],
            cell[1] + offset[1],
            cell[2] + offset[2],
        ];
        accumulated += weight * hash_cell(corner_cell, salt);
    }
    accumulated
}

/// Fractal value noise, normalised back into `0.0..1.0`.
///
/// Lacunarity 2, gain 0.5, and the sum divided by the amplitude total — so the
/// period always names the largest feature and the octave count changes the
/// texture without changing the contrast, which is what makes the octave slider
/// usable while the amount slider is set.
pub(crate) fn fractal_noise(point: [f32; 3], octaves: u32, salt_base: u32) -> f32 {
    let mut frequency = 1.0;
    let mut amplitude = 1.0;
    let mut total = 0.0;
    let mut normalisation = 0.0;
    for octave in 0..octaves.clamp(1, MAX_NOISE_OCTAVES) {
        let scaled = [
            point[0] * frequency,
            point[1] * frequency,
            point[2] * frequency,
        ];
        total += amplitude * value_noise(scaled, salt_base ^ octave);
        normalisation += amplitude;
        frequency *= 2.0;
        amplitude *= 0.5;
    }
    total / normalisation
}
