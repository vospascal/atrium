//! What a control surface needs to show a scene.
//!
//! Every value is the one the engine builds from, so a UI never re-derives
//! positions or colors from the YAML (and never disagrees with what is
//! heard). It holds only what a consumer reads today; grow it when a consumer
//! needs more, not before.

use std::path::Path;

use atrium_core::directivity::DirectivityPattern;
use atrium_core::source::EmitterKind;
use atrium_core::telemetry::MAX_SOURCES;
use atrium_core::types::Vec3;

use super::built_source::source_color;
use super::parse::arr_to_vec3;
use super::scene::SceneConfig;
use super::source::display_name;

/// The built scene as a UI sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneDescription {
    /// Environment floor size along x (m).
    pub width: f32,
    /// Environment floor size along y (m).
    pub depth: f32,
    /// The air at load time: temperature (°C) and relative humidity (%).
    pub temperature_c: f32,
    pub humidity_pct: f32,
    /// Listener position at load time.
    pub listener: Vec3,
    /// The scene's sources, each with the slot it plays from.
    pub sources: Vec<SourceDescription>,
    /// The room's loudspeakers, wherever the scene puts them, whether or not
    /// the output in use plays through them.
    pub speakers: Vec<SpeakerDescription>,
}

/// One loudspeaker of the room.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeakerDescription {
    /// The output channel it plays: what telemetry's peaks are counted by.
    pub channel: u16,
    /// Where it stands in the world (m).
    pub position: Vec3,
}

/// One source as built.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceDescription {
    /// What the sound is known by in the scene file. It stays the same when
    /// sounds around it are added or removed; its slot does not.
    pub id: String,
    /// The engine voice it plays from: the index commands and telemetry use.
    pub slot: u16,
    /// How wide it goes out (degrees; 360 is all round).
    pub width: f32,
    /// How much is heard straight behind it (dB below the front).
    pub behind: f32,
    /// What it can utter, in the order telemetry counts its takes. Empty
    /// for a generator, which has no recordings.
    pub takes: Vec<TakeDescription>,
    /// Shortest and longest rest between two takes (s). `None` for a sound
    /// that plays all the time, round and round.
    pub gap: Option<[f32; 2]>,
    /// How many times running one take is uttered, fewest and most, before
    /// another is chosen. `[1, 1]` for a sound that changes every time.
    pub bout: [u32; 2],
    /// How it follows the place's conditions, as its file names them.
    pub follows: Vec<atrium_world::Follow>,
    pub name: String,
    /// The family it belongs to in a list, when the scene gives it one.
    pub group: Option<String>,
    /// UI color as `#rrggbb`.
    pub color: String,
    /// Position at load time.
    pub position: Vec3,
    /// Calibrated level at `reference_distance` (dB SPL).
    pub spl: f32,
    /// Distance at which `spl` applies (m).
    pub reference_distance: f32,
    /// Whether this is a localized sound or an environmental field.
    pub emitter_kind: EmitterKind,
    /// Outer half-angle in radians for a cone, or the half-gain angle for a polar pattern.
    /// Absent for sources that render without a directional beam.
    pub beam_half_angle: Option<f32>,
    /// How wide it sounds, from 0 (a point) to 1 (all around).
    pub size: f32,
    /// The node's initial forward vector in world coordinates.
    pub facing: Vec3,
}

/// One take of a sound: its name, and how often it is chosen next to the
/// others.
#[derive(Debug, Clone, PartialEq)]
pub struct TakeDescription {
    pub name: String,
    pub chance: f32,
}

pub(super) fn beam_half_angle(kind: EmitterKind, pattern: DirectivityPattern) -> Option<f32> {
    if kind == EmitterKind::Field {
        return None;
    }
    match pattern {
        DirectivityPattern::Cone {
            outer, outer_gain, ..
        } if outer_gain < 1.0 => Some(outer),
        DirectivityPattern::Beam { half_width, .. } if half_width < std::f32::consts::PI - 1e-4 => {
            Some(half_width)
        }
        DirectivityPattern::Polar { alpha } if alpha < 1.0 => {
            Some(((0.5 - alpha) / (1.0 - alpha)).clamp(-1.0, 1.0).acos())
        }
        _ => None,
    }
}

/// A scene as a chooser shows it, read from the scene file alone: no audio
/// is decoded and no referenced file is opened.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneSummary {
    pub title: String,
    /// In slot order, as they will play.
    pub sounds: Vec<SoundSummary>,
}

/// One sound in a scene summary.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundSummary {
    pub name: String,
    /// UI color as `#rrggbb`, the same one the built scene reports.
    pub color: String,
    /// Where the sound starts, relative to the listener (m). Scene positions
    /// are all offset by the same spawn point, so this holds in the world too.
    pub offset_from_listener: Vec3,
}

impl SceneConfig {
    /// The summary of this scene, loaded from `path`.
    pub fn summary(&self, path: &str) -> SceneSummary {
        let listener = arr_to_vec3(self.listener.position);
        SceneSummary {
            title: self.title.clone().unwrap_or_else(|| title_from_path(path)),
            sounds: self
                .sounds
                .iter()
                .take(MAX_SOURCES)
                .enumerate()
                .map(|(slot, (id, entity))| SoundSummary {
                    name: display_name(id, entity),
                    color: source_color(entity, slot),
                    offset_from_listener: arr_to_vec3(entity.place.into()) - listener,
                })
                .collect(),
        }
    }
}

/// `scenes/river-bend.yaml` → "River Bend".
pub(super) fn title_from_path(path: &str) -> String {
    let stem = Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(path);
    stem.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut letters = word.chars();
            letters
                .next()
                .map(|first| first.to_uppercase().chain(letters).collect())
                .unwrap_or_default()
        })
        .collect::<Vec<String>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beam_shape_comes_from_the_rendered_pattern() {
        use std::f32::consts::FRAC_PI_2;
        let cone = DirectivityPattern::Cone {
            inner: 0.3,
            outer: 0.7,
            outer_gain: 0.15,
        };
        assert_eq!(beam_half_angle(EmitterKind::Object, cone), Some(0.7));
        assert_eq!(
            beam_half_angle(EmitterKind::Object, DirectivityPattern::OMNI),
            None
        );
        assert_eq!(beam_half_angle(EmitterKind::Field, cone), None);
        assert_eq!(
            beam_half_angle(EmitterKind::Object, DirectivityPattern::cardioid()),
            Some(FRAC_PI_2)
        );
    }

    #[test]
    fn titles_come_from_the_file_name_when_the_scene_has_none() {
        assert_eq!(title_from_path("scenes/river-bend.yaml"), "River Bend");
        assert_eq!(
            title_from_path("nature_headphones.yaml"),
            "Nature Headphones"
        );
    }

    /// The summary is read without building, so it must still point each
    /// sound the way the built scene places it, and color it the same.
    #[test]
    fn summary_agrees_with_the_built_scene() {
        let path = "scenes/nature.yaml";
        let config = SceneConfig::load(path).expect("scene should load");
        let summary = config.summary(path);
        let built = config.build().expect("scene should build").description;

        assert_eq!(summary.title, "Nature");
        assert_eq!(summary.sounds.len(), built.sources.len());
        for (sound, source) in summary.sounds.iter().zip(&built.sources) {
            assert_eq!(sound.name, source.name);
            assert_eq!(sound.color, source.color);
            let built_offset = source.position - built.listener;
            assert!(
                (sound.offset_from_listener - built_offset).length() < 1e-4,
                "{} points elsewhere than it plays",
                sound.name
            );
        }
    }
}
