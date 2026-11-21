//! Building one sound of a scene: the single path a scene's own sounds and a
//! sound added while it plays both go through, so the two cannot differ.

use atrium_core::directivity::DirectivityPattern;
use atrium_core::source::{EmitterKind, SoundSource};
use atrium_core::types::Vec3;
use atrium_world::{Entity, EntityId, Moves, Place};

use super::build::{
    arr_to_vec3, build_one_source, build_one_synth_source, source_color, BuiltSource, SourceMeta,
};
use super::description::{beam_half_angle, SourceDescription, TakeDescription};
use super::scene::NormalizationConfig;
use super::source::{display_name, resolve, ResolvedSound};
use crate::audio::silence_node::SilenceNode;
use crate::audio::spectral_profile::BARK_BANDS;

/// What it takes to build a sound for one scene: its loudness calibration,
/// its distance model, and where the scene sits in the world.
#[derive(Clone)]
pub struct SoundBuilder {
    normalization: NormalizationConfig,
    reference_distance: f32,
    max_distance: f32,
    spawn: Vec3,
}

/// A sound ready to be handed to the audio thread, and how a UI shows it.
pub struct BuiltSound {
    pub source: Box<dyn SoundSource>,
    /// 24 Bark-band spectral profile (dB relative to RMS).
    pub bands: [f32; BARK_BANDS],
    /// Base playback amplitude, before spatial attenuation.
    pub amplitude: f32,
    pub description: SourceDescription,
}

impl SoundBuilder {
    pub(super) fn new(
        normalization: NormalizationConfig,
        reference_distance: f32,
        max_distance: f32,
        spawn: Vec3,
    ) -> Self {
        Self {
            normalization,
            reference_distance,
            max_distance,
            spawn,
        }
    }

    /// Build `entity` for the voice `slot`. Decodes its recording or measures
    /// its generator, so it takes time and belongs off the audio thread.
    pub fn build(
        &self,
        id: &EntityId,
        entity: &Entity,
        slot: usize,
    ) -> Result<BuiltSound, Box<dyn std::error::Error>> {
        let (built, meta) = self.build_parts(id, entity, slot)?;
        let description = SourceDescription {
            id: meta.id,
            slot: slot as u16,
            width: meta.width,
            behind: meta.behind,
            takes: meta.takes,
            gap: meta.gap,
            name: meta.name,
            group: meta.group,
            color: meta.color,
            position: arr_to_vec3(meta.position) + self.spawn,
            spl: meta.spl,
            reference_distance: meta.ref_dist,
            emitter_kind: meta.emitter_kind,
            beam_half_angle: beam_half_angle(meta.emitter_kind, built.source.directivity()),
            facing: built.source.orientation(),
        };
        Ok(BuiltSound {
            source: built.source,
            bands: built.bands,
            amplitude: built.amplitude,
            description,
        })
    }

    /// Where a point of the world is in the scene's own coordinates, the ones
    /// an entity's place is written in.
    pub fn place_at(&self, world: Vec3) -> Place {
        let local = world - self.spawn;
        Place {
            x: local.x,
            y: local.y,
            z: local.z,
        }
    }

    /// The placeholder that fills a voice when its sound is taken out.
    pub fn silence() -> Box<dyn SoundSource> {
        Box::new(SilenceNode)
    }

    pub(super) fn build_parts(
        &self,
        id: &EntityId,
        entity: &Entity,
        slot: usize,
    ) -> Result<(BuiltSource, SourceMeta), Box<dyn std::error::Error>> {
        // Merge the preset (if any) with what the entity changes.
        let resolved = resolve(entity)?;
        let place: [f32; 3] = entity.place.into();
        let position = arr_to_vec3(place) + self.spawn;
        let (orbit_radius, orbit_speed) = match entity.moves {
            Moves::Still => (0.0, 0.0),
            Moves::Orbit { radius, speed } => (radius, speed),
        };
        let built = match &resolved.sound {
            ResolvedSound::Sample { takes, rests } => {
                if rests.is_some() && entity.moves != Moves::Still {
                    return Err(format!(
                        "sound `{id}`: a voice that rests between takes cannot orbit yet"
                    )
                    .into());
                }
                build_one_source(
                    takes,
                    *rests,
                    // Slot-stable, like a generator's: each voice keeps its own rhythm,
                    // and the same way when the scene is loaded again.
                    1000 + slot as u64,
                    resolved.reference_spl,
                    DirectivityPattern::beam(resolved.width, resolved.behind),
                    resolved.spread,
                    position,
                    orbit_radius,
                    orbit_speed,
                    &self.normalization,
                    self.reference_distance,
                    self.max_distance,
                )?
            }
            ResolvedSound::Synth { spec } => {
                if entity.moves != Moves::Still {
                    return Err(format!("sound `{id}`: a generator cannot orbit yet").into());
                }
                build_one_synth_source(
                    spec,
                    resolved.reference_spl,
                    DirectivityPattern::beam(resolved.width, resolved.behind),
                    resolved.spread,
                    position,
                    // Slot-stable default seed: reloading the scene sounds
                    // statistically identical, multiple synths decorrelate.
                    1000 + slot as u64,
                    &self.normalization,
                    self.reference_distance,
                    self.max_distance,
                )?
            }
        };

        let mut built = built;
        if let Some(bearing) = entity.shape.facing {
            built.source.set_facing(yaw_for_bearing(bearing));
        }

        let synth_kind = match &resolved.sound {
            ResolvedSound::Synth { spec } => Some(spec.kind_name().to_string()),
            ResolvedSound::Sample { .. } => None,
        };
        let meta = SourceMeta {
            id: id.to_string(),
            takes: match &resolved.sound {
                ResolvedSound::Sample { takes, .. } => takes
                    .iter()
                    .map(|take| TakeDescription {
                        name: take.name.clone(),
                        chance: take.chance,
                    })
                    .collect(),
                ResolvedSound::Synth { .. } => Vec::new(),
            },
            gap: match &resolved.sound {
                ResolvedSound::Sample {
                    rests: Some(rests), ..
                } => Some([rests.gap_seconds.0, rests.gap_seconds.1]),
                _ => None,
            },
            group: entity.group.clone(),
            name: display_name(id, entity),
            color: source_color(entity, slot),
            spl: resolved.reference_spl,
            ref_dist: built.ref_dist,
            amplitude: built.amplitude,
            audible_radius: built.audible_radius,
            width: resolved.width,
            behind: resolved.behind,
            directivity_alpha: built.directivity_alpha,
            spread: resolved.spread,
            position: place,
            orbit_radius,
            orbit_speed,
            synth_kind,
            emitter_kind: built.source.emitter_kind(),
        };
        Ok((built, meta))
    }
}

/// The engine's heading for a compass bearing: a bearing is degrees clockwise
/// from north (+Y), a heading is radians counter-clockwise from +X.
fn yaw_for_bearing(bearing_degrees: f32) -> f32 {
    (90.0 - bearing_degrees).to_radians()
}

/// The compass bearing a scene file writes for the engine's heading `yaw`:
/// degrees clockwise from north, from 0 up to 360.
pub fn bearing_for_heading(yaw: f32) -> f32 {
    (90.0 - yaw.to_degrees()).rem_euclid(360.0)
}

/// How the engine radiates a sound `width_degrees` wide with `behind_db`
/// behind it, and how wide its beam is drawn: `None` for one that goes out
/// all round.
pub fn radiation_of(width_degrees: f32, behind_db: f32) -> (DirectivityPattern, Option<f32>) {
    let radiation = DirectivityPattern::beam(width_degrees, behind_db);
    (radiation, beam_half_angle(EmitterKind::Object, radiation))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// North is +Y, which is a heading of π/2; east is +X, heading 0; and a
    /// bearing turns clockwise while a heading turns the other way.
    #[test]
    fn a_compass_bearing_becomes_the_engines_heading() {
        use std::f32::consts::{FRAC_PI_2, PI};
        assert!((yaw_for_bearing(0.0) - FRAC_PI_2).abs() < 1e-6);
        assert!(yaw_for_bearing(90.0).abs() < 1e-6);
        assert!(
            (yaw_for_bearing(270.0).cos() - PI.cos()).abs() < 1e-6,
            "west"
        );
        assert!(
            (yaw_for_bearing(180.0).sin() + 1.0).abs() < 1e-6,
            "south is −Y"
        );
        for bearing in [0.0, 45.0, 90.0, 180.0, 270.0, 359.0] {
            let back = bearing_for_heading(yaw_for_bearing(bearing));
            assert!(
                (back - bearing).abs() < 1e-3,
                "{bearing} came back as {back}"
            );
        }
    }

    /// A beam is drawn as wide as it was given; all round has no beam to draw.
    #[test]
    fn a_width_gives_the_beam_a_map_draws() {
        assert_eq!(radiation_of(360.0, -60.0).1, None);
        let (_, half) = radiation_of(180.0, -60.0);
        assert!((half.expect("a beam") - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        let (_, narrow) = radiation_of(60.0, -12.0);
        assert!((narrow.expect("a beam") - 30f32.to_radians()).abs() < 1e-5);
    }
}
