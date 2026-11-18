//! Shared support for the pipeline tests: scripted sources, the end-to-end
//! `Rig`, layout helpers and measurement helpers. The tests themselves live in
//! the sibling files, grouped by subject.

use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::distance::DistanceModel;
use crate::audio::propagation::GroundProperties;
use crate::pipeline::builders;
use crate::pipeline::environment::Environment;
use crate::pipeline::mix_stage::MixContext;
use crate::pipeline::wall_material;
use crate::pipeline::{render_pipeline, PipelineParams, RenderParams, RenderPipeline};
use atrium_core::listener::Listener;
use atrium_core::source::SoundSource;
use atrium_core::speaker::{RenderMode, SpeakerLayout};
use atrium_core::types::Vec3;

mod channel_routing;
mod dbap;
mod field_emitters;
mod levels_and_timing;
mod mode_layout_matrix;
mod mode_switching;
mod reflections;
mod reverb;
mod world_locked;

fn default_wall_materials() -> [wall_material::WallMaterial; 6] {
    std::array::from_fn(|_| wall_material::WallMaterial::default())
}

/// Constant-value test source.
struct ConstSource {
    pos: Vec3,
    val: f32,
}

impl SoundSource for ConstSource {
    fn next_sample(&mut self, _sr: f32) -> f32 {
        self.val
    }
    fn position(&self) -> Vec3 {
        self.pos
    }
    fn tick(&mut self, _dt: f32) {}
    fn is_active(&self) -> bool {
        true
    }
}

/// Deterministic stochastic texture used to verify that Field rendering is
/// invariant to object-space geometry and room acoustics.
struct FieldTextureSource {
    pos: Vec3,
    rng: crate::synth::noise::Rng,
}

impl SoundSource for FieldTextureSource {
    fn next_sample(&mut self, _sr: f32) -> f32 {
        self.rng.next_bipolar() * 0.25
    }
    fn position(&self) -> Vec3 {
        self.pos
    }
    fn emitter_kind(&self) -> atrium_core::source::EmitterKind {
        atrium_core::source::EmitterKind::Field
    }
    fn tick(&mut self, _dt: f32) {}
}

fn default_atmosphere() -> AtmosphericParams {
    AtmosphericParams {
        temperature_c: 20.0,
        humidity_pct: 50.0,
        pressure_kpa: 101.325,
    }
}

fn default_ground() -> GroundProperties {
    GroundProperties::default()
}

/// The environment a test renders in: the given bounds and wall materials,
/// with the default air and default ground.
fn test_environment(
    min: Vec3,
    max: Vec3,
    wall_materials: [wall_material::WallMaterial; 6],
) -> Environment {
    Environment {
        min,
        max,
        wall_materials,
        ground: default_ground(),
        atmosphere: default_atmosphere(),
    }
}

fn default_distance_model() -> DistanceModel {
    DistanceModel::default()
}

fn layout_5_1() -> SpeakerLayout {
    SpeakerLayout::surround_5_1(
        Vec3::new(0.0, 4.0, 0.0), // FL
        Vec3::new(6.0, 4.0, 0.0), // FR
        Vec3::new(3.0, 4.0, 0.0), // C
        Vec3::new(0.0, 0.0, 0.0), // RL
        Vec3::new(6.0, 0.0, 0.0), // RR
    )
}

/// A 1 kHz source that plays for `remaining` samples then goes silent, so we
/// can measure what rings out after it stops. 1 kHz survives the reverb-send
/// HPF (a DC burst would not).
struct BurstSource {
    pos: Vec3,
    n: usize,
    remaining: usize,
}

impl SoundSource for BurstSource {
    fn next_sample(&mut self, sr: f32) -> f32 {
        if self.remaining == 0 {
            return 0.0;
        }
        self.remaining -= 1;
        let s = (std::f32::consts::TAU * 1000.0 * self.n as f32 / sr).sin() * 0.5;
        self.n += 1;
        s
    }
    fn position(&self) -> Vec3 {
        self.pos
    }
    fn tick(&mut self, _dt: f32) {}
    fn is_active(&self) -> bool {
        true
    }
}

// ═════════════════════════════════════════════════════════════════════
// End-to-end cross-stage physics
//
// Most tests above never call `RenderPipeline::init`, which leaves the
// FDN, LFE bass management and delay compensation as silent no-ops. Every
// test below initialises the mix stages exactly like
// `AudioScene::init_pipelines` (`Rig::init`) and then drives
// `render_pipeline` block by block like the cpal callback (`Rig::render`).
//
// Hand-derived constants used throughout (20 °C, 50 % RH, 101.325 kPa):
//   c = 331.3 + 0.606 · 20 = 343.42 m/s
//   10 m of air = 29.119 ms = 1284.15 / 1397.67 / 2795.35 samples
//   at 44.1 / 48 / 96 kHz.
// ═════════════════════════════════════════════════════════════════════

use atrium_core::panner::DistanceModelType;

const BLOCK_FRAMES: usize = 512;

const SAMPLE_RATES: [f32; 3] = [44_100.0, 48_000.0, 96_000.0];

/// One sample of 1.0 at sample index `emit_at`, silence otherwise.
struct ImpulseSource {
    position: Vec3,
    emit_at: usize,
    sample_index: usize,
}

impl ImpulseSource {
    fn boxed(position: Vec3, emit_at: usize) -> Box<dyn SoundSource> {
        Box::new(Self {
            position,
            emit_at,
            sample_index: 0,
        })
    }
}

impl SoundSource for ImpulseSource {
    fn next_sample(&mut self, _sample_rate: f32) -> f32 {
        let sample = if self.sample_index == self.emit_at {
            1.0
        } else {
            0.0
        };
        self.sample_index += 1;
        sample
    }
    fn position(&self) -> Vec3 {
        self.position
    }
    fn tick(&mut self, _dt: f32) {}
}

/// A 0.5-amplitude sine that plays for `duration_samples`, then stops.
struct ToneSource {
    position: Vec3,
    frequency: f32,
    sample_index: usize,
    duration_samples: usize,
}

impl ToneSource {
    fn boxed(position: Vec3, frequency: f32, duration_samples: usize) -> Box<dyn SoundSource> {
        Box::new(Self {
            position,
            frequency,
            sample_index: 0,
            duration_samples,
        })
    }
}

impl SoundSource for ToneSource {
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        if self.sample_index >= self.duration_samples {
            return 0.0;
        }
        let phase = std::f64::consts::TAU * self.frequency as f64 * self.sample_index as f64
            / sample_rate as f64;
        self.sample_index += 1;
        0.5 * phase.sin() as f32
    }
    fn position(&self) -> Vec3 {
        self.position
    }
    fn tick(&mut self, _dt: f32) {}
}

fn build_mode(mode: RenderMode, params: &PipelineParams) -> RenderPipeline {
    match mode {
        RenderMode::WorldLocked => builders::world_locked::build(params),
        RenderMode::Vbap => builders::vbap::build(params),
        RenderMode::Hrtf => builders::hrtf::build(params),
        RenderMode::Dbap => builders::dbap::build(params),
        RenderMode::Ambisonics => builders::ambisonics::build(params),
    }
}

fn uniform_walls(material: wall_material::WallMaterial) -> [wall_material::WallMaterial; 6] {
    std::array::from_fn(|_| material.clone())
}

/// Shipped quad layout (masked 5.1: FL 0, FR 1, RL 4, RR 5; LFE 3 masked)
/// with every speaker 2 m from `centre`, so listener-relative delay
/// compensation at `centre` is exactly zero.
fn quad_around(centre: Vec3) -> SpeakerLayout {
    let offset = std::f32::consts::SQRT_2;
    SpeakerLayout::quad(
        centre + Vec3::new(-offset, offset, 0.0),
        centre + Vec3::new(offset, offset, 0.0),
        centre + Vec3::new(-offset, -offset, 0.0),
        centre + Vec3::new(offset, -offset, 0.0),
    )
}

/// The default scene's 5.1 ring (6 × 4 m) centred on the origin.
fn living_room_5_1() -> SpeakerLayout {
    SpeakerLayout::surround_5_1(
        Vec3::new(-3.0, 2.0, 0.0),
        Vec3::new(3.0, 2.0, 0.0),
        Vec3::new(0.0, 2.0, 0.0),
        Vec3::new(-3.0, -2.0, 0.0),
        Vec3::new(3.0, -2.0, 0.0),
    )
}

/// Everything one end-to-end render needs, owned so a test varies one field.
struct Rig {
    layout: SpeakerLayout,
    listener: Listener,
    environment: Environment,
    wall_reflectivity: f32,
    reverb_rt60_seconds: Option<f32>,
    distance_model: DistanceModel,
    /// Device rate the stream runs at.
    sample_rate: f32,
    channels: usize,
    master_gain: f32,
    measurement_mode: bool,
}

impl Rig {
    /// Anechoic free field: listener at the origin facing +y, every wall
    /// `open` (reflection gain √(1−1) = 0, FDN gated off), hard ground,
    /// inverse law with ref 1 m and a 1 km clamp, quad speakers 2 m out.
    /// The environment is centred on the listener, so the static delay
    /// compensation (referenced to the environment centre) is zero too.
    fn free_field(sample_rate: f32) -> Self {
        let layout = quad_around(Vec3::ZERO);
        let channels = layout.total_channels();
        Self {
            layout,
            listener: Listener::new(Vec3::ZERO, std::f32::consts::FRAC_PI_2),
            environment: Environment {
                min: Vec3::new(-30.0, -30.0, -30.0),
                max: Vec3::new(30.0, 30.0, 30.0),
                wall_materials: uniform_walls(wall_material::WallMaterial::open()),
                ground: GroundProperties::hard(),
                atmosphere: default_atmosphere(),
            },
            wall_reflectivity: 0.0,
            reverb_rt60_seconds: None,
            distance_model: DistanceModel {
                ref_distance: 1.0,
                max_distance: 1000.0,
                rolloff: 1.0,
                model: DistanceModelType::Inverse,
            },
            sample_rate,
            channels,
            master_gain: 1.0,
            measurement_mode: true,
        }
    }

    fn pipeline_params(&self) -> PipelineParams {
        PipelineParams {
            er_wall_reflectivity: self.wall_reflectivity,
            distance_model: self.distance_model,
            environment_min: self.environment.min,
            environment_max: self.environment.max,
            reverb_rt60_seconds: self.reverb_rt60_seconds,
            ..PipelineParams::default()
        }
    }

    /// Mirror of `AudioScene::init_pipelines` for one pipeline: the mix
    /// context uses the layout's total channel count, not the device's.
    fn init(&self, pipeline: &mut RenderPipeline) {
        let total_channels = self.layout.total_channels();
        let render_channels = if pipeline.render_channels > 0 {
            pipeline.render_channels
        } else {
            total_channels
        };
        let mix_context = MixContext {
            listener: &self.listener,
            layout: &self.layout,
            sample_rate: self.sample_rate,
            channels: total_channels,
            master_gain: self.master_gain,
            render_channels,
            reverb_input: None,
            wall_reflectivity: pipeline.wall_reflectivity,
            environment: &self.environment,
            measurement_mode: self.measurement_mode,
        };
        pipeline.init(&mix_context);
        pipeline.ensure_topology(1, &self.layout, self.sample_rate);
    }

    fn build(&self, mode: RenderMode) -> RenderPipeline {
        let mut pipeline = build_mode(mode, &self.pipeline_params());
        self.init(&mut pipeline);
        pipeline
    }

    fn render_params(&self) -> RenderParams<'_> {
        RenderParams {
            listener: &self.listener,
            channels: self.channels,
            sample_rate: self.sample_rate,
            master_gain: self.master_gain,
            distance_model: &self.distance_model,
            layout: &self.layout,
            environment: &self.environment,
            measurement_mode: self.measurement_mode,
        }
    }

    /// Render `total_frames` in `BLOCK_FRAMES`-sized callbacks.
    fn render(
        &self,
        pipeline: &mut RenderPipeline,
        sources: &mut [Box<dyn SoundSource>],
        total_frames: usize,
    ) -> Vec<f32> {
        let mut output = Vec::with_capacity(total_frames * self.channels);
        let mut block = vec![0.0f32; BLOCK_FRAMES * self.channels];
        let mut rendered = 0;
        while rendered < total_frames {
            let frames = BLOCK_FRAMES.min(total_frames - rendered);
            let buffer = &mut block[..frames * self.channels];
            render_pipeline(pipeline, sources, &self.render_params(), buffer);
            output.extend_from_slice(buffer);
            rendered += frames;
        }
        output
    }
}

fn channel_of(output: &[f32], channels: usize, channel: usize) -> Vec<f32> {
    output
        .iter()
        .skip(channel)
        .step_by(channels)
        .copied()
        .collect()
}

fn first_non_zero_frame(output: &[f32], channels: usize) -> Option<usize> {
    output
        .chunks_exact(channels)
        .position(|frame| frame.iter().any(|sample| *sample != 0.0))
}

/// Σ x² over the given channels from `from_frame` on.
fn energy(output: &[f32], channels: usize, selected: &[usize], from_frame: usize) -> f64 {
    output
        .chunks_exact(channels)
        .skip(from_frame)
        .map(|frame| {
            selected
                .iter()
                .map(|&channel| (frame[channel] as f64).powi(2))
                .sum::<f64>()
        })
        .sum()
}

/// Steady-state 1 kHz output power (all channels) in dB, second half of
/// `frames`. At 1 kHz ISO 9613-1 air absorption is ≈ 0.005 dB/m, so a few
/// metres of extra path change it by < 0.02 dB.
fn tone_power_db(rig: &Rig, mode: RenderMode, position: Vec3, frames: usize) -> f64 {
    let mut pipeline = rig.build(mode);
    let mut sources = vec![ToneSource::boxed(position, 1000.0, usize::MAX)];
    let output = rig.render(&mut pipeline, &mut sources, frames);
    let all_channels: Vec<usize> = (0..rig.channels).collect();
    10.0 * energy(&output, rig.channels, &all_channels, frames / 2).log10()
}
