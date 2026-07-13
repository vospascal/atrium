//! Runtime scene reload: swap the whole scene (audio + UI) at runtime.
//!
//! The audio engine lives entirely on the audio thread; the `cpal::Stream` is
//! `!Send` and Bevy owns the main thread. So the stream handle is stored as a
//! Bevy **NonSend** resource and rebuilt from a main-thread system. The heavy
//! lifting (load YAML, decode audio, build pipelines, open a fresh stream)
//! happens behind an injected [`SceneHost`] trait object supplied by the
//! top-level `atrium` crate — this keeps `atrium-bevy` free of any `atrium`
//! / yaml / `AudioScene` types (the dependency only runs one way).

use std::path::PathBuf;

use bevy::prelude::*;

use super::landscape::{self, LandscapeTheme};
use super::{import, SceneDescription};
use crate::camera::ListenerState;
use crate::hud;
use crate::input::SourceOrbitSpeeds;
use crate::telemetry::{LatestTelemetry, TelemetryReceiver};
use atrium_behavior::CommandSender;

// ── Injected audio host (implemented in `atrium`) ────────────────────────────

/// Opaque owner of the live audio stream. `atrium` wraps its `Box<dyn StreamHandle>`.
/// bevy-ui only ever stores it and drops it — dropping stops audio. Not `Send`
/// (a `cpal::Stream` isn't), so it is kept in a NonSend resource.
pub trait AudioHandle {}

/// What to (re)load.
pub enum ReloadTarget {
    /// Load a scene YAML from disk (scene picker, or reloading a saved scene).
    ScenePath(PathBuf),
}

/// The product of a reload: a fresh audio stream + the channels and description
/// the Bevy side needs to rewire itself.
pub struct ReloadOutput {
    pub audio: Box<dyn AudioHandle>,
    pub command_sender: CommandSender,
    pub telemetry_receiver: TelemetryReceiver,
    pub description: SceneDescription,
}

// ── Live source edits (add / remove, no audio gap) ──────────────────────────

/// Where a live-added source's audio + definition comes from.
pub enum AddOrigin {
    /// A `sources/*.yaml` preset — the host resolves the audio path, SPL,
    /// directivity, and spread from it.
    Preset(String),
    /// An arbitrary audio file the host builds with sensible defaults.
    AudioFile(String),
}

/// A fully-specified live add-source request handed to the host.
pub struct AddSpec {
    pub origin: AddOrigin,
    /// Where to place the new source, in world coordinates.
    pub position: [f32; 3],
}

/// The result of a successful live add: the pool slot the source landed in plus
/// a description so the UI can spawn the badge, card, and HUD row.
pub struct AddedSource {
    pub slot: u16,
    pub description: super::schema::SourceDescription,
}

/// A live source-pool edit requested by a UI handler; consumed by
/// [`drive_scene_edits`].
pub enum SceneEditRequest {
    /// Add a source from a `sources/*.yaml` preset.
    AddPreset(String),
    /// Open a native file dialog, then add the chosen audio file.
    AddBrowsed,
    /// Remove the source occupying `slot`.
    Remove(u16),
}

/// Set by UI handlers to request a live add/remove; drained by the edit driver.
#[derive(Resource, Default)]
pub struct PendingSceneEdit(pub Option<SceneEditRequest>);

/// Builds/rebuilds the audio scene + stream. Implemented in `atrium` and
/// injected into [`crate::run`]. `!Send` (returns a `!Send` handle) → NonSend.
pub trait SceneHost {
    fn reload(&mut self, target: ReloadTarget) -> Result<ReloadOutput, String>;

    /// Serialize the current scene (with the given live state overlaid) to a
    /// loadable YAML file at `path`. `description` carries the live listener +
    /// source positions in world coordinates.
    fn save(
        &mut self,
        path: &std::path::Path,
        description: &SceneDescription,
    ) -> Result<(), String>;

    /// Build a source and splice it into a free pool slot on the audio thread
    /// (no gap). Records it in the authoritative config so Save captures it.
    fn add_source(&mut self, spec: AddSpec) -> Result<AddedSource, String>;

    /// Retire the source in `slot` from the audio thread and drop it from the
    /// authoritative config.
    fn remove_source(&mut self, slot: u16) -> Result<(), String>;

    /// Open a native file dialog to pick an audio file. `None` if cancelled.
    fn browse_audio(&mut self) -> Option<String>;

    /// Drain the retire channel, dropping displaced source boxes on this (the
    /// control) thread. Called every frame.
    fn drain_retired(&mut self);
}

/// NonSend resource: the live stream handle + the injected builder.
pub struct AudioHost {
    pub current: Box<dyn AudioHandle>,
    pub host: Box<dyn SceneHost>,
}

/// Set by UI handlers (scene picker) to request a reload; drained by the driver.
#[derive(Resource, Default)]
pub struct PendingReload(pub Option<ReloadTarget>);

/// Set by the Save button to request a scene save to the given path.
#[derive(Resource, Default)]
pub struct PendingSave(pub Option<PathBuf>);

/// Filter for both HUD panel roots (rebuilt together on a live edit).
type HudPanels = Or<(With<hud::HudPanel>, With<hud::PipelinePanel>)>;

/// Place a new source a short offset from the listener so it's visible and
/// draggable rather than stacked on top of the listener badge.
fn spawn_near_listener(listener: &ListenerState) -> [f32; 3] {
    [listener.position[0] + 1.5, listener.position[1] + 1.5, 0.0]
}

/// Slot-indexed orbit speeds (length `MAX_SOURCES`) from a description, so the
/// pause button can address a source by its (possibly sparse) slot.
fn orbit_speeds_from_description(description: &SceneDescription) -> Vec<f32> {
    let mut speeds = vec![0.0; atrium_core::telemetry::MAX_SOURCES];
    for source in &description.sources {
        if source.slot < speeds.len() {
            speeds[source.slot] = if source.orbit_radius > 0.0 { 1.0 } else { 0.0 };
        }
    }
    speeds
}

// ── Reload driver ────────────────────────────────────────────────────────────

/// Every scene/HUD root marker — despawned (recursively) on reload. The camera
/// and FPS overlay are deliberately excluded so they persist across reloads.
type SceneEntities = Or<(
    With<crate::ecs::SoundSource>,
    With<crate::ecs::SoundSpeaker>,
    With<crate::ecs::SoundListener>,
    With<crate::ecs::SoundEnvironment>,
    With<crate::ecs::SoundAtrium>,
    With<landscape::FloorSprite>,
    With<landscape::LandscapeDecor>,
    With<super::SourceCard>,
    With<super::SpeakerLabel>,
    With<super::ListenerTag>,
    With<hud::HudPanel>,
    With<hud::PipelinePanel>,
)>;

/// On a pending reload: rebuild the audio stream via the host, then tear down
/// and respawn all scene + HUD entities from the new description, and reset the
/// dependent resources. Runs as a normal system — every spawn helper reads the
/// new `SceneDescription` (passed by ref) rather than querying ECS, so the
/// despawn/spawn command ordering within one flush is safe.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_scene_reload(
    mut commands: Commands,
    mut pending: ResMut<PendingReload>,
    mut audio_host: NonSendMut<AudioHost>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    theme: Res<LandscapeTheme>,
    old_entities: Query<Entity, SceneEntities>,
    mut listener_state: ResMut<ListenerState>,
    mut drag: ResMut<super::SourceDragState>,
    mut latest: ResMut<LatestTelemetry>,
) {
    let Some(target) = pending.0.take() else {
        return;
    };

    let output = match audio_host.host.reload(target) {
        Ok(output) => output,
        Err(err) => {
            error!("scene reload failed: {err}");
            return;
        }
    };

    // Stop the old stream (drop its handle) and install the new one.
    audio_host.current = output.audio;

    // Tear down the old scene + HUD (recursive despawn covers mesh/text children).
    for entity in &old_entities {
        commands.entity(entity).despawn();
    }

    // Rebuild from the new description (read by ref — no ECS query dependency).
    import::spawn_scene(
        &mut commands,
        &mut meshes,
        &mut materials,
        &output.description,
        *theme,
    );
    landscape::spawn_landscape(
        &mut commands,
        &mut meshes,
        &mut materials,
        &output.description,
        *theme,
    );
    hud::build_hud_panels(&mut commands, &output.description);

    // Reset dependent resources.
    listener_state.position = output.description.listener.position;
    listener_state.yaw = output.description.listener.yaw_degrees.to_radians();
    *drag = super::SourceDragState::default();
    *latest = LatestTelemetry::default();
    commands.insert_resource(SourceOrbitSpeeds {
        speeds: orbit_speeds_from_description(&output.description),
    });
    commands.insert_resource(output.command_sender);
    commands.insert_resource(output.telemetry_receiver);
    commands.insert_resource(output.description);

    info!("scene reloaded");
}

/// On a pending save: build a `SceneDescription` from the live ECS (capturing
/// dragged positions), then hand it to the host to serialize as loadable YAML.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_scene_save(
    mut pending: ResMut<PendingSave>,
    mut audio_host: NonSendMut<AudioHost>,
    description: Res<SceneDescription>,
    sources: Query<(
        &crate::ecs::SoundSourceIndex,
        &crate::ecs::SoundSource,
        &Transform,
        &crate::ecs::AtriumHeight,
    )>,
    speakers: Query<(
        &crate::ecs::SoundSpeaker,
        &Transform,
        &crate::ecs::AtriumHeight,
    )>,
    listener: Query<(
        &crate::ecs::SoundListener,
        &Transform,
        &crate::ecs::AtriumHeight,
    )>,
    environment: Query<&crate::ecs::SoundEnvironment>,
    atrium: Query<&crate::ecs::SoundAtrium>,
) {
    let Some(path) = pending.0.take() else {
        return;
    };

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

    let exported = super::export::export_scene(
        &description,
        &source_data,
        &speaker_data,
        listener_data,
        environment.iter().next(),
        atrium.iter().next(),
    );

    match audio_host.host.save(&path, &exported) {
        Ok(()) => info!("scene saved to {}", path.display()),
        Err(err) => error!("scene save failed: {err}"),
    }
}

/// On a pending live edit: add or remove a source via the host (which splices
/// it on the audio thread with no gap), then update the Bevy scene entities,
/// HUD panels, and dependent resources incrementally. Runs before the reload
/// driver in the chain.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_scene_edits(
    mut commands: Commands,
    mut pending: ResMut<PendingSceneEdit>,
    mut audio_host: NonSendMut<AudioHost>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut description: ResMut<SceneDescription>,
    listener: Res<ListenerState>,
    mut orbit: ResMut<SourceOrbitSpeeds>,
    panels: Query<Entity, HudPanels>,
    source_entities: Query<(Entity, &crate::ecs::SoundSourceIndex), With<crate::ecs::SoundSource>>,
    cards: Query<(Entity, &super::SourceCard)>,
) {
    let Some(request) = pending.0.take() else {
        return;
    };

    // Resolve the request into a concrete add spec or a slot to remove.
    let spec_or_remove = match request {
        SceneEditRequest::AddPreset(yaml) => Ok(AddSpec {
            origin: AddOrigin::Preset(yaml),
            position: spawn_near_listener(&listener),
        }),
        SceneEditRequest::AddBrowsed => match audio_host.host.browse_audio() {
            Some(path) => Ok(AddSpec {
                origin: AddOrigin::AudioFile(path),
                position: spawn_near_listener(&listener),
            }),
            None => return, // dialog cancelled
        },
        SceneEditRequest::Remove(slot) => Err(slot),
    };

    match spec_or_remove {
        Ok(spec) => match audio_host.host.add_source(spec) {
            Ok(added) => {
                import::spawn_one_source(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    &added.description,
                    import::map_visual_scale(&description),
                );
                if (added.slot as usize) < orbit.speeds.len() {
                    orbit.speeds[added.slot as usize] = if added.description.orbit_radius > 0.0 {
                        1.0
                    } else {
                        0.0
                    };
                }
                description.sources.push(added.description);
                rebuild_hud(&mut commands, &panels, &description);
                info!("source added to slot {}", added.slot);
            }
            Err(err) => error!("add source failed: {err}"),
        },
        Err(slot) => match audio_host.host.remove_source(slot) {
            Ok(()) => {
                for (entity, index) in &source_entities {
                    if index.0 == slot as usize {
                        commands.entity(entity).despawn();
                    }
                }
                for (entity, card) in &cards {
                    if card.index == slot as usize {
                        commands.entity(entity).despawn();
                    }
                }
                if (slot as usize) < orbit.speeds.len() {
                    orbit.speeds[slot as usize] = 0.0;
                }
                description.sources.retain(|s| s.slot != slot as usize);
                rebuild_hud(&mut commands, &panels, &description);
                info!("source removed from slot {slot}");
            }
            Err(err) => error!("remove source failed: {err}"),
        },
    }
}

/// Despawn both HUD panels and rebuild them from the (updated) description, so
/// the source list, pipeline, and add-source controls reflect the live edit.
fn rebuild_hud(
    commands: &mut Commands,
    panels: &Query<Entity, HudPanels>,
    description: &SceneDescription,
) {
    for entity in panels {
        commands.entity(entity).despawn();
    }
    hud::build_hud_panels(commands, description);
}

/// Every frame, drop any source boxes the audio thread retired (removed or
/// replaced), off the audio thread. Cheap when the channel is empty.
pub(crate) fn drain_retired_sources(mut audio_host: NonSendMut<AudioHost>) {
    audio_host.host.drain_retired();
}
