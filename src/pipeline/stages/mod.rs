//! Concrete stage implementations.
//!
//! Each file contains one or more stage structs implementing
//! `SourceStage`, `PathStage`, or `MixStage`.

// MixStages (post-mix, whole-buffer)
pub mod delay_comp;
pub mod early_reflections;
pub mod fdn_reverb;
pub mod lfe_crossover;
pub mod master_gain;

// SourceStages (per-source, before routing)
pub mod air_absorption;
pub mod dbap_gains;
pub mod distance_gains;
pub mod ground_effect;
pub mod reflections;
pub mod vbap_gains;

// PathStages (per source × output path, inside renderer)
// Air absorption, ground effect, reflections PathStage variants live in their
// respective files above. Distance+directivity is WorldLocked-only:
pub mod distance_directivity;
