//! Which output channels carry signal: speaker masks, bleed, LFE, stereo devices.

use super::{
    default_distance_model, default_wall_materials, energy, layout_5_1, living_room_5_1,
    test_environment, ConstSource, Rig, ToneSource,
};
use crate::pipeline::builders;
use crate::pipeline::mix_stage::MixContext;
use crate::pipeline::{render_pipeline, PipelineParams, RenderParams, RenderPipeline};
use atrium_core::listener::Listener;
use atrium_core::source::SoundSource;
use atrium_core::speaker::{RenderMode, SpeakerLayout};
use atrium_core::types::Vec3;

// ── Speaker mask: masked channels get zero output ────────────────────

#[test]
fn speaker_mask_zeroes_masked_channels() {
    let mut layout = layout_5_1();
    // Mask center channel (2) and LFE (3)
    layout.set_active_channels(&[0, 1, 4, 5]);

    let dm = default_distance_model();

    // Source at center of room
    let source_pos = Vec3::new(3.0, 2.0, 0.0);

    let params = PipelineParams::default();
    let mut pipeline = builders::world_locked::build(&params);

    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
        pos: source_pos,
        val: 1.0,
    })];

    let channels = 6;
    let frames = 512;
    let mut buffer = vec![0.0f32; frames * channels];

    let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);
    let environment = test_environment(
        Vec3::ZERO,
        Vec3::new(6.0, 4.0, 3.0),
        default_wall_materials(),
    );
    let rp = RenderParams {
        listener: &listener,
        channels,
        sample_rate: 48000.0,
        master_gain: 1.0,
        distance_model: &dm,
        layout: &layout,
        environment: &environment,
        measurement_mode: false,
    };
    render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

    // Channels 2 and 3 should be completely silent
    for frame in 0..frames {
        assert_eq!(
            buffer[frame * channels + 2],
            0.0,
            "Center ch should be masked"
        );
        assert_eq!(buffer[frame * channels + 3], 0.0, "LFE ch should be masked");
    }

    // Active channels should have some signal
    let has_signal =
        |ch: usize| -> bool { (0..frames).any(|f| buffer[f * channels + ch].abs() > 1e-10) };
    assert!(has_signal(0), "FL should have signal");
    assert!(has_signal(1), "FR should have signal");
    assert!(has_signal(4), "RL should have signal");
    assert!(has_signal(5), "RR should have signal");
}

// ── VBAP mask: masked channels zeroed via apply_mask ─────────────────

#[test]
fn vbap_mask_zeroes_masked_channels() {
    let mut layout = layout_5_1();
    // Quad profile: mask center (2) and LFE (3)
    layout.set_active_channels(&[0, 1, 4, 5]);

    let dm = default_distance_model();

    let source_pos = Vec3::new(3.0, 3.0, 0.0);

    let params = PipelineParams::default();
    let mut pipeline = builders::vbap::build(&params);

    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
        pos: source_pos,
        val: 1.0,
    })];

    let channels = 6;
    let frames = 512;
    let mut buffer = vec![0.0f32; frames * channels];

    let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);
    let environment = test_environment(
        Vec3::ZERO,
        Vec3::new(6.0, 4.0, 3.0),
        default_wall_materials(),
    );
    let rp = RenderParams {
        listener: &listener,
        channels,
        sample_rate: 48000.0,
        master_gain: 1.0,
        distance_model: &dm,
        layout: &layout,
        environment: &environment,
        measurement_mode: false,
    };
    render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

    // Masked channels 2 and 3 should be silent
    for frame in 0..frames {
        assert_eq!(
            buffer[frame * channels + 2],
            0.0,
            "Center ch masked in VBAP"
        );
        assert_eq!(buffer[frame * channels + 3], 0.0, "LFE ch masked in VBAP");
    }
}

// ── Ambisonics: all speakers get signal ─────────────────────────────

#[test]
fn ambisonics_all_speakers_active() {
    let layout = layout_5_1();
    let dm = default_distance_model();

    let source_pos = Vec3::new(1.0, 3.0, 0.0);
    let params = PipelineParams::default();
    let mut pipeline = builders::ambisonics::build(&params);
    pipeline.ensure_topology(1, &layout, 48000.0);

    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
        pos: source_pos,
        val: 1.0,
    })];

    let channels = 6;
    let frames = 2048;
    let mut buffer = vec![0.0f32; frames * channels];

    let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);
    let environment = test_environment(
        Vec3::new(-20.0, -20.0, -5.0),
        Vec3::new(20.0, 20.0, 5.0),
        default_wall_materials(),
    );
    let rp = RenderParams {
        listener: &listener,
        channels,
        sample_rate: 48000.0,
        master_gain: 1.0,
        distance_model: &dm,
        layout: &layout,
        environment: &environment,
        measurement_mode: false,
    };
    render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

    // All spatial speakers (not LFE ch3) should have signal
    let quarter = frames * 3 / 4;
    let has_signal =
        |ch: usize| -> bool { (quarter..frames).any(|f| buffer[f * channels + ch].abs() > 1e-10) };
    assert!(has_signal(0), "FL should have signal");
    assert!(has_signal(1), "FR should have signal");
    assert!(has_signal(2), "C should have signal");
    assert!(has_signal(4), "RL should have signal");
    assert!(has_signal(5), "RR should have signal");
}

#[test]
fn ambisonics_differs_from_vbap() {
    let layout = layout_5_1();
    let dm = default_distance_model();

    let source_pos = Vec3::new(1.0, 3.0, 0.0);
    let params = PipelineParams::default();
    let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);

    let channels = 6;
    let frames = 2048;

    let render = |pipeline: &mut RenderPipeline| -> Vec<f32> {
        pipeline.ensure_topology(1, &layout, 48000.0);
        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: source_pos,
            val: 1.0,
        })];
        let mut buffer = vec![0.0f32; frames * channels];
        let environment = test_environment(
            Vec3::new(-20.0, -20.0, -5.0),
            Vec3::new(20.0, 20.0, 5.0),
            default_wall_materials(),
        );
        let rp = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &dm,
            layout: &layout,
            environment: &environment,
            measurement_mode: false,
        };
        render_pipeline(pipeline, &mut sources, &rp, &mut buffer);
        buffer
    };

    let vbap_buf = render(&mut builders::vbap::build(&params));
    let ambi_buf = render(&mut builders::ambisonics::build(&params));

    let quarter = frames * 3 / 4;
    let rms = |buf: &[f32], ch: usize| -> f32 {
        let sum: f32 = (quarter..frames)
            .map(|f| buf[f * channels + ch].powi(2))
            .sum();
        (sum / (frames - quarter) as f32).sqrt()
    };

    let mut total_diff = 0.0f32;
    for ch in [0, 1, 2, 4, 5] {
        let v = rms(&vbap_buf, ch);
        let a = rms(&ambi_buf, ch);
        total_diff += (v - a).abs();
    }
    assert!(
        total_diff > 0.01,
        "Ambisonics and VBAP should produce different output (total_diff={total_diff})"
    );
}

// ── HRTF: only stereo channels have signal on 5.1 layout ─────────

#[test]
fn hrtf_no_bleed_to_surround_channels() {
    let layout = layout_5_1();
    let dm = default_distance_model();

    let source_pos = Vec3::new(1.0, 3.0, 0.0);
    let params = PipelineParams::default();
    let mut pipeline = builders::hrtf::build(&params);
    pipeline.ensure_topology(1, &layout, 48000.0);

    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
        pos: source_pos,
        val: 1.0,
    })];

    let channels = 6;
    let frames = 2048;
    let mut buffer = vec![0.0f32; frames * channels];

    let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);
    let environment = test_environment(
        Vec3::new(-20.0, -20.0, -5.0),
        Vec3::new(20.0, 20.0, 5.0),
        default_wall_materials(),
    );
    let rp = RenderParams {
        listener: &listener,
        channels,
        sample_rate: 48000.0,
        master_gain: 1.0,
        distance_model: &dm,
        layout: &layout,
        environment: &environment,
        measurement_mode: false,
    };
    render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

    // Channels 2-5 (C, LFE, RL, RR) must be silent — HRTF only writes to L/R
    for ch in 2..channels {
        let max_abs = (0..frames)
            .map(|f| buffer[f * channels + ch].abs())
            .fold(0.0f32, f32::max);
        assert_eq!(
            max_abs, 0.0,
            "HRTF should not bleed to channel {ch}, but got max abs {max_abs}"
        );
    }

    // Stereo channels should have signal
    let has_signal =
        |ch: usize| -> bool { (0..frames).any(|f| buffer[f * channels + ch].abs() > 1e-10) };
    assert!(has_signal(0), "L should have signal in HRTF mode");
    assert!(has_signal(1), "R should have signal in HRTF mode");
}

// ── VBAP: FDN reverb stays within speaker channels ───────────────

#[test]
fn vbap_fdn_no_bleed_beyond_channels() {
    // Stereo layout on a 2-channel device — FDN should not bleed beyond ch 0-1
    let layout = SpeakerLayout::stereo(Vec3::new(-1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0));
    let dm = default_distance_model();

    let source_pos = Vec3::new(0.0, 2.0, 0.0);
    let params = PipelineParams::default();
    let mut pipeline = builders::vbap::build(&params);
    pipeline.ensure_topology(1, &layout, 48000.0);

    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
        pos: source_pos,
        val: 1.0,
    })];

    let channels = 2;
    let frames = 2048;
    let mut buffer = vec![0.0f32; frames * channels];

    let listener = Listener::new(Vec3::ZERO, 0.0);
    let environment = test_environment(
        Vec3::new(-5.0, -5.0, -5.0),
        Vec3::new(5.0, 5.0, 5.0),
        default_wall_materials(),
    );
    let rp = RenderParams {
        listener: &listener,
        channels,
        sample_rate: 48000.0,
        master_gain: 1.0,
        distance_model: &dm,
        layout: &layout,
        environment: &environment,
        measurement_mode: false,
    };
    render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

    // Both stereo channels should have signal (renderer + FDN wet)
    let has_signal =
        |ch: usize| -> bool { (0..frames).any(|f| buffer[f * channels + ch].abs() > 1e-10) };
    assert!(has_signal(0), "L should have signal");
    assert!(has_signal(1), "R should have signal");
}

/// Verify that with the corrected stage ordering (FDN → LFE crossover),
/// the LFE channel receives bass content including reverb bass. Previously,
/// LFE crossover ran first (highpassing mains), then FDN added reverb
/// whose bass stayed on the mains and never reached the subwoofer.
#[test]
fn vbap_lfe_has_signal_in_5_1() {
    let layout = layout_5_1();
    let dm = default_distance_model();
    let params = PipelineParams::default();

    // Source at moderate distance — close enough for strong signal,
    // far enough for meaningful reverb send.
    let source_pos = Vec3::new(5.0, 5.0, 0.0);
    let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);

    let mut pipeline = builders::vbap::build(&params);
    let channels = 6;
    let frames = 4096;
    let wall_materials = default_wall_materials();
    let environment_min = Vec3::new(-20.0, -20.0, -5.0);
    let environment_max = Vec3::new(20.0, 20.0, 5.0);

    // Init mix stages (FDN needs init to set up delay lines + damping,
    // LFE crossover needs init to detect LFE channel and create filters).
    let environment = test_environment(environment_min, environment_max, wall_materials.clone());
    let init_ctx = MixContext {
        listener: &listener,
        layout: &layout,
        sample_rate: 48000.0,
        channels,
        master_gain: 1.0,
        render_channels: channels,
        reverb_input: None,
        environment: &environment,
        measurement_mode: false,
    };
    pipeline.init(&init_ctx);
    pipeline.ensure_topology(1, &layout, 48000.0);

    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
        pos: source_pos,
        val: 1.0,
    })];
    let mut buffer = vec![0.0f32; frames * channels];
    let rp = RenderParams {
        listener: &listener,
        channels,
        sample_rate: 48000.0,
        master_gain: 1.0,
        distance_model: &dm,
        layout: &layout,
        environment: &environment,
        measurement_mode: false,
    };
    render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

    // LFE is channel 3 in 5.1 layout.
    let lfe_ch = 3;
    let lfe_max: f32 = (0..frames)
        .map(|f| buffer[f * channels + lfe_ch].abs())
        .fold(0.0f32, f32::max);

    // With correct ordering (FDN before LFE crossover), the LFE channel
    // should have signal — either direct bass from the source or
    // redirected bass from the reverb tail.
    assert!(
        lfe_max > 1e-6,
        "LFE channel should have signal in 5.1 VBAP. Max abs = {lfe_max}"
    );
}

/// Channel-order convention end to end: listener at the origin facing +y,
/// the default scene's 5.1 ring, a 1 kHz source 4 m to the listener's left
/// (−x). Every mode must put more energy on the left (FL 0 + RL 4, or
/// headphone L 0) than on the right (FR 1 + RR 5, or R 1), and mirror it.
#[test]
fn left_source_is_louder_on_left_channels_in_every_mode() {
    let mut rig = Rig::free_field(48_000.0);
    rig.layout = living_room_5_1();
    rig.channels = rig.layout.total_channels();
    let frames = 4096;
    for mode in RenderMode::ALL {
        let (left_channels, right_channels): (&[usize], &[usize]) = if mode == RenderMode::Hrtf {
            (&[0], &[1])
        } else {
            (&[0, 4], &[1, 5])
        };
        let left_over_right_db = |x: f32| -> f64 {
            let mut pipeline = rig.build(mode);
            let mut sources = vec![ToneSource::boxed(
                Vec3::new(x, 0.5, 0.0),
                1000.0,
                usize::MAX,
            )];
            let output = rig.render(&mut pipeline, &mut sources, frames);
            10.0 * (energy(&output, rig.channels, left_channels, frames / 2)
                / energy(&output, rig.channels, right_channels, frames / 2))
            .log10()
        };
        let from_left = left_over_right_db(-4.0);
        let from_right = left_over_right_db(4.0);
        assert!(
            from_left > 3.0 && from_right < -3.0,
            "{mode:?}: left/right balance {from_left:.1} dB for a left source, \
             {from_right:.1} dB for a right source"
        );
    }
}

/// `output.rs` falls back to the device's channel count when it cannot
/// open the layout's (5.1 scene on a 2-channel laptop) and masks the
/// channels that do not fit. VBAP, DBAP and WorldLocked keep a left
/// source on the left channel. Ambisonics has its own test below.
#[test]
fn surround_layout_on_a_stereo_device_keeps_left_right_imaging() {
    for mode in [RenderMode::Vbap, RenderMode::Dbap, RenderMode::WorldLocked] {
        let balance_db = left_over_right_on_stereo_device(mode);
        assert!(
            balance_db.0 > 3.0 && balance_db.1 < -3.0,
            "{mode:?}: L/R balance {:.1} dB (left source), {:.1} dB (right source)",
            balance_db.0,
            balance_db.1
        );
    }
}

fn left_over_right_on_stereo_device(mode: RenderMode) -> (f64, f64) {
    let mut rig = Rig::free_field(48_000.0);
    rig.layout = living_room_5_1();
    rig.layout.set_active_channels(&[0, 1]);
    rig.channels = 2;
    let frames = 4096;
    let balance = |x: f32| -> f64 {
        let mut pipeline = rig.build(mode);
        let mut sources = vec![ToneSource::boxed(
            Vec3::new(x, 0.5, 0.0),
            1000.0,
            usize::MAX,
        )];
        let output = rig.render(&mut pipeline, &mut sources, frames);
        10.0 * (energy(&output, 2, &[0], frames / 2) / energy(&output, 2, &[1], frames / 2)).log10()
    };
    (balance(-4.0), balance(4.0))
}

/// Regression: the renderer used to choose B-format by *speaker count*
/// (5 ≥ 4) while the decode stage skipped on a 2-channel device, so the
/// output was raw L = W, R = Y. `ambisonic_bus` now decides for both: a
/// two-channel bus → inline bilateral decode.
#[test]
fn ambisonics_surround_layout_on_a_stereo_device_keeps_left_right_imaging() {
    let balance_db = left_over_right_on_stereo_device(RenderMode::Ambisonics);
    assert!(
        balance_db.0 > 3.0 && balance_db.1 < -3.0,
        "Ambisonics: L/R balance {:.1} dB (left source), {:.1} dB (right source)",
        balance_db.0,
        balance_db.1
    );
}

/// The quad layout is "masked 5.1 with centre and LFE silent" (speaker.rs).
/// Once initialised, LFE bass management still high-passes the four mains
/// at 120 Hz and sums their bass into the masked LFE channel 3, so a quad
/// rig without a sub loses everything below 120 Hz. (The mode × layout
/// matrix above passes only because it never calls `init`.) The same
/// happens to the Stereo channel mode on a 5.1 layout.
#[test]
fn quad_layout_keeps_bass_on_its_four_speakers() {
    let rig = Rig::free_field(48_000.0);
    let mut pipeline = rig.build(RenderMode::Vbap);
    let frames = 16_384;
    let mut sources = vec![ToneSource::boxed(
        Vec3::new(0.0, 3.0, 0.0),
        50.0,
        usize::MAX,
    )];
    let output = rig.render(&mut pipeline, &mut sources, frames);
    let masked_lfe = energy(&output, rig.channels, &[3], frames / 2);
    let mains = energy(&output, rig.channels, &[0, 1, 4, 5], frames / 2);
    assert!(
        masked_lfe == 0.0,
        "masked LFE carries {:.1} dB of the 50 Hz tone relative to the mains",
        10.0 * (masked_lfe / mains.max(1e-30)).log10()
    );
}
