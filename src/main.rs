use std::sync::{Arc, Mutex};
use std::time::Duration;

use atrium::audio::output::{AudioOutput, CpalOutput};
use atrium::config::SceneConfig;
use atrium::engine::commands::Command;
use atrium::engine::telemetry::{telemetry_to_json, TelemetryFrame};
use atrium::server::websocket::{run_server, TelemetryBroadcast};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let ui_enabled = cfg!(feature = "tui") && args.iter().any(|a| a == "--ui");
    let scene_path = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .cloned()
        .unwrap_or_else(|| "scenes/default.yaml".to_string());

    println!("Loading scene: {}", scene_path);
    let config = SceneConfig::load(&scene_path)?;
    let mut result = config.build()?;

    // Telemetry channel: audio thread → broadcaster thread (small ring, latest-wins)
    let (telem_producer, mut telem_consumer) =
        rtrb::RingBuffer::<TelemetryFrame>::new(4);
    result.scene.telemetry_out = Some(telem_producer);

    #[cfg(feature = "tui")]
    let source_names = result.source_names.clone();
    #[cfg(feature = "tui")]
    let render_mode = format!("{:?}", result.scene.speaker_layout.mode);
    #[cfg(feature = "tui")]
    let pipeline_pre = result.pipeline_pre.clone();
    #[cfg(feature = "tui")]
    let pipeline_post = result.pipeline_post.clone();

    // Start audio output
    let (producer, consumer) = rtrb::RingBuffer::<Command>::new(256);
    let handle = CpalOutput.start(result.scene, consumer)?;

    println!();
    println!("=== Atrium Spatial Audio ===");
    println!("Scene: {}", scene_path);
    if ui_enabled {
        println!("Terminal dashboard: active");
    }
    println!();

    // Telemetry broadcaster: drains ring buffer at ~15 Hz, publishes latest JSON
    let broadcast = Arc::new(TelemetryBroadcast::new());
    let bc = broadcast.clone();

    // Build optional TUI dashboard
    #[cfg(feature = "tui")]
    let mut dashboard = if ui_enabled {
        Some(atrium_tui::Dashboard::new(atrium_tui::DeviceInfo {
            device_name: handle.device_name().to_string(),
            sample_rate: handle.sample_rate(),
            channels: handle.channels(),
            render_mode,
            scene_path: scene_path.clone(),
            source_names,
            pipeline_pre,
            pipeline_post,
        }))
    } else {
        None
    };

    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(66)); // ~15 Hz
        // Drain all available frames, keep the latest
        let mut latest: Option<TelemetryFrame> = None;
        while let Ok(frame) = telem_consumer.pop() {
            latest = Some(frame);
        }
        if let Some(frame) = latest {
            let json = telemetry_to_json(&frame);
            bc.update(json.clone());

            // Update terminal dashboard
            #[cfg(feature = "tui")]
            if let Some(ref mut dash) = dashboard {
                let mode_name = format!("{:?}", frame.render_mode);
                let statuses: Vec<atrium_tui::SourceStatus> = (0..frame.source_count as usize)
                    .map(|i| {
                        let s = &frame.sources[i];
                        atrium_tui::SourceStatus {
                            distance: s.distance,
                            gain_db: if s.gain_db.is_finite() { s.gain_db } else { -60.0 },
                            is_muted: s.is_muted,
                            render_mode: mode_name.clone(),
                        }
                    })
                    .collect();
                dash.update(&statuses);
            }
        }
    });

    // Start WebSocket server (blocks on main thread, keeps _handle alive)
    let producer = Arc::new(Mutex::new(producer));
    let _handle = handle;
    run_server("0.0.0.0:3333", producer, result.scene_json, broadcast)?;

    Ok(())
}
