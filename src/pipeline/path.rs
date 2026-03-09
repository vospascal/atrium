//! Path-based propagation types.
//!
//! A `PathResolver` computes how sound travels from source to target (direct,
//! reflected, diffracted). Each propagation path is a `PathContribution` with
//! its own direction, distance, delay, and gain. `PathEffect`s process audio
//! per-path (air absorption, propagation delay, distance attenuation).
//!
//! The renderer uses these to pan each path independently — reflections arrive
//! from their image-source direction instead of sharing the direct signal's pan.

use atrium_core::types::Vec3;

use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::propagation::{Barrier, GroundProperties};

/// Maximum paths per source (1 direct + 6 reflections + up to 5 diffraction).
pub const MAX_PATHS: usize = 12;

// ─────────────────────────────────────────────────────────────────────────────
// Path data
// ─────────────────────────────────────────────────────────────────────────────

/// The kind of propagation path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathKind {
    /// Line-of-sight direct path.
    Direct,
    /// First-order wall reflection (image-source method).
    Reflection,
    /// Edge diffraction (future: Biot-Tolstoy-Medwin).
    Diffraction,
}

/// A single propagation path from source to target.
///
/// Produced by a `PathResolver`. Carries enough information for the renderer
/// to pan the path and for `PathEffect`s to filter it.
#[derive(Clone, Copy, Debug)]
pub struct PathContribution {
    pub kind: PathKind,
    /// Unit direction from target toward the path's apparent origin.
    /// The renderer uses this for panning (VBAP azimuth, HRTF lookup, etc.).
    pub direction: Vec3,
    /// Total path length in meters (for distance attenuation and air absorption).
    pub distance: f32,
    /// Propagation delay relative to the direct path, in seconds.
    /// Direct path always has delay = 0.
    pub delay_seconds: f32,
    /// Geometric gain factor (e.g., wall reflectivity for reflections, 1.0 for direct).
    pub gain: f32,
    /// Which wall this reflection bounced off (0-5 for the 6 room faces).
    /// `None` for direct paths and diffraction.
    pub wall_index: Option<u8>,
}

/// Fixed-capacity set of propagation paths. No heap allocation.
pub struct PathSet {
    paths: [PathContribution; MAX_PATHS],
    len: usize,
}

impl Default for PathSet {
    fn default() -> Self {
        Self::new()
    }
}

impl PathSet {
    pub fn new() -> Self {
        Self {
            paths: [PathContribution {
                kind: PathKind::Direct,
                direction: Vec3::new(1.0, 0.0, 0.0),
                distance: 0.0,
                delay_seconds: 0.0,
                gain: 0.0,
                wall_index: None,
            }; MAX_PATHS],
            len: 0,
        }
    }

    /// Add a path. Silently drops if at capacity.
    pub fn push(&mut self, path: PathContribution) {
        if self.len < MAX_PATHS {
            self.paths[self.len] = path;
            self.len += 1;
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_slice(&self) -> &[PathContribution] {
        &self.paths[..self.len]
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PathResolver
// ─────────────────────────────────────────────────────────────────────────────

/// Context for resolving propagation paths (geometry only).
pub struct ResolveContext<'a> {
    pub source_pos: Vec3,
    pub target_pos: Vec3,
    pub room_min: Vec3,
    pub room_max: Vec3,
    pub barriers: &'a [Barrier],
    pub atmosphere: &'a AtmosphericParams,
}

/// Resolves propagation paths from source to target.
///
/// Implementations:
/// - `DirectPathResolver`: 1 direct path (Phase 1.2)
/// - `ImageSourceResolver`: 1 direct + up to 6 reflections (Phase 1.5)
/// - Future: ray-traced resolver, diffraction resolver
pub trait PathResolver: Send {
    /// Compute propagation paths and write them into `out`.
    fn resolve(&self, ctx: &ResolveContext<'_>, out: &mut PathSet);
}

// ─────────────────────────────────────────────────────────────────────────────
// Wall materials
// ─────────────────────────────────────────────────────────────────────────────

/// Surface absorption coefficients at 6 octave bands (125, 250, 500, 1k, 2k, 4k Hz).
///
/// Values from Yeoward 2021 / ISO 11654. α ranges from 0.0 (fully reflective)
/// to 1.0 (fully absorptive).
#[derive(Clone, Debug)]
pub struct WallMaterial {
    pub name: &'static str,
    /// Absorption coefficients at [125, 250, 500, 1000, 2000, 4000] Hz.
    pub alpha: [f32; 6],
}

impl WallMaterial {
    /// Hard wall as a const for use in static arrays and test fixtures.
    pub const HARD_WALL: WallMaterial = WallMaterial {
        name: "hard_wall",
        alpha: [0.02, 0.02, 0.03, 0.04, 0.05, 0.05],
    };

    /// Hard wall (concrete/plaster). Yeoward 2021 Table 1.
    pub fn hard_wall() -> Self {
        Self {
            name: "hard_wall",
            alpha: [0.02, 0.02, 0.03, 0.04, 0.05, 0.05],
        }
    }

    /// Carpet on concrete. Yeoward 2021 Table 1.
    pub fn carpet() -> Self {
        Self {
            name: "carpet",
            alpha: [0.02, 0.04, 0.08, 0.20, 0.35, 0.40],
        }
    }

    /// Acoustic ceiling tile. Yeoward 2021 Table 1.
    pub fn ceiling_tile() -> Self {
        Self {
            name: "ceiling_tile",
            alpha: [0.20, 0.40, 0.70, 0.80, 0.60, 0.40],
        }
    }

    /// Broadband reflectivity (energy domain, 0.0–1.0).
    ///
    /// Computed as `1.0 - α_broadband`, where α_broadband is the energy-weighted
    /// average of the mid bands (250 Hz–4 kHz, indices 1–5). The 125 Hz band is
    /// excluded because it contributes little to perceived broadband reflection level.
    pub fn broadband_reflectivity(&self) -> f32 {
        // Energy weights for bands 250, 500, 1k, 2k, 4k Hz.
        // Flat weighting across these perceptually dominant bands.
        let alpha_broadband =
            (self.alpha[1] + self.alpha[2] + self.alpha[3] + self.alpha[4] + self.alpha[5]) / 5.0;
        (1.0 - alpha_broadband).clamp(0.0, 1.0)
    }

    /// Broadband amplitude reflection gain: √(broadband_reflectivity).
    ///
    /// Used by `ImageSourceResolver` for per-wall reflection energy.
    pub fn broadband_reflection_gain(&self) -> f32 {
        self.broadband_reflectivity().sqrt()
    }
}

impl Default for WallMaterial {
    fn default() -> Self {
        Self::hard_wall()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PathEffect
// ─────────────────────────────────────────────────────────────────────────────

/// Context passed to a PathEffect for buffer-rate updates.
pub struct PathEffectContext<'a> {
    pub path: &'a PathContribution,
    pub atmosphere: &'a AtmosphericParams,
    pub ground: &'a GroundProperties,
    pub sample_rate: f32,
    /// Source position (for geometry-dependent effects like ground reflection).
    pub source_pos: atrium_core::types::Vec3,
    /// Target/listener position.
    pub target_pos: atrium_core::types::Vec3,
    /// Wall materials for the 6 room faces (indexed by `path.wall_index`).
    /// Order: -X, +X, -Y, +Y, -Z (floor), +Z (ceiling).
    pub wall_materials: &'a [WallMaterial; 6],
}

/// Per-path audio effect (air absorption, propagation delay, etc.).
///
/// Each instance is bound to one propagation path. Updates at buffer rate,
/// processes audio at sample rate.
pub trait PathEffect: Send {
    /// Update parameters for the current buffer.
    fn update(&mut self, ctx: &PathEffectContext);

    /// Process a single audio sample.
    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        sample
    }

    fn name(&self) -> &str;

    fn reset(&mut self) {}
}

/// Ordered chain of per-path effects. Processes a sample through each effect
/// in sequence.
pub struct PathEffectChain {
    effects: Vec<Box<dyn PathEffect>>,
}

impl PathEffectChain {
    pub fn new(effects: Vec<Box<dyn PathEffect>>) -> Self {
        Self { effects }
    }

    pub fn update(&mut self, ctx: &PathEffectContext) {
        for effect in &mut self.effects {
            effect.update(ctx);
        }
    }

    #[inline]
    pub fn process_sample(&mut self, sample: f32) -> f32 {
        let mut s = sample;
        for effect in &mut self.effects {
            s = effect.process_sample(s);
        }
        s
    }

    pub fn reset(&mut self) {
        for effect in &mut self.effects {
            effect.reset();
        }
    }
}

/// Factory for creating PathEffect instances.
///
/// Used by renderers to create per-path effect chains when topology changes.
pub type PathEffectFactory = Box<dyn Fn(f32) -> Box<dyn PathEffect> + Send>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_set_push_and_slice() {
        let mut set = PathSet::new();
        assert!(set.is_empty());

        set.push(PathContribution {
            kind: PathKind::Direct,
            direction: Vec3::new(1.0, 0.0, 0.0),
            distance: 5.0,
            delay_seconds: 0.0,
            gain: 1.0,
            wall_index: None,
        });
        set.push(PathContribution {
            kind: PathKind::Reflection,
            direction: Vec3::new(-1.0, 0.0, 0.0),
            distance: 8.0,
            delay_seconds: 0.009,
            gain: 0.9,
            wall_index: Some(0),
        });

        assert_eq!(set.len(), 2);
        assert_eq!(set.as_slice()[0].kind, PathKind::Direct);
        assert_eq!(set.as_slice()[1].kind, PathKind::Reflection);
        assert!((set.as_slice()[0].distance - 5.0).abs() < 1e-6);
    }

    #[test]
    fn path_set_capacity_limit() {
        let mut set = PathSet::new();
        for i in 0..MAX_PATHS + 3 {
            set.push(PathContribution {
                kind: PathKind::Direct,
                direction: Vec3::new(1.0, 0.0, 0.0),
                distance: i as f32,
                delay_seconds: 0.0,
                gain: 1.0,
                wall_index: None,
            });
        }
        assert_eq!(set.len(), MAX_PATHS);
    }

    #[test]
    fn path_set_clear() {
        let mut set = PathSet::new();
        set.push(PathContribution {
            kind: PathKind::Direct,
            direction: Vec3::new(1.0, 0.0, 0.0),
            distance: 1.0,
            delay_seconds: 0.0,
            gain: 1.0,
            wall_index: None,
        });
        assert_eq!(set.len(), 1);
        set.clear();
        assert!(set.is_empty());
    }

    #[test]
    fn effect_chain_processes_in_order() {
        struct GainEffect(f32);
        impl PathEffect for GainEffect {
            fn update(&mut self, _ctx: &PathEffectContext) {}
            fn process_sample(&mut self, sample: f32) -> f32 {
                sample * self.0
            }
            fn name(&self) -> &str {
                "gain"
            }
        }

        let mut chain =
            PathEffectChain::new(vec![Box::new(GainEffect(0.5)), Box::new(GainEffect(0.8))]);

        let out = chain.process_sample(1.0);
        assert!((out - 0.4).abs() < 1e-6); // 1.0 * 0.5 * 0.8
    }
}
