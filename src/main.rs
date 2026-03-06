use std::sync::{Arc, Mutex};
use std::time::Duration;

use atrium::audio::output::{AudioOutput, CpalOutput};
use atrium::config::SceneConfig;
use atrium::engine::commands::Command;
use atrium::engine::telemetry::{telemetry_to_json, TelemetryFrame};
use atrium::server::websocket::{run_server, TelemetryBroadcast};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scene_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "scenes/default.yaml".to_string());

    println!("Loading scene: {}", scene_path);
    let config = SceneConfig::load(&scene_path)?;
    let mut result = config.build()?;

    // Telemetry channel: audio thread → broadcaster thread (small ring, latest-wins)
    let (telem_producer, mut telem_consumer) =
        rtrb::RingBuffer::<TelemetryFrame>::new(4);
    result.scene.telemetry_out = Some(telem_producer);

    // Start audio output
    let (producer, consumer) = rtrb::RingBuffer::<Command>::new(256);
    let _handle = CpalOutput.start(result.scene, consumer)?;

    println!();
    println!("=== Atrium Spatial Audio ===");
    println!("Scene: {}", scene_path);
    println!();

    // Telemetry broadcaster: drains ring buffer at ~15 Hz, publishes latest JSON
    let broadcast = Arc::new(TelemetryBroadcast::new());
    let bc = broadcast.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(66)); // ~15 Hz
        // Drain all available frames, keep the latest
        let mut latest: Option<TelemetryFrame> = None;
        while let Ok(frame) = telem_consumer.pop() {
            latest = Some(frame);
        }
        if let Some(frame) = latest {
            let json = telemetry_to_json(&frame);
            bc.update(json);
        }
    });

    // Start WebSocket server (blocks on main thread, keeps _handle alive)
    let producer = Arc::new(Mutex::new(producer));
    run_server("0.0.0.0:3333", producer, result.scene_json, broadcast)?;

    Ok(())
}
