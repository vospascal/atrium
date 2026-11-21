//! The scene description serialized to JSON for a UI client: every computed
//! value (world coordinates, calibrated source levels, speaker layout).

use atrium_core::speaker::{ChannelMode, RenderMode, SpeakerLayout};
use atrium_core::types::Vec3;

use super::build::{arr_to_vec3, parse_render_mode, SourceMeta};
use super::environment::EnvironmentConfig;
use super::scene::{AtriumConfig, SceneConfig};
use crate::audio::atmosphere::AtmosphericParams;

impl SceneConfig {
    pub(super) fn build_scene_json(
        &self,
        environment_cfg: &EnvironmentConfig,
        atrium_cfg: &AtriumConfig,
        spawn: Vec3,
        layout: &SpeakerLayout,
        source_metas: &[SourceMeta],
        atmosphere: &AtmosphericParams,
    ) -> String {
        // Speakers
        let channel_labels = match self.speakers.layout.as_str() {
            "5.1" => &["FL", "FR", "C", "LFE", "RL", "RR"][..],
            "quad" => &["FL", "FR", "—", "—", "RL", "RR"][..],
            _ => &["L", "R"][..],
        };
        let mut speakers = Vec::new();
        for i in 0..layout.speaker_count() {
            if let Some(sp) = layout.speaker_by_index(i) {
                let label = channel_labels.get(sp.channel).unwrap_or(&"?");
                speakers.push(serde_json::json!({
                    "label": label,
                    "x": sp.position.x,
                    "y": sp.position.y,
                    "z": sp.position.z,
                    "channel": sp.channel,
                }));
            }
        }

        // Sources (all computed values from the engine, world coordinates)
        let sources: Vec<_> = source_metas
            .iter()
            .map(|s| {
                let world_pos = arr_to_vec3(s.position) + spawn;
                serde_json::json!({
                    "name": s.name,
                    "color": s.color,
                    "spl": s.spl,
                    "ref_dist": s.ref_dist,
                    "amplitude": s.amplitude,
                    "audible_radius": s.audible_radius,
                    "width": s.width,
                    "behind": s.behind,
                    "directivity_alpha": s.directivity_alpha,
                    "spread": s.spread,
                    "position": [world_pos.x, world_pos.y, world_pos.z],
                    "orbit_radius": s.orbit_radius,
                    "orbit_speed": s.orbit_speed,
                    "synth_kind": s.synth_kind,
                    "emitter_kind": s.emitter_kind.as_str(),
                })
            })
            .collect();

        let listener_world = arr_to_vec3(self.listener.position) + spawn;
        serde_json::json!({
            "type": "scene_state",
            "room": {
                "width": environment_cfg.width,
                "depth": environment_cfg.depth,
                "height": environment_cfg.height,
            },
            "atrium": {
                "width": atrium_cfg.width,
                "depth": atrium_cfg.depth,
                "height": atrium_cfg.height,
            },
            "spawn": {
                "x": spawn.x,
                "y": spawn.y,
                "z": spawn.z,
            },
            "listener": {
                "x": listener_world.x,
                "y": listener_world.y,
                "z": listener_world.z,
                "yaw": self.listener.yaw_degrees.to_radians(),
            },
            "master_gain": self.master_gain,
            "distance_model": {
                "model": self.distance_model.model,
                "ref_distance": self.distance_model.ref_distance,
                "max_distance": self.distance_model.max_distance,
                "rolloff": self.distance_model.rolloff,
            },
            "normalization": {
                "spl_threshold": self.normalization.spl_threshold,
                "target_rms": self.normalization.target_rms,
            },
            "render_mode": self.speakers.render_mode,
            "dbap_rolloff_db": self.speakers.dbap_rolloff_db,
            "channel_mode": ChannelMode::valid_for(parse_render_mode(&self.speakers.render_mode))
                .last().map(|m| m.as_str()).unwrap_or("5.1"),
            "render_modes": RenderMode::ALL.iter().map(|m| {
                serde_json::json!({
                    "mode": m.as_str(),
                    "channel_modes": ChannelMode::valid_for(*m).iter()
                        .map(|c| c.as_str()).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
            "speakers": speakers,
            "total_channels": layout.total_channels(),
            "lfe_channel": layout.lfe_channel(),
            "sources": sources,
            "atmosphere": {
                "temperature_c": atmosphere.temperature_c,
                "humidity_pct": atmosphere.humidity_pct,
                "pressure_kpa": atmosphere.pressure_kpa,
            },
            "experiments": [],
        })
        .to_string()
    }
}
