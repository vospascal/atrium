use std::sync::{Arc, Mutex};
use std::time::Duration;

use atrium::audio::output::{AudioOutput, CpalOutput};
use atrium::config::SceneConfig;
#[cfg(feature = "bevy")]
use atrium::config::{build_one_source, NormalizationConfig, SourceDef, SourceEntry};
#[cfg(feature = "bevy")]
use atrium::engine::edit::{Retired, SceneEdit};
use atrium::engine::telemetry::{telemetry_to_json, TelemetryFrame};
use atrium::server::websocket::{run_server, TelemetryBroadcast};
#[cfg(feature = "bevy")]
use atrium_bevy::SceneHost as _;
use atrium_core::commands::Command;

#[cfg(feature = "memprof")]
#[global_allocator]
static ALLOC: atrium::engine::memprof::TrackingAllocator =
    atrium::engine::memprof::TrackingAllocator;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let ui_enabled = cfg!(feature = "tui") && args.iter().any(|a| a == "--ui");
    #[cfg(feature = "bevy")]
    let bevy_enabled = args.iter().any(|a| a == "--bevy");
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

    // ── Bevy mode: Bevy owns the audio lifecycle so it can reload scenes ──
    // The scene host builds the initial scene + stream, and rebuilds them on
    // reload. Bevy takes over the main thread and blocks until the window closes.
    #[cfg(feature = "bevy")]
    if bevy_enabled {
        let mut host = AtriumSceneHost::default();
        let initial = match host.reload(atrium_bevy::ReloadTarget::ScenePath(
            scene_path.clone().into(),
        )) {
            Ok(output) => output,
            Err(err) => {
                eprintln!("failed to load scene: {err}");
                std::process::exit(1);
            }
        };
        println!();
        println!("=== Atrium Spatial Audio ===");
        println!("Bevy visualization: active");
        println!();
        atrium_bevy::run(
            initial.description,
            initial.telemetry_receiver,
            initial.command_sender,
            initial.audio,
            Box::new(host),
        );
        return Ok(());
    }

    let config = SceneConfig::load(&scene_path)?;
    let mut result = config.build()?;

    // Telemetry channel: audio thread → broadcaster/Bevy (small ring, latest-wins)
    let (telem_producer, telem_consumer) = rtrb::RingBuffer::<TelemetryFrame>::new(4);
    result.scene.telemetry_out = Some(telem_producer);

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
    println!();

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

/// Number of audio-thread source pool slots (must match `config::build` padding).
#[cfg(feature = "bevy")]
const MAX_SOURCES: usize = atrium_core::telemetry::MAX_SOURCES;

/// Colors handed to live-added sources (cycled by slot).
#[cfg(feature = "bevy")]
const ADD_COLORS: &[&str] = &[
    "#ff6b35", "#ffc107", "#ce93d8", "#4fc3f7", "#66bb6a", "#ef5350", "#ff8a65", "#ab47bc",
    "#26c6da", "#9ccc65",
];

/// Injected scene host: builds (and rebuilds) the audio scene + stream for the
/// Bevy app, keeping `atrium`'s yaml/decode/pipeline machinery out of bevy-ui.
///
/// Holds the authoritative scene state so Save serializes exactly what's live
/// (positions, edits, and add/removed sources) and so live add/remove can build
/// sources on the control thread and splice them onto the audio thread with no
/// gap. `slot_entries` mirrors the audio thread's 16-slot pool (slot → entry,
/// `None` = free) — sparse after a live remove.
#[cfg(feature = "bevy")]
#[derive(Default)]
struct AtriumSceneHost {
    config: Option<SceneConfig>,
    slot_entries: Vec<Option<SourceEntry>>,
    spawn: [f32; 3],
    normalization: NormalizationConfig,
    global_ref_dist: f32,
    max_distance: f32,
    edit_tx: Option<rtrb::Producer<SceneEdit>>,
    retire_rx: Option<rtrb::Consumer<Retired>>,
}

/// Build the slot pool (length `MAX_SOURCES`) from a scene's dense source list.
#[cfg(feature = "bevy")]
fn build_slot_entries(sources: &[SourceEntry]) -> Vec<Option<SourceEntry>> {
    let mut entries: Vec<Option<SourceEntry>> = sources
        .iter()
        .take(MAX_SOURCES)
        .map(|e| Some(e.clone()))
        .collect();
    entries.resize(MAX_SOURCES, None);
    entries
}

/// Opaque wrapper so bevy-ui can hold the `!Send` stream handle. Never read —
/// held only to keep the stream alive; dropping it stops audio.
#[cfg(feature = "bevy")]
#[allow(dead_code)]
struct StreamHost(Box<dyn atrium::audio::output::StreamHandle>);

#[cfg(feature = "bevy")]
impl atrium_bevy::AudioHandle for StreamHost {}

#[cfg(feature = "bevy")]
impl atrium_bevy::SceneHost for AtriumSceneHost {
    fn reload(
        &mut self,
        target: atrium_bevy::ReloadTarget,
    ) -> Result<atrium_bevy::ReloadOutput, String> {
        let atrium_bevy::ReloadTarget::ScenePath(path) = target;
        let path = path.to_str().ok_or("scene path is not valid UTF-8")?;

        let config = SceneConfig::load(path).map_err(|e| e.to_string())?;
        // Build from a clone; keep the original for Save.
        let mut result = config.clone().build().map_err(|e| e.to_string())?;

        // Channels: telemetry (audio→main), scene-edits (main→audio, non-Copy),
        // retire (audio→main). The Command channel is created below.
        let (telem_producer, telem_consumer) = rtrb::RingBuffer::<TelemetryFrame>::new(4);
        result.scene.telemetry_out = Some(telem_producer);
        let (edit_tx, edit_rx) = rtrb::RingBuffer::<SceneEdit>::new(MAX_SOURCES + 4);
        let (retire_tx, retire_rx) = rtrb::RingBuffer::<Retired>::new(MAX_SOURCES + 4);
        result.scene.scene_edits = Some(edit_rx);
        result.scene.retired_out = Some(retire_tx);

        let description = build_scene_description(&result.scene_json).map_err(|e| e.to_string())?;

        let (producer, consumer) = rtrb::RingBuffer::<Command>::new(256);
        let handle = CpalOutput
            .start(result.scene, consumer)
            .map_err(|e| e.to_string())?;

        // Authoritative state for live edits + Save.
        self.spawn = description.environment.spawn;
        self.normalization = config.normalization.clone();
        self.global_ref_dist = config.distance_model.ref_distance;
        self.max_distance = config.distance_model.max_distance;
        self.slot_entries = build_slot_entries(&config.sources);
        self.config = Some(config);
        self.edit_tx = Some(edit_tx);
        self.retire_rx = Some(retire_rx);

        Ok(atrium_bevy::ReloadOutput {
            audio: Box::new(StreamHost(handle)),
            command_sender: atrium_bevy::CommandSender::new(producer),
            telemetry_receiver: atrium_bevy::TelemetryReceiver::new(telem_consumer),
            description,
        })
    }

    fn save(
        &mut self,
        path: &std::path::Path,
        description: &atrium_bevy::SceneDescription,
    ) -> Result<(), String> {
        let mut out = self
            .config
            .as_ref()
            .ok_or("no scene loaded to save")?
            .clone();

        // Live state is world coords; SceneConfig is atrium-local (offset by spawn).
        let spawn = self.spawn;
        let to_local = |w: [f32; 3]| [w[0] - spawn[0], w[1] - spawn[1], w[2] - spawn[2]];

        // Overlay live source state (position, orbit, and any live SPL/spread/
        // directivity edits) into the authoritative slot entries.
        for src in &description.sources {
            if let Some(Some(entry)) = self.slot_entries.get_mut(src.slot) {
                entry.position = to_local(src.position);
                entry.orbit_radius = src.orbit_radius;
                entry.orbit_speed = src.orbit_speed;
                entry.name = Some(src.name.clone());
                entry.color = Some(src.color.clone());
                entry.reference_spl = Some(src.spl);
                entry.spread = Some(src.spread);
                entry.directivity = Some(src.directivity.clone());
            }
        }

        // Rebuild the dense source list (slot order) from the live pool.
        out.sources = self.slot_entries.iter().flatten().cloned().collect();
        out.listener.position = to_local(description.listener.position);
        out.listener.yaw_degrees = description.listener.yaw_degrees;

        let yaml = serde_yaml::to_string(&out).map_err(|e| e.to_string())?;
        std::fs::write(path, yaml).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn add_source(
        &mut self,
        spec: atrium_bevy::AddSpec,
    ) -> Result<atrium_bevy::AddedSource, String> {
        // First free slot.
        let slot = self
            .slot_entries
            .iter()
            .position(|entry| entry.is_none())
            .ok_or(format!("source pool is full ({MAX_SOURCES} max)"))?;

        // Resolve the origin into audio + intrinsic sound properties.
        let (audio_path, spl, directivity, spread, source_yaml, name) = match spec.origin {
            atrium_bevy::AddOrigin::Preset(yaml) => {
                let contents =
                    std::fs::read_to_string(&yaml).map_err(|e| format!("{yaml}: {e}"))?;
                let def: SourceDef =
                    serde_yaml::from_str(&contents).map_err(|e| format!("{yaml}: {e}"))?;
                let name = file_stem_or(&yaml, "source");
                (
                    def.path,
                    def.reference_spl,
                    def.directivity,
                    def.spread,
                    Some(yaml),
                    name,
                )
            }
            atrium_bevy::AddOrigin::AudioFile(path) => {
                let name = file_stem_or(&path, "source");
                (path, 70.0, "omni".to_string(), 0.3, None, name)
            }
        };

        // Build the source on this thread (decode + amplitude + spectral bands).
        let world =
            atrium_core::types::Vec3::new(spec.position[0], spec.position[1], spec.position[2]);
        let built = build_one_source(
            &audio_path,
            spl,
            &directivity,
            spread,
            world,
            0.0,
            0.0,
            &self.normalization,
            self.global_ref_dist,
            self.max_distance,
        )
        .map_err(|e| e.to_string())?;
        let ref_dist = built.ref_dist;
        let directivity_alpha = built.directivity_alpha;

        // Splice into the audio thread's free slot (no gap).
        {
            let edit_tx = self.edit_tx.as_mut().ok_or("no audio stream")?;
            let edit = SceneEdit::AddSource {
                slot: slot as u16,
                source: built.source,
                bands: built.bands,
                amplitude: built.amplitude,
            };
            if edit_tx.push(edit).is_err() {
                return Err("scene-edit channel full".into());
            }
        }

        // Record the authoritative (self-contained) entry so Save round-trips.
        let color = ADD_COLORS[slot % ADD_COLORS.len()].to_string();
        let local = [
            spec.position[0] - self.spawn[0],
            spec.position[1] - self.spawn[1],
            spec.position[2] - self.spawn[2],
        ];
        self.slot_entries[slot] = Some(SourceEntry {
            source: source_yaml,
            audio: Some(audio_path),
            reference_spl: Some(spl),
            directivity: Some(directivity.clone()),
            spread: Some(spread),
            name: Some(name.clone()),
            color: Some(color.clone()),
            position: local,
            orbit_radius: 0.0,
            orbit_speed: 0.0,
        });

        Ok(atrium_bevy::AddedSource {
            slot: slot as u16,
            description: atrium_bevy::scene::schema::SourceDescription {
                id: format!("source_{slot}"),
                slot,
                name,
                color,
                position: spec.position,
                spl,
                ref_distance: ref_dist,
                directivity,
                directivity_alpha,
                spread,
                orbit_radius: 0.0,
                orbit_speed: 0.0,
            },
        })
    }

    fn remove_source(&mut self, slot: u16) -> Result<(), String> {
        let index = slot as usize;
        if self
            .slot_entries
            .get(index)
            .map(|e| e.is_none())
            .unwrap_or(true)
        {
            return Err(format!("no source in slot {slot}"));
        }
        {
            let edit_tx = self.edit_tx.as_mut().ok_or("no audio stream")?;
            let filler: Box<dyn atrium_core::source::SoundSource> =
                Box::new(atrium::audio::silence_node::SilenceNode);
            if edit_tx
                .push(SceneEdit::RemoveSource { slot, filler })
                .is_err()
            {
                return Err("scene-edit channel full".into());
            }
        }
        self.slot_entries[index] = None;
        Ok(())
    }

    fn browse_audio(&mut self) -> Option<String> {
        rfd::FileDialog::new()
            .add_filter("audio", &["mp3", "ogg", "wav", "flac", "aac"])
            .set_directory("assets")
            .pick_file()
            .and_then(|path| path.to_str().map(|s| s.to_string()))
    }

    fn drain_retired(&mut self) {
        if let Some(rx) = self.retire_rx.as_mut() {
            // Dropping each `Retired` here frees the box on the control thread.
            while rx.pop().is_ok() {}
        }
    }
}

/// File stem of a path, or a fallback if it has none.
#[cfg(feature = "bevy")]
fn file_stem_or(path: &str, fallback: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(fallback)
        .to_string()
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
                slot: i,
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
