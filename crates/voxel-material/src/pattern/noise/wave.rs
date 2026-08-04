//! Sine bands with optional distortion.

use super::value::value_noise;

/// Bands along X, bent by noise. Mirrors `pattern_wave`.
pub(crate) fn wave(point: [f32; 3], distortion: f32, salt: u32) -> f32 {
    let mut coordinate = point[0] + point[1] * 0.25;
    if distortion > 0.0 {
        coordinate += distortion * (2.0 * value_noise(point, salt ^ 41) - 1.0);
    }
    let phase = coordinate - coordinate.floor();
    1.0 - (2.0 * phase - 1.0).abs()
}
