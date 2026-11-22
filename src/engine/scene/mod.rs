//! `AudioScene`: everything the audio thread owns, and what it does with it
//! once per buffer.
//!
//! - `commands`: the one `match` over `Command`, drained at the top of a callback
//! - `channels`: channel modes and the fit to the output device
//! - `reset`: back to the state the scene was built with
//! - `edits`: sounds swapped into and out of the fixed pool
//! - `render`: one buffer of audio
//!
//! Nothing in here allocates, frees or takes a lock.

mod channels;
mod commands;
mod edits;
mod render;
mod reset;

use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::distance::DistanceModel;
use crate::audio::spectral_profile::BARK_BANDS;
use crate::engine::edit::{Retired, SceneEdit};
#[cfg(feature = "memprof")]
use crate::engine::memprof::MemProfiler;
use crate::pipeline::environment::Environment;
use crate::pipeline::mix_stage::MixContext;
use crate::pipeline::perceptual::{PerceptualLayer, SourcePerceptualState};
use crate::pipeline::RenderPipeline;
use atrium_core::listener::Listener;
use atrium_core::probe::Probe;
use atrium_core::source::SoundSource;
use atrium_core::speaker::{ChannelMode, RenderMode, SpeakerLayout};
use atrium_core::telemetry::TelemetryFrame;
use atrium_core::types::Vec3;

/// Snapshot of per-source initial state for scene reset.
#[derive(Clone, Copy)]
pub struct InitialSourceState {
    pub position: Vec3,
    pub orbit_radius: f32,
    pub orbit_speed: f32,
}

/// How long pausing and resuming fade (s): long enough not to click.
pub const PAUSE_FADE_SECONDS: f32 = 0.05;

/// The complete audio state owned by the audio thread.
/// Updated by draining commands from the ring buffer.
/// Never shared with the control thread directly.
pub struct AudioScene {
    pub listener: Listener,
    pub sources: Vec<Box<dyn SoundSource>>,
    /// The virtual acoustic space: bounds, surfaces, ground and air.
    pub environment: Environment,
    pub master_gain: f32,
    /// Whether the scene is paused: its clock stands still and it is silent.
    pub paused: bool,
    /// How far the sound is faded in, 0 (paused, silent) to 1, moving toward
    /// `paused` over [`PAUSE_FADE_SECONDS`].
    pub pause_gain: f32,
    pub sample_rate: f32,
    pub distance_model: DistanceModel,
    pub speaker_layout: SpeakerLayout,
    /// Ring buffer producer for sending telemetry to the main thread.
    pub telemetry_out: Option<rtrb::Producer<TelemetryFrame>>,
    /// The session's clock, counted here in output samples and read by the
    /// planner.
    pub clock: std::sync::Arc<crate::plan::Clock>,
    /// Utterances the session's plan hands over, a fraction of a second
    /// ahead of when they are due.
    pub plan_in: Option<rtrb::Consumer<crate::plan::Scheduled>>,
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
    /// All 5 pipelines (WorldLocked, Vbap, Hrtf, Dbap, Ambisonics), pre-allocated.
    pub pipelines: [RenderPipeline; 5],
    /// Which pipeline is active.
    pub active_pipeline: RenderMode,
    /// Current speaker configuration (tracked for telemetry).
    pub active_channel_mode: ChannelMode,
    /// Channel mode and channel mask from before the first `SetChannelMode`,
    /// so `ResetScene` can restore them. `None` while they are unchanged.
    pub startup_channels: Option<(ChannelMode, [bool; atrium_core::speaker::MAX_CHANNELS])>,
    /// How many channels the output device opened with. `None` until the
    /// device is open. A speaker on a channel the device does not have can
    /// never be heard, so no channel mode may switch it on.
    pub output_channels: Option<usize>,
    /// Bypass soft clipping and gain clamping for acoustic measurement.
    pub measurement_mode: bool,
    /// Which parts of the render reach the output: everything, unless
    /// someone is listening to one alone.
    pub probe: Probe,
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
    /// Initialize pipelines with environment geometry and sample rate.
    /// Must be called after sample_rate is set and before the audio callback starts.
    pub fn init_pipelines(&mut self) {
        // One perceptual state per source slot is pushed every callback; make
        // room now so the audio thread never grows the list.
        self.perceptual_states.reserve(self.sources.len());
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
                environment: &self.environment,
                master_gain: self.master_gain,
                render_channels,
                reverb_input: None,
                measurement_mode: self.measurement_mode,
            };
            pipeline.init(&mix_ctx);
            pipeline.ensure_topology(self.sources.len(), &self.speaker_layout, self.sample_rate);
            pipeline.prepare_sources(&self.sources);
        }
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
