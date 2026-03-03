use serde::Deserialize;

use crate::engine::commands::Command;
use crate::world::types::Vec3;

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
        }
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
}
