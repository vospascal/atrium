//! Simplex noise — the skewed-lattice variant, cheaper per octave than Perlin in 3D.

use super::perlin::gradient;

pub(crate) const SIMPLEX_SKEW: f32 = 0.333_333_33;

pub(crate) const SIMPLEX_UNSKEW: f32 = 0.166_666_67;

/// One simplex corner's contribution: the `(0.6 - r^2)^4` falloff.
/// Mirrors `pattern_simplex_corner`.
pub(crate) fn simplex_corner(offset: [f32; 3], cell: [i32; 3], salt: u32) -> f32 {
    let radius = offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2];
    let falloff = 0.6 - radius;
    if falloff <= 0.0 {
        return 0.0;
    }
    let squared = falloff * falloff;
    let corner_gradient = gradient(cell, salt);
    let dot = corner_gradient[0] * offset[0]
        + corner_gradient[1] * offset[1]
        + corner_gradient[2] * offset[2];
    squared * squared * dot
}

/// Gradient noise on the simplex lattice — four corners rather than eight.
/// Mirrors `pattern_simplex_noise`.
pub(crate) fn simplex_noise(point: [f32; 3], salt: u32) -> f32 {
    let skew = (point[0] + point[1] + point[2]) * SIMPLEX_SKEW;
    let skewed = [
        (point[0] + skew).floor(),
        (point[1] + skew).floor(),
        (point[2] + skew).floor(),
    ];
    let unskew = (skewed[0] + skewed[1] + skewed[2]) * SIMPLEX_UNSKEW;
    let offset0 = [
        point[0] - (skewed[0] - unskew),
        point[1] - (skewed[1] - unskew),
        point[2] - (skewed[2] - unskew),
    ];
    // Which of the six tetrahedra — rank the components, in exactly the branch
    // order the shader uses so the two pick the same one on a tie.
    let (step1, step2): ([f32; 3], [f32; 3]) = if offset0[0] >= offset0[1] {
        if offset0[1] >= offset0[2] {
            ([1.0, 0.0, 0.0], [1.0, 1.0, 0.0])
        } else if offset0[0] >= offset0[2] {
            ([1.0, 0.0, 0.0], [1.0, 0.0, 1.0])
        } else {
            ([0.0, 0.0, 1.0], [1.0, 0.0, 1.0])
        }
    } else if offset0[1] < offset0[2] {
        ([0.0, 0.0, 1.0], [0.0, 1.0, 1.0])
    } else if offset0[0] < offset0[2] {
        ([0.0, 1.0, 0.0], [0.0, 1.0, 1.0])
    } else {
        ([0.0, 1.0, 0.0], [1.0, 1.0, 0.0])
    };
    let cell = [skewed[0] as i32, skewed[1] as i32, skewed[2] as i32];
    let mut offset1 = [0.0f32; 3];
    let mut offset2 = [0.0f32; 3];
    let mut offset3 = [0.0f32; 3];
    for axis in 0..3 {
        offset1[axis] = offset0[axis] - step1[axis] + SIMPLEX_UNSKEW;
        offset2[axis] = offset0[axis] - step2[axis] + 2.0 * SIMPLEX_UNSKEW;
        offset3[axis] = offset0[axis] - 1.0 + 3.0 * SIMPLEX_UNSKEW;
    }
    let cell1 = [
        cell[0] + step1[0] as i32,
        cell[1] + step1[1] as i32,
        cell[2] + step1[2] as i32,
    ];
    let cell2 = [
        cell[0] + step2[0] as i32,
        cell[1] + step2[1] as i32,
        cell[2] + step2[2] as i32,
    ];
    let total = simplex_corner(offset0, cell, salt)
        + simplex_corner(offset1, cell1, salt)
        + simplex_corner(offset2, cell2, salt)
        + simplex_corner(offset3, [cell[0] + 1, cell[1] + 1, cell[2] + 1], salt);
    (0.5 + 16.0 * total).clamp(0.0, 1.0)
}
