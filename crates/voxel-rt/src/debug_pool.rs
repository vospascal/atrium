//! **Test tools** that write water into the world at runtime: [`WaterPool`], a
//! swimmable bowl carved into terrain (E2b), and [`WaterBlob`], a free-standing
//! body hanging in open air (E6's isolated optics target).
//!
//! Not a stage and not a lever: a debug affordance. The generated island's water
//! is 0.6–1.75 m deep, all of it under the 1.44 m the body needs before it
//! [`Submersion::Swimming`](crate::character::Submersion::Swimming)s, so the
//! swim half of E2b could be tested in code but never *felt* in the app. This
//! carves a 5 m deep pool on demand instead.
//!
//! Two constraints shaped it:
//!
//! - **It goes through E2's edit pipeline, not through generation.** Every voxel
//!   is a [`Brickmap::set_voxel`] behind the world authority
//!   ([`crate::world_host::WorldHost::request_bulk_edit`]), so `voxel-core` is
//!   untouched: every recorded bench baseline and every pixel gate still
//!   describes the seed-1 island, and voxel-sandbox's world is unaffected.
//! - **Nothing makes water flow yet** (that is backlog B6), so removing the bed
//!   would leave a dry pit. The carve therefore *writes water* up to
//!   [`WATER_LEVEL`] rather than removing bed voxels and hoping.
//!
//! ## The pool's shape
//!
//! A round bowl with a graded shore, so you can WALK in and cross the
//! wade → swim threshold on foot instead of falling off a ledge:
//!
//! ```text
//!   natural terrain            waterline = WATER_LEVEL
//!        ____                        v
//!            \____                 ~~~~~~~~~~~~~~~~
//!                 \___            /                \
//!   shore band --------\_________/  water, 5 m deep \______  <- bed
//!   |<- 4 m ->|<---------- 8 m of water ---------->|
//! ```
//!
//! Both flanks are `smoothstep` ramps — flat at the top and the bottom, steepest
//! halfway — which keeps the worst gradient inside the body's 3-voxel auto-step
//! (the inner wall rises 1.9 voxels per voxel, the auto-step clears 3), so the
//! bowl is walkable in both directions. The bed profile is defined against
//! `WATER_LEVEL`, not against the local ground, so the water is 5 m deep wherever
//! it is carved; the shore band then rises from the waterline to whatever the
//! terrain actually is, which is what blends the excavation into the island.
//! (Trigger it on a hilltop and that band is steep — the swim is still there, the
//! stroll back out is not.)

use glam::Vec3;
use voxel_core::world::{Voxel, VOXEL_SIZE, WATER_LEVEL, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z};

use crate::brickmap::Brickmap;
use crate::material::material_is_empty_for_edits;
use crate::world_edit::{BulkEdit, VoxelSpan};

/// Water depth at the pool's centre, meters — 3.5x the 1.44 m swim threshold, so
/// there is depth to dive INTO rather than a threshold to hover on.
pub const POOL_DEPTH_METERS: f32 = 5.0;
/// Radius of the water surface, meters (8 m across): room to swim a few strokes
/// in any direction, and the wade → swim line lands ~2.5 m from the centre.
pub const POOL_WATER_RADIUS_METERS: f32 = 4.0;
/// Width of the graded shore outside the water, meters — the walk-in ramp from
/// the natural terrain down to the waterline.
pub const POOL_SHORE_WIDTH_METERS: f32 = 4.0;
/// Radius the carve touches at all, meters.
pub const POOL_RADIUS_METERS: f32 = POOL_WATER_RADIUS_METERS + POOL_SHORE_WIDTH_METERS;
/// How far in front of the eye the pool is centred, meters. Two metres clear of
/// the excavation, so the trigger never drops the body into its own hole and the
/// pool is something you walk *towards*.
pub const POOL_DISTANCE_AHEAD_METERS: f32 = POOL_RADIUS_METERS + 2.0;

/// A pool to carve, identified by the voxel column it is centred on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaterPool {
    pub centre_voxel_x: i32,
    pub centre_voxel_z: i32,
}

impl WaterPool {
    /// Centre the pool [`POOL_DISTANCE_AHEAD_METERS`] ahead of an eye looking
    /// along `forward` — the trigger's placement. Pitch is dropped (a pool is
    /// horizontal), and a straight-up/down look falls back to +X rather than
    /// carving at the feet.
    pub fn in_front_of(eye_position: Vec3, forward: Vec3) -> WaterPool {
        let mut direction = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
        if direction == Vec3::ZERO {
            direction = Vec3::X;
        }
        WaterPool::at_position(eye_position + direction * POOL_DISTANCE_AHEAD_METERS)
    }

    /// Centre the pool on the COLUMN of the voxel the crosshair is on — the same
    /// place a click edits. The voxel's height is discarded on purpose: the bowl's
    /// depth is measured from [`WATER_LEVEL`], so only its column matters.
    pub fn at_voxel(voxel: [i32; 3]) -> WaterPool {
        let margin = (POOL_RADIUS_METERS / VOXEL_SIZE).ceil() as i32 + 1;
        WaterPool {
            centre_voxel_x: voxel[0].clamp(margin, WORLD_SIZE_X as i32 - 1 - margin),
            centre_voxel_z: voxel[2].clamp(margin, WORLD_SIZE_Z as i32 - 1 - margin),
        }
    }

    /// Centre the pool on a world position, clamped so the whole bowl stays
    /// inside the world box.
    pub fn at_position(position: Vec3) -> WaterPool {
        let margin = (POOL_RADIUS_METERS / VOXEL_SIZE).ceil() as i32 + 1;
        WaterPool {
            centre_voxel_x: ((position.x / VOXEL_SIZE).floor() as i32)
                .clamp(margin, WORLD_SIZE_X as i32 - 1 - margin),
            centre_voxel_z: ((position.z / VOXEL_SIZE).floor() as i32)
                .clamp(margin, WORLD_SIZE_Z as i32 - 1 - margin),
        }
    }

    /// World position of the water surface at the pool's centre — where the
    /// swimming test drops a body, and what the trigger logs.
    pub fn surface_centre(&self) -> Vec3 {
        Vec3::new(
            (self.centre_voxel_x as f32 + 0.5) * VOXEL_SIZE,
            (WATER_LEVEL + 1) as f32 * VOXEL_SIZE,
            (self.centre_voxel_z as f32 + 0.5) * VOXEL_SIZE,
        )
    }

    /// Highest voxel that stays terrain in a column `radius_meters` out, given
    /// the terrain height the shore band has to rise to meet — the bowl's
    /// profile, in one place.
    fn bed_voxel_y(&self, radius_meters: f32, rim_ground_voxel_y: i32) -> i32 {
        let bottom_voxel_y = WATER_LEVEL - (POOL_DEPTH_METERS / VOXEL_SIZE).round() as i32;
        if radius_meters <= POOL_WATER_RADIUS_METERS {
            // Inside the water: bottom at the centre, rising to the waterline.
            let ramp = smoothstep(radius_meters / POOL_WATER_RADIUS_METERS);
            bottom_voxel_y + ((WATER_LEVEL - bottom_voxel_y) as f32 * ramp).round() as i32
        } else {
            // The shore band: the waterline rising to meet the terrain.
            let ramp =
                smoothstep((radius_meters - POOL_WATER_RADIUS_METERS) / POOL_SHORE_WIDTH_METERS);
            WATER_LEVEL + ((rim_ground_voxel_y - WATER_LEVEL) as f32 * ramp).round() as i32
        }
    }

    /// Terrain height the shore band rises to in one direction: the column on the
    /// RIM, which the carve never writes to.
    ///
    /// Why the rim and not each column's own surface: the profile has to be a
    /// function of terrain the carve does not change, or the tool is not
    /// idempotent — pressing the key twice would re-read the shore it just cut
    /// and erode it another step, and a third press again, until the bank was a
    /// flat shelf at the waterline. Falls back to the column's own surface at the
    /// island's edge, where the rim column is void.
    fn rim_ground_voxel_y(
        &self,
        brickmap: &Brickmap,
        offset_x: f32,
        offset_z: f32,
        radius_meters: f32,
        column_top_voxel_y: i32,
    ) -> i32 {
        if radius_meters <= f32::EPSILON {
            return column_top_voxel_y;
        }
        let scale = POOL_RADIUS_METERS / (radius_meters * VOXEL_SIZE);
        let rim_x = self.centre_voxel_x + (offset_x * scale).round() as i32;
        let rim_z = self.centre_voxel_z + (offset_z * scale).round() as i32;
        brickmap
            .column_top_occupied_voxel(rim_x, rim_z)
            .unwrap_or(column_top_voxel_y)
    }
}

impl BulkEdit for WaterPool {
    /// Two spans per column at most: water from the bed up to the waterline, air
    /// from there up through whatever the terrain (or the tree standing in it)
    /// had. Columns with no terrain at all are skipped, so the carve can never
    /// hang water over the void off the island's edge.
    fn spans(&self, brickmap: &Brickmap) -> Vec<VoxelSpan> {
        let radius_voxels = (POOL_RADIUS_METERS / VOXEL_SIZE).ceil() as i32;
        let mut spans = Vec::new();
        for z in self.centre_voxel_z - radius_voxels..=self.centre_voxel_z + radius_voxels {
            for x in self.centre_voxel_x - radius_voxels..=self.centre_voxel_x + radius_voxels {
                let offset_x = (x - self.centre_voxel_x) as f32 * VOXEL_SIZE;
                let offset_z = (z - self.centre_voxel_z) as f32 * VOXEL_SIZE;
                let radius_meters = (offset_x * offset_x + offset_z * offset_z).sqrt();
                if radius_meters > POOL_RADIUS_METERS {
                    continue;
                }
                let Some(column_top_voxel_y) = brickmap.column_top_occupied_voxel(x, z) else {
                    continue;
                };
                let bed_voxel_y = self.bed_voxel_y(
                    radius_meters,
                    self.rim_ground_voxel_y(
                        brickmap,
                        offset_x,
                        offset_z,
                        radius_meters,
                        column_top_voxel_y,
                    ),
                );
                if bed_voxel_y < WATER_LEVEL {
                    spans.push(VoxelSpan {
                        x,
                        z,
                        y_from: bed_voxel_y + 1,
                        y_to: WATER_LEVEL,
                        material: Voxel::Water,
                    });
                }
                let air_from = bed_voxel_y.max(WATER_LEVEL) + 1;
                if column_top_voxel_y >= air_from {
                    spans.push(VoxelSpan {
                        x,
                        z,
                        y_from: air_from,
                        y_to: column_top_voxel_y,
                        material: Voxel::Air,
                    });
                }
            }
        }
        spans
    }

    fn label(&self) -> &'static str {
        "5 m test pool"
    }
}

/// Radius of the free-standing body, meters — the pool's own water radius, so
/// `Shift+P` spawns *the same size water* `P` carves.
pub const BLOB_RADIUS_METERS: f32 = POOL_WATER_RADIUS_METERS;
/// Height, meters — the pool's depth, for the same reason.
pub const BLOB_HEIGHT_METERS: f32 = POOL_DEPTH_METERS;
/// Where a blob lands when the crosshair hits nothing (aimed at open sky). Three
/// radii out: far enough to see the whole body against the sky, near enough to
/// walk to.
pub const BLOB_DISTANCE_AHEAD_METERS: f32 = BLOB_RADIUS_METERS * 3.0;

/// A free-standing body of water, centred on a voxel — the isolated test object
/// for the E6 water optics.
///
/// The pool is the wrong instrument for judging refraction: it is a bowl sunk
/// into terrain, so every ray that crosses its surface lands on a lit bed a few
/// metres below, and the bed's own shading is mixed into every judgement about
/// the water. A body hanging in open air has *sky* behind it instead, which is
/// the clean read — and you can walk straight through it, because water does not
/// block movement.
///
/// Two rules make it safe to fire anywhere:
///
/// - **It only fills what is already empty.** Air and liquid are overwritten,
///   terrain never is (the plan's "you expect to dig the earth but not the water"
///   rule, applied to a tool that *writes*). Aim at a hillside and the water
///   wraps the hill instead of eating it.
/// - **Nothing makes water flow yet** (backlog B6), so a blob spawned in mid-air
///   simply hangs there. That is the point rather than a defect: a cube of water
///   in the sky is the cheapest possible refraction test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaterBlob {
    pub centre_voxel_x: i32,
    pub centre_voxel_y: i32,
    pub centre_voxel_z: i32,
}

impl WaterBlob {
    /// Centre the body on the voxel the crosshair is on — the same place a click
    /// edits, which is the whole point of the trigger.
    pub fn at_voxel(voxel: [i32; 3]) -> WaterBlob {
        let horizontal_margin = (BLOB_RADIUS_METERS / VOXEL_SIZE).ceil() as i32 + 1;
        let vertical_margin = (BLOB_HEIGHT_METERS / VOXEL_SIZE).ceil() as i32 + 1;
        WaterBlob {
            centre_voxel_x: voxel[0].clamp(
                horizontal_margin,
                WORLD_SIZE_X as i32 - 1 - horizontal_margin,
            ),
            centre_voxel_y: voxel[1]
                .clamp(vertical_margin, WORLD_SIZE_Y as i32 - 1 - vertical_margin),
            centre_voxel_z: voxel[2].clamp(
                horizontal_margin,
                WORLD_SIZE_Z as i32 - 1 - horizontal_margin,
            ),
        }
    }

    /// Where the body goes when the crosshair hit nothing. Unlike the pool this
    /// keeps PITCH: looking up at open sky is exactly how you ask for a body of
    /// water hanging in the air, so throwing the pitch away would defeat the tool.
    pub fn in_front_of(eye_position: Vec3, forward: Vec3) -> WaterBlob {
        let direction = forward.normalize_or_zero();
        let centre = eye_position + direction * BLOB_DISTANCE_AHEAD_METERS;
        WaterBlob::at_voxel([
            (centre.x / VOXEL_SIZE).floor() as i32,
            (centre.y / VOXEL_SIZE).floor() as i32,
            (centre.z / VOXEL_SIZE).floor() as i32,
        ])
    }

    /// World position of the body's centre — what the trigger logs.
    pub fn centre(&self) -> Vec3 {
        Vec3::new(
            (self.centre_voxel_x as f32 + 0.5) * VOXEL_SIZE,
            (self.centre_voxel_y as f32 + 0.5) * VOXEL_SIZE,
            (self.centre_voxel_z as f32 + 0.5) * VOXEL_SIZE,
        )
    }
}

impl BulkEdit for WaterBlob {
    /// One span per unobstructed RUN in each column of the disc, so terrain
    /// inside the body's volume survives and the water closes around it.
    ///
    /// Runs rather than one span per column because a column may be interrupted
    /// more than once — a branch, an overhang, a boulder — and a single span
    /// would have to either stop at the first obstruction or write straight
    /// through it.
    fn spans(&self, brickmap: &Brickmap) -> Vec<VoxelSpan> {
        let radius_voxels = (BLOB_RADIUS_METERS / VOXEL_SIZE).ceil() as i32;
        let half_height_voxels = (BLOB_HEIGHT_METERS / VOXEL_SIZE * 0.5).round() as i32;
        let lowest_voxel_y = (self.centre_voxel_y - half_height_voxels).max(0);
        let highest_voxel_y =
            (self.centre_voxel_y + half_height_voxels).min(WORLD_SIZE_Y as i32 - 1);
        let mut spans = Vec::new();
        for z in self.centre_voxel_z - radius_voxels..=self.centre_voxel_z + radius_voxels {
            for x in self.centre_voxel_x - radius_voxels..=self.centre_voxel_x + radius_voxels {
                let offset_x = (x - self.centre_voxel_x) as f32 * VOXEL_SIZE;
                let offset_z = (z - self.centre_voxel_z) as f32 * VOXEL_SIZE;
                if (offset_x * offset_x + offset_z * offset_z).sqrt() > BLOB_RADIUS_METERS {
                    continue;
                }
                let mut run_start: Option<i32> = None;
                for y in lowest_voxel_y..=highest_voxel_y {
                    if material_is_empty_for_edits(brickmap.get(x, y, z)) {
                        run_start.get_or_insert(y);
                    } else if let Some(y_from) = run_start.take() {
                        spans.push(VoxelSpan {
                            x,
                            z,
                            y_from,
                            y_to: y - 1,
                            material: Voxel::Water,
                        });
                    }
                }
                if let Some(y_from) = run_start {
                    spans.push(VoxelSpan {
                        x,
                        z,
                        y_from,
                        y_to: highest_voxel_y,
                        material: Voxel::Water,
                    });
                }
            }
        }
        spans
    }

    fn label(&self) -> &'static str {
        "free-standing water body"
    }
}

/// Hermite ramp, flat at both ends — the reason the bowl has no lip to trip over
/// and no cliff at the waterline.
fn smoothstep(fraction: f32) -> f32 {
    let clamped = fraction.clamp(0.0, 1.0);
    clamped * clamped * (3.0 - 2.0 * clamped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::character::{BODY_HEIGHT_METERS, SWIM_SUBMERSION_FRACTION};
    use crate::material::material_is_liquid;
    use crate::world_edit::{apply_bulk, BulkEditRequest, WorldEditSettings};
    use voxel_core::world::VoxelWorld;

    /// The island the pool is carved into. NOTE: generates the full world — run
    /// the suite with `--release`.
    fn island() -> Brickmap {
        Brickmap::build(&VoxelWorld::generate(1234, 0.0))
    }

    fn carve(brickmap: &mut Brickmap, pool: WaterPool) -> crate::world_edit::WorldDelta {
        apply_bulk(
            brickmap,
            &BulkEditRequest {
                shape: Box::new(pool),
                light_grid: None,
                material_attributes: crate::cagi::MaterialAttributes::compiled(),
            },
            &WorldEditSettings::default(),
        )
        .expect("carving a pool into the island changes something")
    }

    /// The blob's one safety rule: it FILLS, it never excavates. Every voxel it
    /// writes must have been empty-for-edits (air or liquid) beforehand, so
    /// firing it into a hillside wraps the hill instead of eating it.
    ///
    /// Checked against a centre buried in terrain, which is the case that would
    /// fail if the spans were one-per-column instead of one-per-run.
    #[test]
    fn a_water_body_never_replaces_terrain() {
        let brickmap = island();
        let ground_voxel_y = brickmap
            .column_top_occupied_voxel(420, 560)
            .expect("the test column is on the island");
        // Centre it ON the ground, so half the volume is inside terrain.
        let blob = WaterBlob::at_voxel([420, ground_voxel_y, 560]);
        let spans = blob.spans(&brickmap);
        assert!(
            !spans.is_empty(),
            "a body on the ground must write something"
        );
        let mut written = 0_usize;
        for span in &spans {
            assert_eq!(span.material, Voxel::Water);
            assert!(span.y_from <= span.y_to, "empty span {span:?}");
            for y in span.y_from..=span.y_to {
                assert!(
                    material_is_empty_for_edits(brickmap.get(span.x, y, span.z)),
                    "the body would overwrite terrain at ({}, {y}, {})",
                    span.x,
                    span.z,
                );
                written += 1;
            }
        }
        // And it must be a real body, not a handful of stragglers above the rim.
        assert!(written > 10_000, "only {written} voxels of water");
    }

    /// Hanging in open air — the refraction test object. With nothing to interrupt
    /// any column, every column inside the disc becomes ONE full-height span, and
    /// applying it leaves swimmable water with sky on every side.
    #[test]
    fn a_water_body_in_open_air_is_one_unbroken_span_per_column() {
        let mut brickmap = island();
        let high_voxel_y = WORLD_SIZE_Y as i32 - 40;
        let blob = WaterBlob::at_voxel([420, high_voxel_y, 560]);
        let spans = blob.spans(&brickmap);
        let half_height_voxels = (BLOB_HEIGHT_METERS / VOXEL_SIZE * 0.5).round() as i32;
        let expected_height = half_height_voxels * 2 + 1;
        for span in &spans {
            assert_eq!(
                span.y_to - span.y_from + 1,
                expected_height,
                "column ({}, {}) is interrupted in open air",
                span.x,
                span.z,
            );
        }
        apply_bulk(
            &mut brickmap,
            &BulkEditRequest {
                shape: Box::new(blob),
                light_grid: None,
                material_attributes: crate::cagi::MaterialAttributes::compiled(),
            },
            &WorldEditSettings::default(),
        )
        .expect("spawning a body in open air changes something");
        assert!(material_is_liquid(brickmap.get(420, high_voxel_y, 560)));
        // Deep enough to swim in, measured the same way the pool's test measures.
        let swim_depth_voxels =
            (BODY_HEIGHT_METERS * SWIM_SUBMERSION_FRACTION / VOXEL_SIZE).ceil() as i32;
        assert!(
            half_height_voxels * 2 >= swim_depth_voxels,
            "a {BLOB_HEIGHT_METERS} m body is too shallow to swim in"
        );
        // Firing the identical body again must find nothing left to do.
        assert!(
            apply_bulk(
                &mut brickmap,
                &BulkEditRequest {
                    shape: Box::new(blob),
                    light_grid: None,
                    material_attributes: crate::cagi::MaterialAttributes::compiled(),
                },
                &WorldEditSettings::default(),
            )
            .is_none(),
            "the body is not idempotent"
        );
    }

    /// The whole point of the tool: the water at the centre must be deep enough
    /// to swim in, its surface must be the world's waterline, and the bowl must
    /// hold a proper bed under it (not a hole in the island).
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn the_pool_is_deep_enough_to_swim_in() {
        let mut brickmap = island();
        let pool = WaterPool {
            centre_voxel_x: 500,
            centre_voxel_z: 500,
        };
        carve(&mut brickmap, pool);

        assert!(
            material_is_liquid(brickmap.get(pool.centre_voxel_x, WATER_LEVEL, pool.centre_voxel_z)),
            "the pool's surface voxel is not water"
        );
        assert_eq!(
            brickmap.get(pool.centre_voxel_x, WATER_LEVEL + 1, pool.centre_voxel_z),
            0,
            "the water must stop at the waterline, not spill above it"
        );
        let bed_voxel_y = (0..WATER_LEVEL)
            .rev()
            .find(|y| {
                !material_is_liquid(brickmap.get(pool.centre_voxel_x, *y, pool.centre_voxel_z))
            })
            .expect("the pool has a bed");
        let depth_meters = (WATER_LEVEL - bed_voxel_y) as f32 * VOXEL_SIZE;
        assert!(
            (depth_meters - POOL_DEPTH_METERS).abs() < 0.2,
            "the pool is {depth_meters:.2} m deep, expected {POOL_DEPTH_METERS} m"
        );
        assert!(
            depth_meters > BODY_HEIGHT_METERS * SWIM_SUBMERSION_FRACTION + 1.0,
            "{depth_meters:.2} m leaves no margin over the 1.44 m swim threshold"
        );
        assert_ne!(
            brickmap.get(pool.centre_voxel_x, bed_voxel_y, pool.centre_voxel_z),
            0,
            "the bed under the water must be solid, not air"
        );
    }

    /// The walk-in property: from the shore to the centre the bed must descend
    /// monotonically, in steps the body's 3-voxel auto-step can climb back out
    /// of, and it must cross the swim threshold on the way.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn the_bowl_can_be_waded_into_and_walked_back_out_of() {
        let mut brickmap = island();
        let pool = WaterPool {
            centre_voxel_x: 500,
            centre_voxel_z: 500,
        };
        carve(&mut brickmap, pool);

        let bed_of = |voxel_x: i32| {
            (0..=WATER_LEVEL)
                .rev()
                .find(|y| !material_is_liquid(brickmap.get(voxel_x, *y, pool.centre_voxel_z)))
                .expect("every column in the bowl has a bed")
        };
        let water_edge_x = pool.centre_voxel_x + (POOL_WATER_RADIUS_METERS / VOXEL_SIZE) as i32;
        let mut previous = bed_of(water_edge_x);
        let mut crossed_the_swim_threshold = false;
        for voxel_x in (pool.centre_voxel_x..water_edge_x).rev() {
            let bed = bed_of(voxel_x);
            assert!(
                bed <= previous,
                "the bed rises again walking inwards at x = {voxel_x} ({bed} after {previous})"
            );
            let step = previous - bed;
            assert!(
                step as f32 * VOXEL_SIZE <= crate::character::STEP_UP_METERS,
                "a {step}-voxel step at x = {voxel_x} is more than the body can climb out of"
            );
            crossed_the_swim_threshold |= (WATER_LEVEL - bed) as f32 * VOXEL_SIZE
                > BODY_HEIGHT_METERS * SWIM_SUBMERSION_FRACTION;
            previous = bed;
        }
        assert!(
            crossed_the_swim_threshold,
            "walking in from the water's edge never reaches swimming depth"
        );
    }

    /// The bulk edit must publish ONE delta, and it must be a delta the render
    /// thread can apply the same way it applies a click: word payloads that equal
    /// the brickmap's own words, coalesced instead of one range per voxel.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn the_carve_is_one_coalesced_delta() {
        let mut brickmap = island();
        let delta = carve(
            &mut brickmap,
            WaterPool {
                centre_voxel_x: 420,
                centre_voxel_z: 560,
            },
        );
        assert!(
            delta.voxels_written > 50_000,
            "only {} voxels written — the pool is not being carved",
            delta.voxels_written
        );
        assert!(
            delta.writes.len() * 20 < delta.voxels_written,
            "{} uploads for {} voxels: the dirty ranges are not coalescing",
            delta.writes.len(),
            delta.voxels_written
        );
        for write in &delta.writes {
            let words = brickmap.array_words(write.array);
            assert_eq!(
                &words[write.first_word..write.first_word + write.words.len()],
                write.words.as_slice(),
                "{:?} payload does not match the brickmap",
                write.array
            );
        }
        // Carving the same pool again must find nothing left to do.
        assert!(
            apply_bulk(
                &mut brickmap,
                &BulkEditRequest {
                    shape: Box::new(WaterPool {
                        centre_voxel_x: 420,
                        centre_voxel_z: 560,
                    }),
                    light_grid: None,
                    material_attributes: crate::cagi::MaterialAttributes::compiled(),
                },
                &WorldEditSettings::default(),
            )
            .is_none(),
            "re-carving an existing pool changed something"
        );
    }

    /// Placement: the pool lands in front of the eye, far enough that the body is
    /// outside the excavation, and a look straight down still picks a direction.
    #[test]
    fn the_pool_is_placed_ahead_of_the_eye() {
        let eye = Vec3::new(60.0, 12.0, 60.0);
        let pool = WaterPool::in_front_of(eye, Vec3::new(0.0, -0.9, 0.435).normalize());
        let centre = pool.surface_centre();
        let distance = (centre - eye).with_y(0.0).length();
        assert!(
            (distance - POOL_DISTANCE_AHEAD_METERS).abs() < 0.2,
            "the pool is {distance:.2} m ahead, expected {POOL_DISTANCE_AHEAD_METERS} m"
        );
        assert!(
            distance > POOL_RADIUS_METERS,
            "the excavation reaches the body that triggered it"
        );
        assert!(centre.z > eye.z, "the pool is not in the direction of view");

        // Looking straight down: no horizontal direction to project.
        let overhead = WaterPool::in_front_of(eye, Vec3::NEG_Y);
        assert!(
            (overhead.surface_centre() - eye).with_y(0.0).length() > POOL_RADIUS_METERS,
            "a straight-down look carved the pool under the body"
        );
    }
}
