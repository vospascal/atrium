//! Concrete `PathEffect` implementations.
//!
//! - `AirAbsorptionEffect`: ISO 9613-1 frequency-dependent atmospheric absorption.
//! - `GroundEffectFilter`: ISO 9613-2 ground effect where the floor has no image.
//! - `PropagationDelayEffect`: fractional-delay line for reflection timing.
//! - `TurbulenceEffect`: level and treble wavering through wind-stirred, sunlit air.
//! - `WallAbsorptionEffect`: per-wall material colouring of a reflection.

mod air_absorption;
mod ground_effect;
mod propagation_delay;
#[cfg(test)]
mod tests;
mod turbulence;
mod wall_absorption;

pub use air_absorption::AirAbsorptionEffect;
pub use ground_effect::GroundEffectFilter;
pub use propagation_delay::PropagationDelayEffect;
pub use turbulence::TurbulenceEffect;
pub use wall_absorption::WallAbsorptionEffect;
