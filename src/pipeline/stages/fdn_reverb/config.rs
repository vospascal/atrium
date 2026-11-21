//! Designer voicing for the tail, resolved once at pipeline-build time.

pub(super) const MAX_OUT: usize = 8;

/// When RT60 is overridden explicitly, the 4 kHz decay is set to this fraction
/// of the mid-band (500 Hz) RT60 — highs ring out somewhat faster than lows for
/// a naturally bright-but-controlled "glass atrium" tail without going sizzly.
pub(super) const HF_DECAY_RATIO: f32 = 0.6;

/// Voicing/tuning for the FDN reverb, resolved once at pipeline-build time from
/// the environment config. Separates *what the room does* (RT60, pre-delay from
/// geometry) from *how the designer wants the tail to sit* (per-channel balance,
/// low-end control).
#[derive(Clone)]
pub struct FdnConfig {
    /// Explicit mid-band (500 Hz) RT60 in seconds. `Some` bypasses the
    /// geometry+material Sabine RT60; the 4 kHz decay is derived via
    /// `HF_DECAY_RATIO`. `None` keeps the physically-derived decay.
    pub rt60_seconds: Option<f32>,
    /// Explicit pre-delay in milliseconds. `Some` bypasses the mean-free-path
    /// pre-delay (bounded to `PRE_DELAY_MAXIMUM_SECONDS`, 250 ms; the
    /// pre-delay ring is sized to it at init). `None` keeps it.
    pub pre_delay_ms: Option<f32>,
    /// Per-output-channel wet trim, indexed by the canonical 5.1 hardware layout
    /// (0=L, 1=R, 2=C, 3=LFE, 4=Ls, 5=Rs, 6/7=back surrounds). Lets surrounds run
    /// wetter than fronts and keeps the centre near-dry for dialogue clarity.
    pub channel_gains: [f32; MAX_OUT],
    /// High-pass cutoff (Hz) applied to the reverb send. Keeps low-mid mud out of
    /// the tail — and, since the tail then has little sub-bass, keeps reverb bass
    /// out of the LFE too. `0.0` disables it.
    pub hpf_cutoff_hz: f32,
    /// Overall wet level multiplier applied to the tail. Lets the reverb sit under
    /// the dry signal (the send law + normalization set the D/R balance; this trims
    /// the absolute prominence). `1.0` = full.
    pub wet: f32,
}

impl Default for FdnConfig {
    /// Neutral FDN: physically-derived decay/pre-delay, uniform channel level,
    /// full wet, no send high-pass. Matches the reverb's historical behaviour.
    fn default() -> Self {
        Self {
            rt60_seconds: None,
            pre_delay_ms: None,
            channel_gains: [1.0; MAX_OUT],
            hpf_cutoff_hz: 0.0,
            wet: 1.0,
        }
    }
}

impl FdnConfig {
    /// Voicing for the physical speaker array: surrounds wettest, centre near-dry,
    /// fronts moderate, LFE unused; reverb send high-passed to control the low end.
    /// This is the idea.md atrium recipe (surrounds > fronts > centre, LFE dry).
    pub fn speaker(rt60_seconds: Option<f32>, pre_delay_ms: Option<f32>) -> Self {
        Self {
            rt60_seconds,
            pre_delay_ms,
            //             L    R    C    LFE  Ls   Rs   (7.1 back ×2)
            channel_gains: [0.6, 0.6, 0.15, 0.0, 1.0, 1.0, 1.0, 1.0],
            hpf_cutoff_hz: 160.0,
            // Sit the tail clearly under the dry signal — a living-room atrium,
            // not a wash. Keeps the wet from dominating in energy-concentrating
            // modes (VBAP/WorldLocked/Ambisonics).
            wet: 0.5,
        }
    }

    /// Voicing for headphone (HRTF) output: full, uniform stereo reverb with only
    /// a gentle rumble filter — no speaker-array channel balancing.
    pub fn headphone(rt60_seconds: Option<f32>, pre_delay_ms: Option<f32>) -> Self {
        Self {
            rt60_seconds,
            pre_delay_ms,
            channel_gains: [1.0; MAX_OUT],
            hpf_cutoff_hz: 100.0,
            wet: 0.7,
        }
    }
}
