//! Octave summation, and the two variants that fold the signal: ridged and turbulence.

use super::value::value_noise;
use crate::pattern::MAX_NOISE_OCTAVES;

/// The four fractal families share one octave loop shape; only the per-octave
/// value differs. Mirrors `pattern_fractal_noise` / `_perlin` / `_simplex`,
/// `pattern_ridged_noise` and `pattern_turbulence`.
pub(crate) fn fractal<F: Fn([f32; 3], u32) -> f32>(
    point: [f32; 3],
    octaves: u32,
    salt_base: u32,
    octave_value: F,
) -> f32 {
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
        total += amplitude * octave_value(scaled, salt_base ^ octave);
        normalisation += amplitude;
        frequency *= 2.0;
        amplitude *= 0.5;
    }
    total / normalisation
}

/// Ridged multifractal. Mirrors `pattern_ridged_noise`.
pub(crate) fn ridged_noise(point: [f32; 3], octaves: u32, salt_base: u32) -> f32 {
    fractal(point, octaves, salt_base, |scaled, salt| {
        let folded = 1.0 - (2.0 * value_noise(scaled, salt) - 1.0).abs();
        folded * folded
    })
    .clamp(0.0, 1.0)
}

/// Turbulence. Mirrors `pattern_turbulence`.
pub(crate) fn turbulence(point: [f32; 3], octaves: u32, salt_base: u32) -> f32 {
    fractal(point, octaves, salt_base, |scaled, salt| {
        (2.0 * value_noise(scaled, salt) - 1.0).abs()
    })
    .clamp(0.0, 1.0)
}
