//! The records and error that cross into Kotlin, and their conversion from
//! engine types. No engine type appears in a public signature here.

use atrium_core::telemetry::TelemetryFrame;
use atrium_core::types::Vec3;

use crate::head_tracking::HeadTrackingStatus;
use crate::output::Output;

/// Gain reported for a source that cannot be heard at all.
const SILENT_GAIN_DB: f32 = -120.0;

/// A point in world coordinates (m).
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct Position {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl From<Vec3> for Position {
    fn from(vector: Vec3) -> Self {
        Self {
            x: vector.x,
            y: vector.y,
            z: vector.z,
        }
    }
}

impl From<Position> for Vec3 {
    fn from(position: Position) -> Self {
        Vec3::new(position.x, position.y, position.z)
    }
}

/// The steps a UI can undo and redo, by name.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct EditHistory {
    /// Oldest first; the last is undone next.
    pub undoable: Vec<String>,
    /// The first is redone next.
    pub redoable: Vec<String>,
}

/// The scene the engine is playing.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SceneDescription {
    /// Environment floor size along x (m).
    pub width: f32,
    /// Environment floor size along y (m).
    pub depth: f32,
    /// The air sound travels through: the scene's, until
    /// `Engine::set_atmosphere` changes it.
    pub temperature_c: f32,
    /// Relative humidity (%).
    pub humidity_pct: f32,
    /// Where the scene puts the listener now: where it was loaded, or where
    /// it was moved since. Which way it faces is in telemetry.
    pub listener: Position,
    /// The scene's sounds, each with the slot it plays from. Their order is
    /// the scene file's, with sounds added since at the end; it is not slot
    /// order once a sound has been removed and another added.
    pub sources: Vec<SourceDescription>,
    /// The room's loudspeakers. Which of them the output in use plays
    /// through is in telemetry: none on headphones.
    pub speakers: Vec<Speaker>,
}

/// One loudspeaker of the room.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Speaker {
    /// The output channel it plays; `Telemetry::peaks_db` is counted by it.
    pub channel: u16,
    /// Its name in a 5.1 room: L, R, C, Ls or Rs.
    pub name: String,
    pub position: Position,
}

/// One source as it is now.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SourceDescription {
    /// What the sound is known by in the scene file. Unlike its slot, it
    /// stays the same when sounds around it are added or removed.
    pub id: String,
    /// The engine voice it plays from: what every call that takes a `slot`
    /// means, and what telemetry reports it under.
    pub slot: u16,
    /// How wide it goes out, in degrees: 360 is all round. A field ignores it.
    pub width: f32,
    /// How much is heard straight behind it, in dB below the front; −60 is
    /// nothing. It only matters when the width is less than all round.
    pub behind: f32,
    pub name: String,
    /// The family it belongs to in a list, when the scene gives it one.
    pub group: Option<String>,
    /// UI color as `#rrggbb`.
    pub color: String,
    pub position: Position,
    /// Level at `reference_distance` (dB SPL): the scene's, until
    /// `Engine::set_source_level` changes it.
    pub spl: f32,
    /// Distance at which `spl` applies (m).
    pub reference_distance: f32,
    /// How the source occupies the acoustic scene.
    pub emitter_kind: EmitterKind,
    /// Visual half-angle of a directional lobe in radians, when present.
    pub beam_half_angle: Option<f32>,
    /// How wide it sounds, from 0 (a point) to 1 (all around). A field
    /// ignores it.
    pub size: f32,
    /// The arc an ambient sound arrives from, in degrees, centred on the
    /// side its place lies on: 360 all round. A point ignores it.
    pub arc: f32,
    /// Initial forward vector in world coordinates.
    pub facing: Position,
    /// What it can utter, in the order `SourceTelemetry::take` counts them.
    /// One for a recording that loops, none for a generator.
    pub takes: Vec<Take>,
    /// How long it rests between two takes, or `None` when it plays all the
    /// time.
    pub gap: Option<Gap>,
    /// How many times running one take is uttered before another is chosen.
    pub bout: Bout,
    /// How it follows the place's conditions.
    pub follows: Vec<Follow>,
}

/// A setting tied to a condition: `from` when the condition is 0, `to` when
/// it is 1, a straight line between.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Follow {
    /// `wind`, `rain`, `storm` or `daylight`.
    pub condition: String,
    /// `activity`, `level` or a generator's own setting such as `intensity`.
    pub setting: String,
    pub from: f32,
    pub to: f32,
    /// The part of the condition over which the setting moves, 0 to 1.
    pub over: Band,
}

/// The fewest and most times running one take is uttered before another is
/// chosen: 1 and 1 changes every time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct Bout {
    pub fewest: u32,
    pub most: u32,
}

/// One thing a sound can utter, and how often it is chosen next to the
/// others: a take with chance 3 three times as often as one with chance 1.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Take {
    pub name: String,
    pub chance: f32,
}

/// The shortest and longest rest between two takes (s).
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct Gap {
    pub shortest: f32,
    pub longest: f32,
}

/// Whether the engine renders a source as a localized object or a diffuse field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EmitterKind {
    /// A localized sound with propagation paths.
    Object,
    /// Environmental sound without point-source attenuation.
    Field,
}

impl From<atrium_core::source::EmitterKind> for EmitterKind {
    fn from(kind: atrium_core::source::EmitterKind) -> Self {
        match kind {
            atrium_core::source::EmitterKind::Object => Self::Object,
            atrium_core::source::EmitterKind::Field => Self::Field,
        }
    }
}

impl From<atrium::config::SceneDescription> for SceneDescription {
    fn from(scene: atrium::config::SceneDescription) -> Self {
        Self {
            width: scene.width,
            depth: scene.depth,
            temperature_c: scene.temperature_c,
            humidity_pct: scene.humidity_pct,
            listener: scene.listener.into(),
            sources: scene.sources.into_iter().map(Into::into).collect(),
            speakers: scene
                .speakers
                .into_iter()
                .map(|speaker| Speaker {
                    channel: speaker.channel,
                    name: atrium_core::speaker::channel_name(speaker.channel as usize).to_owned(),
                    position: speaker.position.into(),
                })
                .collect(),
        }
    }
}

impl From<atrium::config::SourceDescription> for SourceDescription {
    fn from(source: atrium::config::SourceDescription) -> Self {
        Self {
            id: source.id,
            slot: source.slot,
            width: source.width,
            behind: source.behind,
            name: source.name,
            group: source.group,
            color: source.color,
            position: source.position.into(),
            spl: source.spl,
            reference_distance: source.reference_distance,
            emitter_kind: source.emitter_kind.into(),
            beam_half_angle: source.beam_half_angle,
            size: source.size,
            arc: source.arc,
            facing: source.facing.into(),
            takes: source
                .takes
                .into_iter()
                .map(|take| Take {
                    name: take.name,
                    chance: take.chance,
                })
                .collect(),
            gap: source
                .gap
                .map(|[shortest, longest]| Gap { shortest, longest }),
            bout: Bout {
                fewest: source.bout[0],
                most: source.bout[1],
            },
            follows: source
                .follows
                .into_iter()
                .map(|follow| Follow {
                    condition: follow.condition,
                    setting: follow.setting,
                    from: follow.from,
                    to: follow.to,
                    over: follow.over.map_or(
                        Band {
                            low: 0.0,
                            high: 1.0,
                        },
                        |[low, high]| Band { low, high },
                    ),
                })
                .collect(),
        }
    }
}

/// Which parts of the render reach the output, for listening to one at a
/// time. With every part and effect on and no channel alone, the scene is
/// heard as it is. It belongs to the listener, like a mute: it is not saved
/// with the scene and undo leaves it alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct Probe {
    /// The sound that travels straight from a source.
    pub direct: bool,
    /// The first bounces off the environment's faces.
    pub reflections: bool,
    /// The late reverb; heard alone it is still fed by the direct sound.
    pub reverb: bool,
    /// Ambient sounds, heard everywhere.
    pub ambient: bool,
    /// Treble lost to the air over distance.
    pub air_absorption: bool,
    /// The wavering of a far sound in wind-stirred, sunlit air.
    pub moving_air: bool,
    /// The ground's effect on a path over it.
    pub ground_effect: bool,
    /// The colour a wall gives a reflection.
    pub wall_colouring: bool,
    /// What a head does to a sound inside about a metre: the near ear hears
    /// it louder, most of all in the bass. Headphones only.
    pub near_field: bool,
    /// The headphone equalisation: the measured head's own loudspeaker and
    /// ear canal divided out of what reaches the ears. Headphones only.
    pub headphone_equalisation: bool,
    /// One output channel heard alone, as `Telemetry::peaks_db` counts them:
    /// a loudspeaker, or an ear.
    pub only_channel: Option<u8>,
}

impl From<atrium_core::probe::Probe> for Probe {
    fn from(probe: atrium_core::probe::Probe) -> Self {
        Self {
            direct: probe.direct,
            reflections: probe.reflections,
            reverb: probe.reverb,
            ambient: probe.ambient,
            air_absorption: probe.air_absorption,
            moving_air: probe.moving_air,
            ground_effect: probe.ground_effect,
            wall_colouring: probe.wall_colouring,
            near_field: probe.near_field,
            headphone_equalisation: probe.headphone_equalisation,
            only_channel: probe.only_channel,
        }
    }
}

impl From<Probe> for atrium_core::probe::Probe {
    fn from(probe: Probe) -> Self {
        Self {
            direct: probe.direct,
            reflections: probe.reflections,
            reverb: probe.reverb,
            ambient: probe.ambient,
            air_absorption: probe.air_absorption,
            moving_air: probe.moving_air,
            ground_effect: probe.ground_effect,
            wall_colouring: probe.wall_colouring,
            near_field: probe.near_field,
            headphone_equalisation: probe.headphone_equalisation,
            only_channel: probe.only_channel,
        }
    }
}

/// How loud the scene's direct sound is over a patch of ground, for a map
/// to colour. A guide to reach: echoes, reverb and the air are left out.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LoudnessMap {
    /// The first point, in world coordinates; its height is every point's.
    pub corner: Position,
    /// Distance between neighbouring points (m).
    pub cell: f32,
    /// Points along +x and along +y.
    pub columns: u32,
    pub rows: u32,
    /// Level at each point (dB SPL), row by row from the corner: `columns`
    /// values going east, then the next row to the north. 0 is silence.
    pub levels_db: Vec<f32>,
}

/// What the engine is doing right now.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Telemetry {
    /// Live sources only; a removed source leaves its slot out.
    pub sources: Vec<SourceTelemetry>,
    /// Peak of each output channel in the last rendered buffer (dBFS).
    pub peaks_db: Vec<f32>,
    /// The output being rendered.
    pub output: Output,
    /// The output channels it plays through loudspeakers: none on
    /// headphones, and on a 5.1 room only those its channels use.
    pub channels_in_use: Vec<u16>,
    /// Master gain in effect (linear, 0–1).
    pub volume: f32,
    /// The listener's current horizontal forward vector. It turns with the
    /// head while a tracker is sending.
    pub listener_facing: Position,
    /// Whether a head tracker is chosen and whether it is sending. A frame
    /// alone cannot know this; `Engine::latest_telemetry` fills it in.
    pub head_tracking: HeadTrackingStatus,
}

/// One live source's state.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SourceTelemetry {
    pub slot: u16,
    pub position: Position,
    /// Total gain from the source to the listener (dB): add the source's
    /// `spl` for the level heard. Floored at −120 dB.
    pub gain_db: f32,
    pub muted: bool,
    /// The current horizontal forward vector.
    pub facing: Position,
    /// Short-time measurements of the mono source before spatial effects.
    pub signal: Option<SourceSignal>,
    /// Which of its takes it is uttering, counted from 0, or nothing while
    /// it rests between two. A source that never rests is always on take 0.
    pub take: Option<u8>,
}

/// Frequency balance and strength measured from the source audio.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SourceSignal {
    /// RMS relative to this source's calibration level.
    pub relative_rms: f32,
    /// Fraction of the measured power below roughly 350 Hz.
    pub low_share: f32,
    /// Fraction above roughly 2.4 kHz. The remainder is midrange.
    pub high_share: f32,
}

impl From<&TelemetryFrame> for Telemetry {
    fn from(frame: &TelemetryFrame) -> Self {
        let source_count = (frame.source_count as usize).min(frame.sources.len());
        let channel_count = (frame.channel_count as usize).min(frame.channel_peaks.len());
        Self {
            sources: (0..source_count)
                .filter(|slot| frame.active_mask & (1 << slot) != 0)
                .map(|slot| {
                    let source = &frame.sources[slot];
                    SourceTelemetry {
                        slot: slot as u16,
                        position: Position {
                            x: source.x,
                            y: source.y,
                            z: source.z,
                        },
                        gain_db: if source.gain_db.is_finite() {
                            source.gain_db.max(SILENT_GAIN_DB)
                        } else {
                            SILENT_GAIN_DB
                        },
                        muted: source.is_muted,
                        facing: Position {
                            x: source.orientation_x,
                            y: source.orientation_y,
                            z: 0.0,
                        },
                        signal: source.signal.map(|signal| SourceSignal {
                            relative_rms: signal.relative_rms,
                            low_share: signal.low_share,
                            high_share: signal.high_share,
                        }),
                        take: source.sounding_take,
                    }
                })
                .collect(),
            peaks_db: frame.channel_peaks[..channel_count]
                .iter()
                .map(|peak| atrium::engine::telemetry::peak_db(*peak))
                .collect(),
            output: Output {
                render_mode: frame.render_mode.into(),
                channel_mode: frame.channel_mode.into(),
            },
            channels_in_use: match frame.render_mode {
                atrium_core::speaker::RenderMode::Hrtf => Vec::new(),
                _ => frame
                    .channel_mode
                    .active_channels()
                    .iter()
                    .map(|channel| *channel as u16)
                    .collect(),
            },
            volume: frame.master_gain,
            listener_facing: Position {
                x: frame.listener_yaw.cos(),
                y: frame.listener_yaw.sin(),
                z: 0.0,
            },
            head_tracking: HeadTrackingStatus::Off,
        }
    }
}

/// Why an engine call failed.
#[derive(Debug, uniffi::Error)]
#[uniffi(flat_error)]
pub enum EngineError {
    /// The scene or a file it references could not be loaded or built.
    Scene(String),
    /// The audio output device could not be opened.
    Audio(String),
    /// The audio thread has not drained earlier commands; this one was dropped.
    CommandQueueFull,
    /// The output is not one of `Engine::outputs()` for this scene and device.
    UnplayableOutput,
    /// The head tracker could not start, or is not one of `head_trackers()`.
    HeadTracking(String),
    /// Every voice of the engine is taken, so no further sound can play.
    NoFreeVoice,
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scene(reason) => write!(formatter, "scene could not be built: {reason}"),
            Self::Audio(reason) => write!(formatter, "audio output could not start: {reason}"),
            Self::CommandQueueFull => write!(formatter, "engine command queue is full"),
            Self::UnplayableOutput => {
                write!(formatter, "this scene cannot play on that output here")
            }
            Self::HeadTracking(reason) => {
                write!(formatter, "head tracking could not start: {reason}")
            }
            Self::NoFreeVoice => write!(formatter, "every voice is taken"),
        }
    }
}

impl std::error::Error for EngineError {}

#[cfg(test)]
mod tests {
    use super::*;
    use atrium_core::telemetry::SourceTelemetry as FrameSource;

    fn frame_with_slots(active_mask: u16, source_count: u8) -> TelemetryFrame {
        let mut frame = TelemetryFrame {
            source_count,
            active_mask,
            channel_count: 2,
            ..TelemetryFrame::default()
        };
        for slot in 0..source_count as usize {
            frame.sources[slot] = FrameSource {
                x: slot as f32,
                y: 10.0 + slot as f32,
                z: 1.5,
                ..FrameSource::default()
            };
        }
        frame.channel_peaks[0] = 1.0;
        frame.channel_peaks[1] = 0.0;
        frame
    }

    #[test]
    fn removed_slots_are_left_out_and_keep_their_numbers() {
        let telemetry = Telemetry::from(&frame_with_slots(0b101, 3));
        let slots: Vec<u16> = telemetry.sources.iter().map(|source| source.slot).collect();
        assert_eq!(slots, [0, 2]);
        assert_eq!(
            telemetry.sources[1].position,
            Position {
                x: 2.0,
                y: 12.0,
                z: 1.5
            }
        );
    }

    #[test]
    fn an_inaudible_source_reports_the_gain_floor_not_infinity() {
        let mut frame = frame_with_slots(0b11, 2);
        frame.sources[0].gain_db = f32::NEG_INFINITY;
        frame.sources[1].gain_db = -12.5;
        frame.sources[1].is_muted = true;
        let telemetry = Telemetry::from(&frame);
        assert_eq!(telemetry.sources[0].gain_db, SILENT_GAIN_DB);
        assert_eq!(telemetry.sources[1].gain_db, -12.5);
        assert!(telemetry.sources[1].muted && !telemetry.sources[0].muted);
    }

    #[test]
    fn peaks_use_the_engine_meter_scale() {
        let telemetry = Telemetry::from(&frame_with_slots(0b1, 1));
        assert_eq!(telemetry.peaks_db.len(), 2);
        assert_eq!(telemetry.peaks_db[0], 0.0, "full scale is 0 dBFS");
        assert_eq!(
            telemetry.peaks_db[1],
            atrium::engine::telemetry::peak_db(0.0),
            "silence is the same finite floor the WebSocket reports"
        );
    }

    /// The engine's heading is an angle (0 = +X, counter-clockwise). The map
    /// gets a vector in the same axes as the sound positions, so it needs no
    /// angle convention: a listener facing north (π/2) faces +Y.
    #[test]
    fn the_listener_heading_becomes_a_forward_vector() {
        let mut frame = frame_with_slots(0, 0);
        frame.listener_yaw = std::f32::consts::FRAC_PI_2;
        let telemetry = Telemetry::from(&frame);
        assert!(telemetry.listener_facing.x.abs() < 1e-6);
        assert!((telemetry.listener_facing.y - 1.0).abs() < 1e-6);
        assert_eq!(telemetry.listener_facing.z, 0.0);
        assert_eq!(
            telemetry.head_tracking,
            HeadTrackingStatus::Off,
            "a frame alone never claims a tracker"
        );
    }

    #[test]
    fn position_round_trips_through_the_engine_vector() {
        let position = Position {
            x: 1.25,
            y: -3.5,
            z: 0.75,
        };
        assert_eq!(Position::from(Vec3::from(position)), position);
    }
}

/// How the place's conditions move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Story {
    /// They stay in the middle of their bands.
    Held,
    /// They wander slowly inside their bands.
    Drifting,
    /// They drift, and now and then a storm passes.
    PassingStorm,
}

impl From<atrium::plan::Story> for Story {
    fn from(story: atrium::plan::Story) -> Self {
        match story {
            atrium::plan::Story::Held => Self::Held,
            atrium::plan::Story::Drifting => Self::Drifting,
            atrium::plan::Story::PassingStorm => Self::PassingStorm,
        }
    }
}

impl From<Story> for atrium::plan::Story {
    fn from(story: Story) -> Self {
        match story {
            Story::Held => Self::Held,
            Story::Drifting => Self::Drifting,
            Story::PassingStorm => Self::PassingStorm,
        }
    }
}

/// The lowest and highest a condition goes, each from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct Band {
    pub low: f32,
    pub high: f32,
}

/// A scene's weather: the story and the band of each condition.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Weather {
    pub story: Story,
    pub wind: Band,
    pub rain: Band,
    pub storm: Band,
    pub daylight: Band,
    /// The compass bearing the wind comes from, in degrees: 0 north, 90
    /// east, 0 up to 360.
    pub wind_from: f32,
}

/// Every condition at one moment, each from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct Conditions {
    pub wind: f32,
    pub rain: f32,
    pub storm: f32,
    pub daylight: f32,
}

impl From<atrium::plan::Conditions> for Conditions {
    fn from(conditions: atrium::plan::Conditions) -> Self {
        use atrium::plan::Condition;
        Self {
            wind: conditions.get(Condition::Wind),
            rain: conditions.get(Condition::Rain),
            storm: conditions.get(Condition::Storm),
            daylight: conditions.get(Condition::Daylight),
        }
    }
}

/// A take the plan has decided, still to come.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct PlannedTake {
    /// The voice it will sound from.
    pub slot: u16,
    /// Which take, counted from 0.
    pub take: u8,
    /// How long from now it starts (s).
    pub starts_in: f32,
    /// How long it lasts (s).
    pub seconds: f32,
}

/// 50 ms of what was heard, measured as the scene played: the analyser's
/// definitions (`crates/analysis`), the same as a render's report.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AnalysisColumn {
    /// When the column's first sample sounded, in the time
    /// `Engine::scene_time` reports: it lands under the playhead that played
    /// it. After a seek, columns carry the new times.
    pub scene_seconds: f64,
    /// The band level of each of `AnalysisScale::row_edges_hertz`'s rows,
    /// lowest first, from 0 at `spectrum_floor_db` to 255 at
    /// `spectrum_top_db` (dB SPL). The channels combine as the report's
    /// overall level: two ears averaged, speakers summed.
    pub spectrum: Vec<u8>,
    /// Per output channel, counted as `Telemetry::peaks_db`, A-weighted (dB
    /// SPL): the level over the column (LAeq), and the largest FAST level in
    /// it (LAF).
    pub leq_db: Vec<f32>,
    pub laf_db: Vec<f32>,
    /// The same for every channel together, as an `analyse` report's
    /// overall level: two ears' powers averaged, speakers' powers summed.
    pub leq_overall_db: f32,
    pub laf_overall_db: f32,
    /// Whether something started in the column.
    pub onset: bool,
    /// 1 − IACC over 500 Hz to 2 kHz (0 one source, 1 diffuse) and the ILD,
    /// right minus left (dB), of the 100 ms that end with the column, for
    /// the ears or the speakers' L and R; nothing on other outputs or while
    /// either is silent.
    pub width: Option<f32>,
    pub ild_db: Option<f32>,
}

/// How to read `AnalysisColumn`s, from the engine, so an axis is not
/// labelled from a copy.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AnalysisScale {
    /// The dB SPL a spectrum value of 0 and of 255 stand for; between them
    /// the scale is linear in dB.
    pub spectrum_floor_db: f32,
    pub spectrum_top_db: f32,
    /// The spectrum rows' edges (Hz), lowest first: one more than there are
    /// rows, row r from edge r to edge r + 1, log-spaced 20 Hz to 20 kHz.
    pub row_edges_hertz: Vec<f32>,
    /// The time a column covers (s); columns follow each other this far
    /// apart.
    pub column_seconds: f32,
}
