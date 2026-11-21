//! How a head changes what each ear hears of a sound that comes close.
//!
//! A sound beside the head reaches the near ear directly and the far ear
//! round the head. Far off, the head shadows the treble and leaves the bass:
//! a wavelength of a metre and more bends round it as if it were not there.
//! Inside about a metre that changes. The source is now close enough that the
//! near ear is markedly nearer to it than the head's centre and the far ear
//! markedly further, and the head's own diffraction sharpens; the ears'
//! level difference grows strongly, most of all in the bass, where far-field
//! ears differ hardly at all. That bass difference is what makes a sound feel
//! beside your ear and not merely loud.
//!
//! The physics is the rigid sphere: Duda and Martens, "Range dependence of
//! the response of a spherical head model", JASA 104 (1998), equations 7–8
//! and the recursion of their appendix A, for a point source at any distance
//! ρ = r / a head radii and normalised frequency μ = 2π f a / c:
//!
//! ```text
//! H(ρ, μ, α) = −(ρ / μ) e^(−iμρ) Σ (2m+1) P_m(cos α) h_m(μρ) / h'_m(μ)
//! ```
//!
//! the pressure on the sphere over the free-field pressure at its centre, at
//! the point whose angle from the source direction is α. H leaves out the
//! 1/r of distance and its delay: those the engine already applies from the
//! head's centre.
//!
//! Head-related impulse responses are measured at one distance. Kan, Jin and
//! van Schaik (JASA 125, 2009) give them another with the distance variation
//! function: the sphere's response at the near distance over its response at
//! the measured one, for each ear. Without the 1/r term (which the engine has)
//! that is
//!
//! ```text
//! DVF(μ, α, ρ) = |H(ρ, μ, α)| / |H(ρ_measured, μ, α)|
//! ```
//!
//! Spagnol, Tavazzi and Avanzini (Applied Acoustics 115, 2017) showed that a
//! gain and a first-order shelving filter per ear follow it within about a
//! decibel. This module keeps their filter and not their coefficients: it
//! works the sphere out itself, once, when the renderer is set up, and fits
//! each shelf to it (see `table.rs` and the root README for why). The gain at
//! zero frequency has a closed form, which the fit keeps exactly.
//!
//! Everything but the filter is pure, and tested without a device. Which ear
//! sees a source at which angle is the renderer's to decide.

mod filter;
mod fit;
mod shelf;
mod sphere;
mod table;

pub(crate) use filter::EarFilter;
pub(crate) use shelf::EarCorrection;
pub(crate) use table::NearFieldTable;

/// The head's radius (m): the standard 8.75 cm of Hartley and Fry (1921),
/// which Duda and Martens use and Spagnol et al. fit their model at.
pub(crate) const HEAD_RADIUS_METRES: f32 = 0.0875;

/// The nearest distance from the head's centre the sphere is taken at, in
/// head radii. The theory holds for any ρ > 1, but a real source is not a
/// point, and the response of one grazing the surface grows without bound.
/// Spagnol et al. fit their model from ρ = 1.15 (their equation 7); nearer
/// than that, 10 mm from the skin, a source counts as there.
pub(crate) const NEAREST_NORMALISED_DISTANCE: f32 = 1.15;

/// The speed of sound the head's size is measured against (m/s), at 20 °C.
/// Duda and Martens' 625 Hz for μ = 1 implies the same.
const SPEED_OF_SOUND: f64 = 343.0;

#[cfg(test)]
pub(crate) mod reference;
#[cfg(test)]
mod tests;
