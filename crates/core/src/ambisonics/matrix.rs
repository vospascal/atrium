//! Small fixed-size matrix helpers shared by the decoders: inversion,
//! symmetric eigendecomposition, and applying a decode matrix.

use super::encode::BFormat;
use crate::speaker::{ChannelGains, MAX_CHANNELS};

/// 4×4 matrix inverse via cofactor expansion. Returns None if singular.
pub(super) fn invert_4x4(m: &[[f32; 4]; 4]) -> Option<[[f32; 4]; 4]> {
    // 2×2 sub-determinants from rows 0-1
    let s0 = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    let s1 = m[0][0] * m[1][2] - m[0][2] * m[1][0];
    let s2 = m[0][0] * m[1][3] - m[0][3] * m[1][0];
    let s3 = m[0][1] * m[1][2] - m[0][2] * m[1][1];
    let s4 = m[0][1] * m[1][3] - m[0][3] * m[1][1];
    let s5 = m[0][2] * m[1][3] - m[0][3] * m[1][2];

    // 2×2 sub-determinants from rows 2-3
    let c5 = m[2][2] * m[3][3] - m[2][3] * m[3][2];
    let c4 = m[2][1] * m[3][3] - m[2][3] * m[3][1];
    let c3 = m[2][1] * m[3][2] - m[2][2] * m[3][1];
    let c2 = m[2][0] * m[3][3] - m[2][3] * m[3][0];
    let c1 = m[2][0] * m[3][2] - m[2][2] * m[3][0];
    let c0 = m[2][0] * m[3][1] - m[2][1] * m[3][0];

    let det = s0 * c5 - s1 * c4 + s2 * c3 + s3 * c2 - s4 * c1 + s5 * c0;

    if det.abs() < 1e-10 {
        return None;
    }

    let inv_det = 1.0 / det;

    Some([
        [
            (m[1][1] * c5 - m[1][2] * c4 + m[1][3] * c3) * inv_det,
            (-m[0][1] * c5 + m[0][2] * c4 - m[0][3] * c3) * inv_det,
            (m[3][1] * s5 - m[3][2] * s4 + m[3][3] * s3) * inv_det,
            (-m[2][1] * s5 + m[2][2] * s4 - m[2][3] * s3) * inv_det,
        ],
        [
            (-m[1][0] * c5 + m[1][2] * c2 - m[1][3] * c1) * inv_det,
            (m[0][0] * c5 - m[0][2] * c2 + m[0][3] * c1) * inv_det,
            (-m[3][0] * s5 + m[3][2] * s2 - m[3][3] * s1) * inv_det,
            (m[2][0] * s5 - m[2][2] * s2 + m[2][3] * s1) * inv_det,
        ],
        [
            (m[1][0] * c4 - m[1][1] * c2 + m[1][3] * c0) * inv_det,
            (-m[0][0] * c4 + m[0][1] * c2 - m[0][3] * c0) * inv_det,
            (m[3][0] * s4 - m[3][1] * s2 + m[3][3] * s0) * inv_det,
            (-m[2][0] * s4 + m[2][1] * s2 - m[2][3] * s0) * inv_det,
        ],
        [
            (-m[1][0] * c3 + m[1][1] * c1 - m[1][2] * c0) * inv_det,
            (m[0][0] * c3 - m[0][1] * c1 + m[0][2] * c0) * inv_det,
            (-m[3][0] * s3 + m[3][1] * s1 - m[3][2] * s0) * inv_det,
            (m[2][0] * s3 - m[2][1] * s1 + m[2][2] * s0) * inv_det,
        ],
    ])
}

/// 3×3 matrix inverse via Cramer's rule. Returns None if singular.
pub(super) fn invert_3x3(m: &[[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);

    if det.abs() < 1e-10 {
        return None;
    }

    let inv_det = 1.0 / det;
    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv_det,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv_det,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv_det,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv_det,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv_det,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv_det,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv_det,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv_det,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv_det,
        ],
    ])
}

/// Jacobi eigendecomposition of a 4×4 symmetric matrix.
///
/// Returns (eigenvectors, eigenvalues) where eigenvectors[col][row] contains
/// the eigenvector components. Converges in a few sweeps for small matrices.
#[allow(clippy::needless_range_loop)]
pub(super) fn jacobi_eigen_4x4(matrix: &[[f32; 4]; 4]) -> ([[f32; 4]; 4], [f32; 4]) {
    let mut a = *matrix;
    let mut v = [[0.0f32; 4]; 4];
    for i in 0..4 {
        v[i][i] = 1.0;
    }

    // 20 sweeps is far more than needed for 4×4 convergence
    for _ in 0..20 {
        for p in 0..4 {
            for q in (p + 1)..4 {
                if a[p][q].abs() < 1e-12 {
                    continue;
                }

                // Compute Jacobi rotation to zero a[p][q]
                let tau = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = if tau >= 0.0 {
                    1.0 / (tau + (1.0 + tau * tau).sqrt())
                } else {
                    -1.0 / (-tau + (1.0 + tau * tau).sqrt())
                };
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = t * c;

                // Update diagonal and zero the (p,q) pair
                let app = a[p][p] - t * a[p][q];
                let aqq = a[q][q] + t * a[p][q];
                a[p][p] = app;
                a[q][q] = aqq;
                a[p][q] = 0.0;
                a[q][p] = 0.0;

                // Update off-diagonal elements in rows/columns p and q
                for r in 0..4 {
                    if r == p || r == q {
                        continue;
                    }
                    let arp = a[r][p];
                    let arq = a[r][q];
                    a[r][p] = c * arp - s * arq;
                    a[p][r] = a[r][p];
                    a[r][q] = s * arp + c * arq;
                    a[q][r] = a[r][q];
                }

                // Accumulate eigenvector rotations
                for r in 0..4 {
                    let vrp = v[r][p];
                    let vrq = v[r][q];
                    v[r][p] = c * vrp - s * vrq;
                    v[r][q] = s * vrp + c * vrq;
                }
            }
        }
    }

    let eigenvalues = [a[0][0], a[1][1], a[2][2], a[3][3]];
    (v, eigenvalues)
}

/// Apply a decode matrix (one `[w, y, z, x]` row per speaker) to a B-format
/// signal, writing each speaker's gain to its output channel. Rows without a
/// channel entry are skipped.
pub(super) fn decode_rows(
    rows: &[[f32; 4]],
    channels: &[usize],
    bformat: &BFormat,
) -> ChannelGains {
    let mut gains = ChannelGains::silent(MAX_CHANNELS);
    for (i, row) in rows.iter().enumerate() {
        let g = row[0] * bformat.w + row[1] * bformat.y + row[2] * bformat.z + row[3] * bformat.x;
        if i < channels.len() {
            gains.gains[channels[i]] = g;
        }
    }
    gains
}

#[cfg(test)]
mod tests {
    use super::super::test_support::assert_close;
    use super::*;

    #[test]
    fn invert_3x3_identity() {
        let id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let inv = invert_3x3(&id).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!(
                    (inv[i][j] - expected).abs() < 1e-6,
                    "inv[{i}][{j}] = {}, expected {expected}",
                    inv[i][j]
                );
            }
        }
    }

    #[test]
    fn invert_3x3_singular() {
        let m = [[1.0, 2.0, 3.0], [1.0, 2.0, 3.0], [1.0, 2.0, 3.0]];
        assert!(invert_3x3(&m).is_none());
    }

    #[test]
    fn invert_4x4_identity() {
        let id = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let inv = invert_4x4(&id).unwrap();
        for i in 0..4 {
            for j in 0..4 {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!(
                    (inv[i][j] - expected).abs() < 1e-6,
                    "inv[{i}][{j}] = {}, expected {expected}",
                    inv[i][j]
                );
            }
        }
    }

    #[test]
    fn invert_4x4_round_trip() {
        // Non-trivial symmetric positive definite matrix
        let m = [
            [4.0, 1.0, 0.5, 0.2],
            [1.0, 3.0, 0.3, 0.1],
            [0.5, 0.3, 2.0, 0.4],
            [0.2, 0.1, 0.4, 1.5],
        ];
        let inv = invert_4x4(&m).unwrap();
        // M · M⁻¹ should be identity
        for i in 0..4 {
            for j in 0..4 {
                let mut sum = 0.0_f32;
                for k in 0..4 {
                    sum += m[i][k] * inv[k][j];
                }
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!(
                    (sum - expected).abs() < 1e-4,
                    "(M·M⁻¹)[{i}][{j}] = {sum}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn invert_4x4_singular() {
        // All rows identical — singular
        let m = [
            [1.0, 2.0, 3.0, 4.0],
            [1.0, 2.0, 3.0, 4.0],
            [1.0, 2.0, 3.0, 4.0],
            [1.0, 2.0, 3.0, 4.0],
        ];
        assert!(invert_4x4(&m).is_none());
    }

    // ─────────────────────────────────────────────────────────────────────
    // Audit 2026-11: hand-derived values and invariants
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn jacobi_eigen_reconstructs_symmetric_matrix() {
        // [[2,1],[1,2]] ⊕ [3] ⊕ [0] has eigenvalues {1, 3, 3, 0}.
        let matrix = [
            [2.0, 1.0, 0.0, 0.0],
            [1.0, 2.0, 0.0, 0.0],
            [0.0, 0.0, 3.0, 0.0],
            [0.0, 0.0, 0.0, 0.0],
        ];
        let (eigenvectors, eigenvalues) = jacobi_eigen_4x4(&matrix);
        let mut sorted = eigenvalues;
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for (actual, expected) in sorted.iter().zip([0.0, 1.0, 3.0, 3.0]) {
            assert_close(*actual, expected, 1e-5, "eigenvalue");
        }
        for row in 0..4 {
            for column in 0..4 {
                // V·Λ·Vᵀ = A and Vᵀ·V = I
                let reconstructed: f32 = (0..4)
                    .map(|k| eigenvectors[row][k] * eigenvalues[k] * eigenvectors[column][k])
                    .sum();
                assert_close(reconstructed, matrix[row][column], 1e-5, "V·Λ·Vᵀ");
                let orthogonality: f32 = (0..4)
                    .map(|k| eigenvectors[k][row] * eigenvectors[k][column])
                    .sum();
                let identity = if row == column { 1.0 } else { 0.0 };
                assert_close(orthogonality, identity, 1e-5, "Vᵀ·V");
            }
        }
    }
}
