//! How one sound is heard: its level, where it stands, which way it faces,
//! its beam, its size, whether it is a point or ambient, and the arc an
//! ambient sound arrives from.

use atrium::config::{bearing_for_heading, radiation_of};
use atrium_core::commands::Command;
use atrium_core::directivity::SILENT_BEHIND_DB;
use atrium_core::source::{ALL_ROUND_DEGREES, NARROWEST_ARC_DEGREES};

use super::{lock, rounded, sound_in, Engine, Live};
use crate::api::{EmitterKind, EngineError, Position};

/// The narrowest beam a sound can be given, and all round (degrees).
const MIN_WIDTH: f32 = 10.0;
pub(super) const ALL_ROUND: f32 = 360.0;

#[uniffi::export]
impl Engine {
    /// Set the level of the source in `slot`, in dB SPL at its reference
    /// distance. `scene()` reports the new level from then on. A slot that
    /// holds no source, or a level that is not a number, changes nothing.
    pub fn set_source_level(&self, slot: u16, level: f32) -> Result<(), EngineError> {
        let mut scene = lock(&self.scene);
        if !level.is_finite() {
            return Ok(());
        }
        let Some(entity) = scene.entity_in(slot) else {
            return Ok(());
        };
        let level = rounded(level, 10.0);
        entity.sound.level = Some(level);
        self.play_level(&mut scene, slot, level)
    }

    /// Move the source in `slot` to `position`. `scene()` reports it there
    /// from then on, as where the source was placed; telemetry has where it
    /// is, which differs for a source that moves by itself.
    pub fn set_source_position(&self, slot: u16, position: Position) -> Result<(), EngineError> {
        let mut scene = lock(&self.scene);
        self.play_position(&mut scene, slot, position)?;
        let place = self.place_rounded(position);
        if let Some(entity) = scene.entity_in(slot) {
            entity.place = place;
        }
        Ok(())
    }

    /// Change how the source in `slot` radiates: `width` degrees wide, from
    /// a narrow beam up to 360 (all round), with the sound straight behind
    /// it `behind` dB below the front, −60 or lower for nothing. `scene()`
    /// reports both and the beam they give from then on. A field has no
    /// beam, whatever its width.
    pub fn set_source_beam(&self, slot: u16, width: f32, behind: f32) -> Result<(), EngineError> {
        if !(width.is_finite() && behind.is_finite()) {
            return Ok(());
        }
        let width = rounded(width.clamp(MIN_WIDTH, ALL_ROUND), 1.0);
        let behind = rounded(behind.clamp(SILENT_BEHIND_DB, 0.0), 1.0);
        let mut scene = lock(&self.scene);
        if let Some(entity) = scene.entity_in(slot) {
            entity.shape.width = Some(width);
            entity.shape.behind = Some(behind);
        }
        self.play_beam(&mut scene, slot, width, behind)
    }

    /// Make the source in `slot` sound `size` wide, from 0 (a point) to 1
    /// (all around). A wider sound spreads over more loudspeakers and feeds
    /// less of the room's reverb; from 0.75 it feeds none. `scene()` reports
    /// it from then on. A field ignores its size.
    pub fn set_source_size(&self, slot: u16, size: f32) -> Result<(), EngineError> {
        if !size.is_finite() {
            return Ok(());
        }
        let size = rounded(size.clamp(0.0, 1.0), 100.0);
        let mut scene = lock(&self.scene);
        if let Some(entity) = scene.entity_in(slot) {
            entity.shape.size = Some(size);
        }
        self.play_size(&mut scene, slot, size)
    }

    /// Make the ambient source in `slot` arrive from an arc `degrees` wide,
    /// centred on the side its place lies on: from 10 (narrow) to 360 (all
    /// round), in whole degrees. A shoreline or a tree line is heard from
    /// one side; moving the sound's place moves the arc, and its level stays
    /// its level. `scene()` reports it from then on, and the scene file
    /// keeps it (all round is left out). A point ignores it. On headphones a
    /// new width is heard once its filters are designed, a fraction of a
    /// second later.
    pub fn set_source_arc(&self, slot: u16, degrees: f32) -> Result<(), EngineError> {
        if !degrees.is_finite() {
            return Ok(());
        }
        let degrees = degrees
            .clamp(NARROWEST_ARC_DEGREES, ALL_ROUND_DEGREES)
            .round();
        let mut scene = lock(&self.scene);
        if let Some(entity) = scene.entity_in(slot) {
            entity.shape.arc = (degrees < ALL_ROUND_DEGREES).then_some(degrees);
        }
        self.play_arc(&mut scene, slot, degrees)
    }

    /// Make the source in `slot` ambient, heard everywhere as the air of the
    /// place, or a point again. An ambient sound has no distance, direction,
    /// beam or echo: its level is what is heard wherever the listener
    /// stands, so one that stood far off becomes much louder. This builds
    /// the sound again in its voice, which takes a moment, like `add_sound`,
    /// and a recording starts over.
    pub fn set_source_ambient(&self, slot: u16, ambient: bool) -> Result<(), EngineError> {
        let mut edits = lock(&self.edits);
        let mut scene = lock(&self.scene);
        let Some(entity) = scene.entity_in(slot) else {
            return Ok(());
        };
        let before = entity.shape.ambient;
        entity.shape.ambient = Some(ambient);
        let rebuilt = self.rebuild(&mut edits, &mut scene, slot);
        if rebuilt.is_err() {
            if let Some(entity) = scene.entity_in(slot) {
                entity.shape.ambient = before;
            }
        }
        rebuilt
    }

    /// Turn the source in `slot` to face along `facing`, a direction over
    /// the ground; its length and height are ignored. A direction with no
    /// length changes nothing.
    pub fn set_source_facing(&self, slot: u16, facing: Position) -> Result<(), EngineError> {
        let length = facing.x.hypot(facing.y);
        if !length.is_finite() || length < 1e-6 {
            return Ok(());
        }
        let mut scene = lock(&self.scene);
        let yaw = facing.y.atan2(facing.x);
        if let Some(entity) = scene.entity_in(slot) {
            entity.shape.facing = Some(rounded(bearing_for_heading(yaw), 1.0) % 360.0);
        }
        self.play_facing(&mut scene, slot, yaw)
    }
}

impl Engine {
    /// Set the level of the sound in `slot`.
    pub(super) fn play_level(
        &self,
        scene: &mut Live,
        slot: u16,
        level: f32,
    ) -> Result<(), EngineError> {
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        self.command(Command::SetSourceSpl {
            index: slot,
            spl: level,
        })?;
        source.spl = level;
        Ok(())
    }

    /// Move the sound in `slot` to `position`, in the world.
    pub(super) fn play_position(
        &self,
        scene: &mut Live,
        slot: u16,
        position: Position,
    ) -> Result<(), EngineError> {
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        self.command(Command::SetSourcePosition {
            index: slot,
            position: position.into(),
        })?;
        source.position = position;
        Ok(())
    }

    /// Give the sound in `slot` a beam `width` degrees wide, `behind` dB
    /// down straight behind it.
    pub(super) fn play_beam(
        &self,
        scene: &mut Live,
        slot: u16,
        width: f32,
        behind: f32,
    ) -> Result<(), EngineError> {
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        let (radiation, beam_half_angle) = radiation_of(width, behind);
        self.command(Command::SetSourceDirectivity {
            index: slot,
            pattern: radiation,
        })?;
        source.width = width;
        source.behind = behind;
        source.beam_half_angle = match source.emitter_kind {
            EmitterKind::Object => beam_half_angle,
            EmitterKind::Field => None,
        };
        Ok(())
    }

    /// Make the sound in `slot` sound `size` wide.
    pub(super) fn play_size(
        &self,
        scene: &mut Live,
        slot: u16,
        size: f32,
    ) -> Result<(), EngineError> {
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        self.command(Command::SetSourceSpread {
            index: slot,
            spread: size,
        })?;
        source.size = size;
        Ok(())
    }

    /// Make the sound in `slot` arrive from an arc `degrees` wide.
    pub(super) fn play_arc(
        &self,
        scene: &mut Live,
        slot: u16,
        degrees: f32,
    ) -> Result<(), EngineError> {
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        self.command(Command::SetSourceArc {
            index: slot,
            degrees,
        })?;
        source.arc = degrees;
        Ok(())
    }

    /// Turn the sound in `slot` to the heading `yaw` (radians from +x).
    pub(super) fn play_facing(
        &self,
        scene: &mut Live,
        slot: u16,
        yaw: f32,
    ) -> Result<(), EngineError> {
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        self.command(Command::SetSourceFacing { index: slot, yaw })?;
        source.facing = Position {
            x: yaw.cos(),
            y: yaw.sin(),
            z: 0.0,
        };
        Ok(())
    }
}
