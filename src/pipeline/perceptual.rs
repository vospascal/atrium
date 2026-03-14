//! Perceptual scoring layer for per-source masking analysis and spectral salience.
//!
//! Computes a smoothed perceptual score [0, 1] for each source based on:
//! - **Audibility**: how much each source exceeds the masked threshold from all other sources
//! - **Salience**: how spectrally unique each source is compared to competitors
//!
//! Scores are updated once per render buffer and smoothed with asymmetric
//! attack/release dynamics (fast attack for newly-masked sources, slow release
//! with hold for unmasking).

use crate::audio::masking::{SpreadingFunction, MASKING_OFFSET};
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
    /// Whether gain shaping is enabled (default: false).
    pub gain_shaping_enabled: bool,
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
            gain_shaping_enabled: false,
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

    /// Compute optional gain modifier from perceptual score.
    /// Only meaningful when `gain_shaping_enabled` is true.
    /// Maps score [0, 1] to gain [0.85, 1.15].
    pub fn gain_modifier(&self, source_idx: usize) -> f32 {
        if !self.gain_shaping_enabled || source_idx >= self.scores.len() {
            return 1.0;
        }
        0.85 + 0.30 * self.scores[source_idx]
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
                // Compute masked threshold from all other sources.
                let mut mask_linear = 0.0f32;
                let mut max_other_level = f32::NEG_INFINITY;

                for t in 0..source_count {
                    if t == s || !self.active_mask[t] {
                        continue;
                    }
                    // Spreading contribution from source t to band b.
                    for (b2, &level) in self.band_db[t].iter().enumerate() {
                        let dz = b as i32 - b2 as i32;
                        let spread = self.spreading.spread_db(dz);
                        let contribution_db = level + spread - MASKING_OFFSET;
                        mask_linear += 10.0_f32.powf(contribution_db / 10.0);
                    }
                    // Track max other level in this band for uniqueness.
                    if self.band_db[t][b] > max_other_level {
                        max_other_level = self.band_db[t][b];
                    }
                }

                let mask_db = if mask_linear > 0.0 {
                    10.0 * mask_linear.log10()
                } else {
                    f32::NEG_INFINITY
                };

                let source_level = self.band_db[s][b];

                // Skip bands where this source has no energy.
                if source_level <= f32::NEG_INFINITY {
                    continue;
                }

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
mod tests {
    use super::*;

    fn flat_profile() -> [f32; BARK_BANDS] {
        [0.0; BARK_BANDS]
    }

    fn low_freq_profile() -> [f32; BARK_BANDS] {
        // Energy concentrated in bands 1-4 (0-400 Hz).
        let mut p = [-30.0f32; BARK_BANDS];
        p[0] = 5.0;
        p[1] = 5.0;
        p[2] = 3.0;
        p[3] = 1.0;
        p
    }

    fn source(amplitude: f32, profile: [f32; BARK_BANDS]) -> SourcePerceptualState {
        SourcePerceptualState {
            received_amplitude: amplitude,
            spectral_bands: profile,
            active: true,
        }
    }

    #[test]
    fn single_source_fully_salient() {
        let mut layer = PerceptualLayer::new(1);
        let sources = [source(1.0, flat_profile())];
        // Run multiple updates to let smoothing settle.
        for _ in 0..100 {
            layer.update(&sources);
        }
        assert!(
            layer.scores()[0] > 0.9,
            "single source should have high score, got {}",
            layer.scores()[0]
        );
    }

    #[test]
    fn identical_sources_reduce_score() {
        let mut layer = PerceptualLayer::new(2);
        let sources = [source(1.0, flat_profile()), source(1.0, flat_profile())];
        for _ in 0..100 {
            layer.update(&sources);
        }
        // Both sources should have reduced score (masking each other).
        assert!(
            layer.scores()[0] < 0.95,
            "identical sources should mask each other, score = {}",
            layer.scores()[0]
        );
    }

    #[test]
    fn unique_source_scores_higher_than_masked() {
        let mut layer = PerceptualLayer::new(2);
        // Source 0: low-frequency (purring-like).
        // Source 1: high-frequency only.
        // They occupy different spectral regions so should not mask each other much.
        let mut high_freq_profile = [-30.0f32; BARK_BANDS];
        high_freq_profile[18] = 5.0;
        high_freq_profile[19] = 5.0;
        high_freq_profile[20] = 3.0;
        high_freq_profile[21] = 1.0;
        let sources = [
            source(1.0, low_freq_profile()),
            source(1.0, high_freq_profile),
        ];
        for _ in 0..100 {
            layer.update(&sources);
        }
        // Spectrally non-overlapping sources should maintain reasonable scores.
        assert!(
            layer.scores()[0] > 0.3,
            "spectrally unique source should maintain some score, got {}",
            layer.scores()[0]
        );
    }

    #[test]
    fn distant_bands_dont_mask() {
        let mut layer = PerceptualLayer::new(2);
        // Source 0: only in band 2 (low).
        let mut profile_low = [f32::NEG_INFINITY; BARK_BANDS];
        profile_low[2] = 10.0;
        // Source 1: only in band 20 (high).
        let mut profile_high = [f32::NEG_INFINITY; BARK_BANDS];
        profile_high[20] = 10.0;
        let sources = [source(1.0, profile_low), source(1.0, profile_high)];
        for _ in 0..100 {
            layer.update(&sources);
        }
        // Both should have high scores — they don't compete.
        assert!(
            layer.scores()[0] > 0.8,
            "distant-band source 0 should be mostly unmasked, got {}",
            layer.scores()[0]
        );
        assert!(
            layer.scores()[1] > 0.8,
            "distant-band source 1 should be mostly unmasked, got {}",
            layer.scores()[1]
        );
    }

    #[test]
    fn score_floor_prevents_silence() {
        let mut layer = PerceptualLayer::new(2);
        // Very loud masker vs very quiet source, same profile.
        let sources = [
            source(0.01, flat_profile()), // very quiet
            source(1.0, flat_profile()),  // loud masker
        ];
        for _ in 0..100 {
            layer.update(&sources);
        }
        // Even heavily masked, score should not be 0.
        assert!(
            layer.scores()[0] > 0.0,
            "masked source should have non-zero score, got {}",
            layer.scores()[0]
        );
    }

    #[test]
    fn attack_faster_than_release() {
        let mut layer = PerceptualLayer::new(1);
        let active = [source(1.0, flat_profile())];
        let inactive = [SourcePerceptualState {
            received_amplitude: 1.0,
            spectral_bands: flat_profile(),
            active: false,
        }];

        // Start with high score.
        for _ in 0..100 {
            layer.update(&active);
        }
        let start = layer.scores()[0];

        // Attack: deactivate and count blocks to reach 50% of drop.
        let mut attack_blocks = 0;
        let target_drop = start * 0.5;
        while layer.scores()[0] > target_drop && attack_blocks < 1000 {
            layer.update(&inactive);
            attack_blocks += 1;
        }

        // Reset to low score.
        for _ in 0..200 {
            layer.update(&inactive);
        }
        let low = layer.scores()[0];

        // Release: reactivate and count blocks to recover 50% of rise.
        let target_rise = low + (start - low) * 0.5;
        let mut release_blocks = 0;
        while layer.scores()[0] < target_rise && release_blocks < 1000 {
            layer.update(&active);
            release_blocks += 1;
        }

        assert!(
            attack_blocks < release_blocks,
            "attack ({attack_blocks} blocks) should be faster than release ({release_blocks} blocks)"
        );
    }

    #[test]
    fn below_floor_sources_skipped() {
        let mut layer = PerceptualLayer::new(2);
        let sources = [
            source(0.0001, flat_profile()), // below analysis floor (0.001)
            source(1.0, flat_profile()),
        ];
        for _ in 0..100 {
            layer.update(&sources);
        }
        // Source below floor should decay to 0.
        assert!(
            layer.scores()[0] < 0.1,
            "below-floor source should have low score, got {}",
            layer.scores()[0]
        );
        // Loud source should be unaffected by the below-floor source.
        assert!(
            layer.scores()[1] > 0.9,
            "loud source should be unaffected, got {}",
            layer.scores()[1]
        );
    }

    #[test]
    fn removing_masker_increases_score() {
        let mut layer = PerceptualLayer::new(2);
        // Both active, competing.
        let both_active = [source(0.3, flat_profile()), source(1.0, flat_profile())];
        for _ in 0..100 {
            layer.update(&both_active);
        }
        let score_with_masker = layer.scores()[0];

        // Remove the loud masker.
        let masker_gone = [
            source(0.3, flat_profile()),
            SourcePerceptualState {
                received_amplitude: 1.0,
                spectral_bands: flat_profile(),
                active: false,
            },
        ];
        for _ in 0..200 {
            layer.update(&masker_gone);
        }
        let score_without_masker = layer.scores()[0];

        assert!(
            score_without_masker > score_with_masker,
            "removing masker should increase score: {} -> {}",
            score_with_masker,
            score_without_masker
        );
    }

    #[test]
    fn gain_modifier_default_is_unity() {
        let layer = PerceptualLayer::new(1);
        assert_eq!(layer.gain_modifier(0), 1.0, "gain shaping off = unity");
    }

    #[test]
    fn gain_modifier_bounded_when_enabled() {
        let mut layer = PerceptualLayer::new(2);
        layer.gain_shaping_enabled = true;
        // Score 0 -> gain 0.85, score 1 -> gain 1.15.
        layer.scores[0] = 0.0;
        layer.scores[1] = 1.0;
        let g0 = layer.gain_modifier(0);
        let g1 = layer.gain_modifier(1);
        assert!(
            (g0 - 0.85).abs() < 0.01,
            "min gain should be ~0.85, got {g0}"
        );
        assert!(
            (g1 - 1.15).abs() < 0.01,
            "max gain should be ~1.15, got {g1}"
        );
    }
}
