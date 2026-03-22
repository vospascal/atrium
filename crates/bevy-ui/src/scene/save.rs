//! Scene save/load systems — keyboard-triggered JSON persistence.
//!
//! Ctrl+S exports the current ECS state to a JSON file.
//! The saved file uses the stable SceneDescription schema.

use std::path::PathBuf;

use bevy::prelude::*;

use super::export;
use super::schema::SceneDescription;
use crate::ecs::*;

/// Resource tracking the current scene file path.
#[derive(Resource)]
pub struct SceneFilePath {
    pub path: PathBuf,
}

impl Default for SceneFilePath {
    fn default() -> Self {
        Self {
            path: PathBuf::from("scenes/saved.json"),
        }
    }
}

/// System: Ctrl+S saves the current scene to JSON.
pub fn save_scene_on_keypress(
    keyboard: Res<ButtonInput<KeyCode>>,
    description: Res<SceneDescription>,
    file_path: Res<SceneFilePath>,
    sources: Query<(&SoundSourceIndex, &SoundSource, &Transform)>,
    speakers: Query<(&SoundSpeaker, &Transform)>,
    listener: Query<(&SoundListener, &Transform)>,
    environment: Query<&SoundEnvironment>,
    atrium: Query<&SoundAtrium>,
) {
    let ctrl = keyboard.pressed(KeyCode::SuperLeft) || keyboard.pressed(KeyCode::SuperRight);
    if !(ctrl && keyboard.just_pressed(KeyCode::KeyS)) {
        return;
    }

    let source_data: Vec<_> = sources
        .iter()
        .map(|(idx, source, transform)| (*idx, source.clone(), transform.translation))
        .collect();

    let speaker_data: Vec<_> = speakers
        .iter()
        .map(|(speaker, transform)| (speaker.clone(), transform.translation))
        .collect();

    let listener_data = listener.iter().next().map(|(l, t)| (l, t.translation));

    let env = environment.iter().next();
    let atr = atrium.iter().next();

    let exported = export::export_scene(
        &description,
        &source_data,
        &speaker_data,
        listener_data,
        env,
        atr,
    );

    match serde_json::to_string_pretty(&exported) {
        Ok(json) => match std::fs::write(&file_path.path, &json) {
            Ok(()) => {
                info!("Scene saved to {}", file_path.path.display());
            }
            Err(error) => {
                error!("Failed to save scene: {}", error);
            }
        },
        Err(error) => {
            error!("Failed to serialize scene: {}", error);
        }
    }
}
