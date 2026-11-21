//! How loud the scene is across the ground.

use atrium::audio::loudness::{level_map, Emitter, Grid};
use atrium::config::radiation_of;
use atrium_core::types::Vec3;

use super::{finite, lock, Engine};
use crate::api::{EmitterKind, LoudnessMap, Position};

/// The most points a loudness map has along one side.
const MAX_MAP_SIDE: u32 = 160;

#[uniffi::export]
impl Engine {
    /// How loud the scene is over the ground between `from` and `to`, at
    /// `from`'s height, on `columns` by `rows` points: all its sounds that
    /// are not silenced, or only the one in `only`. Each point adds up the
    /// direct sound of every source, from its level as `scene()` reports it,
    /// the scene's distance law and the source's beam.
    pub fn loudness(
        &self,
        from: Position,
        to: Position,
        columns: u32,
        rows: u32,
        only: Option<u16>,
    ) -> LoudnessMap {
        let columns = columns.clamp(1, MAX_MAP_SIDE);
        let rows = rows.clamp(1, MAX_MAP_SIDE);
        let (west, east) = (from.x.min(to.x), from.x.max(to.x));
        let (south, north) = (from.y.min(to.y), from.y.max(to.y));
        let corner = Position {
            x: west,
            y: south,
            z: from.z,
        };
        if !(finite(from) && finite(to)) {
            return LoudnessMap {
                corner,
                cell: 0.0,
                columns: 0,
                rows: 0,
                levels_db: Vec::new(),
            };
        }
        // Square cells that cover the wider side.
        let cell = ((east - west) / columns as f32).max((north - south) / rows as f32);
        let grid = Grid {
            // Each point stands in the middle of its cell.
            corner: Vec3::new(west + cell / 2.0, south + cell / 2.0, from.z),
            cell,
            columns: columns as usize,
            rows: rows as usize,
        };
        let scene = lock(&self.scene);
        let emitters: Vec<Emitter> = scene
            .description
            .sources
            .iter()
            .filter(|source| match only {
                Some(slot) => source.slot == slot,
                None => !scene.muted.contains(&source.slot),
            })
            .map(|source| Emitter {
                position: source.position.into(),
                level_db: source.spl,
                reference_distance: source.reference_distance,
                pattern: radiation_of(source.width, source.behind).0,
                facing: source.facing.into(),
                ambient: source.emitter_kind == EmitterKind::Field,
            })
            .collect();
        LoudnessMap {
            corner,
            cell,
            columns,
            rows,
            levels_db: level_map(&emitters, &self.distance, &grid),
        }
    }
}
