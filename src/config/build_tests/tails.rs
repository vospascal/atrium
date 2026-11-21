//! What is left 0.3 s after every source stops: nothing in an open meadow,
//! a tail in the glass atrium.

use crate::config::scene::SceneConfig;

/// Render a scene in one mode, then silence every source. Returns the
/// output power while playing and at 0.3 s after silence (sum over all
/// channels, averaged over a 10 ms block).
fn playing_and_tail_power(scene_path: &str, mode: atrium_core::speaker::RenderMode) -> (f32, f32) {
    let scene = SceneConfig::load(scene_path).expect("scene should load");
    let mut result = scene.build().expect("scene should build");
    let channels = result.scene.speaker_layout.total_channels();
    result.scene.sample_rate = 48000.0;
    result.scene.active_pipeline = mode;
    result.scene.init_pipelines();
    let frames = 480;
    let mut buffer = vec![0.0f32; frames * channels];
    let power = |block: &[f32]| block.iter().map(|x| x * x).sum::<f32>() / frames as f32;

    let mut playing = 0.0f32;
    for block in 0..80 {
        result.scene.render(&mut buffer, channels);
        if block >= 30 {
            playing += power(&buffer) / 50.0;
        }
    }
    for source in result.scene.sources.iter_mut() {
        source.set_muted(true);
    }
    // 0.3 s is far longer than any sound can still be in the air: the
    // farthest meadow source is 19 m away (55 ms).
    for _ in 0..30 {
        result.scene.render(&mut buffer, channels);
    }
    result.scene.render(&mut buffer, channels);
    (playing, power(&buffer))
}

/// An open meadow has no reverb tail, in every render mode. Before the
/// fix the 5.1 scene was still only 25 dB down 0.3 s after silence, with a
/// 1.2 s decay that Sabine's equation invents for a fully absorbing box.
#[test]
fn open_meadow_is_silent_after_sources_stop_in_every_mode() {
    for mode in atrium_core::speaker::RenderMode::ALL {
        let (playing, tail) = playing_and_tail_power("scenes/nature.yaml", mode);
        assert!(playing > 0.0, "{mode:?}: scene should produce sound");
        let tail_db = 10.0 * (tail / playing).max(1e-20).log10();
        assert!(
            tail_db < -100.0,
            "{mode:?}: open field still ringing 0.3 s after silence: {tail_db:.1} dB re playing"
        );
    }
}

/// The glass atrium must still ring: the gate removes reverb only where
/// the space cannot hold it. WorldLocked is excluded — it has no late
/// reverb stage by design, only per-speaker early reflections.
#[test]
fn glass_atrium_still_rings_after_sources_stop() {
    use atrium_core::speaker::RenderMode;
    for mode in RenderMode::ALL {
        if mode == RenderMode::WorldLocked {
            continue;
        }
        let (playing, tail) = playing_and_tail_power("scenes/default.yaml", mode);
        let tail_db = 10.0 * (tail / playing).max(1e-20).log10();
        assert!(
            tail_db > -60.0,
            "{mode:?}: atrium tail missing 0.3 s after silence: {tail_db:.1} dB re playing"
        );
    }
}
