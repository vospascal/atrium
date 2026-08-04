//! Worley / cellular noise: distances to the nearest jittered feature points.

use super::hash::hash_cell;

pub(crate) const WORLEY_JITTER_X_SALT: u32 = 21;

pub(crate) const WORLEY_JITTER_Y_SALT: u32 = 22;

pub(crate) const WORLEY_JITTER_Z_SALT: u32 = 23;

pub(crate) const WORLEY_SMOOTH_K: f32 = 6.0;

/// The nearest feature point is at most ~1.5 cells away in the worst case.
pub(crate) const WORLEY_RANGE: f32 = 1.5;

/// F1, F2 and the smooth minimum from ONE 27-cell walk, exactly as
/// `pattern_worley_distances` does it — three variants, one loop, one set of
/// jitter salts to get wrong.
pub(crate) fn worley_distances(point: [f32; 3], salt_base: u32) -> [f32; 3] {
    let base = [point[0].floor(), point[1].floor(), point[2].floor()];
    let cell = [base[0] as i32, base[1] as i32, base[2] as i32];
    let local = [point[0] - base[0], point[1] - base[1], point[2] - base[2]];
    let mut nearest = 1e9f32;
    let mut second = 1e9f32;
    let mut smooth_sum = 0.0f32;
    for index in 0..27u32 {
        let neighbour = [
            (index % 3) as i32 - 1,
            ((index / 3) % 3) as i32 - 1,
            (index / 9) as i32 - 1,
        ];
        let neighbour_cell = [
            cell[0] + neighbour[0],
            cell[1] + neighbour[1],
            cell[2] + neighbour[2],
        ];
        let jitter = [
            hash_cell(neighbour_cell, salt_base ^ WORLEY_JITTER_X_SALT),
            hash_cell(neighbour_cell, salt_base ^ WORLEY_JITTER_Y_SALT),
            hash_cell(neighbour_cell, salt_base ^ WORLEY_JITTER_Z_SALT),
        ];
        let mut squared = 0.0;
        for axis in 0..3 {
            let offset = neighbour[axis] as f32 + jitter[axis] - local[axis];
            squared += offset * offset;
        }
        if squared < nearest {
            second = nearest;
            nearest = squared;
        } else if squared < second {
            second = squared;
        }
        smooth_sum += (-WORLEY_SMOOTH_K * squared.sqrt()).exp();
    }
    [
        nearest.sqrt(),
        second.sqrt(),
        -smooth_sum.ln() / WORLEY_SMOOTH_K,
    ]
}
