use std::sync::{Arc, Mutex};
use std::time::Duration;

use atrium::audio::output::{AudioOutput, CpalOutput};
use atrium::config::SceneConfig;
use atrium::engine::telemetry::{telemetry_to_json, TelemetryFrame};
use atrium::server::websocket::{run_server, TelemetryBroadcast};
use atrium_core::commands::Command;

#[cfg(feature = "memprof")]
#[global_allocator]
static ALLOC: atrium::engine::memprof::TrackingAllocator =
    atrium::engine::memprof::TrackingAllocator;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let ui_enabled = cfg!(feature = "tui") && args.iter().any(|a| a == "--ui");
    let bevy_enabled = cfg!(feature = "bevy") && args.iter().any(|a| a == "--bevy");
    let scene_path = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .cloned()
        .unwrap_or_else(|| "scenes/default.yaml".to_string());

    // Initialize profiling subscriber (--profile fmt|perfetto|flame)
    #[cfg(feature = "profiler")]
    let _profiler_guard = {
        let profile_mode = args
            .iter()
            .position(|a| a == "--profile")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str());

        init_profiler(profile_mode)?
    };

    println!("Loading scene: {}", scene_path);
    let config = SceneConfig::load(&scene_path)?;
    let mut result = config.build()?;

    // Telemetry channel: audio thread → broadcaster/Bevy (small ring, latest-wins)
    let (telem_producer, telem_consumer) = rtrb::RingBuffer::<TelemetryFrame>::new(4);
    result.scene.telemetry_out = Some(telem_producer);

    // Extract Bevy scene description from scene_json before anything consumes it
    #[cfg(feature = "bevy")]
    let bevy_description = if bevy_enabled {
        Some(build_scene_description(&result.scene_json)?)
    } else {
        None
    };

    #[cfg(feature = "tui")]
    let source_names = result.source_names.clone();
    #[cfg(feature = "tui")]
    let render_mode = format!("{:?}", result.scene.active_pipeline);
    #[cfg(feature = "tui")]
    let pipeline_post = result.pipeline_post.clone();
    #[cfg(feature = "tui")]
    let channel_labels = result.channel_labels.clone();

    // Start audio output
    let (producer, consumer) = rtrb::RingBuffer::<Command>::new(256);
    let handle = CpalOutput.start(result.scene, consumer)?;

    println!();
    println!("=== Atrium Spatial Audio ===");
    println!("Scene: {}", scene_path);
    if ui_enabled {
        println!("Terminal dashboard: active");
    }
    if bevy_enabled {
        println!("Bevy 3D visualization: active");
    }
    println!();

    // ── Bevy mode: Bevy owns the main thread, WS server on background thread ──
    #[cfg(feature = "bevy")]
    if bevy_enabled {
        let description = bevy_description.unwrap();
        let telemetry_receiver = atrium_bevy::TelemetryReceiver::new(telem_consumer);
        let command_sender = atrium_bevy::CommandSender::new(producer);

        // Keep audio handle alive
        let _handle = handle;

        // Bevy takes over the main thread (blocks until window closes)
        atrium_bevy::run(description, telemetry_receiver, command_sender);
        return Ok(());
    }

    // ── Default mode: telemetry broadcaster + WS server on main thread ─────────
    let mut telem_consumer = telem_consumer;

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
            pipeline_post,
            channel_labels,
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
                            gain_db: if s.gain_db.is_finite() {
                                s.gain_db
                            } else {
                                -60.0
                            },
                            is_muted: s.is_muted,
                            render_mode: mode_name.clone(),
                        }
                    })
                    .collect();
                let channel_statuses: Vec<atrium_tui::ChannelStatus> = (0..frame.channel_count
                    as usize)
                    .map(|ch| {
                        let peak = frame.channel_peaks[ch];
                        let peak_db = if peak > 0.0 {
                            20.0 * peak.log10()
                        } else {
                            -60.0
                        };
                        atrium_tui::ChannelStatus { peak_db }
                    })
                    .collect();
                let experiments = atrium_tui::ExperimentStatus::default();
                dash.update(&statuses, &channel_statuses, &experiments);
            }
        }
    });

    // Start WebSocket server (blocks on main thread, keeps _handle alive)
    let producer = Arc::new(Mutex::new(producer));
    let _handle = handle;
    run_server("0.0.0.0:3333", producer, result.scene_json, broadcast)?;

    Ok(())
}

/// Build a SceneDescription from the engine's scene JSON string.
/// This bridges the YAML config → audio engine → Bevy ECS path.
/// In the future, config.rs could produce SceneDescription directly.
#[cfg(feature = "bevy")]
fn build_scene_description(
    scene_json: &str,
) -> Result<atrium_bevy::SceneDescription, Box<dyn std::error::Error>> {
    use atrium_bevy::scene::schema::*;

    let json: serde_json::Value = serde_json::from_str(scene_json)?;

    let room = &json["room"];
    let atrium_json = &json["atrium"];
    let spawn = &json["spawn"];
    let listener_json = &json["listener"];
    let dm = &json["distance_model"];

    let speakers: Vec<SpeakerDescription> = json["speakers"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .enumerate()
        .map(|(i, speaker)| {
            let label = speaker["label"].as_str().unwrap_or("?").to_string();
            SpeakerDescription {
                id: label.to_lowercase(),
                label,
                position: [
                    speaker["x"].as_f64().unwrap_or(0.0) as f32,
                    speaker["y"].as_f64().unwrap_or(0.0) as f32,
                    speaker["z"].as_f64().unwrap_or(0.0) as f32,
                ],
                channel: speaker["channel"].as_u64().unwrap_or(i as u64) as usize,
            }
        })
        .collect();

    let sources: Vec<SourceDescription> = json["sources"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .enumerate()
        .map(|(i, source)| {
            let name = source["name"].as_str().unwrap_or("?").to_string();
            let pos = source["position"].as_array();
            SourceDescription {
                id: format!("source_{}", i),
                name,
                color: source["color"].as_str().unwrap_or("#ffffff").to_string(),
                position: [
                    pos.and_then(|a| a[0].as_f64()).unwrap_or(0.0) as f32,
                    pos.and_then(|a| a[1].as_f64()).unwrap_or(0.0) as f32,
                    pos.and_then(|a| a[2].as_f64()).unwrap_or(0.0) as f32,
                ],
                spl: source["spl"].as_f64().unwrap_or(80.0) as f32,
                ref_distance: source["ref_dist"].as_f64().unwrap_or(1.0) as f32,
                directivity: source["directivity"].as_str().unwrap_or("omni").to_string(),
                directivity_alpha: source["directivity_alpha"].as_f64().unwrap_or(1.0) as f32,
                spread: source["spread"].as_f64().unwrap_or(0.0) as f32,
                orbit_radius: source["orbit_radius"].as_f64().unwrap_or(0.0) as f32,
                orbit_speed: source["orbit_speed"].as_f64().unwrap_or(0.0) as f32,
            }
        })
        .collect();

    let layout_str = json["channel_mode"]
        .as_str()
        .or_else(|| {
            // Fall back to speaker count heuristic
            None
        })
        .unwrap_or(match speakers.len() {
            2 => "stereo",
            4 => "quad",
            _ => "5.1",
        })
        .to_string();

    Ok(SceneDescription {
        version: 1,
        environment: EnvironmentDescription {
            width: room["width"].as_f64().unwrap_or(20.0) as f32,
            depth: room["depth"].as_f64().unwrap_or(20.0) as f32,
            height: room["height"].as_f64().unwrap_or(10.0) as f32,
            spawn: [
                spawn["x"].as_f64().unwrap_or(0.0) as f32,
                spawn["y"].as_f64().unwrap_or(0.0) as f32,
                spawn["z"].as_f64().unwrap_or(0.0) as f32,
            ],
        },
        atrium: AtriumDescription {
            width: atrium_json["width"].as_f64().unwrap_or(6.0) as f32,
            depth: atrium_json["depth"].as_f64().unwrap_or(4.0) as f32,
            height: atrium_json["height"].as_f64().unwrap_or(3.0) as f32,
        },
        listener: ListenerDescription {
            position: [
                listener_json["x"].as_f64().unwrap_or(0.0) as f32,
                listener_json["y"].as_f64().unwrap_or(0.0) as f32,
                listener_json["z"].as_f64().unwrap_or(0.0) as f32,
            ],
            yaw_degrees: (listener_json["yaw"].as_f64().unwrap_or(0.0) as f32).to_degrees(),
        },
        sources,
        speakers: SpeakerLayoutDescription {
            layout: layout_str,
            speakers,
            dbap_rolloff_db: json["dbap_rolloff_db"].as_f64().unwrap_or(6.0) as f32,
        },
        render_mode: json["render_mode"].as_str().unwrap_or("vbap").to_string(),
        master_gain: json["master_gain"].as_f64().unwrap_or(1.0) as f32,
        distance_model: DistanceModelDescription {
            model: dm["model"].as_str().unwrap_or("inverse").to_string(),
            ref_distance: dm["ref_distance"].as_f64().unwrap_or(1.0) as f32,
            max_distance: dm["max_distance"].as_f64().unwrap_or(20.0) as f32,
            rolloff: dm["rolloff"].as_f64().unwrap_or(1.0) as f32,
        },
        atmosphere: AtmosphereDescription {
            temperature_c: json["atmosphere"]["temperature_c"].as_f64().unwrap_or(20.0) as f32,
            humidity_pct: json["atmosphere"]["humidity_pct"].as_f64().unwrap_or(50.0) as f32,
            pressure_kpa: json["atmosphere"]["pressure_kpa"]
                .as_f64()
                .unwrap_or(101.325) as f32,
        },
    })
}

/// Initialize the tracing subscriber based on the --profile mode.
/// Returns a guard that must be held alive for the duration of the program
/// (FlameLayer flushes on guard drop).
#[cfg(feature = "profiler")]
fn init_profiler(
    mode: Option<&str>,
) -> Result<
    Option<tracing_flame::FlushGuard<std::io::BufWriter<std::fs::File>>>,
    Box<dyn std::error::Error>,
> {
    use tracing_subscriber::prelude::*;

    match mode {
        Some("fmt") => {
            tracing_subscriber::registry()
                .with(tracing_subscriber::fmt::layer())
                .init();
            println!("Profiler: fmt (terminal span timing)");
            Ok(None)
        }
        Some("perfetto") => {
            let file = std::sync::Mutex::new(std::fs::File::create("trace.pftrace")?);
            tracing_subscriber::registry()
                .with(tracing_perfetto::PerfettoLayer::new(file))
                .init();
            println!("Profiler: perfetto → trace.pftrace");
            Ok(None)
        }
        Some("flame") => {
            let (flame_layer, guard) = tracing_flame::FlameLayer::with_file("tracing.folded")?;
            tracing_subscriber::registry().with(flame_layer).init();
            println!("Profiler: flame → tracing.folded");
            Ok(Some(guard))
        }
        Some(other) => {
            eprintln!("Unknown --profile mode: {other}. Options: fmt, perfetto, flame");
            std::process::exit(1);
        }
        None => Ok(None),
    }
}
