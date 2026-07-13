//! Live synth-parameter panel (egui). Click a source, drag the sliders, hear
//! the change immediately — each slider sends a `SetSynthParam` command to the
//! audio thread. Only wind-family controls are exposed here; other source
//! types ignore commands they don't model.

use atrium_behavior::CommandSender;
use atrium_core::commands::{Command, SynthParam};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::ecs::{SoundSource, SoundSourceIndex};

/// The source the panel edits (set by clicking a source). Sticky until another
/// source is clicked.
#[derive(Resource, Default)]
pub struct SelectedSource(pub Option<usize>);

/// Current slider values. Reset to baseline defaults whenever the selection
/// changes. (v1: baseline defaults, not the source's exact YAML values.)
#[derive(Resource)]
pub struct SynthPanelState {
    for_index: Option<usize>,
    min_speed: f32,
    max_speed: f32,
    change_time_min: f32,
    change_time_max: f32,
    gust_duration_min: f32,
    gust_duration_max: f32,
    turbulence_time_min: f32,
    turbulence_time_max: f32,
    gust_strength: f32,
    rise_bias: f32,
    gust_brightness: f32,
    turbulence_brightness: f32,
    low_gain: f32,
    body_gain: f32,
    mid_gain: f32,
    presence_gain: f32,
    air_gain: f32,
    turbulence_depth: f32,
    foliage_density: f32,
    leaf_dryness: f32,
    branch_level: f32,
    debris_level: f32,
    structure_level: f32,
    master_gain: f32,
}

impl Default for SynthPanelState {
    fn default() -> Self {
        // Field-wind baseline; selection-specific values are applied below.
        Self {
            for_index: None,
            min_speed: 1.0,
            max_speed: 8.0,
            change_time_min: 20.0,
            change_time_max: 50.0,
            gust_duration_min: 3.0,
            gust_duration_max: 10.0,
            turbulence_time_min: 0.12,
            turbulence_time_max: 0.80,
            gust_strength: 0.35,
            rise_bias: 0.25,
            gust_brightness: 0.0,
            turbulence_brightness: 0.0,
            low_gain: 0.12,
            body_gain: 1.0,
            mid_gain: 0.40,
            presence_gain: 1.0,
            air_gain: 0.28,
            turbulence_depth: 0.20,
            foliage_density: 0.75,
            leaf_dryness: 0.25,
            branch_level: 0.12,
            debris_level: 0.06,
            structure_level: 0.05,
            master_gain: 1.0,
        }
    }
}

/// egui window with synth-specific sliders; changes are sent live.
pub fn synth_param_panel(
    mut contexts: EguiContexts,
    mut selected: ResMut<SelectedSource>,
    sources: Query<(&SoundSourceIndex, &SoundSource)>,
    mut state: ResMut<SynthPanelState>,
    mut command_sender: ResMut<CommandSender>,
) {
    let Some(index) = selected.0 else {
        return;
    };
    // Only wind synth sources get this panel.
    let Some((_, source)) = sources.iter().find(|(i, _)| i.0 == index) else {
        return;
    };
    let synth_kind = source.synth_kind.as_deref();
    if !matches!(
        synth_kind,
        Some("field_wind" | "soft_wind" | "canopy_wind" | "storm_wind")
    ) {
        return;
    }
    let is_canopy = synth_kind == Some("canopy_wind");
    let is_soft = synth_kind == Some("soft_wind");
    let is_storm = synth_kind == Some("storm_wind");
    let name = source.name.clone();

    // Reset sliders to baseline when the selection changes.
    if state.for_index != Some(index) {
        let mut initial = SynthPanelState {
            for_index: Some(index),
            ..Default::default()
        };
        if is_storm {
            initial.min_speed = 8.0;
            initial.max_speed = 18.0;
            initial.change_time_min = 12.0;
            initial.change_time_max = 40.0;
            initial.gust_duration_min = 1.5;
            initial.gust_duration_max = 14.0;
            initial.gust_strength = 0.55;
            initial.rise_bias = 0.45;
            initial.turbulence_depth = 0.65;
            initial.low_gain = 1.03;
            initial.body_gain = 1.50;
            initial.mid_gain = 0.70;
            initial.presence_gain = 0.35;
            initial.master_gain = 0.85;
        } else if is_canopy {
            initial.min_speed = 1.5;
            initial.change_time_min = 15.0;
            initial.change_time_max = 45.0;
            initial.gust_duration_min = 2.0;
            initial.gust_duration_max = 8.0;
            initial.gust_strength = 0.40;
            initial.rise_bias = 0.30;
            initial.turbulence_depth = 0.30;
            initial.body_gain = 0.80;
            initial.presence_gain = 0.90;
            initial.air_gain = 0.75;
        } else if is_soft {
            initial.min_speed = 1.0;
            initial.max_speed = 5.0;
            initial.change_time_min = 10.0;
            initial.change_time_max = 24.0;
            initial.gust_duration_min = 0.8;
            initial.gust_duration_max = 4.0;
            initial.gust_strength = 0.25;
            initial.rise_bias = 0.10;
            initial.turbulence_depth = 0.35;
            initial.turbulence_time_min = 0.05;
            initial.turbulence_time_max = 0.40;
            initial.gust_brightness = 0.18;
            initial.turbulence_brightness = 0.10;
            initial.low_gain = 0.02;
            initial.body_gain = 0.55;
            initial.mid_gain = 1.05;
            initial.presence_gain = 2.25;
            initial.air_gain = 0.11;
        } else {
            initial.presence_gain = 1.10;
            initial.air_gain = 0.08;
        }
        *state = initial;
    }

    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    let mut open = true;
    let title = match synth_kind {
        Some("storm_wind") => "Storm Wind",
        Some("canopy_wind") => "Canopy Wind",
        Some("soft_wind") => "Soft Wind",
        Some("field_wind") => "Field Wind",
        _ => unreachable!("non-wind synth passed the wind-panel filter"),
    };
    egui::Window::new(format!("{title} — {name}"))
        .default_width(280.0)
        .open(&mut open) // adds the ✕ close button
        .show(ctx, |ui| {
            let i = index as u16;
            let mut row = |ui: &mut egui::Ui,
                           label: &str,
                           value: &mut f32,
                           range: std::ops::RangeInclusive<f32>,
                           param: SynthParam| {
                if ui
                    .add(egui::Slider::new(value, range).text(label))
                    .changed()
                {
                    command_sender.send(Command::SetSynthParam {
                        index: i,
                        param,
                        value: *value,
                    });
                }
            };

            ui.label("Wind driver");
            row(
                ui,
                "minimum speed (m/s)",
                &mut state.min_speed,
                0.0..=25.0,
                SynthParam::MinSpeed,
            );
            row(
                ui,
                "maximum speed (m/s)",
                &mut state.max_speed,
                0.0..=25.0,
                SynthParam::MaxSpeed,
            );
            row(
                ui,
                "evolution minimum (s)",
                &mut state.change_time_min,
                1.0..=120.0,
                SynthParam::ChangeTimeMin,
            );
            row(
                ui,
                "evolution maximum (s)",
                &mut state.change_time_max,
                1.0..=180.0,
                SynthParam::ChangeTimeMax,
            );
            row(
                ui,
                "gust minimum (s)",
                &mut state.gust_duration_min,
                0.2..=60.0,
                SynthParam::GustDurationMin,
            );
            row(
                ui,
                "gust maximum (s)",
                &mut state.gust_duration_max,
                0.2..=90.0,
                SynthParam::GustDurationMax,
            );
            row(
                ui,
                "turbulence minimum (s)",
                &mut state.turbulence_time_min,
                0.02..=5.0,
                SynthParam::TurbulenceTimeMin,
            );
            row(
                ui,
                "turbulence maximum (s)",
                &mut state.turbulence_time_max,
                0.02..=10.0,
                SynthParam::TurbulenceTimeMax,
            );
            row(
                ui,
                "gust strength",
                &mut state.gust_strength,
                0.0..=1.0,
                SynthParam::GustStrength,
            );
            row(
                ui,
                "rise bias",
                &mut state.rise_bias,
                -1.0..=1.0,
                SynthParam::RiseBias,
            );
            row(
                ui,
                "turbulence",
                &mut state.turbulence_depth,
                0.0..=1.0,
                SynthParam::TurbulenceDepth,
            );
            row(
                ui,
                "gust brightness",
                &mut state.gust_brightness,
                0.0..=1.0,
                SynthParam::GustBrightness,
            );
            row(
                ui,
                "turbulence brightness",
                &mut state.turbulence_brightness,
                0.0..=1.0,
                SynthParam::TurbulenceBrightness,
            );

            if is_canopy {
                ui.separator();
                ui.label("Canopy character");
                row(
                    ui,
                    "foliage density",
                    &mut state.foliage_density,
                    0.0..=1.0,
                    SynthParam::FoliageDensity,
                );
                row(
                    ui,
                    "leaf dryness",
                    &mut state.leaf_dryness,
                    0.0..=1.0,
                    SynthParam::LeafDryness,
                );
                row(
                    ui,
                    "branch activity",
                    &mut state.branch_level,
                    0.0..=1.0,
                    SynthParam::BranchLevel,
                );

                ui.separator();
                ui.label("Canopy mix");
                row(
                    ui,
                    "foliage body",
                    &mut state.body_gain,
                    0.0..=2.0,
                    SynthParam::BodyGain,
                );
                row(
                    ui,
                    "leaf wash",
                    &mut state.presence_gain,
                    0.0..=2.0,
                    SynthParam::PresenceGain,
                );
                row(
                    ui,
                    "leaf contacts",
                    &mut state.air_gain,
                    0.0..=2.0,
                    SynthParam::AirGain,
                );
            } else if is_storm {
                ui.separator();
                ui.label("Storm interactions");
                row(
                    ui,
                    "debris activity",
                    &mut state.debris_level,
                    0.0..=1.0,
                    SynthParam::DebrisLevel,
                );
                row(
                    ui,
                    "structural strain",
                    &mut state.structure_level,
                    0.0..=1.0,
                    SynthParam::StructureLevel,
                );

                ui.separator();
                ui.label("Storm filterbank");
                row(
                    ui,
                    "pressure / rumble",
                    &mut state.low_gain,
                    0.0..=2.0,
                    SynthParam::LowGain,
                );
                row(
                    ui,
                    "broadband roar",
                    &mut state.body_gain,
                    0.0..=2.0,
                    SynthParam::BodyGain,
                );
                row(
                    ui,
                    "turbulent shear",
                    &mut state.mid_gain,
                    0.0..=2.0,
                    SynthParam::MidGain,
                );
                row(
                    ui,
                    "high-speed tearing",
                    &mut state.presence_gain,
                    0.0..=1.0,
                    SynthParam::PresenceGain,
                );
            } else {
                ui.separator();
                ui.label("Noise filterbank");
                row(
                    ui,
                    "low",
                    &mut state.low_gain,
                    0.0..=2.0,
                    SynthParam::LowGain,
                );
                row(
                    ui,
                    "body",
                    &mut state.body_gain,
                    0.0..=2.0,
                    SynthParam::BodyGain,
                );
                row(
                    ui,
                    "mid",
                    &mut state.mid_gain,
                    0.0..=2.0,
                    SynthParam::MidGain,
                );
                row(
                    ui,
                    "presence",
                    &mut state.presence_gain,
                    0.0..=3.0,
                    SynthParam::PresenceGain,
                );
                row(
                    ui,
                    "air",
                    &mut state.air_gain,
                    0.0..=1.0,
                    SynthParam::AirGain,
                );
            }
            row(
                ui,
                "master_gain",
                &mut state.master_gain,
                0.0..=2.0,
                SynthParam::MasterGain,
            );
        });

    // ✕ pressed → deselect (closes the panel).
    if !open {
        selected.0 = None;
    }
}
