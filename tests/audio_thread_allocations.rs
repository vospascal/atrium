//! The audio callback must not touch the heap.
//!
//! An allocation (or a free) on the audio thread can block on the allocator's
//! lock for an unbounded time, which is heard as a dropout. This test builds
//! the shipped scenes exactly as the application does, then counts every heap
//! operation made inside `process_commands` + `render`, in every render mode,
//! with telemetry and the output tap attached.

use atrium_core::layers::LayerSetting;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};

use atrium::config::SceneConfig;
use atrium::engine::scene::AudioScene;
use atrium::engine::tap::output_tap;
use atrium_core::commands::Command;
use atrium_core::speaker::{ChannelMode, RenderMode};
use atrium_core::telemetry::TelemetryFrame;
use atrium_core::types::Vec3;

thread_local! {
    /// True while the current thread is inside a measured audio callback.
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static HEAP_OPERATIONS: Cell<u64> = const { Cell::new(0) };
    /// Where the first few heap operations of a measurement came from.
    static ORIGINS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

const MAX_RECORDED_ORIGINS: usize = 6;

struct CountingAllocator;

impl CountingAllocator {
    fn record(kind: &str, size: usize) {
        if !TRACKING.with(Cell::get) {
            return;
        }
        // Recording allocates; switch tracking off so it does not count itself.
        TRACKING.with(|tracking| tracking.set(false));
        HEAP_OPERATIONS.with(|count| count.set(count.get() + 1));
        ORIGINS.with(|origins| {
            let mut origins = origins.borrow_mut();
            if origins.len() < MAX_RECORDED_ORIGINS {
                let backtrace = std::backtrace::Backtrace::force_capture().to_string();
                let frames: Vec<&str> = backtrace
                    .lines()
                    .filter(|line| line.contains("atrium") && !line.contains("CountingAllocator"))
                    .take(6)
                    .collect();
                origins.push(format!("{kind} of {size} bytes:\n{}", frames.join("\n")));
            }
        });
        TRACKING.with(|tracking| tracking.set(true));
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::record("alloc", layout.size());
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        Self::record("free", layout.size());
        System.dealloc(pointer, layout)
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Self::record("realloc", new_size);
        System.realloc(pointer, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const SAMPLE_RATE: f32 = 48_000.0;
const FRAMES_PER_CALLBACK: usize = 512;

const RENDER_MODES: [RenderMode; 5] = [
    RenderMode::WorldLocked,
    RenderMode::Vbap,
    RenderMode::Hrtf,
    RenderMode::Dbap,
    RenderMode::Ambisonics,
];

/// Build a scene from its YAML and prepare it the way the output device does,
/// every ambient sound from an arc `arc_degrees` wide when one is given.
fn open_scene(path: &str, arc_degrees: Option<f32>) -> (AudioScene, usize) {
    let mut scene = SceneConfig::load(path)
        .and_then(SceneConfig::build)
        .unwrap_or_else(|error| panic!("{path}: {error}"))
        .scene;
    if let Some(degrees) = arc_degrees {
        for source in &mut scene.sources {
            if source.emitter_kind() == atrium_core::source::EmitterKind::Field {
                source.set_arc_degrees(degrees);
            }
        }
    }
    scene.sample_rate = SAMPLE_RATE;
    scene.init_pipelines();
    let channels = scene.speaker_layout.total_channels();
    (scene, channels)
}

/// Heap operations made by `callbacks` audio callbacks, and where they came from.
fn heap_operations_during(
    scene: &mut AudioScene,
    commands: &mut rtrb::Consumer<Command>,
    output: &mut [f32],
    channels: usize,
    callbacks: usize,
) -> (u64, Vec<String>) {
    HEAP_OPERATIONS.with(|count| count.set(0));
    ORIGINS.with(|origins| origins.borrow_mut().clear());
    TRACKING.with(|tracking| tracking.set(true));
    for _ in 0..callbacks {
        scene.process_commands(commands);
        scene.render(output, channels);
    }
    TRACKING.with(|tracking| tracking.set(false));
    (
        HEAP_OPERATIONS.with(Cell::get),
        ORIGINS.with(|origins| origins.borrow().clone()),
    )
}

/// The commands a control surface sends while audio runs: the listener walks
/// and turns (head tracking does this 25 times a second), sources move and
/// change level, an ambient sound's arc is dragged wider and narrower (its
/// filters are designed beside the audio thread and handed over), loops are
/// brought into step, a sound's layers are faded, soloed and muted, and now
/// and then the output or the weather changes.
fn push_live_commands(producer: &mut rtrb::Producer<Command>, callback: usize) {
    let phase = callback as f32 * 0.05;
    let mut commands = vec![
        Command::SetListenerPose {
            position: Vec3::new(phase.cos(), phase.sin(), 0.0),
            yaw: phase * 3.0,
        },
        Command::SetSourcePosition {
            index: 0,
            position: Vec3::new(2.0 * phase.sin(), 1.0, 0.5),
        },
        Command::SetSourceSpread {
            index: 0,
            spread: 0.5 + 0.5 * phase.sin(),
        },
        Command::SetMasterGain { gain: 0.8 },
        Command::SetSourceArc {
            index: 0,
            degrees: (90.0 + 60.0 * (phase * 0.5).sin()).round(),
        },
    ];
    match callback % 40 {
        5 => commands.push(Command::SetChannelMode {
            mode: ChannelMode::Stereo,
        }),
        10 => commands.push(Command::SetSourceLayer {
            index: 0,
            layer: 1,
            setting: LayerSetting {
                level_db: -12.0 + 6.0 * phase.sin(),
                ..LayerSetting::UNTOUCHED
            },
        }),
        12 => commands.push(Command::SetSourceLayer {
            index: 0,
            layer: 0,
            setting: LayerSetting {
                solo: callback % 80 == 12,
                ..LayerSetting::UNTOUCHED
            },
        }),
        15 => commands.push(Command::SetChannelMode {
            mode: ChannelMode::Quad,
        }),
        17 => commands.push(Command::SetSourceLayer {
            index: 0,
            layer: 1,
            setting: LayerSetting {
                mute: callback % 80 == 17,
                ..LayerSetting::UNTOUCHED
            },
        }),
        // Loops brought into step: the default scene's first three voices
        // loop, so the second and third dip out, jump and dip back in.
        20 => commands.push(Command::PlayInStep {
            anchor: 0,
            slots: 0b110,
        }),
        25 => commands.push(Command::SetChannelMode {
            mode: ChannelMode::Surround51,
        }),
        30 => commands.push(Command::SetAtmosphere {
            temperature_c: 5.0 + phase.sin() * 10.0,
            humidity_pct: 60.0,
        }),
        35 => commands.push(Command::SetSourceMuted {
            index: 1,
            muted: callback % 80 == 35,
        }),
        39 => commands.push(Command::ResetScene),
        _ => {}
    }
    for command in commands {
        producer.push(command).expect("command queue has room");
    }
}

/// Run every render mode of one scene; return a description of each violation.
fn violations_in(path: &str, arc_degrees: Option<f32>) -> Vec<String> {
    let (mut scene, channels) = open_scene(path, arc_degrees);
    let (mut producer, mut consumer) = rtrb::RingBuffer::<Command>::new(16);
    // Telemetry is pushed from the callback; keep its queue attached and drained.
    let (telemetry_producer, mut telemetry_consumer) = rtrb::RingBuffer::<TelemetryFrame>::new(8);
    scene.telemetry_out = Some(telemetry_producer);
    // So is the output tap. It holds eight callbacks: the steady stretch,
    // never read, fills it and drops blocks; the live stretch reads it after
    // every callback, so blocks are written.
    let (tap, mut tap_reader) = output_tap(channels, 8 * FRAMES_PER_CALLBACK);
    scene.output_tap = Some(tap);
    let mut tapped = Vec::new();
    let mut output = vec![0.0f32; FRAMES_PER_CALLBACK * channels];
    let mut violations = Vec::new();

    for mode in RENDER_MODES {
        producer
            .push(Command::SetRenderMode { mode })
            .expect("command queue has room");
        // The callback that performs the switch, measured on its own: a mode
        // change happens while audio is running, so it must be clean too.
        let (switching, switch_origins) =
            heap_operations_during(&mut scene, &mut consumer, &mut output, channels, 1);
        // Let ramps, delay lines and crossfades settle, then measure.
        heap_operations_during(&mut scene, &mut consumer, &mut output, channels, 20);
        let (steady, steady_origins) =
            heap_operations_during(&mut scene, &mut consumer, &mut output, channels, 200);

        // A control surface in use: commands arrive before every callback.
        let mut live = 0;
        let mut blocks_tapped = 0;
        let mut live_origins = Vec::new();
        for callback in 0..160 {
            push_live_commands(&mut producer, callback);
            let (operations, origins) =
                heap_operations_during(&mut scene, &mut consumer, &mut output, channels, 1);
            live += operations;
            if live_origins.is_empty() {
                live_origins = origins;
            }
            while telemetry_consumer.pop().is_ok() {}
            while tap_reader.next_block(&mut tapped).is_some() {
                blocks_tapped += 1;
            }
            // Real time passes between callbacks: an arc's new width is
            // designed meanwhile and handed over in a later one.
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        if blocks_tapped == 0 {
            violations.push(format!("{path} {mode:?}: the tap received nothing"));
        }
        if live > 0 {
            violations.push(format!(
                "{path} {mode:?}: {live} heap operations in 160 callbacks with live commands\n{}",
                live_origins.join("\n")
            ));
        }

        if switching > 0 {
            violations.push(format!(
                "{path} {mode:?}: {switching} heap operations in the callback that switches to it\n{}",
                switch_origins.join("\n")
            ));
        }
        if steady > 0 {
            violations.push(format!(
                "{path} {mode:?}: {steady} heap operations in 200 steady callbacks\n{}",
                steady_origins.join("\n")
            ));
        }
    }
    violations
}

#[test]
fn audio_callback_never_touches_the_heap() {
    let mut violations = Vec::new();
    for (path, arc_degrees) in [
        ("scenes/default.yaml", None),
        ("scenes/nature.yaml", None),
        // Its field wind is ambient, so on headphones the ears' diffuse
        // field is measured too.
        ("scenes/synth-test.yaml", None),
        // And here from an arc, on headphones and on speakers, its width
        // changed while it plays.
        ("scenes/synth-test.yaml", Some(90.0)),
        ("scenes/wisp.yaml", None),
        // Two voices that are one sound's two layers.
        ("scenes/binaural-beat-test.yaml", None),
    ] {
        violations.extend(violations_in(path, arc_degrees));
    }
    assert!(
        violations.is_empty(),
        "{} violation(s):\n\n{}",
        violations.len(),
        violations.join("\n\n")
    );
}

/// The generator lab's voice: three candidates in step, switched with a
/// fade, every wrap and replay putting back the soft wind's state at the
/// cue. A loop of half a second wraps every 47 callbacks; between callbacks
/// the candidate, the trims, the working wind's live settings, the loop and
/// play change, and the passage is replayed. None of it touches the heap.
#[test]
fn the_labs_voice_never_touches_the_heap() {
    use std::sync::Arc;

    use atrium::audio::decode::AudioBuffer;
    use atrium::audio::spectral_profile::SpectralProfile;
    use atrium::config::soft_wind_of;
    use atrium::lab::{
        placed_like, state_at, Candidate, CandidateSettings, LabControls, LabVoice, VoiceParts,
    };
    use atrium_world::{Entity, EntityId, Moves, Place, Shape, Sound, When};

    let entity = Entity {
        name: None,
        color: None,
        group: None,
        sound: Sound {
            preset: Some("sources/weather/wind/soft-wind.yaml".into()),
            audio: None,
            level: None,
            layers: Default::default(),
        },
        shape: Shape::default(),
        place: Place {
            x: 0.0,
            y: 2.0,
            z: 1.2,
        },
        when: When::default(),
        follows: None,
        moves: Moves::Still,
    };
    let built = SceneConfig::load("scenes/default.yaml")
        .and_then(SceneConfig::build)
        .expect("the default scene builds");
    let (mut scene, sounds) = (built.scene, built.sounds);
    scene.sample_rate = SAMPLE_RATE;
    scene.init_pipelines();
    let channels = scene.speaker_layout.total_channels();
    let (tap, mut tap_reader) = output_tap(channels, 8 * FRAMES_PER_CALLBACK);
    scene.output_tap = Some(tap);

    let preset = soft_wind_of(&entity, 1).expect("the soft wind builds");
    let untouched = CandidateSettings::default().live_values(&preset);
    let mut brighter = CandidateSettings::default();
    brighter.values.insert("gust_brightness".into(), 0.4);
    let brighter = brighter.live_values(&preset);
    let controls = Arc::new(LabControls::new(Candidate::Reference, untouched));
    controls.set_playing(true);
    let kept = state_at(preset, 5.0, SAMPLE_RATE);
    let reference = Arc::new(AudioBuffer {
        samples: (0..48_000)
            .map(|index| ((index as f32) * 0.07).sin() * 0.1)
            .collect(),
        sample_rate: 44_100,
        rms: 0.0,
        spectral_profile: SpectralProfile::default(),
    });
    let voice = LabVoice::new(VoiceParts {
        controls: controls.clone(),
        sample_rate: SAMPLE_RATE,
        reference,
        reference_gain: 1.0,
        reference_from_seconds: 0.2,
        loop_seconds: 0.5,
        previous: kept.clone(),
        working: kept,
    });
    let slot = scene.sources.len() - 1;
    let lab = sounds
        .build(&EntityId::new("lab").expect("an id"), &entity, slot)
        .expect("builds");
    scene.sources[slot] = placed_like(voice, &lab);

    let (_producer, mut consumer) = rtrb::RingBuffer::<Command>::new(4);
    let mut output = vec![0.0f32; FRAMES_PER_CALLBACK * channels];
    let mut tapped = Vec::new();
    let mut violations = Vec::new();
    for callback in 0..600 {
        match callback % 60 {
            7 | 37 => controls.hear(Candidate::Working),
            14 => controls.hear(Candidate::Previous),
            21 | 51 => controls.hear(Candidate::Reference),
            25 => controls.set_trim(Candidate::Working, -3.0 + (callback % 7) as f32),
            30 => controls.set_working(if callback % 120 == 30 {
                brighter
            } else {
                untouched
            }),
            33 => controls.replay(),
            40 => controls.set_looping(callback % 120 != 40),
            45 => controls.set_playing(false),
            48 => controls.set_playing(true),
            _ => {}
        }
        let (operations, origins) =
            heap_operations_during(&mut scene, &mut consumer, &mut output, channels, 1);
        if operations > 0 && violations.is_empty() {
            violations.push(format!(
                "callback {callback}: {operations} heap operations\n{}",
                origins.join("\n")
            ));
        }
        while tap_reader.next_block(&mut tapped).is_some() {}
    }
    assert!(controls.position_frames() > 0, "the voice played");
    assert!(violations.is_empty(), "{}", violations.join("\n\n"));
}
