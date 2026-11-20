//! Building one sound of a scene: the single path a scene's own sounds and a
//! sound added while it plays both go through, so the two cannot differ.

use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;
use atrium_world::{Entity, EntityId, Moves, Place};

use super::build::{
    arr_to_vec3, build_one_source, build_one_synth_source, source_color, BuiltSource, SourceMeta,
};
use super::description::{beam_half_angle, SourceDescription};
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
            ResolvedSound::Sample { audio_path } => build_one_source(
                audio_path,
                resolved.reference_spl,
                &resolved.directivity,
                resolved.spread,
                position,
                orbit_radius,
                orbit_speed,
                &self.normalization,
                self.reference_distance,
                self.max_distance,
            )?,
            ResolvedSound::Synth { spec } => {
                if entity.moves != Moves::Still {
                    return Err(format!("sound `{id}`: a generator cannot orbit yet").into());
                }
                build_one_synth_source(
                    spec,
                    resolved.reference_spl,
                    &resolved.directivity,
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

        let synth_kind = match &resolved.sound {
            ResolvedSound::Synth { spec } => Some(spec.kind_name().to_string()),
            ResolvedSound::Sample { .. } => None,
        };
        let meta = SourceMeta {
            id: id.to_string(),
            group: entity.group.clone(),
            name: display_name(id, entity),
            color: source_color(entity, slot),
            spl: resolved.reference_spl,
            ref_dist: built.ref_dist,
            amplitude: built.amplitude,
            audible_radius: built.audible_radius,
            directivity: resolved.directivity.clone(),
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
