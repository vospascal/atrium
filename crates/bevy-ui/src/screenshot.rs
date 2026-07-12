//! Automated screenshot tour for visual verification.
//!
//! When `ATRIUM_SCREENSHOT_DIR` is set, the app steps through a sequence of
//! states, saves a PNG of each to that directory, and exits. Uses Bevy's own
//! render capture, so no OS screen-recording permission is needed.
//!
//! `ATRIUM_SCREENSHOT_TOUR=modes` cycles the five render modes (verifying the
//! per-mode speaker visuals); anything else cycles the biome × day/night themes.

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

use atrium_core::commands::Command;
use atrium_core::speaker::RenderMode;

use crate::scene::landscape::{
    self, Biome, FloorSprite, LandscapeDecor, LandscapeTheme, TimeOfDay,
};
use crate::scene::reload::{PendingReload, PendingSave, ReloadTarget};
use crate::scene::SceneDescription;
use crate::telemetry::CommandSender;

/// One step in the tour: apply a state (theme / render mode / scene) then screenshot.
#[derive(Clone)]
enum Step {
    Theme(LandscapeTheme),
    Mode(RenderMode),
    Scene(std::path::PathBuf),
    Save(std::path::PathBuf),
}

impl Step {
    fn label(&self) -> String {
        match self {
            Step::Theme(theme) => format!("{:?}_{:?}", theme.biome, theme.time_of_day),
            Step::Mode(mode) => format!("mode_{}", mode.as_str()),
            Step::Scene(path) => format!(
                "scene_{}",
                path.file_stem().and_then(|s| s.to_str()).unwrap_or("?")
            ),
            Step::Save(path) => format!(
                "save_{}",
                path.file_stem().and_then(|s| s.to_str()).unwrap_or("?")
            ),
        }
    }
}

enum TourStage {
    /// Apply `steps[index]` (theme retint or render-mode command).
    Apply,
    /// Screenshot `steps[index]`, then advance (or exit).
    Capture,
    /// All steps captured; exit next tick (grace time for the last disk write).
    Exit,
}

#[derive(Resource)]
pub(crate) struct ScreenshotTour {
    directory: String,
    timer: Timer,
    steps: Vec<Step>,
    index: usize,
    stage: TourStage,
}

impl ScreenshotTour {
    pub fn new(directory: String) -> Self {
        let steps = match std::env::var("ATRIUM_SCREENSHOT_TOUR").as_deref() {
            Ok("modes") => RenderMode::ALL.iter().copied().map(Step::Mode).collect(),
            Ok("scenes") => [
                "scenes/default.yaml",
                "scenes/nature.yaml",
                "scenes/default.yaml",
            ]
            .iter()
            .map(|p| Step::Scene(std::path::PathBuf::from(p)))
            .collect(),
            // Round-trip test: load nature, save it, reload the saved file.
            Ok("save") => vec![
                Step::Scene(std::path::PathBuf::from("scenes/nature.yaml")),
                Step::Save(std::path::PathBuf::from("scenes/saved.yaml")),
                Step::Scene(std::path::PathBuf::from("scenes/saved.yaml")),
            ],
            _ => [
                Biome::Wetland,
                Biome::Jungle,
                Biome::Desert,
                Biome::Snow,
                Biome::Beach,
            ]
            .iter()
            .flat_map(|&biome| {
                [TimeOfDay::Night, TimeOfDay::Day]
                    .iter()
                    .map(move |&time_of_day| Step::Theme(LandscapeTheme { biome, time_of_day }))
            })
            .collect(),
        };

        Self {
            directory,
            timer: Timer::from_seconds(1.0, TimerMode::Repeating),
            steps,
            index: 0,
            // Always apply a step before capturing it — the app's initial
            // render mode (VBAP) need not match steps[0].
            stage: TourStage::Apply,
        }
    }
}

/// Tick the tour: capture → switch → capture → … → exit.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_screenshot_tour(
    time: Res<Time>,
    mut tour: ResMut<ScreenshotTour>,
    mut commands: Commands,
    mut theme: ResMut<LandscapeTheme>,
    mut clear_color: ResMut<ClearColor>,
    decor: Query<Entity, With<LandscapeDecor>>,
    mut floor: Query<&mut Sprite, With<FloorSprite>>,
    description: Res<SceneDescription>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut command_sender: ResMut<CommandSender>,
    mut pending: ResMut<PendingReload>,
    mut pending_save: ResMut<PendingSave>,
    mut exit: MessageWriter<AppExit>,
) {
    if !tour.timer.tick(time.delta()).just_finished() {
        return;
    }

    match tour.stage {
        TourStage::Apply => {
            match &tour.steps[tour.index] {
                Step::Theme(next) => {
                    let next = *next;
                    *theme = next;
                    landscape::apply_theme(
                        &mut commands,
                        &mut clear_color,
                        &decor,
                        &mut floor,
                        &description,
                        &mut meshes,
                        &mut materials,
                        next,
                    );
                }
                Step::Mode(mode) => {
                    // Engine echoes the new mode back via telemetry; the next
                    // tick (1 s later) is plenty for it to settle before capture.
                    command_sender.send(Command::SetRenderMode { mode: *mode });
                }
                Step::Scene(path) => {
                    // Reload driver rebuilds audio + UI; captured next tick.
                    pending.0 = Some(ReloadTarget::ScenePath(path.clone()));
                }
                Step::Save(path) => {
                    pending_save.0 = Some(path.clone());
                }
            }
            tour.stage = TourStage::Capture;
        }
        TourStage::Capture => {
            let label = tour.steps[tour.index].label();
            let path = format!("{}/{:02}_{}.png", tour.directory, tour.index, label).to_lowercase();
            info!("Screenshot tour: capturing {path}");
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path));
            tour.index += 1;
            tour.stage = if tour.index >= tour.steps.len() {
                TourStage::Exit
            } else {
                TourStage::Apply
            };
        }
        TourStage::Exit => {
            info!("Screenshot tour: done, exiting");
            exit.write(AppExit::Success);
        }
    }
}
