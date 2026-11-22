//! An arc on the first-order ambisonic bus.
//!
//! Unrelated sound from every direction of an arc is, in B-format, not one
//! encoded direction but a covariance: the mean over the arc of each
//! direction's encoding vector times itself (the spatial covariance of an
//! incoherent field; Epain and Jin, "Spherical harmonic signal covariance
//! and sound field diffuseness", IEEE/ACM TASLP 24, 2016). With the bus's
//! own encoding, e(θ) = (w, m·sin θ, 0, m·cos θ) for W, Y, Z, X, and an arc
//! 2α wide about θ₀, it is in closed form:
//!
//! ```text
//! ⟨W W⟩ = w²
//! ⟨W X⟩ = w·m·cos θ₀·sinc α           ⟨W Y⟩ = w·m·sin θ₀·sinc α
//! ⟨X X⟩ = m²·(1 + cos 2θ₀·sinc 2α)/2  ⟨Y Y⟩ = m²·(1 − cos 2θ₀·sinc 2α)/2
//! ⟨X Y⟩ = m²·sin 2θ₀·sinc 2α / 2      Z: nothing
//! ```
//!
//! with sinc x = sin x / x. Its trace is a plane wave's, w² + m², so the
//! decoder, energy preserving, plays the arc as loud as a point of the same
//! level. Its intensity ⟨W·(X, Y)⟩ points at the arc's middle with length
//! sinc α, which is the arc's energy vector in Gerzon's sense ("General
//! metatheory of auditory localisation", AES 92nd Convention, 1992): the
//! mean of its unit directions, shorter the wider it is.
//!
//! Three decorrelated copies of the sound mixed by a square root of that
//! covariance (its Cholesky factor) give the bus exactly that covariance,
//! as far as the copies are unrelated.

use atrium_core::ambisonics::foa_encode;

/// The mix, `[channel][voice]` with channels W, Y, Z, X, of three unrelated
/// voices that gives an arc `width` radians wide centred `centre` radians
/// anticlockwise from where the listener faces its B-format covariance.
pub(super) fn bformat_mix(width: f32, centre: f32) -> [[f32; 4]; 4] {
    let plane_wave = foa_encode(0.0, 0.0, 1.0);
    let (w, m) = (plane_wave.w as f64, plane_wave.x as f64);
    let half = width as f64 / 2.0;
    let sinc = |x: f64| if x.abs() < 1e-9 { 1.0 } else { x.sin() / x };
    let (first, second) = (sinc(half), sinc(2.0 * half));
    let centre = centre as f64;
    // In the order W, X, Y.
    let covariance = [
        [
            w * w,
            w * m * centre.cos() * first,
            w * m * centre.sin() * first,
        ],
        [
            w * m * centre.cos() * first,
            m * m * (1.0 + (2.0 * centre).cos() * second) / 2.0,
            m * m * (2.0 * centre).sin() * second / 2.0,
        ],
        [
            w * m * centre.sin() * first,
            m * m * (2.0 * centre).sin() * second / 2.0,
            m * m * (1.0 - (2.0 * centre).cos() * second) / 2.0,
        ],
    ];
    let factor = cholesky(covariance);
    let mut mix = [[0.0f32; 4]; 4];
    for (channel, row) in [(0, 0), (3, 1), (1, 2)] {
        for voice in 0..3 {
            mix[channel][voice] = factor[row][voice] as f32;
        }
    }
    mix
}

/// The lower triangular L with L·Lᵀ = `matrix`, for a covariance that may
/// be singular: a narrow arc is nearly one direction, of rank one, and what
/// it lacks is left at zero.
fn cholesky(matrix: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut factor = [[0.0f64; 3]; 3];
    for row in 0..3 {
        for column in 0..=row {
            let sum: f64 = (0..column)
                .map(|inner| factor[row][inner] * factor[column][inner])
                .sum();
            if row == column {
                factor[row][row] = (matrix[row][row] - sum).max(0.0).sqrt();
            } else if factor[column][column] > 1e-9 {
                factor[row][column] = (matrix[row][column] - sum) / factor[column][column];
            }
        }
    }
    factor
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The covariance of the mix, `[channel][channel]`.
    fn covariance_of(mix: &[[f32; 4]; 4]) -> [[f64; 4]; 4] {
        let mut covariance = [[0.0f64; 4]; 4];
        for a in 0..4 {
            for b in 0..4 {
                covariance[a][b] = (0..4)
                    .map(|voice| mix[a][voice] as f64 * mix[b][voice] as f64)
                    .sum();
            }
        }
        covariance
    }

    /// Against the covariance summed direction by direction over the arc,
    /// from the encoder itself: within 10⁻⁵, at every width and centre.
    #[test]
    fn the_mix_gives_the_arcs_covariance() {
        for width in [10.0f32, 60.0, 120.0, 300.0] {
            for centre in [0.0f32, 50.0, 135.0, 270.0] {
                let mix = bformat_mix(width.to_radians(), centre.to_radians());
                let rendered = covariance_of(&mix);
                let steps = 20_000;
                let mut summed = [[0.0f64; 4]; 4];
                for step in 0..steps {
                    let direction = (centre - width / 2.0
                        + width * (step as f32 + 0.5) / steps as f32)
                        .to_radians();
                    let encoded = foa_encode(direction, 0.0, 1.0);
                    let vector = [encoded.w, encoded.y, encoded.z, encoded.x];
                    for a in 0..4 {
                        for b in 0..4 {
                            summed[a][b] += vector[a] as f64 * vector[b] as f64 / steps as f64;
                        }
                    }
                }
                for a in 0..4 {
                    for b in 0..4 {
                        assert!(
                            (rendered[a][b] - summed[a][b]).abs() < 1e-5,
                            "{width}° at {centre}°, [{a}][{b}]: {} against {}",
                            rendered[a][b],
                            summed[a][b]
                        );
                    }
                }
            }
        }
    }

    /// Its trace is a plane wave's, and its intensity points at the middle
    /// with length sinc α (w·m·sinc α over w·m for a plane wave).
    #[test]
    fn the_intensity_points_at_the_middle_and_shortens_as_the_arc_widens() {
        let plane = foa_encode(0.0, 0.0, 1.0);
        for (width, centre) in [(60.0f32, 30.0f32), (120.0, 200.0), (240.0, 300.0)] {
            let covariance = covariance_of(&bformat_mix(width.to_radians(), centre.to_radians()));
            let trace: f64 = (0..4).map(|channel| covariance[channel][channel]).sum();
            let plane_trace = (plane.w * plane.w + plane.x * plane.x) as f64;
            assert!((trace - plane_trace).abs() < 1e-5);
            let (x, y) = (covariance[0][3], covariance[0][1]);
            let direction = y.atan2(x).to_degrees();
            let off = (direction - centre as f64 + 540.0).rem_euclid(360.0) - 180.0;
            assert!(off.abs() < 1e-3, "{width}° at {centre}°: {off}° off");
            let half = (width as f64).to_radians() / 2.0;
            let length = (x * x + y * y).sqrt() / (plane.w as f64 * plane.x as f64);
            assert!((length - half.sin() / half).abs() < 1e-5);
        }
    }
}
