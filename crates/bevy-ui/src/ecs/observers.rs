//! Change detection systems: ECS mutations → audio engine commands.
//!
//! Watches `Changed<SoundSource>` and `Changed<SoundSpeaker>` to sync
//! property edits (from inspector, gizmo, or scene reload) to the audio engine.
//!
//! Position syncing for sources is NOT handled here — that's done by
//! the drag system (scene/mod.rs). Transform is updated every frame from
//! telemetry, so watching Changed<Transform> would create a feedback loop.

use bevy::prelude::*;

use super::components::*;
use crate::telemetry::CommandSender;
use atrium_core::commands::Command;
use atrium_core::types::Vec3 as AtriumVec3;

/// Sync `SoundSource` component property changes to the audio engine.
///
/// Fires when any field on `SoundSource` is mutated (e.g. via inspector).
/// Sends the appropriate commands for spread, orbit speed, and orbit radius.
pub fn sync_source_properties(
    sources: Query<(&SoundSourceIndex, &SoundSource), Changed<SoundSource>>,
    mut command_sender: ResMut<CommandSender>,
) {
    for (index, source) in &sources {
        let idx = index.0 as u16;

        command_sender.send(Command::SetSourceSpread {
            index: idx,
            spread: source.spread,
        });
        command_sender.send(Command::SetSourceOrbitSpeed {
            index: idx,
            speed: source.orbit_speed,
        });
        command_sender.send(Command::SetSourceOrbitRadius {
            index: idx,
            radius: source.orbit_radius,
        });
    }
}

/// Sync `SoundSpeaker` position changes to the audio engine.
///
/// Unlike sources, speaker positions are NOT updated from telemetry,
/// so Changed<Transform> is safe to watch here.
pub fn sync_speaker_positions(
    speakers: Query<(&SoundSpeaker, &Transform), Changed<Transform>>,
    mut command_sender: ResMut<CommandSender>,
) {
    for (speaker, transform) in &speakers {
        // Convert Bevy Y-up back to Atrium Z-up
        let position = AtriumVec3::new(
            transform.translation.x,
            -transform.translation.z,
            transform.translation.y,
        );
        command_sender.send(Command::SetSpeakerPosition {
            channel: speaker.channel as u8,
            position,
        });
    }
}
