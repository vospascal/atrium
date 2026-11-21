//! The rigid sphere's response to a point source, as Duda and Martens give
//! it, and its closed form at zero frequency.

use realfft::num_complex::Complex;

/// Terms are added until two in a row change the sum by less than this
/// share. Duda and Martens leave the threshold to the user; this one is a
/// billionth of a decibel, far below anything a fit or an ear resolves.
const CONVERGED: f64 = 1e-10;
/// The series converges once its order passes the head's size in
/// wavelengths, μ; this bounds it for any frequency the table asks for.
const MOST_TERMS: usize = 400;

/// |H(ρ, μ, α)|: the pressure a point source `normalised_distance` head radii
/// from the centre makes on the sphere at `incidence` radians from the
/// direction to the source, over the free-field pressure at the centre, at
/// normalised frequency μ = 2π f a / c > 0.
///
/// The series is Duda and Martens' equation A10. Their recursion builds the
/// polynomials Q_m(1/(iμ)) themselves, which grow like (2m−1)!!/μ^m and
/// overflow at low frequencies close to the head; this carries their ratios
/// Q_m / Q_(m−1) instead (the same recurrence divided through), so the terms
/// stay the size they are. The first two terms are exactly their appendix B's.
pub(super) fn response_magnitude(
    normalised_distance: f64,
    normalised_frequency: f64,
    incidence: f64,
) -> f64 {
    let (distance, frequency) = (normalised_distance, normalised_frequency);
    let cosine = incidence.cos();
    let i = Complex::new(0.0, 1.0);
    let at_source = 1.0 / (i * frequency * distance);
    let at_surface = 1.0 / (i * frequency);

    // Q_m / Q_(m−1) at the source's and the surface's argument, starting
    // from Q_0 = Q_(−1) = z, and Q_m(source) / Q_(m−1)(surface), the part of
    // each term that would overflow if its two factors were kept apart.
    let mut source_ratio = Complex::new(1.0, 0.0);
    let mut surface_ratio = Complex::new(1.0, 0.0);
    let mut scale = at_source / at_surface;
    let (mut legendre_before, mut legendre) = (0.0, 1.0);
    let mut sum = Complex::new(0.0, 0.0);
    let mut small_in_a_row = 0;
    for order in 0..MOST_TERMS {
        if order >= 1 {
            let previous_surface_ratio = surface_ratio;
            if order == 1 {
                // Q_1(z) = z − z².
                source_ratio = 1.0 - at_source;
                surface_ratio = 1.0 - at_surface;
                (legendre_before, legendre) = (legendre, cosine);
            } else {
                let step = (2 * order - 1) as f64;
                source_ratio = -step * at_source + 1.0 / source_ratio;
                surface_ratio = -step * at_surface + 1.0 / surface_ratio;
                let next = (step * cosine * legendre - (order - 1) as f64 * legendre_before)
                    / order as f64;
                (legendre_before, legendre) = (legendre, next);
            }
            scale *= source_ratio / previous_surface_ratio;
        }
        let term = (2 * order + 1) as f64 * legendre * scale
            / ((order + 1) as f64 * at_surface * surface_ratio - 1.0);
        sum += term;
        if term.norm() <= CONVERGED * sum.norm() {
            small_in_a_row += 1;
            if small_in_a_row >= 2 && order as f64 > frequency {
                break;
            }
        } else {
            small_in_a_row = 0;
        }
    }
    // H = ρ e^(−iμ) Σ / (iμ); the phase factor has unit magnitude.
    distance * sum.norm() / frequency
}

/// H(ρ, 0, α): the response at zero frequency, which the series cannot reach
/// (it divides by μ). With h_m(z) ≈ −i (2m−1)!! / z^(m+1) for small z each
/// term tends to (2m+1)/(m+1) P_m(cos α) ρ^(−m), and with x = 1/ρ,
/// R = √(1 − 2x cos α + x²) the generating functions of the Legendre
/// polynomials sum that to
///
/// ```text
/// H(ρ, 0, α) = 2/R − ln((x − cos α + R) / (1 − cos α)) / x
/// ```
///
/// Straight at the source (α = 0) it is 2/(1 − x) + ln(1 − x)/x: 7.988, or
/// 18.05 dB, at ρ = 1.25, the value Duda and Martens' figure 3 shows.
pub(super) fn response_at_zero_frequency(normalised_distance: f64, incidence: f64) -> f64 {
    let x = 1.0 / normalised_distance;
    let cosine = incidence.cos();
    let root = (1.0 - 2.0 * x * cosine + x * x).sqrt();
    // (x − cos α + R)(R − x + cos α) = 1 − cos²α: the second form keeps its
    // precision towards the source, where the first divides two vanishing
    // numbers, and the first away from it.
    let ratio = if cosine >= 0.0 {
        (1.0 + cosine) / (root - x + cosine)
    } else {
        (x - cosine + root) / (1.0 - cosine)
    };
    2.0 / root - ratio.ln() / x
}
