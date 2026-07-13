//! ECS → SceneDescription export.
//!
//! Queries the Bevy world for all sound entities and builds a
//! `SceneDescription` suitable for JSON serialization.
//! Converts Bevy Y-up coordinates back to Atrium Z-up coordinates.

use bevy::prelude::*;

use super::schema::*;
use crate::ecs::*;

/// Convert a world (Bevy 2D) ground-plane position back to Atrium coordinates.
/// Transform.z is the draw-layer index, not a spatial coordinate — the entity's
/// real height is carried by its `AtriumHeight` component.
fn bevy_to_atrium(position: Vec3, height: AtriumHeight) -> [f32; 3] {
    [position.x, position.y, height.0]
}

/// Export the current ECS scene state to a `SceneDescription`.
///
/// Reads the base description (for fields not on entities, like render_mode,
/// distance_model, atmosphere) and overlays the current entity state on top.
pub fn export_scene(
    description: &SceneDescription,
    sources: &[(SoundSourceIndex, SoundSource, Vec3, AtriumHeight)],
    speakers: &[(SoundSpeaker, Vec3, AtriumHeight)],
    listener: Option<(&SoundListener, Vec3, AtriumHeight)>,
    environment: Option<&SoundEnvironment>,
    atrium: Option<&SoundAtrium>,
) -> SceneDescription {
    let source_descriptions: Vec<SourceDescription> = {
        let mut pairs: Vec<_> = sources.iter().collect();
        pairs.sort_by_key(|(idx, _, _, _)| idx.0);
        pairs
            .into_iter()
            .map(|(idx, source, position, height)| SourceDescription {
                id: source.id.clone(),
                slot: idx.0,
                name: source.name.clone(),
                color: color_to_hex(source.color),
                position: bevy_to_atrium(*position, *height),
                emitter_kind: source.emitter_kind.clone(),
                spl: source.spl,
                ref_distance: source.ref_distance,
                directivity: source.directivity.clone(),
                directivity_alpha: source.directivity_alpha,
                spread: source.spread,
                orbit_radius: source.orbit_radius,
                orbit_speed: source.orbit_speed,
                synth_kind: source.synth_kind.clone(),
            })
            .collect()
    };

    let speaker_descriptions: Vec<SpeakerDescription> = speakers
        .iter()
        .map(|(speaker, position, height)| SpeakerDescription {
            id: speaker.id.clone(),
            label: speaker.label.clone(),
            position: bevy_to_atrium(*position, *height),
            channel: speaker.channel,
        })
        .collect();

    let listener_description = listener
        .map(|(l, position, height)| ListenerDescription {
            position: bevy_to_atrium(position, height),
            yaw_degrees: l.yaw_degrees,
        })
        .unwrap_or_else(|| description.listener.clone());

    let environment_description = environment
        .map(|e| EnvironmentDescription {
            width: e.width,
            depth: e.depth,
            height: e.height,
            spawn: e.spawn,
        })
        .unwrap_or_else(|| description.environment.clone());

    let atrium_description = atrium
        .map(|a| AtriumDescription {
            width: a.width,
            depth: a.depth,
            height: a.height,
        })
        .unwrap_or_else(|| description.atrium.clone());

    SceneDescription {
        version: description.version,
        environment: environment_description,
        atrium: atrium_description,
        listener: listener_description,
        sources: source_descriptions,
        speakers: SpeakerLayoutDescription {
            layout: description.speakers.layout.clone(),
            speakers: speaker_descriptions,
            dbap_rolloff_db: description.speakers.dbap_rolloff_db,
        },
        render_mode: description.render_mode.clone(),
        master_gain: description.master_gain,
        distance_model: description.distance_model.clone(),
        atmosphere: description.atmosphere.clone(),
    }
}

/// Bevy system that exports the current scene to a `SceneDescription`.
/// Can be called on-demand (e.g. from a "Save" button).
pub fn export_scene_system(
    description: Res<SceneDescription>,
    sources: Query<(&SoundSourceIndex, &SoundSource, &Transform, &AtriumHeight)>,
    speakers: Query<(&SoundSpeaker, &Transform, &AtriumHeight)>,
    listener: Query<(&SoundListener, &Transform, &AtriumHeight)>,
    environment: Query<&SoundEnvironment>,
    atrium: Query<&SoundAtrium>,
) -> SceneDescription {
    let source_data: Vec<_> = sources
        .iter()
        .map(|(idx, source, transform, height)| {
            (*idx, source.clone(), transform.translation, *height)
        })
        .collect();

    let speaker_data: Vec<_> = speakers
        .iter()
        .map(|(speaker, transform, height)| (speaker.clone(), transform.translation, *height))
        .collect();

    let listener_data = listener
        .iter()
        .next()
        .map(|(l, t, height)| (l, t.translation, *height));

    let env = environment.iter().next();
    let atr = atrium.iter().next();

    export_scene(
        &description,
        &source_data,
        &speaker_data,
        listener_data,
        env,
        atr,
    )
}
