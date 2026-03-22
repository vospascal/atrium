//! Data-bearing ECS components for the Atrium audio scene.
//!
//! These components hold the authoritative runtime state of the audio scene.
//! All derive `Reflect` + `Serialize` + `Deserialize` for future editor/inspector
//! integration and JSON scene persistence.
//!
//! Position lives on Bevy's `Transform` component, not here — these hold
//! audio-domain properties only.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

// ── Audio source ────────────────────────────────────────────────────────────

/// Authoring data for a sound source. Lives on the same entity as Transform,
/// Mesh3d, etc. Position is on Transform (in Bevy coordinates).
#[derive(Component, Reflect, Serialize, Deserialize, Clone, Debug)]
#[reflect(Component, Serialize, Deserialize)]
pub struct SoundSource {
    /// Stable identifier for persistence and cross-references.
    pub id: String,
    /// Display name shown in HUD and labels.
    pub name: String,
    /// RGB color in 0.0..1.0 range.
    pub color: [f32; 3],
    /// Reference SPL at 1 meter (dB).
    pub spl: f32,
    /// Reference distance for distance attenuation (meters).
    pub ref_distance: f32,
    /// Directivity type: "omni", "cardioid", "supercardioid".
    pub directivity: String,
    /// Alpha parameter for polar patterns (1.0 = omni, 0.5 = cardioid).
    pub directivity_alpha: f32,
    /// MDAP spread [0.0, 1.0].
    pub spread: f32,
    /// Orbit radius (0.0 = stationary).
    pub orbit_radius: f32,
    /// Orbit angular speed (rad/s).
    pub orbit_speed: f32,
}

/// Runtime-only mapping from this Bevy entity to the audio engine's source index.
/// NOT serialized — ephemeral, rebuilt each time a scene loads.
#[derive(Component, Debug, Clone, Copy)]
pub struct SoundSourceIndex(pub usize);

// ── Listener ────────────────────────────────────────────────────────────────

/// Authoring data for the listener. Position is on Transform.
/// At runtime, yaw is driven by the camera system.
#[derive(Component, Reflect, Serialize, Deserialize, Clone, Debug)]
#[reflect(Component, Serialize, Deserialize)]
pub struct SoundListener {
    /// Stable identifier.
    pub id: String,
    /// Initial yaw in degrees (serialized). Runtime yaw is camera-driven.
    pub yaw_degrees: f32,
}

// ── Speaker ─────────────────────────────────────────────────────────────────

/// Authoring data for a physical speaker. Position is on Transform.
#[derive(Component, Reflect, Serialize, Deserialize, Clone, Debug)]
#[reflect(Component, Serialize, Deserialize)]
pub struct SoundSpeaker {
    /// Stable identifier (e.g. "fl", "rr").
    pub id: String,
    /// Display label (e.g. "FL", "RR").
    pub label: String,
    /// Output channel index.
    pub channel: usize,
}

// ── Environment ─────────────────────────────────────────────────────────────

/// The virtual acoustic space — dimensions and spawn point.
/// No Transform needed; dimensions define the simulation bounds.
#[derive(Component, Reflect, Serialize, Deserialize, Clone, Debug)]
#[reflect(Component, Serialize, Deserialize)]
pub struct SoundEnvironment {
    pub id: String,
    pub width: f32,
    pub depth: f32,
    pub height: f32,
    /// Spawn point: where the atrium center sits in the environment.
    pub spawn: [f32; 3],
}

// ── Atrium ──────────────────────────────────────────────────────────────────

/// The physical speaker room — dimensions only.
#[derive(Component, Reflect, Serialize, Deserialize, Clone, Debug)]
#[reflect(Component, Serialize, Deserialize)]
pub struct SoundAtrium {
    pub id: String,
    pub width: f32,
    pub depth: f32,
    pub height: f32,
}
