//! EPAD (Energy-Preserving AllRAD) correction of a decode matrix.

use super::matrix::jacobi_eigen_4x4;

/// Apply EPAD (Energy-Preserving AllRAD) correction to a decode matrix.
///
/// Replaces all singular values with their mean, equalizing energy across
/// source directions while preserving the spatial mapping.
///
/// For the N×4 decode matrix D:
///   1. Compute Dᵀ×D (4×4 symmetric positive semi-definite)
///   2. Eigendecompose via Jacobi rotations → V, λ
///   3. σᵢ = √λᵢ, σ_mean = mean of non-zero σ
///   4. D_EPAD = D × V × diag(σ_mean/σᵢ) × Vᵀ
pub(super) fn apply_epad(decode_matrix: &mut [[f32; 4]]) {
    let n = decode_matrix.len();
    if n == 0 {
        return;
    }

    // Compute Dᵀ×D (4×4)
    let mut dtd = [[0.0f32; 4]; 4];
    for k in 0..4 {
        for l in k..4 {
            let mut sum = 0.0f32;
            for row in decode_matrix.iter() {
                sum += row[k] * row[l];
            }
            dtd[k][l] = sum;
            dtd[l][k] = sum;
        }
    }

    let (eigenvectors, eigenvalues) = jacobi_eigen_4x4(&dtd);

    // Singular values = √eigenvalues, compute mean of non-zero
    let mut sigmas = [0.0f32; 4];
    let mut sigma_sum = 0.0f32;
    let mut count = 0;
    for i in 0..4 {
        sigmas[i] = eigenvalues[i].max(0.0).sqrt();
        if sigmas[i] > 1e-6 {
            sigma_sum += sigmas[i];
            count += 1;
        }
    }
    if count == 0 {
        return;
    }
    let sigma_mean = sigma_sum / count as f32;

    // Scale factors: σ_mean / σᵢ
    let mut scales = [0.0f32; 4];
    for i in 0..4 {
        scales[i] = if sigmas[i] > 1e-6 {
            sigma_mean / sigmas[i]
        } else {
            0.0
        };
    }

    // Apply: D_EPAD = D × V × diag(scales) × Vᵀ
    // Row i of the result depends only on row i of the input, so rows are
    // rewritten in place.
    for row in decode_matrix.iter_mut() {
        let mut new_row = [0.0f32; 4];
        for k in 0..4 {
            let mut sum = 0.0f32;
            for j in 0..4 {
                // (D × V)[i][j] = Σ_l D[i][l] × V[l][j]
                let dv: f32 = (0..4).map(|l| row[l] * eigenvectors[l][j]).sum();
                // × scales[j] × Vᵀ[j][k] = V[k][j]
                sum += dv * scales[j] * eigenvectors[k][j];
            }
            new_row[k] = sum;
        }
        *row = new_row;
    }
}

#[cfg(test)]
mod tests {
    use super::super::allrad::AllRadDecoder;
    use super::super::encode::foa_encode;
    use super::super::mode_matching::FoaDecoder;
    use super::super::test_support::{
        assert_close, gains_for, origin_listener, speaker_at, test_5_1_speakers, test_listener,
    };
    use super::*;
    use crate::speaker::Speaker;

    #[test]
    fn epad_reduces_energy_variation() {
        // EPAD should produce significantly more uniform energy than plain AllRAD.
        let speakers = test_5_1_speakers();
        let listener = test_listener();
        let allrad = AllRadDecoder::from_listener(&speakers, 6, &listener);
        let epad = AllRadDecoder::from_listener_epad(&speakers, 6, &listener);

        let mut allrad_energies = Vec::new();
        let mut epad_energies = Vec::new();

        for deg in 0..360 {
            let az = (deg as f32).to_radians();
            let b = foa_encode(az, 0.0, 1.0);

            let g_allrad = allrad.decode(&b);
            let e_allrad: f32 = (0..6).map(|ch| g_allrad.gains[ch].powi(2)).sum();
            allrad_energies.push(e_allrad);

            let g_epad = epad.decode(&b);
            let e_epad: f32 = (0..6).map(|ch| g_epad.gains[ch].powi(2)).sum();
            epad_energies.push(e_epad);
        }

        let allrad_min = allrad_energies.iter().cloned().fold(f32::MAX, f32::min);
        let allrad_max = allrad_energies.iter().cloned().fold(f32::MIN, f32::max);
        let allrad_db = 10.0 * (allrad_max / allrad_min.max(1e-10)).log10();

        let epad_min = epad_energies.iter().cloned().fold(f32::MAX, f32::min);
        let epad_max = epad_energies.iter().cloned().fold(f32::MIN, f32::max);
        let epad_db = 10.0 * (epad_max / epad_min.max(1e-10)).log10();

        eprintln!("AllRAD energy variation: {allrad_db:.2} dB");
        eprintln!("EPAD   energy variation: {epad_db:.2} dB");

        // EPAD should be strictly better than AllRAD
        assert!(
            epad_db < allrad_db,
            "EPAD ({epad_db:.2} dB) should have less variation than AllRAD ({allrad_db:.2} dB)"
        );
        // EPAD should get energy variation under 1 dB
        assert!(
            epad_db < 1.0,
            "EPAD energy variation {epad_db:.2} dB exceeds 1 dB target"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // Audit 2026-11: hand-derived values and invariants
    // ─────────────────────────────────────────────────────────────────────

    /// ITU-R BS.775 5.0 ring (radius 2 m) around a listener at the origin
    /// facing +X: L 30°, R −30°, C 0°, Ls 110°, Rs −110°. Channels 0,1,2,4,5.
    fn itu_5_0_speakers() -> Vec<Speaker> {
        let place = |degrees: f32, channel: usize| {
            let azimuth = degrees.to_radians();
            speaker_at(2.0 * azimuth.cos(), 2.0 * azimuth.sin(), 0.0, channel)
        };
        vec![
            place(30.0, 0),
            place(-30.0, 1),
            place(0.0, 2),
            place(110.0, 4),
            place(-110.0, 5),
        ]
    }

    #[test]
    fn epad_on_itu_5_0_gives_exactly_flat_energy_and_mirror_symmetry() {
        // After EPAD, DᵀD = σ̄²·P(W,Y,X), so Σgᵢ² = σ̄²·(W² + Y² + X²)
        // = σ̄²·g²·(½ + a²) = σ̄²·g² — independent of azimuth.
        // An independent numpy re-implementation of AllRAD(12)+EPAD on this
        // layout gives σ̄² = 0.48951 and plain-AllRAD variation 4.63 dB.
        let speakers = itu_5_0_speakers();
        let listener = origin_listener();
        let channels = [0, 1, 2, 4, 5];
        let epad = AllRadDecoder::from_listener_epad(&speakers, 5, &listener);
        let allrad = AllRadDecoder::from_listener(&speakers, 5, &listener);

        let mut allrad_min = f32::MAX;
        let mut allrad_max = f32::MIN;
        for degrees in 0..360 {
            let azimuth = (degrees as f32).to_radians();
            let bformat = foa_encode(azimuth, 0.0, 1.0);
            let gains = gains_for(&epad.decode(&bformat), &channels);
            let energy: f32 = gains.iter().map(|gain| gain * gain).sum();
            assert_close(energy, 0.48951, 2e-4, "EPAD energy");

            let allrad_energy: f32 = gains_for(&allrad.decode(&bformat), &channels)
                .iter()
                .map(|gain| gain * gain)
                .sum();
            allrad_min = allrad_min.min(allrad_energy);
            allrad_max = allrad_max.max(allrad_energy);

            // L/R mirror: L(φ) = R(−φ), Ls(φ) = Rs(−φ), C(φ) = C(−φ).
            let mirror = gains_for(&epad.decode(&foa_encode(-azimuth, 0.0, 1.0)), &channels);
            assert_close(gains[0], mirror[1], 1e-4, "L↔R mirror");
            assert_close(gains[2], mirror[2], 1e-4, "C mirror");
            assert_close(gains[3], mirror[4], 1e-4, "Ls↔Rs mirror");
        }
        let allrad_variation_db = 10.0 * (allrad_max / allrad_min).log10();
        assert_close(
            allrad_variation_db,
            4.63,
            0.05,
            "AllRAD 5.0 energy variation",
        );
    }

    #[test]
    fn epad_on_itu_5_0_peaks_at_each_side_speaker_but_not_centre() {
        // Encode→decode at a speaker's own direction should make that speaker
        // the loudest. FOA cannot resolve C (0°) against L/R (±30°): a front
        // source decodes to L = R = 0.4499 > C = 0.2805 (numpy reference). This
        // is a first-order resolution limit, not a decoder defect.
        let speakers = itu_5_0_speakers();
        let decoder = AllRadDecoder::from_listener_epad(&speakers, 5, &origin_listener());
        let channels = [0, 1, 2, 4, 5];
        for (degrees, expected_channel) in [(30.0_f32, 0), (-30.0, 1), (110.0, 4), (-110.0, 5)] {
            let gains = decoder.decode(&foa_encode(degrees.to_radians(), 0.0, 1.0));
            let loudest = *channels
                .iter()
                .max_by(|&&a, &&b| gains.gains[a].partial_cmp(&gains.gains[b]).unwrap())
                .unwrap();
            assert_eq!(
                loudest, expected_channel,
                "source at {degrees}° should peak at channel {expected_channel}"
            );
        }
        let front = decoder.decode(&foa_encode(0.0, 0.0, 1.0));
        assert_close(front.gains[0], 0.4499, 2e-3, "front → L");
        assert_close(front.gains[2], 0.2805, 2e-3, "front → C");
    }

    #[test]
    fn epad_leaves_an_already_balanced_decoder_unchanged() {
        // Regular-quad mode-matching: DᵀD = ¼·Σ[½, s², 0, c²] = diag(½, ½, 0, ½),
        // so every non-zero singular value is already 1/√2 and EPAD is identity.
        let azimuths = [
            45.0_f32.to_radians(),
            -45.0_f32.to_radians(),
            135.0_f32.to_radians(),
            -135.0_f32.to_radians(),
        ];
        let decoder = FoaDecoder::new(&azimuths, &[0.0; 4], &[0, 1, 2, 3]);
        let mut equalised = decoder.decode_matrix.clone();
        apply_epad(&mut equalised);
        for (row, original) in equalised.iter().zip(&decoder.decode_matrix) {
            for column in 0..4 {
                assert_close(row[column], original[column], 1e-5, "EPAD identity");
            }
        }
    }
}
