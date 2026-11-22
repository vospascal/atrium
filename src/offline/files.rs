//! A render on disk: the WAV, and beside it a JSON sidecar with everything
//! needed to make the file again and to read it: the scene and the commit,
//! the seed and every setting, the channels in order, and each channel's
//! level.

use std::path::{Path, PathBuf};

use atrium_core::probe::Probe;
use serde_json::{json, Value};

use super::provenance::Provenance;
use super::rendering::Rendered;
use crate::audio::wav::{write_wav, SampleFormat};

/// What was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    pub wav: PathBuf,
    /// `<name>.json` beside the WAV.
    pub sidecar: PathBuf,
    /// Samples beyond full scale, clipped in a 16-bit file; 0 in a float one.
    pub clipped: usize,
}

/// Write `rendered` to `wav` as `format`, and its sidecar beside it.
pub fn write_files(
    rendered: &Rendered,
    provenance: &Provenance,
    wav: &Path,
    format: SampleFormat,
) -> std::io::Result<Written> {
    if let Some(directory) = wav.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(directory)?;
    }
    let clipped = write_wav(
        wav,
        rendered.frames(),
        rendered.channels() as u16,
        rendered.sample_rate(),
        format,
    )?;
    let sidecar = wav.with_extension("json");
    let text = serde_json::to_string_pretty(&sidecar_json(rendered, provenance, format, clipped))
        .map_err(std::io::Error::other)?;
    std::fs::write(&sidecar, text + "\n")?;
    Ok(Written {
        wav: wav.to_path_buf(),
        sidecar,
        clipped,
    })
}

fn sidecar_json(
    rendered: &Rendered,
    provenance: &Provenance,
    format: SampleFormat,
    clipped: usize,
) -> Value {
    let settings = rendered.settings();
    let names = rendered.channel_names();
    json!({
        "scene": {
            "path": provenance.scene_path,
            "fnv1a_64": provenance.scene_hash,
        },
        "git": {
            "commit": provenance.commit,
            "dirty": provenance.dirty,
        },
        "seed": rendered.seed(),
        "layout": rendered.layout(),
        "mode": rendered.render_mode().as_str(),
        "channels": names,
        "sample_rate": rendered.sample_rate(),
        "block_frames": settings.block_frames,
        "start_seconds": settings.start,
        "seconds": settings.seconds,
        "frames": rendered.frames().len() / rendered.channels(),
        "probe": probe_json(&settings.probe),
        "turn_degrees_per_second": settings.turn_degrees_per_second,
        "weather_speed": settings.weather_speed,
        "format": match format {
            SampleFormat::Float32 => "f32",
            SampleFormat::Pcm16 => "s16",
        },
        "clipped_samples": clipped,
        // What the frames were raised by after rendering; the levels below
        // are the file's, the engine's own are these minus this gain.
        "gain_decibels": rendered.gain_decibels(),
        "levels": names
            .iter()
            .zip(rendered.statistics())
            .map(|(name, statistics)| json!({
                "channel": name,
                "peak": statistics.peak,
                "rms": statistics.rms,
            }))
            .collect::<Vec<_>>(),
    })
}

/// Every switch of the probe by its name in `Probe`; `only_channel` is a
/// channel index into `channels`, or null.
fn probe_json(probe: &Probe) -> Value {
    json!({
        "direct": probe.direct,
        "reflections": probe.reflections,
        "reverb": probe.reverb,
        "ambient": probe.ambient,
        "air_absorption": probe.air_absorption,
        "moving_air": probe.moving_air,
        "ground_effect": probe.ground_effect,
        "wall_colouring": probe.wall_colouring,
        "near_field": probe.near_field,
        "headphone_equalisation": probe.headphone_equalisation,
        "only_channel": probe.only_channel,
    })
}
