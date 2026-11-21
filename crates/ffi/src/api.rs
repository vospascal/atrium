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

/// The scene the engine is playing.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SceneDescription {
    /// Environment floor size along x (m).
    pub width: f32,
    /// Environment floor size along y (m).
    pub depth: f32,
    /// Listener position at load time.
    pub listener: Position,
    /// The scene's sounds, each with the slot it plays from. Their order is
    /// the scene file's, with sounds added since at the end; it is not slot
    /// order once a sound has been removed and another added.
    pub sources: Vec<SourceDescription>,
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
    /// Initial forward vector in world coordinates.
    pub facing: Position,
    /// What it can utter, in the order `SourceTelemetry::take` counts them.
    /// One for a recording that loops, none for a generator.
    pub takes: Vec<Take>,
    /// How long it rests between two takes, or `None` when it plays all the
    /// time.
    pub gap: Option<Gap>,
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
            listener: scene.listener.into(),
            sources: scene.sources.into_iter().map(Into::into).collect(),
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
        }
    }
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
