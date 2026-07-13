use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::distance::DistanceModel;
use crate::audio::propagation::GroundProperties;
use crate::audio::spectral_profile::BARK_BANDS;
use crate::engine::edit::{Retired, SceneEdit};
#[cfg(feature = "memprof")]
use crate::engine::memprof::{MemProfiler, MemStage};
use crate::engine::telemetry::{compute_telemetry, TelemetryFrame};
use crate::pipeline::mix_stage::MixContext;
use crate::pipeline::perceptual::{PerceptualLayer, SourcePerceptualState};
use crate::pipeline::{render_pipeline, RenderParams, RenderPipeline};
use crate::profile_span;
use crate::world::room::Room;
use crate::world::types::Vec3;
use atrium_core::commands::Command;
use atrium_core::directivity::directivity_gain;
use atrium_core::listener::Listener;
use atrium_core::panner::distance_gain_at_model;
use atrium_core::source::{EmitterKind, SoundSource};
use atrium_core::speaker::{ChannelMode, RenderMode, SpeakerLayout};

/// Snapshot of per-source initial state for scene reset.
#[derive(Clone, Copy)]
pub struct InitialSourceState {
    pub position: Vec3,
    pub orbit_radius: f32,
    pub orbit_speed: f32,
}

/// The complete audio state owned by the audio thread.
/// Updated by draining commands from the ring buffer.
/// Never shared with the control thread directly.
pub struct AudioScene {
    pub listener: Listener,
    pub sources: Vec<Box<dyn SoundSource>>,
    pub environment: Box<dyn Room>,
    pub master_gain: f32,
    pub sample_rate: f32,
    pub distance_model: DistanceModel,
    pub speaker_layout: SpeakerLayout,
    pub atmosphere: AtmosphericParams,
    /// Ring buffer producer for sending telemetry to the main thread.
    pub telemetry_out: Option<rtrb::Producer<TelemetryFrame>>,
    /// Consumer for live source-pool edits (add/remove) from the control thread.
    pub scene_edits: Option<rtrb::Consumer<SceneEdit>>,
    /// Producer that ships displaced source boxes back to the control thread to
    /// be dropped there (deallocation is not real-time safe).
    pub retired_out: Option<rtrb::Producer<Retired>>,
    /// Callback counter for throttling telemetry (~15 Hz).
    pub telemetry_counter: u32,
    /// Push telemetry every N callbacks.
    pub telemetry_interval: u32,
    /// Audio-thread allocation profiler (bytes/allocs per stage).
    #[cfg(feature = "memprof")]
    pub memprof: MemProfiler,
    // ── Initial state for scene reset ──
    pub initial_listener_pos: Vec3,
    pub initial_listener_yaw: f32,
    pub initial_master_gain: f32,
    pub initial_source_states: Vec<InitialSourceState>,
    pub initial_atmosphere: AtmosphericParams,
    pub initial_render_mode: RenderMode,
    // ── Composable pipeline ──
    /// All 4 pipelines (WorldLocked, Vbap, Hrtf, Dbap), pre-allocated.
    pub pipelines: [RenderPipeline; 5],
    /// Which pipeline is active.
    pub active_pipeline: RenderMode,
    /// Current speaker configuration (tracked for telemetry).
    pub active_channel_mode: ChannelMode,
    /// Ground properties for pipeline propagation stages.
    pub ground: GroundProperties,
    /// Barriers for occlusion/transmission in propagation.
    pub barriers: Vec<crate::audio::propagation::Barrier>,
    /// Wall materials for the 6 environment faces (order: -X, +X, -Y, +Y, -Z, +Z).
    pub wall_materials: [crate::pipeline::path::WallMaterial; 6],
    /// Bypass soft clipping and gain clamping for acoustic measurement.
    pub measurement_mode: bool,
    // ── Perceptual masking layer ──
    /// Per-source spectral profiles (24 Bark bands, dB relative to RMS).
    pub spectral_profiles: Vec<[f32; BARK_BANDS]>,
    /// Per-source base amplitudes (sone-based gain, before spatial attenuation).
    pub source_amplitudes: Vec<f32>,
    /// Perceptual scoring layer (masking + salience analysis).
    pub perceptual_layer: PerceptualLayer,
    /// Reusable buffer for per-source perceptual states (avoids per-frame allocation).
    pub perceptual_states: Vec<SourcePerceptualState>,
}

impl AudioScene {
    /// Drain all pending commands from the consumer.
    /// Called once at the start of each audio callback invocation.
    pub fn process_commands(&mut self, consumer: &mut rtrb::Consumer<Command>) {
        let _s = profile_span!("process_commands").entered();
        while let Ok(cmd) = consumer.pop() {
            match cmd {
                Command::SetListenerPose { position, yaw } => {
                    self.listener.position = position;
                    self.listener.yaw = yaw;
                }
                Command::SetMasterGain { gain } => {
                    self.master_gain = gain;
                }
                Command::SetSourceMuted { index, muted } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_muted(muted);
                    }
                }
                Command::SetSourcePosition { index, position } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_position(position);
                    }
                }
                Command::SetRenderMode { mode } => {
                    let new_pipeline = mode;
                    if new_pipeline != self.active_pipeline {
                        self.pipelines[new_pipeline.index()].reset();
                        self.active_pipeline = new_pipeline;
                    }
                }
                Command::SetSpeakerPosition { channel, position } => {
                    if let Some(speaker) =
                        self.speaker_layout.speaker_by_channel_mut(channel as usize)
                    {
                        speaker.position = position;
                    }
                }
                Command::SetSourceSpread { index, spread } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_spread(spread);
                    }
                }
                Command::SetSourceSpl { index, spl } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        let amplitude = source.set_reference_spl(spl);
                        // Keep the perceptual layer's base amplitude in sync.
                        if let Some(slot) = self.source_amplitudes.get_mut(index as usize) {
                            *slot = amplitude;
                        }
                    }
                }
                Command::SetSourceDirectivity { index, pattern } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_directivity(pattern);
                    }
                }
                Command::SetSourceOrbitSpeed { index, speed } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_orbit_speed(speed);
                    }
                }
                Command::SetSourceOrbitRadius { index, radius } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_orbit_radius(radius);
                    }
                }
                Command::SetSourceOrbitAngle { index, angle } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_orbit_angle(angle);
                    }
                }
                Command::SetChannelMode { mode } => {
                    self.active_channel_mode = mode;
                    self.speaker_layout
                        .set_active_channels(mode.active_channels());
                }
                Command::SetAtmosphere {
                    temperature_c,
                    humidity_pct,
                } => {
                    self.atmosphere.temperature_c = temperature_c;
                    self.atmosphere.humidity_pct = humidity_pct;
                }
                Command::SetSynthParam {
                    index,
                    param,
                    value,
                } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_synth_param(param, value);
                    }
                }
                Command::ResetScene => {
                    self.listener.position = self.initial_listener_pos;
                    self.listener.yaw = self.initial_listener_yaw;
                    self.master_gain = self.initial_master_gain;
                    self.atmosphere = self.initial_atmosphere;
                    self.active_pipeline = self.initial_render_mode;
                    // Reset speaker config to full layout
                    let initial_channel_mode = match self.speaker_layout.total_channels() {
                        2 => ChannelMode::Stereo,
                        4 => ChannelMode::Quad,
                        _ => ChannelMode::Surround51,
                    };
                    self.active_channel_mode = initial_channel_mode;
                    self.speaker_layout
                        .set_active_channels(initial_channel_mode.active_channels());
                    for p in self.pipelines.iter_mut() {
                        p.reset();
                    }
                    for (source, init) in self.sources.iter_mut().zip(&self.initial_source_states) {
                        source.set_position(init.position);
                        source.set_orbit_radius(init.orbit_radius);
                        source.set_orbit_speed(init.orbit_speed);
                        source.set_orbit_angle(0.0);
                        source.set_muted(false);
                    }
                }
            }
        }
    }

    /// Drain live source-pool edits (add/remove) from the control thread.
    /// Called at the top of each audio callback. Swaps `Box`es in and out of the
    /// fixed slot pool — never allocates or deallocates on the audio thread: the
    /// incoming box is pre-built on the control thread, and the displaced box is
    /// shipped to the retire channel to be dropped there.
    pub fn process_scene_edits(&mut self) {
        // Take the consumer out so the slot accesses below don't alias its
        // borrow of `self`; restore it afterwards.
        let Some(mut consumer) = self.scene_edits.take() else {
            return;
        };
        while let Ok(edit) = consumer.pop() {
            match edit {
                SceneEdit::AddSource {
                    slot,
                    source,
                    bands,
                    amplitude,
                } => {
                    let slot = slot as usize;
                    if slot < self.sources.len() {
                        let displaced = std::mem::replace(&mut self.sources[slot], source);
                        if let Some(profile) = self.spectral_profiles.get_mut(slot) {
                            *profile = bands;
                        }
                        if let Some(amp) = self.source_amplitudes.get_mut(slot) {
                            *amp = amplitude;
                        }
                        self.retire(displaced);
                    } else {
                        self.retire(source);
                    }
                }
                SceneEdit::RemoveSource { slot, filler } => {
                    let slot = slot as usize;
                    if slot < self.sources.len() {
                        let displaced = std::mem::replace(&mut self.sources[slot], filler);
                        if let Some(amp) = self.source_amplitudes.get_mut(slot) {
                            *amp = 0.0;
                        }
                        self.retire(displaced);
                    } else {
                        self.retire(filler);
                    }
                }
            }
        }
        self.scene_edits = Some(consumer);
    }

    /// Ship a displaced source box back to the control thread to be dropped
    /// there. If the retire channel is full or absent, the box is dropped here
    /// as a last resort (rare; the control thread drains every frame).
    fn retire(&mut self, source: Box<dyn SoundSource>) {
        if let Some(out) = self.retired_out.as_mut() {
            let _ = out.push(Retired(source));
        }
    }

    /// Initialize pipelines with environment geometry and sample rate.
    /// Must be called after sample_rate is set and before the audio callback starts.
    pub fn init_pipelines(&mut self) {
        let (environment_min, environment_max) = self.environment.bounds();
        let total_channels = self.speaker_layout.total_channels();
        for pipeline in self.pipelines.iter_mut() {
            let render_channels = if pipeline.render_channels > 0 {
                pipeline.render_channels
            } else {
                total_channels
            };
            let mix_ctx = MixContext {
                listener: &self.listener,
                layout: &self.speaker_layout,
                sample_rate: self.sample_rate,
                channels: total_channels,
                environment_min,
                environment_max,
                master_gain: self.master_gain,
                render_channels,
                reverb_input: None,
                wall_reflectivity: pipeline.wall_reflectivity,
                wall_materials: &self.wall_materials,
                atmosphere: &self.atmosphere,
                measurement_mode: self.measurement_mode,
            };
            pipeline.init(&mix_ctx);
            pipeline.ensure_topology(self.sources.len(), &self.speaker_layout, self.sample_rate);
        }
    }

    /// Render one buffer of audio.
    /// `output` is an interleaved sample buffer (e.g. [L, R, L, R, ...] for stereo).
    pub fn render(&mut self, output: &mut [f32], channels: usize) {
        let _total =
            profile_span!("render", sources = self.sources.len(), channels = channels).entered();

        #[cfg(feature = "memprof")]
        self.memprof.begin_callback();

        // Apply any live source-pool edits before rendering this buffer.
        self.process_scene_edits();

        let num_frames = output.len() / channels;
        let dt = num_frames as f32 / self.sample_rate;

        // Advance time-varying state on all sources
        {
            let _s = profile_span!("source_tick").entered();
            for source in &mut self.sources {
                source.tick(dt);
            }
        }
        #[cfg(feature = "memprof")]
        self.memprof.record_stage(MemStage::SourceTick);

        // Perceptual masking analysis (feed-forward, before rendering).
        {
            let _s = profile_span!("perceptual").entered();
            self.perceptual_states.clear();
            for (i, source) in self.sources.iter().enumerate() {
                let pos = source.position();
                let active = source.is_active() && !source.is_muted();
                let amp = if active && i < self.source_amplitudes.len() {
                    if source.emitter_kind() == EmitterKind::Field {
                        self.source_amplitudes[i]
                    } else {
                        let dist_gain = distance_gain_at_model(
                            self.listener.position,
                            pos,
                            source.ref_distance(),
                            self.distance_model.max_distance,
                            self.distance_model.rolloff,
                            self.distance_model.model,
                        );
                        let emit_gain = directivity_gain(
                            pos,
                            source.orientation(),
                            self.listener.position,
                            &source.directivity(),
                        );
                        let hear_gain = self.listener.hearing_gain(pos);
                        self.source_amplitudes[i] * dist_gain * emit_gain * hear_gain
                    }
                } else {
                    0.0
                };
                let bands = if i < self.spectral_profiles.len() {
                    self.spectral_profiles[i]
                } else {
                    [0.0; BARK_BANDS]
                };
                self.perceptual_states.push(SourcePerceptualState {
                    received_amplitude: amp,
                    spectral_bands: bands,
                    active,
                });
            }
            self.perceptual_layer.update(&self.perceptual_states);
        }

        // Render through the composable pipeline
        {
            let (environment_min, environment_max) = self.environment.bounds();
            let pipeline = &mut self.pipelines[self.active_pipeline.index()];
            let _s = profile_span!("pipeline", mode = ?self.active_pipeline).entered();
            let params = RenderParams {
                listener: &self.listener,
                channels,
                sample_rate: self.sample_rate,
                master_gain: self.master_gain,
                distance_model: &self.distance_model,
                layout: &self.speaker_layout,
                atmosphere: &self.atmosphere,
                ground: &self.ground,
                environment_min,
                environment_max,
                barriers: &self.barriers,
                wall_materials: &self.wall_materials,
                measurement_mode: self.measurement_mode,
            };
            render_pipeline(pipeline, &mut self.sources, &params, output);
        }
        #[cfg(feature = "memprof")]
        self.memprof.record_stage(MemStage::Mix);

        // Push telemetry at ~15 Hz (every N callbacks)
        {
            let _s = profile_span!("telemetry").entered();
            self.telemetry_counter += 1;
            if self.telemetry_counter >= self.telemetry_interval {
                self.telemetry_counter = 0;
                if let Some(ref mut producer) = self.telemetry_out {
                    let mut frame =
                        compute_telemetry(&self.sources, &self.listener, &self.distance_model);
                    frame.render_mode = self.active_pipeline;
                    frame.channel_mode = self.active_channel_mode;
                    frame.temperature_c = self.atmosphere.temperature_c;
                    frame.humidity_pct = self.atmosphere.humidity_pct;
                    frame.channel_peaks =
                        crate::engine::telemetry::compute_channel_peaks(output, channels);
                    frame.channel_count = channels as u8;
                    // Stamp perceptual scores from the latest analysis.
                    let scores = self.perceptual_layer.scores();
                    for i in 0..frame.source_count as usize {
                        if i < scores.len() {
                            frame.sources[i].perceptual_score = scores[i];
                        }
                    }
                    let _ = producer.push(frame); // silent drop if full
                }
            }
        }
        #[cfg(feature = "memprof")]
        self.memprof.record_stage(MemStage::Telemetry);

        #[cfg(feature = "memprof")]
        self.memprof.finish_callback();
    }

    /// Set the telemetry interval based on actual audio parameters.
    /// Call after sample_rate is known (i.e. after CpalOutput resolves config).
    pub fn calibrate_telemetry(&mut self, buffer_size: u32) {
        // Target ~15 Hz. callbacks_per_sec = sample_rate / buffer_size
        let callbacks_per_sec = self.sample_rate / buffer_size.max(1) as f32;
        self.telemetry_interval = (callbacks_per_sec / 15.0).round().max(1.0) as u32;
    }

    /// Collect mix stage names from the active pipeline (for TUI display).
    pub fn mix_stage_names(&self) -> Vec<String> {
        self.pipelines[self.active_pipeline.index()]
            .mix_stages
            .iter()
            .map(|s| s.name().to_string())
            .collect()
    }
}
