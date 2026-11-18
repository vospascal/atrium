//! The mode × layout matrix: which channels each mode may drive on each layout.

use super::{
    default_distance_model, default_wall_materials, layout_5_1, test_environment, ConstSource,
};
use crate::pipeline::builders;
use crate::pipeline::{render_pipeline, PipelineParams, RenderParams};
use atrium_core::listener::Listener;
use atrium_core::source::SoundSource;
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

// ── Mode × Layout channel output matrix (12 combinations) ─────────

/// Run a single render mode through the full pipeline on a 5.1 layout
/// with the given channel mode (active channels), using a 6-channel buffer.
///
/// `active_channels`: which channels should have signal (e.g. [0,1] for stereo).
/// `silent_channels`: which channels must be silent (e.g. [2,3,4,5] for stereo).
fn assert_mode_channel_output(
    mode: RenderMode,
    active_channels: &[usize],
    silent_channels: &[usize],
    label: &str,
) {
    let dm = default_distance_model();
    let params = PipelineParams::default();

    let mut pipeline = match mode {
        RenderMode::WorldLocked => builders::world_locked::build(&params),
        RenderMode::Vbap => builders::vbap::build(&params),
        RenderMode::Hrtf => builders::hrtf::build(&params),
        RenderMode::Dbap => builders::dbap::build(&params),
        RenderMode::Ambisonics => builders::ambisonics::build(&params),
    };

    // Always 5.1 layout (6 hardware channels) with active mask applied
    let mut layout = layout_5_1();
    layout.set_active_channels(active_channels);
    let hardware_channels = 6;
    pipeline.ensure_topology(1, &layout, 48000.0);

    let source_pos = Vec3::new(1.0, 3.0, 0.0);
    let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);
    let frames = 2048;

    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
        pos: source_pos,
        val: 1.0,
    })];
    let mut buffer = vec![0.0f32; frames * hardware_channels];

    let environment = test_environment(
        Vec3::new(-20.0, -20.0, -5.0),
        Vec3::new(20.0, 20.0, 5.0),
        default_wall_materials(),
    );
    let rp = RenderParams {
        listener: &listener,
        channels: hardware_channels,
        sample_rate: 48000.0,
        master_gain: 1.0,
        distance_model: &dm,
        layout: &layout,
        environment: &environment,
        measurement_mode: false,
    };
    render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

    // Silent channels must have no signal
    for &ch in silent_channels {
        let max_abs = (0..frames)
            .map(|f| buffer[f * hardware_channels + ch].abs())
            .fold(0.0f32, f32::max);
        assert_eq!(
            max_abs, 0.0,
            "{label}: channel {ch} should be silent, got max {max_abs}"
        );
    }

    // At least one active channel should have signal
    let has_any_signal = active_channels
        .iter()
        .any(|&ch| (0..frames).any(|f| buffer[f * hardware_channels + ch].abs() > 1e-10));
    assert!(
        has_any_signal,
        "{label}: should produce signal in at least one active channel"
    );
}

// Channel modes on 5.1 hardware:
//   Stereo = [0, 1] active, [2, 3, 4, 5] silent
//   Quad   = [0, 1, 4, 5] active, [2, 3] silent
//   5.1    = [0, 1, 2, 4, 5] active, [3] silent (LFE gets crossover only)

const STEREO_ACTIVE: &[usize] = &[0, 1];

const STEREO_SILENT: &[usize] = &[2, 3, 4, 5];

const QUAD_ACTIVE: &[usize] = &[0, 1, 4, 5];

const QUAD_SILENT: &[usize] = &[2, 3];

const SURROUND_ACTIVE: &[usize] = &[0, 1, 2, 3, 4, 5];

const SURROUND_SILENT: &[usize] = &[];

// ── WorldLocked: Stereo, Quad, 5.1 ──────────────────────────────

#[test]
fn world_locked_stereo_output() {
    assert_mode_channel_output(
        RenderMode::WorldLocked,
        STEREO_ACTIVE,
        STEREO_SILENT,
        "WorldLocked×Stereo",
    );
}

#[test]
fn world_locked_quad_output() {
    assert_mode_channel_output(
        RenderMode::WorldLocked,
        QUAD_ACTIVE,
        QUAD_SILENT,
        "WorldLocked×Quad",
    );
}

#[test]
fn world_locked_5_1_output() {
    assert_mode_channel_output(
        RenderMode::WorldLocked,
        SURROUND_ACTIVE,
        SURROUND_SILENT,
        "WorldLocked×5.1",
    );
}

// ── VBAP: Quad, 5.1 (no stereo — needs ≥3 speakers) ────────────

#[test]
fn vbap_quad_output() {
    assert_mode_channel_output(RenderMode::Vbap, QUAD_ACTIVE, QUAD_SILENT, "Vbap×Quad");
}

#[test]
fn vbap_5_1_output() {
    assert_mode_channel_output(
        RenderMode::Vbap,
        SURROUND_ACTIVE,
        SURROUND_SILENT,
        "Vbap×5.1",
    );
}

// ── HRTF: always stereo (channels 0-1) ──────────────────────────

#[test]
fn hrtf_stereo_output() {
    assert_mode_channel_output(
        RenderMode::Hrtf,
        STEREO_ACTIVE,
        STEREO_SILENT,
        "Hrtf×Stereo",
    );
}

// ── DBAP: Stereo, Quad, 5.1 ─────────────────────────────────────

#[test]
fn dbap_stereo_output() {
    assert_mode_channel_output(
        RenderMode::Dbap,
        STEREO_ACTIVE,
        STEREO_SILENT,
        "Dbap×Stereo",
    );
}

#[test]
fn dbap_quad_output() {
    assert_mode_channel_output(RenderMode::Dbap, QUAD_ACTIVE, QUAD_SILENT, "Dbap×Quad");
}

#[test]
fn dbap_5_1_output() {
    assert_mode_channel_output(
        RenderMode::Dbap,
        SURROUND_ACTIVE,
        SURROUND_SILENT,
        "Dbap×5.1",
    );
}

// ── Ambisonics: Stereo, Quad, 5.1 ───────────────────────────────

#[test]
fn ambisonics_stereo_output() {
    assert_mode_channel_output(
        RenderMode::Ambisonics,
        STEREO_ACTIVE,
        STEREO_SILENT,
        "Ambisonics×Stereo",
    );
}

#[test]
fn ambisonics_quad_output() {
    assert_mode_channel_output(
        RenderMode::Ambisonics,
        QUAD_ACTIVE,
        QUAD_SILENT,
        "Ambisonics×Quad",
    );
}

#[test]
fn ambisonics_5_1_output() {
    assert_mode_channel_output(
        RenderMode::Ambisonics,
        SURROUND_ACTIVE,
        SURROUND_SILENT,
        "Ambisonics×5.1",
    );
}
