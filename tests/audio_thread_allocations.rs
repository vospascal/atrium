//! The audio callback must not touch the heap.
//!
//! An allocation (or a free) on the audio thread can block on the allocator's
//! lock for an unbounded time, which is heard as a dropout. This test builds
//! the shipped scenes exactly as the application does, then counts every heap
//! operation made inside `process_commands` + `render`, in every render mode.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};

use atrium::config::SceneConfig;
use atrium::engine::scene::AudioScene;
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

/// Build a scene from its YAML and prepare it the way the output device does.
fn open_scene(path: &str) -> (AudioScene, usize) {
    let mut scene = SceneConfig::load(path)
        .and_then(SceneConfig::build)
        .unwrap_or_else(|error| panic!("{path}: {error}"))
        .scene;
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
/// change level, and now and then the output or the weather changes.
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
    ];
    match callback % 40 {
        5 => commands.push(Command::SetChannelMode {
            mode: ChannelMode::Stereo,
        }),
        15 => commands.push(Command::SetChannelMode {
            mode: ChannelMode::Quad,
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
fn violations_in(path: &str) -> Vec<String> {
    let (mut scene, channels) = open_scene(path);
    let (mut producer, mut consumer) = rtrb::RingBuffer::<Command>::new(16);
    // Telemetry is pushed from the callback; keep its queue attached and drained.
    let (telemetry_producer, mut telemetry_consumer) = rtrb::RingBuffer::<TelemetryFrame>::new(8);
    scene.telemetry_out = Some(telemetry_producer);
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
    for path in [
        "scenes/default.yaml",
        "scenes/nature.yaml",
        // Its field wind is ambient, so on headphones the ears' diffuse
        // field is measured too.
        "scenes/synth-test.yaml",
        "scenes/wisp.yaml",
    ] {
        violations.extend(violations_in(path));
    }
    assert!(
        violations.is_empty(),
        "{} violation(s):\n\n{}",
        violations.len(),
        violations.join("\n\n")
    );
}
