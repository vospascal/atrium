use serde::Deserialize;

use crate::engine::commands::Command;
use crate::world::types::Vec3;
use atrium_core::speaker::RenderMode;

/// JSON messages received from the browser via WebSocket.
/// Tagged by the "type" field: {"type": "set_listener", "x": 3.0, ...}
#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "set_listener")]
    SetListener { x: f32, y: f32, z: f32, yaw: f32 },

    #[serde(rename = "set_gain")]
    SetGain { gain: f32 },

    #[serde(rename = "set_source_muted")]
    SetSourceMuted { index: u8, muted: bool },

    #[serde(rename = "set_source_position")]
    SetSourcePosition { index: u8, x: f32, y: f32, z: f32 },

    /// Switch rendering mode: "speaker_as_mic" or "vbap".
    #[serde(rename = "set_render_mode")]
    SetRenderMode { mode: String },

    /// Reposition a speaker by channel index.
    #[serde(rename = "set_speaker_position")]
    SetSpeakerPosition { channel: u8, x: f32, y: f32, z: f32 },

    /// Set orbit speed for a source (0 = paused).
    #[serde(rename = "set_source_orbit_speed")]
    SetSourceOrbitSpeed { index: u8, speed: f32 },

    /// Set orbit radius for a source.
    #[serde(rename = "set_source_orbit_radius")]
    SetSourceOrbitRadius { index: u8, radius: f32 },

    /// Set orbit angle for a source.
    #[serde(rename = "set_source_orbit_angle")]
    SetSourceOrbitAngle { index: u8, angle: f32 },

    /// Set atmospheric conditions (temperature, humidity) for ISO 9613-1 air absorption.
    #[serde(rename = "set_atmosphere")]
    SetAtmosphere { temperature: f32, humidity: f32 },

    /// Reset scene to initial state.
    #[serde(rename = "reset_scene")]
    ResetScene,
}

impl ClientMessage {
    /// Convert a JSON client message into an engine Command.
    pub fn into_command(self) -> Command {
        match self {
            ClientMessage::SetListener { x, y, z, yaw } => Command::SetListenerPose {
                position: Vec3::new(x, y, z),
                yaw,
            },
            ClientMessage::SetGain { gain } => Command::SetMasterGain {
                gain: gain.clamp(0.0, 1.0),
            },
            ClientMessage::SetSourceMuted { index, muted } => Command::SetSourceMuted {
                index,
                muted,
            },
            ClientMessage::SetSourcePosition { index, x, y, z } => Command::SetSourcePosition {
                index,
                position: Vec3::new(x, y, z),
            },
            ClientMessage::SetRenderMode { mode } => {
                let render_mode = match mode.as_str() {
                    "vbap" | "5.1" => RenderMode::Vbap,
                    "stereo" => RenderMode::Stereo,
                    "mono" => RenderMode::Mono,
                    "quad" | "4.0" => RenderMode::Quad,
                    "binaural" | "hrtf" => RenderMode::Binaural,
                    _ => RenderMode::SpeakerAsMic,
                };
                Command::SetRenderMode { mode: render_mode }
            }
            ClientMessage::SetSpeakerPosition { channel, x, y, z } => {
                Command::SetSpeakerPosition {
                    channel,
                    position: Vec3::new(x, y, z),
                }
            }
            ClientMessage::SetSourceOrbitSpeed { index, speed } => {
                Command::SetSourceOrbitSpeed { index, speed }
            }
            ClientMessage::SetSourceOrbitRadius { index, radius } => {
                Command::SetSourceOrbitRadius { index, radius }
            }
            ClientMessage::SetSourceOrbitAngle { index, angle } => {
                Command::SetSourceOrbitAngle { index, angle }
            }
            ClientMessage::SetAtmosphere { temperature, humidity } => {
                Command::SetAtmosphere {
                    temperature_c: temperature.clamp(-20.0, 50.0),
                    humidity_pct: humidity.clamp(0.0, 100.0),
                }
            }
            ClientMessage::ResetScene => Command::ResetScene,
        }
    }

    /// Returns true if this message should trigger re-sending the initial scene state.
    pub fn needs_scene_resend(&self) -> bool {
        matches!(self, ClientMessage::ResetScene)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_set_listener() {
        let json = r#"{"type":"set_listener","x":3.0,"y":2.0,"z":0.5,"yaw":1.57}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::SetListener { x, y, z, yaw } => {
                assert!((x - 3.0).abs() < 1e-6);
                assert!((y - 2.0).abs() < 1e-6);
                assert!((z - 0.5).abs() < 1e-6);
                assert!((yaw - 1.57).abs() < 1e-3);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parse_set_gain() {
        let json = r#"{"type":"set_gain","gain":0.75}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::SetGain { gain } => {
                assert!((gain - 0.75).abs() < 1e-6);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parse_unknown_type_fails() {
        let json = r#"{"type":"explode","power":9000}"#;
        let result = serde_json::from_str::<ClientMessage>(json);
        assert!(result.is_err());
    }

    #[test]
    fn into_command_set_listener() {
        let msg = ClientMessage::SetListener {
            x: 1.0,
            y: 2.0,
            z: 3.0,
            yaw: 0.5,
        };
        let cmd = msg.into_command();
        match cmd {
            Command::SetListenerPose { position, yaw } => {
                assert!((position.x - 1.0).abs() < 1e-6);
                assert!((position.y - 2.0).abs() < 1e-6);
                assert!((position.z - 3.0).abs() < 1e-6);
                assert!((yaw - 0.5).abs() < 1e-6);
            }
            _ => panic!("wrong command variant"),
        }
    }

    #[test]
    fn parse_set_source_muted() {
        let json = r#"{"type":"set_source_muted","index":1,"muted":true}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::SetSourceMuted { index, muted } => {
                assert_eq!(index, 1);
                assert!(muted);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parse_set_source_position() {
        let json = r#"{"type":"set_source_position","index":0,"x":2.0,"y":3.0,"z":0.5}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::SetSourcePosition { index, x, y, z } => {
                assert_eq!(index, 0);
                assert!((x - 2.0).abs() < 1e-6);
                assert!((y - 3.0).abs() < 1e-6);
                assert!((z - 0.5).abs() < 1e-6);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn into_command_set_source_muted() {
        let msg = ClientMessage::SetSourceMuted { index: 0, muted: true };
        let cmd = msg.into_command();
        match cmd {
            Command::SetSourceMuted { index, muted } => {
                assert_eq!(index, 0);
                assert!(muted);
            }
            _ => panic!("wrong command variant"),
        }
    }

    #[test]
    fn into_command_clamps_gain() {
        let msg = ClientMessage::SetGain { gain: 5.0 };
        let cmd = msg.into_command();
        match cmd {
            Command::SetMasterGain { gain } => {
                assert!((gain - 1.0).abs() < 1e-6, "gain should clamp to 1.0");
            }
            _ => panic!("wrong command variant"),
        }
    }

    #[test]
    fn parse_set_render_mode_vbap() {
        let json = r#"{"type":"set_render_mode","mode":"vbap"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        let cmd = msg.into_command();
        match cmd {
            Command::SetRenderMode { mode } => {
                assert_eq!(mode, RenderMode::Vbap);
            }
            _ => panic!("wrong command variant"),
        }
    }

    #[test]
    fn parse_set_render_mode_speaker_as_mic() {
        let json = r#"{"type":"set_render_mode","mode":"speaker_as_mic"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        let cmd = msg.into_command();
        match cmd {
            Command::SetRenderMode { mode } => {
                assert_eq!(mode, RenderMode::SpeakerAsMic);
            }
            _ => panic!("wrong command variant"),
        }
    }

    #[test]
    fn parse_set_speaker_position() {
        let json = r#"{"type":"set_speaker_position","channel":2,"x":1.5,"y":3.0,"z":0.0}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        let cmd = msg.into_command();
        match cmd {
            Command::SetSpeakerPosition { channel, position } => {
                assert_eq!(channel, 2);
                assert!((position.x - 1.5).abs() < 1e-6);
                assert!((position.y - 3.0).abs() < 1e-6);
            }
            _ => panic!("wrong command variant"),
        }
    }

    #[test]
    fn parse_set_atmosphere() {
        let json = r#"{"type":"set_atmosphere","temperature":25.0,"humidity":60.0}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        let cmd = msg.into_command();
        match cmd {
            Command::SetAtmosphere { temperature_c, humidity_pct } => {
                assert!((temperature_c - 25.0).abs() < 1e-6);
                assert!((humidity_pct - 60.0).abs() < 1e-6);
            }
            _ => panic!("wrong command variant"),
        }
    }

    #[test]
    fn into_command_clamps_atmosphere() {
        let msg = ClientMessage::SetAtmosphere { temperature: 100.0, humidity: -10.0 };
        let cmd = msg.into_command();
        match cmd {
            Command::SetAtmosphere { temperature_c, humidity_pct } => {
                assert!((temperature_c - 50.0).abs() < 1e-6, "temperature should clamp to 50°C");
                assert!((humidity_pct - 0.0).abs() < 1e-6, "humidity should clamp to 0%");
            }
            _ => panic!("wrong command variant"),
        }
    }

    #[test]
    fn parse_reset_scene() {
        let json = r#"{"type":"reset_scene"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(msg.needs_scene_resend());
        let cmd = msg.into_command();
        assert!(matches!(cmd, Command::ResetScene));
    }
}
