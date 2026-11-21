//! The rigid sphere worked out a second way, for tests to hold the rest to.
//!
//! The production series (`sphere.rs`) is Duda and Martens' appendix A10,
//! built from their polynomials Q_m. This one is their equations 7–8 as they
//! stand, with the spherical Hankel functions themselves: h_0 and h_1 from
//! their closed forms, the upward recurrence h_(n+1) = (2n+1)/x h_n − h_(n−1),
//! and the derivative from h'_m = h_(m−1) − (m+1)/x h_m (h'_0 = −h_1). Only
//! ratios h_n / h_(n−1) are carried, so nothing overflows; everything is f64.
//! It shares no code with what it checks.

use realfft::num_complex::Complex;

/// The standard head (m) and the speed of sound (m/s) the tests use.
pub(crate) const HEAD_RADIUS: f64 = 0.0875;
pub(crate) const SPEED_OF_SOUND: f64 = 343.0;

/// |H(ρ, μ, α)| by equations 7–8: surface pressure over free-field pressure
/// at the centre, `incidence` in radians.
pub(crate) fn sphere(normalised_distance: f64, normalised_frequency: f64, incidence: f64) -> f64 {
    let (rho, mu) = (normalised_distance, normalised_frequency);
    let i = Complex::new(0.0, 1.0);
    let hankel_0 = |x: f64| -i * (i * x).exp() / x;
    let hankel_1 = |x: f64| -(i * x).exp() * (x + i) / (x * x);
    let (source, surface) = (mu * rho, mu);
    let cosine = incidence.cos();

    // m = 0: h_0(μρ) / h'_0(μ), h'_0 = −h_1.
    let mut sum = hankel_0(source) / -hankel_1(surface);
    // From m = 1: h_m(μρ) / h_m(μ) as a running product, and the ratios
    // h_m / h_(m−1) at both arguments.
    let mut quotient = hankel_1(source) / hankel_1(surface);
    let mut source_ratio = hankel_1(source) / hankel_0(source);
    let mut surface_ratio = hankel_1(surface) / hankel_0(surface);
    let (mut legendre_before, mut legendre) = (1.0, cosine);
    let mut small_in_a_row = 0;
    for order in 1..2000 {
        let m = order as f64;
        // h'_m / h_m = h_(m−1) / h_m − (m+1)/x.
        let derivative_over_value = 1.0 / surface_ratio - (m + 1.0) / surface;
        let term = (2.0 * m + 1.0) * legendre * quotient / derivative_over_value;
        sum += term;
        if term.norm() < 1e-13 * sum.norm() {
            small_in_a_row += 1;
            if small_in_a_row >= 2 && m > mu {
                break;
            }
        } else {
            small_in_a_row = 0;
        }
        source_ratio = (2.0 * m + 1.0) / source - 1.0 / source_ratio;
        surface_ratio = (2.0 * m + 1.0) / surface - 1.0 / surface_ratio;
        quotient *= source_ratio / surface_ratio;
        let next = ((2.0 * m + 1.0) * cosine * legendre - m * legendre_before) / (m + 1.0);
        (legendre_before, legendre) = (legendre, next);
    }
    // −(ρ/μ) e^(−iμρ) has magnitude ρ/μ.
    rho / mu * sum.norm()
}

/// μ for `frequency_hz` on the standard head.
pub(crate) fn normalised_frequency(frequency_hz: f64) -> f64 {
    std::f64::consts::TAU * frequency_hz * HEAD_RADIUS / SPEED_OF_SOUND
}

/// The distance variation function in dB, without the 1/r term: the sphere
/// at `distance` over the sphere at `reference_distance` (m), for an ear at
/// `incidence_degrees` from the source, at `frequency_hz`.
pub(crate) fn distance_variation_db(
    distance: f64,
    reference_distance: f64,
    incidence_degrees: f64,
    frequency_hz: f64,
) -> f64 {
    let mu = normalised_frequency(frequency_hz);
    let incidence = incidence_degrees.to_radians();
    20.0 * (sphere(distance / HEAD_RADIUS, mu, incidence)
        / sphere(reference_distance / HEAD_RADIUS, mu, incidence))
    .log10()
}
