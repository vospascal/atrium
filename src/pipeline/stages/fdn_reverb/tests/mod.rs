use super::*;
use crate::audio::atmosphere::AtmosphericParams;
use atrium_core::listener::Listener;
use atrium_core::speaker::SpeakerLayout;
use atrium_core::types::Vec3;

fn make_ctx(channels: usize, _render_channels: usize) -> (SpeakerLayout, Listener) {
    let layout = if channels >= 6 {
        SpeakerLayout::surround_5_1(
            Vec3::new(0.0, 4.0, 0.0),
            Vec3::new(6.0, 4.0, 0.0),
            Vec3::new(3.0, 4.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(6.0, 0.0, 0.0),
        )
    } else {
        SpeakerLayout::stereo(Vec3::new(-1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0))
    };
    let listener = Listener::new(Vec3::ZERO, 0.0);
    (layout, listener)
}

use crate::pipeline::environment::Environment;
use crate::pipeline::wall_material::WallMaterial;
use std::sync::LazyLock;

/// A 10 m cube centred on the origin with hard walls and 20 °C air.
static TEST_ENVIRONMENT: LazyLock<Environment> = LazyLock::new(|| {
    Environment::uniform(
        Vec3::new(-5.0, -5.0, -5.0),
        Vec3::new(5.0, 5.0, 5.0),
        WallMaterial::HARD_WALL,
    )
});

fn mix_ctx<'a>(
    layout: &'a SpeakerLayout,
    listener: &'a Listener,
    channels: usize,
    render_channels: usize,
) -> MixContext<'a> {
    MixContext {
        listener,
        layout,
        sample_rate: 48000.0,
        channels,
        environment: &TEST_ENVIRONMENT,
        master_gain: 1.0,
        render_channels,
        reverb_input: None,
        measurement_mode: false,
    }
}

mod configuration;
mod network;
mod normalization;
mod physics;
mod room_derivation;
mod routing;
