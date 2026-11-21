//! Building the engine's `AudioScene` from a loaded `SceneConfig`: loading the
//! referenced files, calibrating each source, and assembling the pipelines.
//!
//! One sound is built in `built_source`; files and strings are read in `parse`.

use atrium_core::listener::Listener;
use atrium_core::speaker::{ChannelMode, SpeakerLayout};
use atrium_core::types::Vec3;

use super::built_source::SourceMeta;
use super::description::{
    beam_half_angle, SceneDescription, SourceDescription, SpeakerDescription,
};
use super::environment::{build_wall_materials, EnvironmentConfig};
use super::parse::{arr_to_vec3, load_yaml, parse_distance_model, parse_render_mode};
use super::scene::{AtriumConfig, SceneConfig};
use super::sound_builder::SoundBuilder;
use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::distance::DistanceModel;
use crate::audio::propagation::GroundProperties;
use crate::engine::scene::{AudioScene, InitialSourceState};
use crate::pipeline::environment::Environment;
use crate::pipeline::{build_all_pipelines, PipelineParams};
use crate::plan::{Clock, Follow, Plan, Planner, Rhythm};

pub struct BuildResult {
    pub scene: AudioScene,
    /// The session's plan: what every voice with takes will do, decided
    /// ahead on its own thread. Keep it as long as the scene plays; dropping
    /// it stops the planning and the voices fall silent.
    pub planner: Planner,
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
    /// Per-source takes and rests, for the plan; `None` for one that needs none.
    rhythms: Vec<Option<Rhythm>>,
    /// Per-source links to the place's conditions.
    follows: Vec<Vec<Follow>>,
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
        let speaker_layout = self.build_speakers(spawn)?;
        let render_mode = parse_render_mode(&self.speakers.render_mode)?;

        // Decode audio and build sources (also collects metadata for the browser)
        let build = self.build_sources(spawn)?;
        let sound_builder = self.sound_builder(spawn);
        let mut sources = build.sources;
        let source_metas = build.metas;
        let mut spectral_profiles = build.spectral_profiles;
        let mut source_amplitudes = build.source_amplitudes;

        // The session: every choice follows from the seed, decided ahead.
        let seed = self.seed.unwrap_or_else(fresh_seed);
        let mut plan = Plan::new(seed);
        plan.set_weather(match &self.weather {
            Some(weather) => weather.story(seed)?,
            None => crate::plan::Weather::calm(seed),
        });
        for (slot, ((id, _), (rhythm, follows))) in self
            .sounds
            .iter()
            .zip(build.rhythms.into_iter().zip(build.follows))
            .enumerate()
        {
            plan.add_voice(slot, id.as_str(), rhythm, follows, 0.0);
        }
        let clock = std::sync::Arc::new(Clock::default());
        let (to_audio, plan_in) = rtrb::RingBuffer::new(PLAN_CAPACITY);
        let planner = Planner::start(plan, clock.clone(), to_audio);

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
            model: parse_distance_model(&self.distance_model.model)?,
        };

        // Load atmosphere from file (or defaults if omitted)
        let air = self.air()?;
        let atmosphere = AtmosphericParams {
            temperature_c: air.temperature_c,
            humidity_pct: air.humidity_pct,
            pressure_kpa: air.pressure_kpa,
        };

        // Build comprehensive JSON for the browser (all computed values)
        let scene_json = self.build_scene_json(
            &environment_cfg,
            &atrium_cfg,
            spawn,
            &speaker_layout,
            render_mode,
            &source_metas,
            &atmosphere,
        );

        let description = SceneDescription {
            width: environment_cfg.width,
            depth: environment_cfg.depth,
            temperature_c: atmosphere.temperature_c,
            humidity_pct: atmosphere.humidity_pct,
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
                    bout: meta.bout,
                    follows: meta.follows.clone(),
                    name: meta.name.clone(),
                    group: meta.group.clone(),
                    color: meta.color.clone(),
                    position: arr_to_vec3(meta.position) + spawn,
                    spl: meta.spl,
                    reference_distance: meta.ref_dist,
                    emitter_kind: meta.emitter_kind,
                    beam_half_angle: beam_half_angle(meta.emitter_kind, node.directivity()),
                    size: meta.spread,
                    facing: node.orientation(),
                })
                .collect(),
            speakers: speaker_layout
                .speakers()
                .iter()
                .map(|speaker| SpeakerDescription {
                    channel: speaker.channel as u16,
                    position: speaker.position,
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
                air: crate::audio::turbulence::AirMotion::STILL,
            },
            master_gain: self.master_gain,
            paused: false,
            pause_gain: 1.0,
            sample_rate: 0.0, // set by audio backend
            distance_model,
            speaker_layout,
            telemetry_out: None,
            clock,
            plan_in: Some(plan_in),
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
            planner,
            sounds: sound_builder,
            description,
            scene_json,
            source_names,
            pipeline_post,
            channel_labels,
        })
    }

    /// The room's loudspeakers as `speakers.layout` names them. A name it does
    /// not know is refused rather than built as a stereo pair.
    fn build_speakers(&self, spawn: Vec3) -> Result<SpeakerLayout, String> {
        let p = &self.speakers.positions;
        let s = |arr: [f32; 3]| arr_to_vec3(arr) + spawn;
        Ok(match self.speakers.layout.as_str() {
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
            "stereo" => SpeakerLayout::stereo(
                s(p.l.or(p.fl).unwrap_or([-3.0, 2.0, 0.0])),
                s(p.r.or(p.fr).unwrap_or([3.0, 2.0, 0.0])),
            ),
            other => {
                return Err(format!(
                    "speakers.layout `{other}` is not one of 5.1, quad, stereo"
                ))
            }
        })
    }

    fn build_sources(&self, spawn: Vec3) -> Result<BuildSourcesResult, Box<dyn std::error::Error>> {
        let builder = self.sound_builder(spawn);
        let mut nodes: Vec<Box<dyn atrium_core::source::SoundSource>> = Vec::new();
        let mut metas: Vec<SourceMeta> = Vec::new();
        let mut spectral_profiles = Vec::<[f32; crate::audio::spectral_profile::BARK_BANDS]>::new();
        let mut source_amplitudes = Vec::<f32>::new();
        let mut rhythms = Vec::new();
        let mut follows = Vec::new();

        for (slot, (id, entity)) in self.sounds.iter().enumerate() {
            let (built, meta) = builder.build_parts(id, entity, slot, |_| Ok(()))?;
            println!(
                "  {} → SPL={:.0} dB, amplitude={:.4}, ref_dist={:.2}m, audible={:.2}m",
                meta.name, meta.spl, built.amplitude, built.ref_dist, built.audible_radius,
            );
            metas.push(meta);
            spectral_profiles.push(built.bands);
            source_amplitudes.push(built.amplitude);
            rhythms.push(built.rhythm);
            follows.push(built.follows);
            nodes.push(built.source);
        }

        Ok(BuildSourcesResult {
            sources: nodes,
            metas,
            spectral_profiles,
            source_amplitudes,
            rhythms,
            follows,
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

/// How many utterances can wait between the planner and the audio thread:
/// the planner hands over less than half a second at a time.
const PLAN_CAPACITY: usize = 256;

/// A seed for a session nobody asked to repeat: the time, to the nanosecond.
fn fresh_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or(1)
}
