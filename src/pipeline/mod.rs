//! Composable render pipeline.
//!
//! A render pipeline defines the complete audio processing chain for a given
//! render mode. Each pipeline consists of:
//!
//! 1. **PathResolver** + **PathEffects** — resolve propagation paths per source,
//!    then apply per-path DSP (delay, air absorption, ground effect, wall absorption)
//! 2. **Renderer** — mode-specific spatialization (gain ramp, HRTF convolution,
//!    per-speaker propagation)
//! 3. **MixStages** — post-mix, whole-buffer (LFE crossover, delay comp,
//!    reverb, master gain)
//!
//! # Pipeline definitions
//!
//! Each render mode is defined as a function returning a `RenderPipeline`.
//! Reading the definition tells you exactly what stages run:
//!
//! ```text
//! WorldLocked {
//!     renderer: WorldLockedRenderer (per-speaker air absorption, ground effect, reflections, distance+directivity)
//!     mix_stages: [LfeCrossover, DelayComp(static), MasterGain]
//! }
//!
//! Vbap {
//!     resolver: ImageSourceResolver (1 direct + up to 6 reflections)
//!     path_effects: [PropagationDelay, AirAbsorption, GroundEffect, WallAbsorption]
//!     renderer: MultichannelRenderer (per-path VBAP gains)
//!     mix_stages: [FdnReverb, LfeCrossover, DelayComp(listener), MasterGain]
//! }
//!
//! Hrtf {
//!     resolver: ImageSourceResolver (1 direct + up to 6 reflections)
//!     path_effects: [PropagationDelay, AirAbsorption, GroundEffect, WallAbsorption]
//!     renderer: HrtfRenderer (per-path HRTF convolution to stereo)
//!     mix_stages: [FdnReverb, MasterGain]
//! }
//!
//! Dbap {
//!     resolver: ImageSourceResolver (1 direct + up to 6 reflections)
//!     path_effects: [PropagationDelay, AirAbsorption, GroundEffect, WallAbsorption]
//!     renderer: DbapRenderer (per-path DBAP gains)
//!     mix_stages: [FdnReverb, LfeCrossover, DelayComp(static), MasterGain]
//! }
//!
//! Ambisonics {
//!     resolver: ImageSourceResolver (1 direct + up to 6 reflections)
//!     path_effects: [PropagationDelay, AirAbsorption, GroundEffect, WallAbsorption]
//!     renderer: AmbisonicsRenderer (per-path FOA encode + decode)
//!     mix_stages: [AmbiDecorrelation, AmbiDecode, FdnReverb, LfeCrossover, DelayComp(listener), MasterGain]
//! }
//! ```

pub mod environment;
pub mod mix_stage;
pub mod path;
pub mod path_effects;
pub mod path_resolvers;
pub mod perceptual;
pub mod renderer;
pub mod renderers;
pub mod room_acoustics;
pub mod stages;
pub mod wall_material;

mod builders;
mod render;
#[cfg(test)]
mod tests;

pub use builders::{build_all_pipelines, PipelineParams};
pub use render::{render_pipeline, RenderParams, RenderPipeline};
