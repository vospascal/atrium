//! Domain warping: displace the sample point by noise before evaluating.

use super::value::value_noise;

pub(crate) const WARP_OFFSET_Y: [f32; 3] = [31.416, 7.913, 19.264];

pub(crate) const WARP_OFFSET_Z: [f32; 3] = [-13.077, 41.502, 5.731];

pub(crate) const WARP_SALT: u32 = 51;

/// Domain warping — displace the sample point by a noise field before the
/// generator reads it. Mirrors `pattern_warp`.
pub(crate) fn domain_warp(point: [f32; 3], strength: f32, salt: u32) -> [f32; 3] {
    if strength == 0.0 {
        return point;
    }
    let warp_salt = salt ^ WARP_SALT;
    let offset_y = [
        point[0] + WARP_OFFSET_Y[0],
        point[1] + WARP_OFFSET_Y[1],
        point[2] + WARP_OFFSET_Y[2],
    ];
    let offset_z = [
        point[0] + WARP_OFFSET_Z[0],
        point[1] + WARP_OFFSET_Z[1],
        point[2] + WARP_OFFSET_Z[2],
    ];
    let displacement = [
        value_noise(point, warp_salt),
        value_noise(offset_y, warp_salt),
        value_noise(offset_z, warp_salt),
    ];
    let mut warped = [0.0f32; 3];
    for axis in 0..3 {
        warped[axis] = point[axis] + (displacement[axis] * 2.0 - 1.0) * strength;
    }
    warped
}
