//! Tests for the path effects, one file per effect plus the whole chain.

use super::*;
use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::propagation::GroundProperties;
use crate::pipeline::environment::{Environment, FLOOR_WALL_INDEX};
use crate::pipeline::path::{PathContribution, PathKind};
use crate::pipeline::path::{PathEffect, PathEffectContext};
use crate::pipeline::wall_material::WallMaterial;
use atrium_core::types::Vec3;

mod air_absorption;
mod chain;
mod ground_effect;
mod propagation_delay;
mod turbulence;
mod wall_absorption;

fn default_wall_materials() -> [WallMaterial; 6] {
    std::array::from_fn(|_| WallMaterial::default())
}

/// Hard walls around an `open` floor: no floor image source, so the ISO
/// ground term is what models the ground.
fn open_floor_wall_materials() -> [WallMaterial; 6] {
    let mut materials = default_wall_materials();
    materials[FLOOR_WALL_INDEX] = WallMaterial::open();
    materials
}

/// The path effects read only the materials, ground and air, so the box
/// bounds are arbitrary but kept the same across the file.
fn test_environment(
    wall_materials: &[WallMaterial; 6],
    ground: &GroundProperties,
    atmosphere: &AtmosphericParams,
) -> Environment {
    Environment {
        min: Vec3::new(-10.0, -10.0, 0.0),
        max: Vec3::new(10.0, 10.0, 10.0),
        wall_materials: wall_materials.clone(),
        ground: ground.clone(),
        atmosphere: atmosphere.clone(),
        air: crate::audio::turbulence::AirMotion::STILL,
    }
}

fn make_ctx(distance: f32) -> (PathContribution, AtmosphericParams, GroundProperties) {
    let path = PathContribution {
        kind: PathKind::Direct,
        direction: Vec3::new(1.0, 0.0, 0.0),
        distance,
        delay_seconds: 0.0,
        gain: 1.0,
        wall_index: None,
    };
    (
        path,
        AtmosphericParams::default(),
        GroundProperties::default(),
    )
}

/// Steady-state gain in dB of any path effect at one frequency, measured
/// with a sine over the second half of one second at 48 kHz.
fn steady_state_response_db(
    effect: &mut dyn PathEffect,
    context: &PathEffectContext,
    frequency: f32,
) -> f32 {
    let sample_rate = 48_000.0;
    effect.reset();
    effect.update(context);
    let total = 48_000;
    let settle = 24_000;
    let mut input_power = 0.0f64;
    let mut output_power = 0.0f64;
    for sample_index in 0..total {
        let input = (std::f32::consts::TAU * frequency * sample_index as f32 / sample_rate).sin();
        let output = effect.process_sample(input);
        if sample_index >= settle {
            input_power += (input as f64).powi(2);
            output_power += (output as f64).powi(2);
        }
    }
    (10.0 * (output_power / input_power).log10()) as f32
}

fn reflection_path(wall_index: u8, delay_seconds: f32) -> PathContribution {
    PathContribution {
        kind: PathKind::Reflection,
        direction: Vec3::new(-1.0, 0.0, 0.0),
        distance: 9.0,
        delay_seconds,
        gain: 1.0,
        wall_index: Some(wall_index),
    }
}
