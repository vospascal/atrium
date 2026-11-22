//! Rendering scenes to files, by subject. Each renders shipped scenes
//! through the whole engine, so they take some seconds.

mod comparison;
mod engine;
mod files;
mod live_analysis;
mod probe;
mod reproducible;
mod start;

use super::{Output, RenderSettings, Rendered, Rendering};
use crate::config::SceneConfig;
use atrium_core::speaker::RenderMode;

const MEADOW: &str = "scenes/meadow-birds.yaml";
const STORM: &str = "scenes/passing-storm.yaml";

fn render(scene: &str, settings: &RenderSettings) -> Rendered {
    let config = SceneConfig::load(scene).unwrap_or_else(|error| panic!("{scene}: {error}"));
    Rendering::prepare(config, settings)
        .unwrap_or_else(|error| panic!("{scene}: {error}"))
        .run()
}

/// `seconds` of the scene on headphones.
fn on_headphones(seconds: f64) -> RenderSettings {
    RenderSettings {
        seconds,
        output: Output::Headphones,
        ..RenderSettings::default()
    }
}

/// `seconds` of the scene on its 5.1 speakers in VBAP, the weather running
/// a minute a second so a storm comes through: wind, rain and thunder.
fn storm_on_speakers(seconds: f64) -> RenderSettings {
    RenderSettings {
        seconds,
        output: Output::Mode(RenderMode::Vbap),
        weather_speed: 60.0,
        ..RenderSettings::default()
    }
}

/// Whether two renders are the same file, bit for bit.
fn identical(first: &[f32], second: &[f32]) -> bool {
    first.len() == second.len()
        && first
            .iter()
            .zip(second)
            .all(|(first, second)| first.to_bits() == second.to_bits())
}

fn peak(frames: &[f32]) -> f32 {
    frames
        .iter()
        .fold(0.0, |peak, sample| peak.max(sample.abs()))
}
