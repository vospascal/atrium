//! How loud a scene is across the ground: the level of its direct sound at
//! every point of a grid, for a map to colour.
//!
//! This is a guide to reach, not a measurement. It adds up, by energy, what
//! arrives straight from each sound: its level, the scene's distance law and
//! the sound's own beam, the same three the engine renders from. Echoes,
//! reverb, the ground and the air's absorption are left out. An ambient
//! sound is the same everywhere, as it is heard.

use atrium_core::directivity::{directivity_gain, DirectivityPattern};
use atrium_core::panner::distance_gain_at_model;
use atrium_core::types::Vec3;

use super::distance::DistanceModel;

/// The quietest level a cell reports (dB SPL): silence, and anything below.
pub const SILENT_DB: f32 = 0.0;

/// One sound, as far as its reach goes.
#[derive(Debug, Clone, Copy)]
pub struct Emitter {
    pub position: Vec3,
    /// Level at `reference_distance` (dB SPL).
    pub level_db: f32,
    pub reference_distance: f32,
    pub pattern: DirectivityPattern,
    /// Unit vector it faces along.
    pub facing: Vec3,
    /// Heard everywhere at `level_db`, whatever its position.
    pub ambient: bool,
}

/// A grid of points over the ground, all at one height: `columns` along +x
/// and `rows` along +y, `cell` metres apart, the first at `corner`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub corner: Vec3,
    pub cell: f32,
    pub columns: usize,
    pub rows: usize,
}

impl Grid {
    /// The point of `column` and `row`.
    pub fn point(&self, column: usize, row: usize) -> Vec3 {
        Vec3::new(
            self.corner.x + column as f32 * self.cell,
            self.corner.y + row as f32 * self.cell,
            self.corner.z,
        )
    }
}

/// The level of `emitters` together at `point` (dB SPL), never below
/// [`SILENT_DB`].
pub fn level_at(point: Vec3, emitters: &[Emitter], distance: &DistanceModel) -> f32 {
    let energy: f32 = emitters
        .iter()
        .map(|emitter| {
            let gain = if emitter.ambient {
                1.0
            } else {
                distance_gain_at_model(
                    point,
                    emitter.position,
                    emitter.reference_distance,
                    distance.max_distance,
                    distance.rolloff,
                    distance.model,
                ) * directivity_gain(emitter.position, emitter.facing, point, &emitter.pattern)
            };
            10.0_f32.powf(emitter.level_db / 10.0) * gain * gain
        })
        .sum();
    if energy <= 1.0 {
        SILENT_DB
    } else {
        10.0 * energy.log10()
    }
}

/// The level at every point of `grid`, row by row from its corner.
pub fn level_map(emitters: &[Emitter], distance: &DistanceModel, grid: &Grid) -> Vec<f32> {
    let mut levels = Vec::with_capacity(grid.columns * grid.rows);
    for row in 0..grid.rows {
        for column in 0..grid.columns {
            levels.push(level_at(grid.point(column, row), emitters, distance));
        }
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;
    use atrium_core::panner::DistanceModelType;

    fn inverse() -> DistanceModel {
        DistanceModel {
            ref_distance: 1.0,
            max_distance: 100.0,
            rolloff: 1.0,
            model: DistanceModelType::Inverse,
        }
    }

    fn point(level_db: f32, x: f32) -> Emitter {
        Emitter {
            position: Vec3::new(x, 0.0, 0.0),
            level_db,
            reference_distance: 1.0,
            pattern: DirectivityPattern::OMNI,
            facing: Vec3::new(1.0, 0.0, 0.0),
            ambient: false,
        }
    }

    /// The inverse law: the scene's level at 1 m, 20 dB less at 10 m.
    #[test]
    fn a_point_loses_20_db_per_tenfold_distance() {
        let robin = [point(70.0, 0.0)];
        let at = |x| level_at(Vec3::new(x, 0.0, 0.0), &robin, &inverse());
        assert!((at(1.0) - 70.0).abs() < 1e-3);
        assert!((at(10.0) - 50.0).abs() < 1e-3);
    }

    /// Two equal sounds at the same distance are 3.01 dB above one.
    #[test]
    fn sounds_add_by_energy() {
        let pair = [point(60.0, -1.0), point(60.0, 1.0)];
        let level = level_at(Vec3::ZERO, &pair, &inverse());
        assert!((level - 63.0103).abs() < 1e-3, "{level}");
    }

    /// An ambient sound is its level wherever it is heard from.
    #[test]
    fn an_ambient_sound_is_the_same_everywhere() {
        let mut rain = point(55.0, 0.0);
        rain.ambient = true;
        for x in [0.0, 3.0, 80.0] {
            let level = level_at(Vec3::new(x, 5.0, 0.0), &[rain], &inverse());
            assert!((level - 55.0).abs() < 1e-3);
        }
    }

    /// A beam with nothing behind it is silent straight behind, and as loud
    /// as an all-round sound straight ahead.
    #[test]
    fn a_beam_reaches_ahead_and_not_behind() {
        let mut horn = point(70.0, 0.0);
        horn.pattern = DirectivityPattern::beam(90.0, -60.0);
        let ahead = level_at(Vec3::new(10.0, 0.0, 0.0), &[horn], &inverse());
        let behind = level_at(Vec3::new(-10.0, 0.0, 0.0), &[horn], &inverse());
        assert!((ahead - 50.0).abs() < 1e-2, "{ahead}");
        assert_eq!(behind, SILENT_DB);
    }

    #[test]
    fn nothing_sounding_is_silence() {
        assert_eq!(level_at(Vec3::ZERO, &[], &inverse()), SILENT_DB);
    }

    /// Rows run along +y from the corner, columns along +x within a row.
    #[test]
    fn the_map_is_row_by_row_from_the_corner() {
        let grid = Grid {
            corner: Vec3::new(1.0, 0.0, 0.0),
            cell: 9.0,
            columns: 2,
            rows: 2,
        };
        let levels = level_map(&[point(70.0, 0.0)], &inverse(), &grid);
        assert_eq!(levels.len(), 4);
        assert!((levels[0] - 70.0).abs() < 1e-3, "1 m east");
        assert!((levels[1] - 50.0).abs() < 1e-3, "10 m east");
        assert!(levels[2] < levels[0] && levels[2] > levels[3]);
    }
}
