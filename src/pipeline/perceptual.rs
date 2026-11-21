//! Perceptual scoring layer for per-source masking analysis and spectral salience.
//!
//! Computes a smoothed perceptual score [0, 1] for each source based on:
//! - **Audibility**: how much each source exceeds the masked threshold from all other sources
//! - **Salience**: how spectrally unique each source is compared to competitors
//!
//! Scores are updated once per render buffer and smoothed with asymmetric
//! attack/release dynamics (fast attack for newly-masked sources, slow release
//! with hold for unmasking).

use crate::audio::masking::{masked_threshold, SpreadingFunction};
use crate::audio::spectral_profile::BARK_BANDS;

/// Smoothing time constants (at 48kHz, 256-sample blocks = 187.5 blocks/sec).
const DEFAULT_ATTACK_ALPHA: f32 = 0.30; // ~15ms attack
const DEFAULT_RELEASE_ALPHA: f32 = 0.035; // ~150ms release
const DEFAULT_HOLD_BLOCKS: u32 = 10; // ~50ms hold

/// Salience floor and range for the perceptual score formula.
const SALIENCE_FLOOR: f32 = 0.7;
const SALIENCE_RANGE: f32 = 0.3;

/// Cap for uniqueness when no competitors exist in a band.
const UNIQUENESS_CAP_DB: f32 = 20.0;

/// Minimum received amplitude for a source to be considered active in analysis.
const AMPLITUDE_FLOOR: f32 = 0.001;

/// Band weights (heuristic perceptual, presence-region emphasis).
const BAND_WEIGHTS: [f32; BARK_BANDS] = [
    0.5, 0.5, 0.5, 0.5, // Bands 1-4   (0-400 Hz)
    0.8, 0.8, 0.8, 0.8, // Bands 5-8   (400-920 Hz)
    1.0, 1.0, 1.0, 1.0, 1.0, // Bands 9-13  (920-2000 Hz)
    1.2, 1.2, 1.2, 1.2, // Bands 14-17 (2-3.7 kHz)
    1.0, 1.0, 1.0, // Bands 18-20 (3.7-6.4 kHz)
    0.6, 0.6, 0.6, 0.6, // Bands 21-24 (6.4-15.5 kHz)
];

/// Per-source state needed for perceptual analysis.
pub struct SourcePerceptualState {
    /// Linear amplitude from physics pipeline (distance x directivity x sone gain).
    pub received_amplitude: f32,
    /// Pre-computed spectral profile (24 Bark bands, dB relative to RMS).
    pub spectral_bands: [f32; BARK_BANDS],
    /// Whether this source is currently active/unmuted.
    pub active: bool,
}

/// Perceptual scoring layer that computes per-source scores based on masking
/// analysis and spectral salience.
pub struct PerceptualLayer {
    /// Pre-computed spreading function for inter-band masking.
    spreading: SpreadingFunction,
    /// Per-source smoothed perceptual scores.
    scores: Vec<f32>,
    /// Hold counters for release smoothing.
    hold_counters: Vec<u32>,
    /// Scratch buffer: per-source received level in dB.
    received_db: Vec<f32>,
    /// Scratch buffer: per-source per-band levels in dB.
    band_db: Vec<[f32; BARK_BANDS]>,
    /// Scratch buffer: per-source active flag (above amplitude floor).
    active_mask: Vec<bool>,
}

impl PerceptualLayer {
    pub fn new(source_count: usize) -> Self {
        Self {
            spreading: SpreadingFunction::new(),
            scores: vec![1.0; source_count],
            hold_counters: vec![0; source_count],
            received_db: vec![f32::NEG_INFINITY; source_count],
            band_db: vec![[f32::NEG_INFINITY; BARK_BANDS]; source_count],
            active_mask: vec![false; source_count],
        }
    }

    /// Ensure capacity for the given number of sources.
    pub fn resize(&mut self, source_count: usize) {
        self.scores.resize(source_count, 1.0);
        self.hold_counters.resize(source_count, 0);
        self.received_db.resize(source_count, f32::NEG_INFINITY);
        self.band_db
            .resize(source_count, [f32::NEG_INFINITY; BARK_BANDS]);
        self.active_mask.resize(source_count, false);
    }

    /// Get the current smoothed perceptual scores.
    pub fn scores(&self) -> &[f32] {
        &self.scores
    }

    /// Update perceptual scores for all sources.
    /// Call once per render buffer, before the source render loop.
    pub fn update(&mut self, sources: &[SourcePerceptualState]) {
        let source_count = sources.len();
        self.resize(source_count);

        if source_count == 0 {
            return;
        }

        // Step 1: Clear and fill scratch buffers for active sources above floor.
        for value in self.received_db.iter_mut() {
            *value = f32::NEG_INFINITY;
        }
        for bands in self.band_db.iter_mut() {
            *bands = [f32::NEG_INFINITY; BARK_BANDS];
        }
        for flag in self.active_mask.iter_mut() {
            *flag = false;
        }

        for (s, src) in sources.iter().enumerate() {
            if !src.active || src.received_amplitude < AMPLITUDE_FLOOR {
                continue;
            }
            self.active_mask[s] = true;
            self.received_db[s] = 20.0 * src.received_amplitude.log10();
            for (band, spectral) in self.band_db[s].iter_mut().zip(&src.spectral_bands) {
                *band = self.received_db[s] + spectral;
            }
        }

        // Step 2: For each active source, compute masking threshold, audibility, salience.
        for s in 0..source_count {
            if !self.active_mask[s] {
                // Inactive source: smoothly decay score toward 0.
                self.smooth_score(s, 0.0);
                continue;
            }

            let mut audibility_sum = 0.0f32;
            let mut salience_sum = 0.0f32;
            let mut weight_sum = 0.0f32;

            #[allow(clippy::needless_range_loop)]
            for b in 0..BARK_BANDS {
                let source_level = self.band_db[s][b];

                // Skip bands where this source has no energy.
                if source_level <= f32::NEG_INFINITY {
                    continue;
                }

                // Every other active source is a masker (inactive ones were
                // cleared to −∞ above and would contribute nothing anyway).
                let active_mask = &self.active_mask;
                let other_band_levels = self
                    .band_db
                    .iter()
                    .enumerate()
                    .filter(|&(other, _)| other != s && active_mask[other])
                    .map(|(_, bands)| bands);

                // Masked threshold from all other active sources.
                let mask_db = masked_threshold(b, other_band_levels.clone(), &self.spreading);

                // Max other level in this band, for uniqueness.
                let max_other_level = other_band_levels
                    .map(|bands| bands[b])
                    .fold(f32::NEG_INFINITY, f32::max);

                // Audibility: sigmoid of excess above masking threshold.
                let audibility = if mask_db <= f32::NEG_INFINITY {
                    1.0 // no masker in this band = fully audible
                } else {
                    let excess = source_level - mask_db;
                    1.0 / (1.0 + (-excess / 10.0).exp())
                };

                // Salience: sigmoid of uniqueness (bounded [0, 1]).
                let uniqueness = if max_other_level > f32::NEG_INFINITY {
                    source_level - max_other_level
                } else {
                    UNIQUENESS_CAP_DB // single source = fully unique
                };
                let salience_band = 1.0 / (1.0 + (-uniqueness / 6.0).exp());

                let w = BAND_WEIGHTS[b];
                audibility_sum += audibility * w;
                salience_sum += salience_band * w;
                weight_sum += w;
            }

            let mean_audibility = if weight_sum > 0.0 {
                audibility_sum / weight_sum
            } else {
                1.0
            };
            let mean_salience = if weight_sum > 0.0 {
                salience_sum / weight_sum
            } else {
                1.0
            };

            // Perceptual score = audibility * lerp(floor, 1.0, salience).
            let raw_score = mean_audibility * (SALIENCE_FLOOR + SALIENCE_RANGE * mean_salience);
            self.smooth_score(s, raw_score);
        }
    }

    fn smooth_score(&mut self, idx: usize, target: f32) {
        let current = self.scores[idx];
        if target < current {
            // Attack: fast reduction (source getting masked).
            self.hold_counters[idx] = 0;
            self.scores[idx] += DEFAULT_ATTACK_ALPHA * (target - current);
        } else {
            // Hold then release.
            self.hold_counters[idx] += 1;
            if self.hold_counters[idx] > DEFAULT_HOLD_BLOCKS {
                self.scores[idx] += DEFAULT_RELEASE_ALPHA * (target - current);
            }
        }
        self.scores[idx] = self.scores[idx].clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests;
