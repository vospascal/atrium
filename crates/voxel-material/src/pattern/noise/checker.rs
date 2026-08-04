//! Axis-aligned checkerboard.

/// Alternating lattice cells. Mirrors `pattern_checker`.
///
/// `&` on a negative `i32` is the same bitwise operation in both languages, so a
/// cell at a negative coordinate lands on the same colour on both sides.
pub(crate) fn checker(point: [f32; 3]) -> f32 {
    let cell = [
        point[0].floor() as i32,
        point[1].floor() as i32,
        point[2].floor() as i32,
    ];
    if (cell[0] + cell[1] + cell[2]) & 1 == 0 {
        1.0
    } else {
        0.0
    }
}
