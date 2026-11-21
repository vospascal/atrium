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

// ── Hand-derived pins (audit 2026-11) ───────────────────────────────

/// One-pole block-rate smoothing: τ = −T_block / ln(1 − α), with
/// T_block = 256 / 48000 = 5.333 ms (the block size the constants assume).
/// Attack α 0.30 → τ = 14.95 ms ("~15 ms"); release α 0.035 → 149.7 ms
/// ("~150 ms"); hold 10 blocks = 53.3 ms ("~50 ms").
#[test]
fn smoothing_constants_match_documented_time_constants() {
    let block_seconds = 256.0_f32 / 48_000.0;
    let attack_seconds = -block_seconds / (1.0 - DEFAULT_ATTACK_ALPHA).ln();
    let release_seconds = -block_seconds / (1.0 - DEFAULT_RELEASE_ALPHA).ln();
    let hold_seconds = DEFAULT_HOLD_BLOCKS as f32 * block_seconds;
    assert!(
        (attack_seconds * 1000.0 - 14.95).abs() < 0.05,
        "{attack_seconds}"
    );
    assert!(
        (release_seconds * 1000.0 - 149.7).abs() < 0.2,
        "{release_seconds}"
    );
    assert!(
        (hold_seconds * 1000.0 - 53.33).abs() < 0.05,
        "{hold_seconds}"
    );
}

/// Attack is a plain one-pole: from 1.0 toward 0 the score is 0.7ⁿ after n
/// blocks (0.7, 0.49, 0.343). Release holds for exactly 10 blocks, then
/// moves 3.5 % of the gap on block 11. A lone source's target is
/// 1 · (0.7 + 0.3 · 1/(1 + e^(−20/6))) = 0.989666, so 0 → 0.0346383.
#[test]
fn attack_and_hold_release_follow_exact_block_recurrence() {
    let mut layer = PerceptualLayer::new(1);
    let inactive = [SourcePerceptualState {
        received_amplitude: 1.0,
        spectral_bands: flat_profile(),
        active: false,
    }];
    for expected in [0.7_f32, 0.49, 0.343] {
        layer.update(&inactive);
        assert!((layer.scores()[0] - expected).abs() < 1e-6);
    }

    layer.scores[0] = 0.0;
    layer.hold_counters[0] = 0;
    let alone = [source(1.0, flat_profile())];
    for _ in 0..10 {
        layer.update(&alone);
        assert_eq!(layer.scores()[0], 0.0, "hold must keep the score");
    }
    layer.update(&alone);
    assert!(
        (layer.scores()[0] - 0.034_638_3).abs() < 1e-6,
        "{}",
        layer.scores()[0]
    );
}

/// Two equal sources sharing only Bark band 10, both at 0 dB:
/// the threshold comes from `masking::masked_threshold`. Every other band
/// of both profiles is −∞, so the 27/24 dB-per-Bark slopes never enter and
/// only spread(0) = 0 dB contributes (the spreading fix left this pin as is):
/// threshold = 0 + spread(0) − 6 = −6 dB → excess 6 dB →
/// audibility = 1/(1 + e^(−0.6)) = 0.645656; uniqueness 0 dB → salience 0.5;
/// score = 0.645656 · (0.7 + 0.3·0.5) = 0.548808.
#[test]
fn equal_single_band_sources_settle_at_hand_derived_score() {
    let mut layer = PerceptualLayer::new(2);
    let mut profile = [f32::NEG_INFINITY; BARK_BANDS];
    profile[10] = 0.0;
    let sources = [source(1.0, profile), source(1.0, profile)];
    for _ in 0..200 {
        layer.update(&sources);
    }
    for score in layer.scores() {
        assert!((score - 0.548_808).abs() < 1e-4, "score {score}");
    }
}

/// Received level is 20·log10(amplitude): a source 20 dB below an
/// identical single-band masker (band 10 only, so again only spread(0)
/// enters the threshold: 0 − 6 = −6 dB) has excess = −20 + 6 = −14 dB →
/// audibility 1/(1 + e^1.4) = 0.197816; uniqueness −20 dB →
/// salience 1/(1 + e^(20/6)) = 0.034445; score = 0.197816 · 0.710334 = 0.140515.
#[test]
fn quieter_source_score_follows_level_difference() {
    let mut layer = PerceptualLayer::new(2);
    let mut profile = [f32::NEG_INFINITY; BARK_BANDS];
    profile[10] = 0.0;
    let sources = [source(0.1, profile), source(1.0, profile)];
    for _ in 0..200 {
        layer.update(&sources);
    }
    let score = layer.scores()[0];
    assert!((score - 0.140_515).abs() < 1e-4, "score {score}");
}
