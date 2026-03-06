use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::binaural::BinauralMixer;
use crate::audio::mixer::{mix_sources, DistanceModel, MixerState};
use crate::engine::commands::Command;
use crate::engine::telemetry::{compute_telemetry, TelemetryFrame};
use crate::processors::AudioProcessor;
use crate::spatial::listener::Listener;
use crate::spatial::source::SoundSource;
use crate::world::room::Room;
use crate::world::types::Vec3;
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
    pub processors: Vec<Box<dyn AudioProcessor>>,
    pub speaker_layout: SpeakerLayout,
    pub mixer_state: MixerState,
    pub atmosphere: AtmosphericParams,
    pub binaural_mixer: Option<BinauralMixer>,
    /// Ring buffer producer for sending telemetry to the main thread.
    pub telemetry_out: Option<rtrb::Producer<TelemetryFrame>>,
    /// Callback counter for throttling telemetry (~15 Hz).
    pub telemetry_counter: u32,
    /// Push telemetry every N callbacks.
    pub telemetry_interval: u32,
    // ── Initial state for scene reset ──
    pub initial_listener_pos: Vec3,
    pub initial_listener_yaw: f32,
    pub initial_master_gain: f32,
    pub initial_source_states: Vec<InitialSourceState>,
    pub initial_atmosphere: AtmosphericParams,
    pub initial_render_mode: RenderMode,
}

impl AudioScene {
    /// Drain all pending commands from the consumer.
    /// Called once at the start of each audio callback invocation.
    pub fn process_commands(&mut self, consumer: &mut rtrb::Consumer<Command>) {
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
                    self.speaker_layout.mode = mode;
                }
                Command::SetSpeakerPosition { channel, position } => {
                    if let Some(speaker) = self.speaker_layout.speaker_by_channel_mut(channel as usize) {
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
                Command::SetAtmosphere { temperature_c, humidity_pct } => {
                    self.atmosphere.temperature_c = temperature_c;
                    self.atmosphere.humidity_pct = humidity_pct;
                }
                Command::ResetScene => {
                    self.listener.position = self.initial_listener_pos;
                    self.listener.yaw = self.initial_listener_yaw;
                    self.master_gain = self.initial_master_gain;
                    self.atmosphere = self.initial_atmosphere;
                    self.speaker_layout.mode = self.initial_render_mode;
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

    /// Initialize all processors with room geometry and sample rate.
    /// Must be called after sample_rate is set and before the audio callback starts.
    pub fn init_processors(&mut self) {
        let (room_min, room_max) = self.room.bounds();
        for processor in &mut self.processors {
            processor.init(room_min, room_max, &self.listener, self.sample_rate);
        }

        // Initialize binaural mixer (load SOFA file, create per-source convolvers)
        match BinauralMixer::new(
            "assets/hrtf/default.sofa",
            self.sample_rate,
            self.sources.len(),
        ) {
            Ok(mixer) => self.binaural_mixer = Some(mixer),
            Err(e) => eprintln!("Binaural HRTF not available: {e}"),
        }
    }

    /// Render one buffer of audio.
    /// `output` is an interleaved sample buffer (e.g. [L, R, L, R, ...] for stereo).
    pub fn render(&mut self, output: &mut [f32], channels: usize) {
        let num_frames = output.len() / channels;
        let dt = num_frames as f32 / self.sample_rate;

        // Advance time-varying state on all sources
        for source in &mut self.sources {
            source.tick(dt);
        }

        // Binaural mode: HRTF convolution to stereo headphone output
        if self.speaker_layout.mode == RenderMode::Binaural {
            if let Some(ref mut mixer) = self.binaural_mixer {
                mixer.mix(
                    &mut self.sources,
                    &self.listener,
                    output,
                    channels,
                    self.sample_rate,
                    self.master_gain,
                    &self.distance_model,
                    &self.atmosphere,
                );
                return;
            }
            // Fall through to multichannel if binaural mixer not available
        }

        // Multichannel speaker rendering (VBAP, SpeakerAsMic, etc.)
        mix_sources(
            &mut self.sources,
            &self.listener,
            output,
            channels,
            self.sample_rate,
            self.master_gain,
            &self.distance_model,
            &self.speaker_layout,
            &mut self.mixer_state,
            &self.atmosphere,
        );

        // Run processor chain (early reflections, reverb, etc.)
        for processor in &mut self.processors {
            processor.process(output, channels, self.sample_rate);
        }

        // Push telemetry at ~15 Hz (every N callbacks)
        self.telemetry_counter += 1;
        if self.telemetry_counter >= self.telemetry_interval {
            self.telemetry_counter = 0;
            if let Some(ref mut producer) = self.telemetry_out {
                let frame =
                    compute_telemetry(&self.sources, &self.listener, &self.distance_model);
                let _ = producer.push(frame); // silent drop if full
            }
        }
    }

    /// Set the telemetry interval based on actual audio parameters.
    /// Call after sample_rate is known (i.e. after CpalOutput resolves config).
    pub fn calibrate_telemetry(&mut self, buffer_size: u32) {
        // Target ~15 Hz. callbacks_per_sec = sample_rate / buffer_size
        let callbacks_per_sec = self.sample_rate / buffer_size.max(1) as f32;
        self.telemetry_interval = (callbacks_per_sec / 15.0).round().max(1.0) as u32;
    }
}
