//! Sparse round specks at jittered cell centres.

use super::hash::{ease, hash_cell};

/// Scattered round specks. See [`PatternGenerator::Speckle`].
pub(crate) fn speckle(point: [f32; 3], density: f32, salt_base: u32) -> f32 {
    let base = [point[0].floor(), point[1].floor(), point[2].floor()];
    let cell = [base[0] as i32, base[1] as i32, base[2] as i32];
    if hash_cell(cell, salt_base ^ SPECKLE_PRESENCE_SALT) >= density {
        return 0.0;
    }
    // The speck sits somewhere inside its cell rather than at the centre, or the
    // specks line up on the lattice and read as a grid.
    let centre = [
        0.25 + 0.5 * hash_cell(cell, salt_base ^ SPECKLE_JITTER_X_SALT),
        0.25 + 0.5 * hash_cell(cell, salt_base ^ SPECKLE_JITTER_Y_SALT),
        0.25 + 0.5 * hash_cell(cell, salt_base ^ SPECKLE_JITTER_Z_SALT),
    ];
    let offset = [
        point[0] - base[0] - centre[0],
        point[1] - base[1] - centre[1],
        point[2] - base[2] - centre[2],
    ];
    let distance = (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt();
    // Smooth rather than a hard disc, so a speck does not alias into a flickering
    // dot the moment it approaches a pixel in size.
    let edge = (1.0 - distance / SPECKLE_RADIUS_CELLS).clamp(0.0, 1.0);
    ease(edge)
}

pub(crate) const SPECKLE_PRESENCE_SALT: u32 = 11;

pub(crate) const SPECKLE_JITTER_X_SALT: u32 = 12;

pub(crate) const SPECKLE_JITTER_Y_SALT: u32 = 13;

pub(crate) const SPECKLE_JITTER_Z_SALT: u32 = 14;

/// A speck's radius as a fraction of its cell. Big enough to read, small enough
/// that neighbouring cells' specks stay separate at full density.
pub(crate) const SPECKLE_RADIUS_CELLS: f32 = 0.32;
