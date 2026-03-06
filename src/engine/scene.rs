use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::distance::DistanceModel;
use crate::audio::propagation::GroundProperties;
use crate::engine::commands::Command;
#[cfg(feature = "memprof")]
use crate::engine::memprof::{MemProfiler, MemStage};
use crate::engine::telemetry::{compute_telemetry, TelemetryFrame};
use crate::pipeline::mix_stage::MixContext;
use crate::pipeline::{render_pipeline, RenderParams, RenderPipeline};
use crate::profile_span;
use crate::world::room::Room;
use crate::world::types::Vec3;
use atrium_core::listener::Listener;
use atrium_core::source::SoundSource;
use atrium_core::speaker::{RenderMode, SpeakerLayout};

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
    pub room: Box<dyn Room>,
    pub master_gain: f32,
    pub sample_rate: f32,
    pub distance_model: DistanceModel,
    pub speaker_layout: SpeakerLayout,
    pub atmosphere: AtmosphericParams,
    /// Ring buffer producer for sending telemetry to the main thread.
    pub telemetry_out: Option<rtrb::Producer<TelemetryFrame>>,
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
    pub pipelines: [RenderPipeline; 4],
    /// Which pipeline is active.
    pub active_pipeline: RenderMode,
    /// Ground properties for pipeline propagation stages.
    pub ground: GroundProperties,
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
                Command::ResetScene => {
                    self.listener.position = self.initial_listener_pos;
                    self.listener.yaw = self.initial_listener_yaw;
                    self.master_gain = self.initial_master_gain;
                    self.atmosphere = self.initial_atmosphere;
                    self.active_pipeline = self.initial_render_mode;
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

    /// Initialize pipelines with room geometry and sample rate.
    /// Must be called after sample_rate is set and before the audio callback starts.
    pub fn init_pipelines(&mut self) {
        let (room_min, room_max) = self.room.bounds();
        let mix_ctx = MixContext {
            listener: &self.listener,
            layout: &self.speaker_layout,
            sample_rate: self.sample_rate,
            channels: self.speaker_layout.total_channels(),
            room_min,
            room_max,
            master_gain: self.master_gain,
        };
        for pipeline in self.pipelines.iter_mut() {
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

        // Render through the composable pipeline
        {
            let (room_min, room_max) = self.room.bounds();
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
                room_min,
                room_max,
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
