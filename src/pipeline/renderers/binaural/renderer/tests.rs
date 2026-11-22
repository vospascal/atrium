use super::*;
use crate::audio::convolver::Convolver;
use atrium_core::directivity::DirectivityPattern;
use atrium_core::listener::Listener;
use atrium_core::panner::DistanceModelType;
use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;

use crate::audio::distance::DistanceModel;
use crate::pipeline::environment::Environment;
use crate::pipeline::path::PathContribution;

const SAMPLE_RATE: f32 = 48000.0;

fn sofa_file() -> String {
    format!("{}/assets/hrtf/default.sofa", env!("CARGO_MANIFEST_DIR"))
}

/// A test signal: sample `n` is `signal_at(n)`. Position is supplied to the
/// renderer through the context, not read from here.
struct TestSignal {
    sample_index: usize,
    signal_at: fn(usize) -> f32,
}

impl SoundSource for TestSignal {
    fn next_sample(&mut self, _sample_rate: f32) -> f32 {
        let sample = (self.signal_at)(self.sample_index);
        self.sample_index += 1;
        sample
    }
    fn position(&self) -> Vec3 {
        Vec3::ZERO
    }
    fn tick(&mut self, _dt: f32) {}
}

fn noise(sample_index: usize) -> f32 {
    let mut state = (sample_index as u32).wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    state ^= state >> 15;
    state = state.wrapping_mul(0x2C1B_3C6D);
    state ^= state >> 12;
    (state as f32 / u32::MAX as f32) * 2.0 - 1.0
}

fn direct_path(listener: &Listener, source_position: Vec3) -> PathSet {
    let offset = source_position - listener.position;
    let mut paths = PathSet::new();
    paths.push(PathContribution {
        kind: PathKind::Direct,
        direction: offset.normalize(),
        distance: offset.length(),
        delay_seconds: 0.0,
        gain: 1.0,
        wall_index: None,
    });
    paths
}

/// Everything a `SourceContext` borrows, for an anechoic free field with
/// the inverse distance law (reference 1 m) and an omni source.
///
/// The probe has the near field and the headphone equalisation off: these
/// tests are about the measured HRIRs and the machinery that convolves
/// them, with sources at 1 m, inside the distance the near-field correction
/// starts at. Each source is rendered alone, without the callback's end
/// where the equaliser filters the sum. The near field and the equaliser
/// have their own tests (`binaural/near_field/tests.rs`,
/// `src/pipeline/tests/headphone_equalisation.rs`).
struct Scene {
    environment: Environment,
    distance_model: DistanceModel,
    directivity: DirectivityPattern,
    layout: SpeakerLayout,
    probe: atrium_core::probe::Probe,
}

impl Scene {
    fn new() -> Self {
        Self {
            environment: Environment::anechoic(
                Vec3::new(-500.0, -500.0, 0.0),
                Vec3::new(500.0, 500.0, 500.0),
            ),
            distance_model: DistanceModel {
                model: DistanceModelType::Inverse,
                ref_distance: 1.0,
                max_distance: 1000.0,
                rolloff: 1.0,
            },
            directivity: DirectivityPattern::Omni,
            layout: SpeakerLayout::stereo(Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, -1.0, 0.0)),
            probe: atrium_core::probe::Probe {
                near_field: false,
                headphone_equalisation: false,
                ..atrium_core::probe::Probe::WHOLE
            },
        }
    }

    /// Render one buffer; returns (interleaved stereo, interleaved reverb send).
    #[allow(clippy::too_many_arguments)]
    fn render(
        &self,
        renderer: &mut HrtfRenderer,
        signal: &mut TestSignal,
        listener: &Listener,
        source_position: Vec3,
        paths: &PathSet,
        reverb_send_level: f32,
        num_frames: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        let ctx = SourceContext {
            listener,
            source_pos: source_position,
            source_orientation: Vec3::new(1.0, 0.0, 0.0),
            source_directivity: &self.directivity,
            source_spread: 0.0,
            source_ref_distance: 1.0,
            path_target: listener.position,
            environment: &self.environment,
            sample_rate: SAMPLE_RATE,
            distance_model: &self.distance_model,
            layout: &self.layout,
            reverb_send: reverb_send_level,
            probe: self.probe,
        };
        let mut chains: Vec<PathEffectChain> = (0..MAX_PATHS)
            .map(|_| PathEffectChain::new(Vec::new()))
            .collect();
        let mut block = vec![0.0f32; num_frames * 2];
        let mut reverb = vec![0.0f32; num_frames * 2];
        let mut out = OutputBuffer {
            buffer: &mut block,
            reverb_send: Some(&mut reverb),
            channels: 2,
            num_frames,
            sample_rate: SAMPLE_RATE,
        };
        renderer.render_source(0, signal, &ctx, paths, &mut chains, &mut out);
        (block, reverb)
    }
}

fn loaded_renderer(scene: &Scene) -> HrtfRenderer {
    let mut renderer = HrtfRenderer::new(&sofa_file());
    renderer.ensure_topology(1, &scene.layout, SAMPLE_RATE);
    assert!(renderer.sofa.is_some(), "default.sofa must load");
    renderer
}

fn split_channels(interleaved: &[f32]) -> (Vec<f32>, Vec<f32>) {
    (
        interleaved.iter().step_by(2).copied().collect(),
        interleaved.iter().skip(1).step_by(2).copied().collect(),
    )
}

fn energy(samples: &[f32]) -> f32 {
    samples.iter().map(|sample| sample * sample).sum()
}

/// Lag (in samples) of `right` relative to `left` that maximises their
/// cross-correlation. Positive = right ear hears it later.
fn interaural_lag(left: &[f32], right: &[f32], max_lag: i32) -> i32 {
    let correlation = |lag: i32| -> f32 {
        (0..left.len() as i32)
            .filter_map(|index| {
                let other = index + lag;
                (other >= 0 && (other as usize) < right.len())
                    .then(|| left[index as usize] * right[other as usize])
            })
            .sum()
    };
    (-max_lag..=max_lag)
        .max_by(|a, b| correlation(*a).total_cmp(&correlation(*b)))
        .unwrap()
}

/// Impulse response of the HRTF renderer for a source 1 m away in
/// `direction` (world frame), listener at the origin with `yaw`.
///
/// The first 1024-sample buffer is silent: it loads this direction's HRIR
/// (crossfading away from the initial one) and settles the gain ramp at
/// unity (1 m = reference distance). The impulse is the first sample of
/// the second buffer, which has no filter update and no ramp.
fn impulse_response(yaw: f32, direction: Vec3) -> (Vec<f32>, Vec<f32>) {
    let scene = Scene::new();
    let mut renderer = loaded_renderer(&scene);
    let listener = Listener::new(Vec3::ZERO, yaw);
    let source_position = direction.normalize();
    let paths = direct_path(&listener, source_position);
    let mut signal = TestSignal {
        sample_index: 0,
        signal_at: |index| if index == 1024 { 1.0 } else { 0.0 },
    };
    let mut render = || {
        scene.render(
            &mut renderer,
            &mut signal,
            &listener,
            source_position,
            &paths,
            0.0,
            1024,
        )
    };
    let (silent, _) = render();
    assert!(silent.iter().all(|sample| *sample == 0.0));
    let (block, _) = render();
    split_channels(&block)
}

fn level_difference_db(louder: &[f32], quieter: &[f32]) -> f32 {
    10.0 * (energy(louder) / energy(quieter)).log10()
}

/// SOFA frame: x forward, y LEFT, z up. World: yaw 0 faces +X and +Y is
/// to the listener's left (right-handed, z up), so at yaw 0 the two frames
/// coincide. Hand-derived cases, source 2 m from the listener at (1, 1, 0):
#[test]
fn sofa_coordinates_rotate_with_listener_yaw() {
    let close = |actual: (f32, f32, f32), expected: (f32, f32, f32)| {
        (actual.0 - expected.0).abs() < 1e-5
            && (actual.1 - expected.1).abs() < 1e-5
            && (actual.2 - expected.2).abs() < 1e-5
    };
    let origin = Vec3::new(1.0, 1.0, 0.0);
    let facing = |yaw: f32| Listener::new(origin, yaw);
    let north = origin + Vec3::new(0.0, 2.0, 0.5);
    let east = origin + Vec3::new(2.0, 0.0, 0.0);
    // Yaw 0 (facing +X): north is to the left → SOFA (0, +2), height passes through.
    assert!(close(to_sofa_coords(north, &facing(0.0)), (0.0, 2.0, 0.5)));
    // Yaw 90° (facing +Y): north is straight ahead.
    let quarter = std::f32::consts::FRAC_PI_2;
    assert!(close(
        to_sofa_coords(north, &facing(quarter)),
        (2.0, 0.0, 0.5)
    ));
    // Facing north, east is on the right → SOFA y = −2.
    assert!(close(
        to_sofa_coords(east, &facing(quarter)),
        (0.0, -2.0, 0.0)
    ));
    // Yaw 180° (facing −X): north, formerly left, is now on the right.
    let half = std::f32::consts::PI;
    assert!(close(
        to_sofa_coords(north, &facing(half)),
        (0.0, -2.0, 0.5)
    ));

    // A rotation: the horizontal distance is preserved at every heading.
    for step in 0..72 {
        let yaw = step as f32 * 5f32.to_radians();
        let (forward, left, _) = to_sofa_coords(origin + Vec3::new(3.0, -4.0, 0.0), &facing(yaw));
        assert!(((forward * forward + left * left).sqrt() - 5.0).abs() < 1e-4);
    }
}

/// A source hard left arrives at the left ear earlier and louder.
///
/// ITD reference (Woodworth): (a/c)(θ + sin θ) with a = 0.0875 m,
/// c = 343 m/s, θ = 90° → 0.000255 × 2.571 = 0.656 ms = 31.5 samples at
/// 48 kHz (28.9 at 44.1 kHz, 63.0 at 96 kHz). KEMAR measures a little wider
/// than the spherical head: the rendered lag is 35 samples (0.73 ms).
/// ILD: KEMAR broadband 3.167 / 0.210 energy = 11.8 dB at 90°.
#[test]
fn hard_left_source_reaches_left_ear_earlier_and_louder() {
    let (left, right) = impulse_response(0.0, Vec3::new(0.0, 1.0, 0.0));
    let lag = interaural_lag(&left, &right, 60);
    let lag_ms = lag as f32 / SAMPLE_RATE * 1000.0;
    assert!(
        (0.6..0.8).contains(&lag_ms),
        "right ear should lag by ≈ 0.66–0.73 ms, got {lag} samples ({lag_ms:.3} ms)"
    );
    let ild = level_difference_db(&left, &right);
    assert!(
        (ild - 11.8).abs() < 0.5,
        "left ear should be 11.8 dB louder, got {ild:.2} dB"
    );
}

/// Straight ahead: both ears identical in level and time.
#[test]
fn source_straight_ahead_is_symmetric() {
    let (left, right) = impulse_response(0.0, Vec3::new(1.0, 0.0, 0.0));
    assert_eq!(interaural_lag(&left, &right, 60), 0);
    assert!(level_difference_db(&left, &right).abs() < 0.05);
}

/// Turning the head 180° puts a left source on the right: the ears swap.
#[test]
fn yaw_of_180_degrees_swaps_the_ears() {
    let source = Vec3::new(0.0, 1.0, 0.0);
    let (left_facing_east, right_facing_east) = impulse_response(0.0, source);
    let (left_facing_west, right_facing_west) = impulse_response(std::f32::consts::PI, source);
    let relative = |a: f32, b: f32| (a - b).abs() / b;
    assert!(relative(energy(&left_facing_east), energy(&right_facing_west)) < 1e-3);
    assert!(relative(energy(&right_facing_east), energy(&left_facing_west)) < 1e-3);
    assert_eq!(
        interaural_lag(&left_facing_east, &right_facing_east, 60),
        -interaural_lag(&left_facing_west, &right_facing_west, 60)
    );
}

/// Head tracking: facing north (yaw 90°), a source due east is on the
/// right — the world stays put while the head turns.
#[test]
fn listener_facing_north_hears_an_eastern_source_on_the_right() {
    let (left, right) = impulse_response(std::f32::consts::FRAC_PI_2, Vec3::new(1.0, 0.0, 0.0));
    assert!(interaural_lag(&left, &right, 60) < -25, "left ear must lag");
    assert!(level_difference_db(&right, &left) > 10.0);
}

fn reflection_only(direction: Vec3, distance: f32, gain: f32) -> PathSet {
    let mut paths = PathSet::new();
    paths.push(PathContribution {
        kind: PathKind::Reflection,
        direction,
        distance,
        delay_seconds: 0.0,
        gain,
        wall_index: Some(0),
    });
    paths
}

/// A reflection from straight ahead, 2 m long, wall gain 0.6:
/// gain = 0.6 × (1 m / 2 m) = 0.3 (inverse law), centre pan → each ear
/// 0.3 × cos 45° = 0.21213. From silence the gain ramps linearly over the
/// first buffer — sample n is 0.21213 · n / 512 — and the next buffer
/// continues at exactly the target (one ramp step past the last sample).
#[test]
fn reflection_gain_ramps_linearly_to_pan_times_distance_gain() {
    let scene = Scene::new();
    let mut renderer = loaded_renderer(&scene);
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let paths = reflection_only(Vec3::new(1.0, 0.0, 0.0), 2.0, 0.6);
    let mut signal = TestSignal {
        sample_index: 0,
        signal_at: |_| 1.0,
    };
    let target = 0.3 * std::f32::consts::FRAC_1_SQRT_2;
    let mut render = || {
        let (block, _) = scene.render(
            &mut renderer,
            &mut signal,
            &listener,
            Vec3::new(2.0, 0.0, 0.0),
            &paths,
            0.0,
            512,
        );
        split_channels(&block)
    };
    let (left, right) = render();
    for frame in [0usize, 1, 256, 511] {
        let expected = target * frame as f32 / 512.0;
        assert!(
            (left[frame] - expected).abs() < 1e-6,
            "frame {frame}: {}",
            left[frame]
        );
        assert!((right[frame] - expected).abs() < 1e-6);
    }
    let (left, right) = render();
    assert!(left
        .iter()
        .chain(&right)
        .all(|sample| (sample - target).abs() < 1e-6));
}

/// A reflection from the listener's left (45°) pans by the lateral
/// component: pan = 0.5 + 0.5·(−sin 45°) = 0.14645, so the left ear gets
/// cos(π/2 · 0.14645) = 0.97366 and the right sin(…) = 0.22803.
#[test]
fn reflection_from_front_left_pans_by_its_lateral_component() {
    let scene = Scene::new();
    let mut renderer = loaded_renderer(&scene);
    renderer.sources[0].paths[0].prev_gain = 1.0;
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let direction = Vec3::new(1.0, 1.0, 0.0).normalize();
    let paths = reflection_only(direction, 1.0, 1.0);
    let mut signal = TestSignal {
        sample_index: 0,
        signal_at: |_| 1.0,
    };
    let (block, _) = scene.render(
        &mut renderer,
        &mut signal,
        &listener,
        direction,
        &paths,
        0.0,
        64,
    );
    let (left, right) = split_channels(&block);
    assert!((left[10] - 0.97366).abs() < 1e-4, "left {}", left[10]);
    assert!((right[10] - 0.22803).abs() < 1e-4, "right {}", right[10]);
}

/// A stationary source must render exactly as its HRIR convolved with the
/// input: when both A/B slots hold the same IR, the crossfade every
/// FILTER_UPDATE_INTERVAL buffers must be inaudible.
///
/// It is not. Only the active slot is processed outside a crossfade, so
/// the slot that becomes active again holds the overlap tail it saved
/// when it last ran — FILTER_UPDATE_INTERVAL buffers ago — and adds that
/// stale tail (filter_len − 1 = 557 samples, longer than the 128-sample
/// crossfade) into the new output. Measured with noise input at 1 m,
/// direction left: buffers 0–3 match to 1e-7, buffer 4 is off by 1.40
/// (the signal peaks near 3.0), buffer 8 by 0.63, and so on every 4
/// buffers — a ~23 Hz periodic glitch at 512 frames / 48 kHz, for every
/// source, moving or not.
#[test]
fn stationary_source_equals_plain_hrir_convolution() {
    let scene = Scene::new();
    let mut renderer = loaded_renderer(&scene);
    // The initial IR is the (0, 1, 0) direction, so a source due left at
    // 1 m needs no direction change and, with the ramp settled, unity gain.
    renderer.sources[0].paths[0].prev_gain = 1.0;
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let source_position = Vec3::new(0.0, 1.0, 0.0);
    let paths = direct_path(&listener, source_position);
    let mut signal = TestSignal {
        sample_index: 0,
        signal_at: noise,
    };

    let sofa = renderer.sofa.as_ref().unwrap();
    let mut filter = Filter::new(sofa.filter_len());
    sofa.filter(0.0, 1.0, 0.0, &mut filter);
    assert_eq!((filter.ldelay, filter.rdelay), (0.0, 0.0));
    let mut reference_left = Convolver::new();
    let mut reference_right = Convolver::new();
    reference_left.init(BLOCK_SIZE, &filter.left);
    reference_right.init(BLOCK_SIZE, &filter.right);

    let frames = 512;
    for buffer in 0..12 {
        let (block, _) = scene.render(
            &mut renderer,
            &mut signal,
            &listener,
            source_position,
            &paths,
            0.0,
            frames,
        );
        let (left, right) = split_channels(&block);
        let mut worst = 0.0f32;
        for start in (0..frames).step_by(BLOCK_SIZE) {
            let input: Vec<f32> = (0..BLOCK_SIZE)
                .map(|index| noise(buffer * frames + start + index))
                .collect();
            let mut expected_left = vec![0.0; BLOCK_SIZE];
            let mut expected_right = vec![0.0; BLOCK_SIZE];
            reference_left.process(&input, &mut expected_left);
            reference_right.process(&input, &mut expected_right);
            for index in 0..BLOCK_SIZE {
                worst = worst
                    .max((left[start + index] - expected_left[index]).abs())
                    .max((right[start + index] - expected_right[index]).abs());
            }
        }
        assert!(worst < 1e-4, "buffer {buffer}: max deviation {worst}");
    }
}

/// The A/B crossfade lasts BLOCK_SIZE = 128 samples with weights
/// (n + 1) / 128. With 64-frame device buffers it spans two calls, and the
/// second call restarts the ramp at (i + 1) / remaining = 1/64 instead of
/// continuing at 65/128: the new IR's weight drops from 0.5 to 0.016 in
/// one sample. Only device buffers < 128 frames hit this.
#[test]
fn crossfade_continues_across_buffers_shorter_than_a_block() {
    let scene = Scene::new();
    let mut renderer = loaded_renderer(&scene);
    let filter_length = renderer.sofa.as_ref().unwrap().filter_len();
    let silence = vec![0.0f32; filter_length];
    let mut unit = vec![0.0f32; filter_length];
    unit[0] = 1.0;
    {
        let path = &mut renderer.sources[0].paths[0];
        // Retiring slot 0 is silent, new slot 1 passes the input through,
        // so the output is the new slot's crossfade weight itself.
        path.conv[0].left.set_response(&silence);
        path.conv[0].right.set_response(&silence);
        path.conv[1].left.set_response(&unit);
        path.conv[1].right.set_response(&unit);
        path.active = 1;
        path.xfade_remaining = BLOCK_SIZE;
        path.prev_gain = 1.0;
    }
    // Skip filter updates for the next calls (1 and 2 are not multiples of 4).
    renderer.sources[0].update_counter = 1;

    let listener = Listener::new(Vec3::ZERO, 0.0);
    let source_position = Vec3::new(0.0, 1.0, 0.0);
    let paths = direct_path(&listener, source_position);
    let mut signal = TestSignal {
        sample_index: 0,
        signal_at: |_| 1.0,
    };
    let mut weights = Vec::new();
    for _ in 0..2 {
        let (block, _) = scene.render(
            &mut renderer,
            &mut signal,
            &listener,
            source_position,
            &paths,
            0.0,
            64,
        );
        weights.extend(split_channels(&block).0);
    }
    for (index, weight) in weights.iter().enumerate() {
        let expected = (index + 1) as f32 / BLOCK_SIZE as f32;
        assert!(
            (weight - expected).abs() < 1e-5,
            "sample {index}: weight {weight}, expected {expected}"
        );
    }
}

/// The late-reverb send contract shared by the VBAP, DBAP and Ambisonics
/// renderers (and docs/ray-tracing-upgrade-analysis.md): send = source
/// signal × ctx.reverb_send, with no distance gain — the reverberant field
/// is diffuse, its level does not fall with listener distance; the
/// d²/(d² + d_c²) send already encodes the direct-to-reverberant balance.
///
/// HRTF writes the gain-ramped direct signal instead, which includes the
/// distance and directivity gains: at 10 m (inverse law, 1 m reference)
/// the send is 0.1 × 0.8 = 0.08 instead of 0.8 — 20 dB less reverb than
/// the same scene on speakers, and beyond d_c the D/R ratio stops
/// changing with distance, removing headphones' main distance cue.
#[test]
fn reverb_send_is_source_signal_times_send_level() {
    let scene = Scene::new();
    let mut renderer = loaded_renderer(&scene);
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let source_position = Vec3::new(10.0, 0.0, 0.0);
    let paths = direct_path(&listener, source_position);
    let mut signal = TestSignal {
        sample_index: 0,
        signal_at: |_| 1.0,
    };
    let mut reverb = Vec::new();
    for _ in 0..2 {
        reverb = scene
            .render(
                &mut renderer,
                &mut signal,
                &listener,
                source_position,
                &paths,
                0.8,
                512,
            )
            .1;
    }
    let send = split_channels(&reverb).0;
    assert!(
        send.iter().all(|sample| (sample - 0.8).abs() < 1e-5),
        "send should be 0.8, got {}",
        send[0]
    );
}

/// Reflection pan gains are recomputed once per buffer and applied as a
/// step, unlike every other gain in the renderers, which ramps. A 10°
/// head turn moves a frontal reflection from pan 0.5 to
/// 0.5 + 0.5·sin 10° = 0.5868: the left gain steps from cos 45° = 0.7071
/// to cos(π/2 · 0.5868) = 0.6044 between two samples — a 0.103 step at
/// full reflection level, where a ramp over 512 frames would be 0.0002.
#[test]
fn reflection_pan_change_is_ramped_not_stepped() {
    let scene = Scene::new();
    let mut renderer = loaded_renderer(&scene);
    renderer.sources[0].paths[0].prev_gain = 1.0;
    let paths = reflection_only(Vec3::new(1.0, 0.0, 0.0), 1.0, 1.0);
    let mut signal = TestSignal {
        sample_index: 0,
        signal_at: |_| 1.0,
    };
    let mut render = |yaw: f32| {
        let listener = Listener::new(Vec3::ZERO, yaw);
        let (block, _) = scene.render(
            &mut renderer,
            &mut signal,
            &listener,
            Vec3::new(1.0, 0.0, 0.0),
            &paths,
            0.0,
            512,
        );
        split_channels(&block).0
    };
    let before = render(0.0);
    let after = render(10f32.to_radians());
    let step = (after[0] - before[511]).abs();
    assert!(
        (before[511] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5,
        "settled centre pan should be cos 45°"
    );
    assert!(step < 0.01, "left gain stepped by {step} between buffers");
}

/// HRIRs are resampled to the rate the device runs at, whatever it is.
/// The bundled set is 558 taps at 48 kHz, so 1116 at 96 kHz and
/// 558 · 44.1/48 ≈ 513 at 44.1 kHz. They used to be loaded once at a
/// guessed 48 kHz and played at the device rate, shifting every spectral
/// cue by the rate ratio (an octave up at 96 kHz).
#[test]
fn hrirs_are_resampled_to_the_device_rate() {
    let scene = Scene::new();
    let mut renderer = HrtfRenderer::new(&sofa_file());
    assert!(
        renderer.sofa.is_none(),
        "nothing is loaded before the device rate is known"
    );

    let mut filter_length_at = |sample_rate: f32| {
        renderer.ensure_topology(1, &scene.layout, sample_rate);
        assert_eq!(renderer.sources.len(), 1);
        renderer
            .sofa
            .as_ref()
            .expect("default.sofa must load")
            .filter_len()
    };
    let at_48_khz = filter_length_at(48_000.0);
    let at_96_khz = filter_length_at(96_000.0);
    let at_44_khz = filter_length_at(44_100.0);

    // The same impulse responses last the same time at every rate.
    let duration = |length: usize, sample_rate: f32| length as f32 / sample_rate;
    let reference = duration(at_48_khz, 48_000.0);
    assert!((duration(at_96_khz, 96_000.0) - reference).abs() < 2.0 / 48_000.0);
    assert!((duration(at_44_khz, 44_100.0) - reference).abs() < 2.0 / 44_100.0);
    assert!(at_96_khz > at_48_khz && at_48_khz > at_44_khz);
}
