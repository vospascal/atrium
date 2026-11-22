//! The generator lab's own generator: the soft wind an entity names, built
//! as the generator itself rather than as a scene's sound.

use atrium_world::Entity;

use super::source::{resolve, ResolvedSound};
use super::synth::SynthSpec;
use crate::synth::soft_wind::SoftWindSource;

/// The soft wind `entity` makes, its preset with the entity's changes, drawn
/// from `seed` whatever seed the preset pins: the lab chooses the run. Its
/// place is the origin; the lab places the voice it plays in. A sound that is
/// not a soft wind is refused.
pub fn soft_wind_of(
    entity: &Entity,
    seed: u64,
) -> Result<SoftWindSource, Box<dyn std::error::Error>> {
    let resolved = resolve(entity)?;
    let ResolvedSound::Synth {
        spec: SynthSpec::SoftWind(params),
    } = resolved.sound
    else {
        return Err("the lab tunes the soft wind, and this sound is not one".into());
    };
    let mut params = params;
    params.seed = Some(seed);
    Ok(params.build(atrium_core::types::Vec3::ZERO, seed))
}
