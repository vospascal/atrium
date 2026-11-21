//! One sound, built: decoded or preview-rendered, calibrated to its level,
//! and described for a UI. Shared by a scene's build and a sound added live.

use std::path::Path;
use std::sync::Arc;

use atrium_core::directivity::DirectivityPattern;
use atrium_core::source::EmitterKind;
use atrium_core::types::Vec3;
use atrium_world::Entity;

use super::description::TakeDescription;
use super::scene::NormalizationConfig;
use super::source::{resolve_spl, ResolvedTake};
use super::synth::SynthSpec;
use crate::audio::decode::decode_file;
use crate::audio::synth_node::SynthNode;
use crate::audio::test_node::TestNode;
use crate::audio::voice_node::VoiceNode;
use crate::plan::{Follow, Rests, Rhythm, TakeRhythm};

/// Default color palette for sources when no color is specified in YAML.
const SOURCE_COLORS: &[&str] = &[
    "#ff6b35", "#ffc107", "#ce93d8", "#4fc3f7", "#66bb6a", "#ef5350", "#ff8a65", "#ab47bc",
    "#26c6da", "#9ccc65",
];

/// A sound's UI color: its own `color`, else the palette entry for its slot.
pub(super) fn source_color(entity: &Entity, slot: usize) -> String {
    entity
        .color
        .clone()
        .unwrap_or_else(|| SOURCE_COLORS[slot % SOURCE_COLORS.len()].to_string())
}

/// Metadata collected during source building, serialized to JSON for the browser.
pub(super) struct SourceMeta {
    pub(super) id: String,
    /// Its takes, in the order telemetry counts them.
    pub(super) takes: Vec<TakeDescription>,
    /// Shortest and longest rest between takes; `None` when it plays all the time.
    pub(super) gap: Option<[f32; 2]>,
    pub(super) bout: [u32; 2],
    pub(super) follows: Vec<atrium_world::Follow>,
    pub(super) group: Option<String>,
    pub(super) name: String,
    pub(super) color: String,
    pub(super) spl: f32,
    pub(super) ref_dist: f32,
    pub(super) amplitude: f32,
    pub(super) audible_radius: f32,
    /// Width (degrees) and level behind (dB) it radiates with.
    pub(super) width: f32,
    pub(super) behind: f32,
    pub(super) directivity_alpha: f32,
    pub(super) spread: f32,
    pub(super) position: [f32; 3],
    pub(super) orbit_radius: f32,
    pub(super) orbit_speed: f32,
    pub(super) synth_kind: Option<String>,
    pub(super) emitter_kind: EmitterKind,
}

// ── Single-source builder (shared by scene build + live add) ────────────────

/// A fully-built source plus the derived values the UI and perceptual layer
/// need. Produced on the control thread and, for live adds, shipped into a pool
/// slot via [`crate::engine::edit::SceneEdit`].
pub(super) struct BuiltSource {
    pub source: Box<dyn atrium_core::source::SoundSource>,
    /// 24 Bark-band spectral profile (dB relative to RMS).
    pub bands: [f32; crate::audio::spectral_profile::BARK_BANDS],
    /// Base playback amplitude (sone-based gain, before spatial attenuation).
    pub amplitude: f32,
    /// Per-source reference distance (m) derived from SPL.
    pub ref_dist: f32,
    /// Free-field audible radius (m) at the hearing threshold.
    pub audible_radius: f32,
    /// Directivity polar coefficient (1.0 omni, 0.5 cardioid, …).
    pub directivity_alpha: f32,
    /// Its takes and rests, for the plan; `None` for a sound that sounds all
    /// the time.
    pub rhythm: Option<Rhythm>,
    /// How it follows the place's conditions, checked against what it is.
    pub follows: Vec<Follow>,
}

/// Decode a kind's recordings and build one `SoundSource` with its amplitude,
/// spectral profile, and reference distance. Used both when building a scene
/// and when adding a source live (the same lossless path, so a live-added
/// source matches a reloaded one exactly). `position` is world coordinates
/// (already offset by the environment spawn point).
///
/// Without `rests` the one take loops. With it, the voice utters its takes
/// when the session's plan says, and the rhythm it returns is what the plan
/// decides from.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_one_source(
    takes: &[ResolvedTake],
    rests: Option<Rests>,
    reference_spl: f32,
    pattern: DirectivityPattern,
    spread: f32,
    position: Vec3,
    orbit_radius: f32,
    orbit_speed: f32,
    normalization: &NormalizationConfig,
    global_ref_dist: f32,
    max_distance: f32,
) -> Result<BuiltSource, Box<dyn std::error::Error>> {
    let buffers = takes
        .iter()
        .map(|take| {
            decode_file(Path::new(&take.file))
                .map(Arc::new)
                .map_err(|error| format!("{}: {}", take.file, error))
        })
        .collect::<Result<Vec<_>, _>>()?;
    // One level for the whole voice: the RMS over all its takes end to end,
    // so a quiet phrase stays quieter than a loud one.
    let (energy, length) = buffers
        .iter()
        .fold((0.0f64, 0usize), |(energy, length), take| {
            let samples = take.samples.len();
            (
                energy + (take.rms as f64).powi(2) * samples as f64,
                length + samples,
            )
        });
    let rms = if length > 0 {
        (energy / length as f64).sqrt() as f32
    } else {
        0.0
    };
    let longest = buffers
        .iter()
        .max_by_key(|take| take.samples.len())
        .ok_or("a sound needs at least one recording")?;

    let profile = resolve_spl(reference_spl);
    let amplitude = profile.amplitude(rms, normalization.target_rms, normalization.spl_reference);
    let ref_dist = profile.ref_distance(global_ref_dist);
    let bands = longest.spectral_profile.bands;
    let audible_radius = profile.audible_radius(normalization.spl_threshold, max_distance);

    // Base amplitude at 0 dB SPL, so a live SPL edit can rescale without
    // re-decoding: amplitude = unit_amplitude · 10^(spl/20).
    let rms_correction = if rms > 1e-6 {
        normalization.target_rms / rms
    } else {
        1.0
    };
    let unit_amplitude = rms_correction * 10.0_f32.powf(-normalization.spl_reference / 20.0);

    let mut rhythm = None;
    let source: Box<dyn atrium_core::source::SoundSource> = match rests {
        None => {
            let mut node = TestNode::new(buffers[0].clone(), position, orbit_radius, orbit_speed);
            node.amplitude = amplitude;
            node.unit_amplitude = unit_amplitude;
            node.ref_dist = ref_dist;
            node.pattern = pattern;
            node.spread = spread;
            node.meter_signal();
            Box::new(node)
        }
        Some(rests) => {
            rhythm = Some(Rhythm {
                takes: buffers
                    .iter()
                    .zip(takes)
                    .map(|(audio, take)| TakeRhythm {
                        seconds: audio.samples.len() as f32 / audio.sample_rate as f32,
                        chance: take.chance,
                    })
                    .collect(),
                rests,
            });
            let mut node = VoiceNode::new(buffers.clone(), position);
            node.amplitude = amplitude;
            node.unit_amplitude = unit_amplitude;
            node.ref_dist = ref_dist;
            node.pattern = pattern;
            node.spread = spread;
            node.meter_signal(rms);
            Box::new(node)
        }
    };

    Ok(BuiltSource {
        source,
        bands,
        amplitude,
        ref_dist,
        audible_radius,
        directivity_alpha: pattern.alpha(),
        rhythm,
        follows: Vec::new(),
    })
}

/// Seconds of preview audio rendered control-side to measure a synth source's
/// RMS and Bark-band spectral profile — the same calibration measurements
/// `build_one_source` reads from a decoded file. Long enough to average over
/// gust cycles and drop statistics.
const SYNTH_PREVIEW_SECONDS: f32 = 20.0;
/// Sample rate for the preview render. The profile is Bark-band coarse, so a
/// device actually running at 44.1 kHz changes nothing meaningful.
const SYNTH_PREVIEW_SAMPLE_RATE: u32 = 48_000;

/// Build one procedural synth `SoundSource` with its amplitude, spectral
/// profile, and reference distance. Mirrors `build_one_source` exactly, except
/// the calibration measurements come from a preview render instead of a
/// decoded file, so synth and sample sources obey the same loudness model.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_one_synth_source(
    spec: &SynthSpec,
    reference_spl: f32,
    pattern: DirectivityPattern,
    spread: f32,
    position: Vec3,
    default_seed: u64,
    normalization: &NormalizationConfig,
    global_ref_dist: f32,
    max_distance: f32,
) -> Result<BuiltSource, Box<dyn std::error::Error>> {
    // Preview render: measure RMS + spectral profile exactly like a decoded file.
    let sample_rate = SYNTH_PREVIEW_SAMPLE_RATE as f32;
    let mut preview_generator = spec.build_generator(position, default_seed);
    let rhythm = spec.rhythm();
    // A generator the plan tells when to sound is silent by itself: book its
    // likeliest take at once, so the preview has something to measure.
    if let Some(rhythm) = &rhythm {
        let likeliest = rhythm
            .takes
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| left.chance.total_cmp(&right.chance))
            .map_or(0, |(index, _)| index as u8);
        preview_generator.schedule(atrium_core::source::Utterance {
            take: likeliest,
            at_sample: 0,
            gain: 1.0,
            rate: 1.0,
            skip_seconds: 0.0,
        });
    }
    let total_samples = (SYNTH_PREVIEW_SECONDS * sample_rate) as usize;
    let mut samples = vec![0.0_f32; total_samples];
    for sample in samples.iter_mut() {
        *sample = preview_generator.next_sample(sample_rate);
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / total_samples as f32).sqrt();
    if rms < 1e-6 {
        return Err(format!(
            "synth source `{}` rendered silence — check its parameters",
            spec.kind_name()
        )
        .into());
    }
    let bands =
        crate::audio::spectral_profile::compute_profile(&samples, SYNTH_PREVIEW_SAMPLE_RATE).bands;

    let profile = resolve_spl(reference_spl);
    let amplitude = profile.amplitude(rms, normalization.target_rms, normalization.spl_reference);
    let ref_dist = profile.ref_distance(global_ref_dist);
    let audible_radius = profile.audible_radius(normalization.spl_threshold, max_distance);

    // Base amplitude at 0 dB SPL, mirroring build_one_source.
    let rms_correction = normalization.target_rms / rms;
    let unit_amplitude = rms_correction * 10.0_f32.powf(-normalization.spl_reference / 20.0);

    // Fresh generator so the audible source starts from a clean state rather
    // than wherever the preview render left its envelopes.
    let mut node = SynthNode::new(spec.build_generator(position, default_seed), position);
    node.amplitude = amplitude;
    node.unit_amplitude = unit_amplitude;
    node.ref_dist = ref_dist;
    node.pattern = pattern;
    node.spread = spread;
    node.meter_signal(rms);

    Ok(BuiltSource {
        source: Box::new(node),
        bands,
        amplitude,
        ref_dist,
        audible_radius,
        directivity_alpha: pattern.alpha(),
        rhythm,
        follows: Vec::new(),
    })
}
