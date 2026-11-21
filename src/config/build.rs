//! Building the engine's `AudioScene` from a loaded `SceneConfig`: loading the
//! referenced files, calibrating each source, and assembling the pipelines.

use std::path::Path;
use std::sync::Arc;

use atrium_core::directivity::DirectivityPattern;
use atrium_core::listener::Listener;
use atrium_core::panner::DistanceModelType;
use atrium_core::source::EmitterKind;
use atrium_core::speaker::{ChannelMode, RenderMode, SpeakerLayout};
use atrium_core::types::Vec3;
use atrium_world::Entity;

use super::description::{beam_half_angle, SceneDescription, SourceDescription, TakeDescription};
use super::environment::{build_wall_materials, EnvironmentConfig};
use super::scene::{AtmosphereConfig, AtriumConfig, NormalizationConfig, SceneConfig};
use super::sound_builder::SoundBuilder;
use super::source::{resolve_spl, ResolvedTake};
use super::synth::SynthSpec;
use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::decode::decode_file;
use crate::audio::distance::DistanceModel;
use crate::audio::propagation::GroundProperties;
use crate::audio::synth_node::SynthNode;
use crate::audio::test_node::TestNode;
use crate::audio::voice_node::{Rests, Take, VoiceNode};
use crate::engine::scene::{AudioScene, InitialSourceState};
use crate::pipeline::environment::Environment;
use crate::pipeline::{build_all_pipelines, PipelineParams};

pub struct BuildResult {
    pub scene: AudioScene,
    /// Builds one more sound for this scene while it plays, the same way
    /// the scene's own sounds were built.
    pub sounds: SoundBuilder,
    /// The built scene for an in-process UI (the Kotlin app via `atrium-ffi`).
    pub description: SceneDescription,
    /// The built scene for a WebSocket client.
    pub scene_json: String,
    pub source_names: Vec<String>,
    /// Pipeline mix stage names (for TUI display).
    pub pipeline_post: Vec<String>,
    /// Channel labels for TUI display (e.g. ["FL", "FR", "C", "LFE", "RL", "RR"]).
    pub channel_labels: Vec<String>,
}

/// Result of building sources.
struct BuildSourcesResult {
    sources: Vec<Box<dyn atrium_core::source::SoundSource>>,
    metas: Vec<SourceMeta>,
    /// Per-source spectral profile bands (24 Bark bands, dB relative to RMS).
    spectral_profiles: Vec<[f32; crate::audio::spectral_profile::BARK_BANDS]>,
    /// Per-source base amplitude (sone-based gain, before spatial attenuation).
    source_amplitudes: Vec<f32>,
}

/// Default color palette for sources when no color is specified in YAML.
const SOURCE_COLORS: &[&str] = &[
    "#ff6b35", "#ffc107", "#ce93d8", "#4fc3f7", "#66bb6a", "#ef5350", "#ff8a65", "#ab47bc",
    "#26c6da", "#9ccc65",
];

/// A sound's UI color: its own `color`, else the palette entry for its slot.
pub(super) fn source_color(entity: &Entity, slot: usize) -> String {
    entity
        .color
        .clone()
        .unwrap_or_else(|| SOURCE_COLORS[slot % SOURCE_COLORS.len()].to_string())
}

/// Metadata collected during source building, serialized to JSON for the browser.
pub(super) struct SourceMeta {
    pub(super) id: String,
    /// Its takes, in the order telemetry counts them.
    pub(super) takes: Vec<TakeDescription>,
    /// Shortest and longest rest between takes; `None` when it plays all the time.
    pub(super) gap: Option<[f32; 2]>,
    pub(super) group: Option<String>,
    pub(super) name: String,
    pub(super) color: String,
    pub(super) spl: f32,
    pub(super) ref_dist: f32,
    pub(super) amplitude: f32,
    pub(super) audible_radius: f32,
    /// Width (degrees) and level behind (dB) it radiates with.
    pub(super) width: f32,
    pub(super) behind: f32,
    pub(super) directivity_alpha: f32,
    pub(super) spread: f32,
    pub(super) position: [f32; 3],
    pub(super) orbit_radius: f32,
    pub(super) orbit_speed: f32,
    pub(super) synth_kind: Option<String>,
    pub(super) emitter_kind: EmitterKind,
}

// ── Loading & building ──────────────────────────────────────────────────────

/// Load and deserialize a YAML file into any serde-compatible type.
pub(super) fn load_yaml<T: serde::de::DeserializeOwned>(
    path: &str,
) -> Result<T, Box<dyn std::error::Error>> {
    // Warn about absolute or parent-traversing paths (not sandboxed, but logged)
    if std::path::Path::new(path).is_absolute() || path.contains("..") {
        eprintln!("warning: loading file from non-relative path: {path}");
    }
    let contents = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))?;
    serde_yaml::from_str(&contents).map_err(|e| format!("{}: {}", path, e).into())
}

impl SceneConfig {
    pub fn load(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        load_yaml(path)
    }

    pub fn build(self) -> Result<BuildResult, Box<dyn std::error::Error>> {
        // Load environment (virtual acoustic space) from file
        let environment_cfg: EnvironmentConfig = load_yaml(&self.environment)?;
        let environment_min = Vec3::ZERO;
        let environment_max = Vec3::new(
            environment_cfg.width,
            environment_cfg.depth,
            environment_cfg.height,
        );

        // Spawn point: where the atrium center sits in the environment.
        // Defaults to the center of the environment floor.
        let spawn = match environment_cfg.spawn {
            Some(arr) => Vec3::new(arr[0], arr[1], arr[2]),
            None => Vec3::new(
                environment_cfg.width / 2.0,
                environment_cfg.depth / 2.0,
                0.0,
            ),
        };

        // Load atrium (physical speaker room) dimensions — optional, defaults to environment
        let atrium_cfg: AtriumConfig = if let Some(ref path) = self.atrium {
            load_yaml(path)?
        } else {
            AtriumConfig {
                width: environment_cfg.width,
                depth: environment_cfg.depth,
                height: environment_cfg.height,
            }
        };

        // Scene positions are atrium-local — offset by spawn to get world coordinates.
        let listener_pos = arr_to_vec3(self.listener.position) + spawn;
        let listener = Listener::new(listener_pos, self.listener.yaw_degrees.to_radians());

        // Build speaker layout (atrium-local positions offset by spawn)
        let speaker_layout = self.build_speakers(spawn);
        let render_mode = parse_render_mode(&self.speakers.render_mode);

        // Decode audio and build sources (also collects metadata for the browser)
        let build = self.build_sources(spawn)?;
        let sound_builder = self.sound_builder(spawn);
        let mut sources = build.sources;
        let source_metas = build.metas;
        let mut spectral_profiles = build.spectral_profiles;
        let mut source_amplitudes = build.source_amplitudes;

        // Pre-warm the source pool to a fixed 16 slots with silent placeholders.
        // Live add/remove only swaps a Box into/out of an existing slot, so the
        // parallel vectors and pipeline topology never grow on the audio thread
        // (which would allocate). Scenes with >16 sources are truncated.
        const MAX_SOURCES: usize = atrium_core::telemetry::MAX_SOURCES;
        if sources.len() > MAX_SOURCES {
            eprintln!(
                "warning: scene has {} sources; truncating to the {MAX_SOURCES}-slot pool",
                sources.len()
            );
            sources.truncate(MAX_SOURCES);
            spectral_profiles.truncate(MAX_SOURCES);
            source_amplitudes.truncate(MAX_SOURCES);
        }
        while sources.len() < MAX_SOURCES {
            sources.push(Box::new(crate::audio::silence_node::SilenceNode));
            spectral_profiles.push([0.0; crate::audio::spectral_profile::BARK_BANDS]);
            source_amplitudes.push(0.0);
        }

        let distance_model = DistanceModel {
            ref_distance: self.distance_model.ref_distance,
            max_distance: self.distance_model.max_distance,
            rolloff: self.distance_model.rolloff,
            model: parse_distance_model(&self.distance_model.model),
        };

        // Load atmosphere from file (or defaults if omitted)
        let atmosphere = match &self.atmosphere {
            Some(path) => {
                let cfg: AtmosphereConfig = load_yaml(path)?;
                AtmosphericParams {
                    temperature_c: cfg.temperature_c,
                    humidity_pct: cfg.humidity_pct,
                    pressure_kpa: cfg.pressure_kpa,
                }
            }
            None => AtmosphericParams::default(),
        };

        // Build comprehensive JSON for the browser (all computed values)
        let scene_json = self.build_scene_json(
            &environment_cfg,
            &atrium_cfg,
            spawn,
            &speaker_layout,
            &source_metas,
            &atmosphere,
        );

        let description = SceneDescription {
            width: environment_cfg.width,
            depth: environment_cfg.depth,
            listener: listener_pos,
            sources: source_metas
                .iter()
                .zip(&sources)
                .take(MAX_SOURCES)
                .enumerate()
                .map(|(slot, (meta, node))| SourceDescription {
                    id: meta.id.clone(),
                    slot: slot as u16,
                    width: meta.width,
                    behind: meta.behind,
                    takes: meta.takes.clone(),
                    gap: meta.gap,
                    name: meta.name.clone(),
                    group: meta.group.clone(),
                    color: meta.color.clone(),
                    position: arr_to_vec3(meta.position) + spawn,
                    spl: meta.spl,
                    reference_distance: meta.ref_dist,
                    emitter_kind: meta.emitter_kind,
                    beam_half_angle: beam_half_angle(meta.emitter_kind, node.directivity()),
                    facing: node.orientation(),
                })
                .collect(),
        };

        let initial_source_states: Vec<InitialSourceState> = source_metas
            .iter()
            .map(|meta| InitialSourceState {
                position: arr_to_vec3(meta.position) + spawn,
                orbit_radius: meta.orbit_radius,
                orbit_speed: meta.orbit_speed,
            })
            .collect();

        // Build composable pipelines
        let ground = GroundProperties::mixed(environment_cfg.ground_factor);

        let wall_materials = build_wall_materials(&environment_cfg);
        let pipeline_params = PipelineParams {
            hrtf_path: self.hrtf,
            distance_model,
            dbap_rolloff_db: self.speakers.dbap_rolloff_db,
            environment_min,
            environment_max,
            reverb_rt60_seconds: environment_cfg.rt60_seconds,
            reverb_pre_delay_ms: environment_cfg.pre_delay_ms,
        };
        let pipelines = build_all_pipelines(&pipeline_params);
        let active_pipeline = render_mode;
        let active_channel_mode = match speaker_layout.total_channels() {
            2 => ChannelMode::Stereo,
            4 => ChannelMode::Quad,
            _ => ChannelMode::Surround51,
        };

        let source_count = sources.len();
        let scene = AudioScene {
            initial_listener_pos: listener_pos,
            initial_listener_yaw: self.listener.yaw_degrees.to_radians(),
            initial_master_gain: self.master_gain,
            initial_source_states,
            initial_atmosphere: atmosphere,
            initial_render_mode: render_mode,
            listener,
            sources,
            environment: Environment {
                min: environment_min,
                max: environment_max,
                wall_materials,
                ground,
                atmosphere,
            },
            master_gain: self.master_gain,
            sample_rate: 0.0, // set by audio backend
            distance_model,
            speaker_layout,
            telemetry_out: None,
            scene_edits: None,
            retired_out: None,
            telemetry_counter: 0,
            telemetry_interval: 6, // ~15 Hz at 512-sample buffers; calibrated later
            #[cfg(feature = "memprof")]
            memprof: crate::engine::memprof::MemProfiler::new(),
            pipelines,
            active_pipeline,
            active_channel_mode,
            startup_channels: None,
            output_channels: None,
            measurement_mode: false,
            perceptual_layer: crate::pipeline::perceptual::PerceptualLayer::new(source_count),
            spectral_profiles,
            source_amplitudes,
            perceptual_states: Vec::new(),
        };

        let source_names: Vec<String> = source_metas.iter().map(|m| m.name.clone()).collect();

        // Build pipeline description for TUI display
        let pipeline_post = scene.mix_stage_names();

        let channel_labels: Vec<String> = match self.speakers.layout.as_str() {
            "5.1" => ["FL", "FR", "C", "LFE", "RL", "RR"].iter(),
            "quad" => ["FL", "FR", "—", "—", "RL", "RR"].iter(),
            _ => ["L", "R"].iter(),
        }
        .map(|s| s.to_string())
        .collect();

        Ok(BuildResult {
            scene,
            sounds: sound_builder,
            description,
            scene_json,
            source_names,
            pipeline_post,
            channel_labels,
        })
    }

    fn build_speakers(&self, spawn: Vec3) -> SpeakerLayout {
        let p = &self.speakers.positions;
        let s = |arr: [f32; 3]| arr_to_vec3(arr) + spawn;
        match self.speakers.layout.as_str() {
            "5.1" => SpeakerLayout::surround_5_1(
                s(p.fl.unwrap_or([-3.0, 2.0, 0.0])),
                s(p.fr.unwrap_or([3.0, 2.0, 0.0])),
                s(p.c.unwrap_or([0.0, 2.0, 0.0])),
                s(p.rl.unwrap_or([-3.0, -2.0, 0.0])),
                s(p.rr.unwrap_or([3.0, -2.0, 0.0])),
            ),
            "quad" => SpeakerLayout::quad(
                s(p.fl.unwrap_or([-3.0, 2.0, 0.0])),
                s(p.fr.unwrap_or([3.0, 2.0, 0.0])),
                s(p.rl.unwrap_or([-3.0, -2.0, 0.0])),
                s(p.rr.unwrap_or([3.0, -2.0, 0.0])),
            ),
            _ => SpeakerLayout::stereo(
                s(p.l.or(p.fl).unwrap_or([-3.0, 2.0, 0.0])),
                s(p.r.or(p.fr).unwrap_or([3.0, 2.0, 0.0])),
            ),
        }
    }

    fn build_sources(&self, spawn: Vec3) -> Result<BuildSourcesResult, Box<dyn std::error::Error>> {
        let builder = self.sound_builder(spawn);
        let mut nodes: Vec<Box<dyn atrium_core::source::SoundSource>> = Vec::new();
        let mut metas: Vec<SourceMeta> = Vec::new();
        let mut spectral_profiles = Vec::<[f32; crate::audio::spectral_profile::BARK_BANDS]>::new();
        let mut source_amplitudes = Vec::<f32>::new();

        for (slot, (id, entity)) in self.sounds.iter().enumerate() {
            let (built, meta) = builder.build_parts(id, entity, slot)?;
            println!(
                "  {} → SPL={:.0} dB, amplitude={:.4}, ref_dist={:.2}m, audible={:.2}m",
                meta.name, meta.spl, built.amplitude, built.ref_dist, built.audible_radius,
            );
            metas.push(meta);
            spectral_profiles.push(built.bands);
            source_amplitudes.push(built.amplitude);
            nodes.push(built.source);
        }

        Ok(BuildSourcesResult {
            sources: nodes,
            metas,
            spectral_profiles,
            source_amplitudes,
        })
    }

    /// The builder for this scene's sounds, with the scene placed at `spawn`.
    fn sound_builder(&self, spawn: Vec3) -> SoundBuilder {
        SoundBuilder::new(
            self.normalization.clone(),
            self.distance_model.ref_distance,
            self.distance_model.max_distance,
            spawn,
        )
    }
}

// ── Single-source builder (shared by scene build + live add) ────────────────

/// A fully-built source plus the derived values the UI and perceptual layer
/// need. Produced on the control thread and, for live adds, shipped into a pool
/// slot via [`crate::engine::edit::SceneEdit`].
pub(super) struct BuiltSource {
    pub source: Box<dyn atrium_core::source::SoundSource>,
    /// 24 Bark-band spectral profile (dB relative to RMS).
    pub bands: [f32; crate::audio::spectral_profile::BARK_BANDS],
    /// Base playback amplitude (sone-based gain, before spatial attenuation).
    pub amplitude: f32,
    /// Per-source reference distance (m) derived from SPL.
    pub ref_dist: f32,
    /// Free-field audible radius (m) at the hearing threshold.
    pub audible_radius: f32,
    /// Directivity polar coefficient (1.0 omni, 0.5 cardioid, …).
    pub directivity_alpha: f32,
}

/// Decode a kind's recordings and build one `SoundSource` with its amplitude,
/// spectral profile, and reference distance. Used both when building a scene
/// and when adding a source live (the same lossless path, so a live-added
/// source matches a reloaded one exactly). `position` is world coordinates
/// (already offset by the environment spawn point).
///
/// Without `rests` the one take loops. With it, the takes are uttered with
/// rests between them, and `seed` decides the choices.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_one_source(
    takes: &[ResolvedTake],
    rests: Option<Rests>,
    seed: u64,
    reference_spl: f32,
    pattern: DirectivityPattern,
    spread: f32,
    position: Vec3,
    orbit_radius: f32,
    orbit_speed: f32,
    normalization: &NormalizationConfig,
    global_ref_dist: f32,
    max_distance: f32,
) -> Result<BuiltSource, Box<dyn std::error::Error>> {
    let buffers = takes
        .iter()
        .map(|take| {
            decode_file(Path::new(&take.file))
                .map(Arc::new)
                .map_err(|error| format!("{}: {}", take.file, error))
        })
        .collect::<Result<Vec<_>, _>>()?;
    // One level for the whole voice: the RMS over all its takes end to end,
    // so a quiet phrase stays quieter than a loud one.
    let (energy, length) = buffers
        .iter()
        .fold((0.0f64, 0usize), |(energy, length), take| {
            let samples = take.samples.len();
            (
                energy + (take.rms as f64).powi(2) * samples as f64,
                length + samples,
            )
        });
    let rms = if length > 0 {
        (energy / length as f64).sqrt() as f32
    } else {
        0.0
    };
    let longest = buffers
        .iter()
        .max_by_key(|take| take.samples.len())
        .ok_or("a sound needs at least one recording")?;

    let profile = resolve_spl(reference_spl);
    let amplitude = profile.amplitude(rms, normalization.target_rms, normalization.spl_reference);
    let ref_dist = profile.ref_distance(global_ref_dist);
    let bands = longest.spectral_profile.bands;
    let audible_radius = profile.audible_radius(normalization.spl_threshold, max_distance);

    // Base amplitude at 0 dB SPL, so a live SPL edit can rescale without
    // re-decoding: amplitude = unit_amplitude · 10^(spl/20).
    let rms_correction = if rms > 1e-6 {
        normalization.target_rms / rms
    } else {
        1.0
    };
    let unit_amplitude = rms_correction * 10.0_f32.powf(-normalization.spl_reference / 20.0);

    let source: Box<dyn atrium_core::source::SoundSource> = match rests {
        None => {
            let mut node = TestNode::new(buffers[0].clone(), position, orbit_radius, orbit_speed);
            node.amplitude = amplitude;
            node.unit_amplitude = unit_amplitude;
            node.ref_dist = ref_dist;
            node.pattern = pattern;
            node.spread = spread;
            node.meter_signal();
            Box::new(node)
        }
        Some(rests) => {
            let voice = buffers
                .iter()
                .zip(takes)
                .map(|(audio, take)| Take {
                    audio: audio.clone(),
                    chance: take.chance,
                })
                .collect();
            let mut node = VoiceNode::new(voice, rests, position, seed);
            node.amplitude = amplitude;
            node.unit_amplitude = unit_amplitude;
            node.ref_dist = ref_dist;
            node.pattern = pattern;
            node.spread = spread;
            node.meter_signal(rms);
            Box::new(node)
        }
    };

    Ok(BuiltSource {
        source,
        bands,
        amplitude,
        ref_dist,
        audible_radius,
        directivity_alpha: pattern.alpha(),
    })
}

/// Seconds of preview audio rendered control-side to measure a synth source's
/// RMS and Bark-band spectral profile — the same calibration measurements
/// `build_one_source` reads from a decoded file. Long enough to average over
/// gust cycles and drop statistics.
const SYNTH_PREVIEW_SECONDS: f32 = 20.0;
/// Sample rate for the preview render. The profile is Bark-band coarse, so a
/// device actually running at 44.1 kHz changes nothing meaningful.
const SYNTH_PREVIEW_SAMPLE_RATE: u32 = 48_000;

/// Build one procedural synth `SoundSource` with its amplitude, spectral
/// profile, and reference distance. Mirrors `build_one_source` exactly, except
/// the calibration measurements come from a preview render instead of a
/// decoded file, so synth and sample sources obey the same loudness model.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_one_synth_source(
    spec: &SynthSpec,
    reference_spl: f32,
    pattern: DirectivityPattern,
    spread: f32,
    position: Vec3,
    default_seed: u64,
    normalization: &NormalizationConfig,
    global_ref_dist: f32,
    max_distance: f32,
) -> Result<BuiltSource, Box<dyn std::error::Error>> {
    // Preview render: measure RMS + spectral profile exactly like a decoded file.
    let sample_rate = SYNTH_PREVIEW_SAMPLE_RATE as f32;
    let mut preview_generator = spec.build_generator(position, default_seed);
    let total_samples = (SYNTH_PREVIEW_SECONDS * sample_rate) as usize;
    let mut samples = vec![0.0_f32; total_samples];
    for sample in samples.iter_mut() {
        *sample = preview_generator.next_sample(sample_rate);
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / total_samples as f32).sqrt();
    if rms < 1e-6 {
        return Err(format!(
            "synth source `{}` rendered silence — check its parameters",
            spec.kind_name()
        )
        .into());
    }
    let bands =
        crate::audio::spectral_profile::compute_profile(&samples, SYNTH_PREVIEW_SAMPLE_RATE).bands;

    let profile = resolve_spl(reference_spl);
    let amplitude = profile.amplitude(rms, normalization.target_rms, normalization.spl_reference);
    let ref_dist = profile.ref_distance(global_ref_dist);
    let audible_radius = profile.audible_radius(normalization.spl_threshold, max_distance);

    // Base amplitude at 0 dB SPL, mirroring build_one_source.
    let rms_correction = normalization.target_rms / rms;
    let unit_amplitude = rms_correction * 10.0_f32.powf(-normalization.spl_reference / 20.0);

    // Fresh generator so the audible source starts from a clean state rather
    // than wherever the preview render left its envelopes.
    let mut node = SynthNode::new(spec.build_generator(position, default_seed), position);
    node.amplitude = amplitude;
    node.unit_amplitude = unit_amplitude;
    node.ref_dist = ref_dist;
    node.pattern = pattern;
    node.spread = spread;
    node.meter_signal(rms);

    Ok(BuiltSource {
        source: Box::new(node),
        bands,
        amplitude,
        ref_dist,
        audible_radius,
        directivity_alpha: pattern.alpha(),
    })
}

fn parse_distance_model(s: &str) -> DistanceModelType {
    match s {
        "linear" => DistanceModelType::Linear,
        "inverse" => DistanceModelType::Inverse,
        "exponential" => DistanceModelType::Exponential,
        _ => DistanceModelType::Inverse,
    }
}

pub(super) fn parse_render_mode(s: &str) -> RenderMode {
    match s {
        "world_locked" => RenderMode::WorldLocked,
        "vbap" => RenderMode::Vbap,
        "hrtf" => RenderMode::Hrtf,
        "dbap" => RenderMode::Dbap,
        "ambisonics" => RenderMode::Ambisonics,
        _ => RenderMode::Vbap,
    }
}

pub(super) fn arr_to_vec3(a: [f32; 3]) -> Vec3 {
    Vec3::new(a[0], a[1], a[2])
}
