//! Concrete stage implementations.
//!
//! Each file contains one or more stage structs implementing
//! `SourceStage` or `MixStage`.

// MixStages (post-mix, whole-buffer)
pub mod delay_comp;
pub mod fdn_reverb;
pub mod lfe_crossover;
pub mod master_gain;

// SourceStages (per-source, before routing)
pub mod air_absorption;
pub mod ground_effect;
pub mod reflections;
