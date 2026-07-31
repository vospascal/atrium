//! CPU voxel DDA over a [`Brickmap`] — E2's picking ray **and the seed of E8's
//! atrium `VoxelDdaResolver`**.
//!
//! WHY THIS LIVES HERE AND LOOKS LIKE THIS (plan E8, modularity rule): atrium's
//! audio bridge needs to shoot rays through the same voxel world the renderer
//! draws — direct-path occlusion, early-reflection hit points and their surface
//! materials — from a background thread, with no GPU involved. That is exactly
//! what voxel picking needs, so E2 builds it ONCE, renderer-independent:
//!
//! - the only input is `&Brickmap` (pure data, no wgpu, no winit) — an
//!   `Arc<RwLock<Brickmap>>` read guard from [`crate::world_host`] derefs
//!   straight into it, which is how the audio thread will call this;
//! - positions are world METERS on the way in and out, so a caller never has to
//!   know about voxel units, bricks, or the packing;
//! - a hit reports the surface it landed on the way a *reflection* needs it
//!   (voxel, face normal, material id, distance), not the way a *pixel* needs it
//!   (no colour, no shading);
//! - [`path_is_clear`] is the occlusion query in its own right, because that is
//!   the one an audio direct path asks for, and it must not have to allocate or
//!   build a full [`VoxelHit`] to answer.
//!
//! The traversal itself is the CPU twin of `shaders/world.wgsl`: the same
//! two-level idea (walk voxels, but jump the chebyshev empty cube whenever the
//! brick under the ray is empty — bindings 9/10's data, S2's headline win), and
//! the same conventions (voxel-space DDA, x-major cell indices, `face_axis` of
//! the boundary last crossed). It is deliberately NOT bit-identical to the
//! shader: it is a separate consumer of the same structure, pinned by tests
//! against a brute-force fine-step walk rather than against the GPU.
//!
//! Ray-tracing note for E8: the cost of one query is dominated by the empty-cube
//! jumps, so a 60 m audio ray over open terrain costs a handful of loop
//! iterations, not 480 voxel steps. The bench's E2 section measures it.

use voxel_core::world::{VOXEL_SIZE, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z};

use crate::brickmap::{Brickmap, BRICK_SIZE};
use crate::material::material_is_empty_for_edits;

/// Nudge past a cell boundary before re-deriving an integer cell from a float
/// position (mirrors `RAY_EPSILON` in `world.wgsl`), in voxel units.
const RAY_EPSILON: f32 = 1e-4;

/// Direction components below this are treated as this, so the reciprocal stays
/// finite and an axis-aligned ray cannot produce `inf * 0 = NaN` boundary times.
const MIN_DIRECTION_COMPONENT: f32 = 1e-7;

/// Iteration cap: a straight line through the world crosses at most
/// `WORLD_SIZE_X + WORLD_SIZE_Y + WORLD_SIZE_Z` voxel boundaries, so anything
/// past that is a float pathology, not a ray. Keeps a malformed direction from
/// hanging the audio thread.
const MAX_STEPS: usize = WORLD_SIZE_X + WORLD_SIZE_Y + WORLD_SIZE_Z;

/// What a ray hit: the surface, in the terms a reflection (or a placement) needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelHit {
    /// The occupied voxel the ray entered.
    pub voxel: [i32; 3],
    /// The empty voxel in FRONT of the hit face — where a placed voxel goes, and
    /// where a reflection's secondary ray starts. Equal to [`Self::voxel`] when
    /// the ray started inside solid geometry (no face was crossed).
    pub face_voxel: [i32; 3],
    /// Unit face normal as integers, pointing back along the ray
    /// (e.g. `[0, 1, 0]` for a floor). All zeros when the ray started inside
    /// solid geometry.
    pub face_normal: [i32; 3],
    /// Distance from the ray origin, world meters.
    pub distance_meters: f32,
    /// Material id of the hit voxel (see `material::material_id`).
    pub material: u8,
}

/// What counts as a hit — the query's own question, not a property of the world.
///
/// Two consumers ask different questions of the same world, and the difference
/// belongs at the call site rather than in the traversal: an **audio** ray wants
/// every occupied voxel (a body of water is very much an obstruction to airborne
/// sound, and its surface is very nearly a perfect acoustic mirror —
/// `material::ACOUSTIC_WATER`), while an **edit** ray treats water as air, per the
/// plan's "to the editor, water IS air" rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastTarget {
    /// Any occupied voxel, liquids included — the occlusion / audio query.
    AnyVoxel,
    /// The first voxel an EDIT can act on: everything
    /// [`material_is_empty_for_edits`] calls empty is passed through, in BOTH edit
    /// directions. That single predicate is the whole rule — placing and removing
    /// share it, so a click into a pond lands on the bed either way and the
    /// placement cell in front of it is the water the solid displaces.
    EditableVoxel,
}

impl CastTarget {
    /// Whether a material id ends the ray.
    fn stops_the_ray(self, material: u8) -> bool {
        match self {
            CastTarget::AnyVoxel => true,
            CastTarget::EditableVoxel => !material_is_empty_for_edits(material),
        }
    }
}

/// First voxel along a ray that `target` accepts, or `None` for a miss inside
/// `max_distance_meters`.
///
/// `origin_meters` may sit outside the world (the ray is clipped to the world
/// box first, which is what an audio source across the map needs); `direction`
/// need not be normalized — distances are reported along the normalized ray.
pub fn cast(
    brickmap: &Brickmap,
    origin_meters: [f32; 3],
    direction: [f32; 3],
    max_distance_meters: f32,
    target: CastTarget,
) -> Option<VoxelHit> {
    let world_size = [
        WORLD_SIZE_X as f32,
        WORLD_SIZE_Y as f32,
        WORLD_SIZE_Z as f32,
    ];
    let grid_max = [
        WORLD_SIZE_X as i32 - 1,
        WORLD_SIZE_Y as i32 - 1,
        WORLD_SIZE_Z as i32 - 1,
    ];
    let voxels_per_meter = 1.0 / VOXEL_SIZE;
    let origin = [
        origin_meters[0] * voxels_per_meter,
        origin_meters[1] * voxels_per_meter,
        origin_meters[2] * voxels_per_meter,
    ];
    let length =
        (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2])
            .sqrt();
    if !length.is_finite()
        || length <= 0.0
        || !max_distance_meters.is_finite()
        || max_distance_meters <= 0.0
    {
        return None;
    }
    let mut unit_direction = [0.0_f32; 3];
    let mut inverse_direction = [0.0_f32; 3];
    let mut step = [0_i32; 3];
    for axis in 0..3 {
        let component = direction[axis] / length;
        let safe = if component.abs() < MIN_DIRECTION_COMPONENT {
            MIN_DIRECTION_COMPONENT * if component < 0.0 { -1.0 } else { 1.0 }
        } else {
            component
        };
        unit_direction[axis] = safe;
        inverse_direction[axis] = 1.0 / safe;
        step[axis] = if safe >= 0.0 { 1 } else { -1 };
    }

    // Clip to the world box, so an origin outside it still traverses correctly.
    let mut t_enter = 0.0_f32;
    let mut t_exit = max_distance_meters * voxels_per_meter;
    let mut entry_axis: Option<usize> = None;
    for axis in 0..3 {
        let near = (0.0 - origin[axis]) * inverse_direction[axis];
        let far = (world_size[axis] - origin[axis]) * inverse_direction[axis];
        let (near, far) = if near <= far {
            (near, far)
        } else {
            (far, near)
        };
        if near > t_enter {
            t_enter = near;
            entry_axis = Some(axis);
        }
        t_exit = t_exit.min(far);
    }
    if t_enter > t_exit {
        return None;
    }

    let mut t = t_enter;
    let mut face_axis = entry_axis;
    let mut cell = [0_i32; 3];
    let mut next_boundary = [0.0_f32; 3];
    let boundary_width = [
        inverse_direction[0].abs(),
        inverse_direction[1].abs(),
        inverse_direction[2].abs(),
    ];
    // Re-seed the integer state from a float position — used at the start and
    // after every empty-cube jump.
    let reseed = |t: f32, cell: &mut [i32; 3], next_boundary: &mut [f32; 3]| {
        for axis in 0..3 {
            let position = origin[axis] + unit_direction[axis] * (t + RAY_EPSILON);
            cell[axis] = (position.floor() as i32).clamp(0, grid_max[axis]);
            let boundary = if step[axis] > 0 {
                (cell[axis] + 1) as f32
            } else {
                cell[axis] as f32
            };
            next_boundary[axis] = (boundary - origin[axis]) * inverse_direction[axis];
        }
    };
    reseed(t, &mut cell, &mut next_boundary);

    for _ in 0..MAX_STEPS {
        if (0..3).any(|axis| cell[axis] < 0 || cell[axis] > grid_max[axis]) || t > t_exit {
            return None;
        }
        let brick = [
            cell[0] / BRICK_SIZE as i32,
            cell[1] / BRICK_SIZE as i32,
            cell[2] / BRICK_SIZE as i32,
        ];
        let clearance = brickmap.brick_clearance_cells(brick);
        if clearance > 0 {
            // The brick is empty and sits centered in a guaranteed-empty cube of
            // half-width clearance - 1 bricks: jump straight to that cube's exit
            // (S2's distance-field skip, on the CPU).
            let half_width = (clearance as i32 - 1) * BRICK_SIZE as i32;
            let mut exit_t = f32::INFINITY;
            let mut exit_axis = 0_usize;
            for axis in 0..3 {
                let boundary = if step[axis] > 0 {
                    ((brick[axis] + 1) * BRICK_SIZE as i32 + half_width) as f32
                } else {
                    (brick[axis] * BRICK_SIZE as i32 - half_width) as f32
                };
                let axis_t = (boundary - origin[axis]) * inverse_direction[axis];
                if axis_t < exit_t {
                    exit_t = axis_t;
                    exit_axis = axis;
                }
            }
            if exit_t <= t || !exit_t.is_finite() {
                // Float pathology (the cube exit is behind us): fall back to a
                // single voxel step so the loop always advances.
                let axis = argmin(&next_boundary);
                t = next_boundary[axis];
                cell[axis] += step[axis];
                next_boundary[axis] += boundary_width[axis];
                face_axis = Some(axis);
                continue;
            }
            t = exit_t;
            if t > t_exit {
                return None;
            }
            face_axis = Some(exit_axis);
            reseed(t, &mut cell, &mut next_boundary);
            continue;
        }

        if brickmap.is_occupied(cell[0], cell[1], cell[2]) {
            let material = brickmap.get(cell[0], cell[1], cell[2]);
            if target.stops_the_ray(material) {
                let mut face_normal = [0_i32; 3];
                let mut face_voxel = cell;
                if let Some(axis) = face_axis {
                    face_normal[axis] = -step[axis];
                    face_voxel[axis] = cell[axis] - step[axis];
                }
                return Some(VoxelHit {
                    voxel: cell,
                    face_voxel,
                    face_normal,
                    distance_meters: t.max(0.0) * VOXEL_SIZE,
                    material,
                });
            }
            // Empty to the editor (a liquid): keep stepping, exactly as through
            // air. The face voxel of whatever the ray eventually stops on is then
            // the liquid cell against it — which IS the placement target, because
            // a placed solid displaces the water.
        }

        let axis = argmin(&next_boundary);
        t = next_boundary[axis];
        cell[axis] += step[axis];
        next_boundary[axis] += boundary_width[axis];
        face_axis = Some(axis);
    }
    None
}

/// Whether the straight segment between two world points is free of occupied
/// voxels — the occlusion query an audio direct path asks for (E8), and the reason
/// this module exists outside the renderer.
///
/// Water occludes here ([`CastTarget::AnyVoxel`]): the acoustic question is
/// whether the path is unobstructed, and a body of water is very much an
/// obstruction to airborne sound.
pub fn path_is_clear(brickmap: &Brickmap, from_meters: [f32; 3], to_meters: [f32; 3]) -> bool {
    let direction = [
        to_meters[0] - from_meters[0],
        to_meters[1] - from_meters[1],
        to_meters[2] - from_meters[2],
    ];
    let distance =
        (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2])
            .sqrt();
    if distance <= 0.0 {
        return !brickmap.is_occupied(
            (from_meters[0] / VOXEL_SIZE).floor() as i32,
            (from_meters[1] / VOXEL_SIZE).floor() as i32,
            (from_meters[2] / VOXEL_SIZE).floor() as i32,
        );
    }
    cast(
        brickmap,
        from_meters,
        direction,
        distance,
        CastTarget::AnyVoxel,
    )
    .is_none()
}

fn argmin(values: &[f32; 3]) -> usize {
    if values[0] <= values[1] && values[0] <= values[2] {
        0
    } else if values[1] <= values[2] {
        1
    } else {
        2
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brickmap::ClearanceUpdate;
    use crate::material::{material_id, material_is_liquid};
    use voxel_core::world::{Voxel, VoxelWorld, WATER_LEVEL};

    /// The default query — spelled once so the miss assertions stay one-liners.
    const ANY: CastTarget = CastTarget::AnyVoxel;

    /// Brute-force reference: step the segment in fine increments and report the
    /// first voxel whose occupancy bit is set.
    ///
    /// ONE-SIDED ON PURPOSE. A discrete sample can *miss* a thin sliver of a voxel
    /// the true ray clips at a shared edge, so "brute force found nothing" is not
    /// evidence of a miss. What it can never do is invent occupancy: if it reports
    /// a hit, geometry really is on the ray, and a `cast` miss would be a bug. The
    /// assertions below use it in that direction only.
    fn brute_force_first_hit(
        brickmap: &Brickmap,
        origin_meters: [f32; 3],
        direction: [f32; 3],
        max_distance_meters: f32,
    ) -> Option<[i32; 3]> {
        let length = (direction[0] * direction[0]
            + direction[1] * direction[1]
            + direction[2] * direction[2])
            .sqrt();
        let unit = [
            direction[0] / length,
            direction[1] / length,
            direction[2] / length,
        ];
        let step_meters = VOXEL_SIZE * 0.02;
        let steps = (max_distance_meters / step_meters) as usize;
        for index in 0..=steps {
            let distance = index as f32 * step_meters;
            let voxel = [
                ((origin_meters[0] + unit[0] * distance) / VOXEL_SIZE).floor() as i32,
                ((origin_meters[1] + unit[1] * distance) / VOXEL_SIZE).floor() as i32,
                ((origin_meters[2] + unit[2] * distance) / VOXEL_SIZE).floor() as i32,
            ];
            if brickmap.is_occupied(voxel[0], voxel[1], voxel[2]) {
                return Some(voxel);
            }
        }
        None
    }

    /// The traversal must be consistent with a fine-step walk over the real island
    /// for a fan of rays from a fly-camera-like viewpoint: same hit voxel, and a
    /// distance consistent with it.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn cast_agrees_with_a_brute_force_walk_on_the_island() {
        let world = VoxelWorld::generate(1234, 0.0);
        let brickmap = Brickmap::build(&world);
        // Deliberately OFF the voxel lattice, and the yaw fan deliberately off the
        // cardinal axes: a ray whose origin sits exactly on a voxel boundary plane
        // while its direction along that axis is ~1e-8 is a genuine float tie (the
        // same class as the 19 tie pixels the shader's gate records), and both
        // answers are defensible. The axis-aligned cases are covered by the
        // dedicated tests below, whose origins are lattice-consistent.
        let origin = [62.53, WATER_LEVEL as f32 * VOXEL_SIZE + 12.0, 100.07];
        let max_distance = 90.0;

        let mut hits = 0_usize;
        let mut misses = 0_usize;
        for yaw_step in 0..24 {
            for pitch_step in 0..9 {
                let yaw = 0.031 + yaw_step as f32 / 24.0 * std::f32::consts::TAU;
                let pitch = -1.05 + pitch_step as f32 * 0.16;
                let direction = [
                    yaw.cos() * pitch.cos(),
                    pitch.sin(),
                    yaw.sin() * pitch.cos(),
                ];
                let expected = brute_force_first_hit(&brickmap, origin, direction, max_distance);
                let actual = cast(
                    &brickmap,
                    origin,
                    direction,
                    max_distance,
                    CastTarget::AnyVoxel,
                );
                match (expected, actual) {
                    (Some(expected_voxel), Some(hit)) => {
                        // Nothing may be occupied strictly BEFORE the reported hit
                        // (the direction the sampler is evidence in), and the
                        // sampler's own first hit must not be closer than ours.
                        let occluder_before = brute_force_first_hit(
                            &brickmap,
                            origin,
                            direction,
                            (hit.distance_meters - VOXEL_SIZE * 0.5).max(0.0),
                        );
                        assert!(
                            occluder_before.is_none(),
                            "yaw {yaw}, pitch {pitch}: reported hit {:?} at {} m, but \
                             {occluder_before:?} is occupied before it",
                            hit.voxel,
                            hit.distance_meters
                        );
                        assert!(
                            hit.voxel == expected_voxel
                                || (0..3)
                                    .map(|axis| (hit.voxel[axis] - expected_voxel[axis]).abs())
                                    .sum::<i32>()
                                    <= 1,
                            "yaw {yaw}, pitch {pitch}: hit {:?} is not the sampler's \
                             {expected_voxel:?} nor a face neighbour of it",
                            hit.voxel
                        );
                        assert!(
                            brickmap.is_occupied(hit.voxel[0], hit.voxel[1], hit.voxel[2]),
                            "reported hit voxel is not occupied"
                        );
                        assert_eq!(
                            hit.material,
                            brickmap.get(hit.voxel[0], hit.voxel[1], hit.voxel[2])
                        );
                        assert!(hit.distance_meters >= 0.0 && hit.distance_meters <= max_distance);
                        // The face voxel is the placement target, so it must be
                        // FREE — and since E6 "free" means empty to the EDITOR:
                        // air, or a liquid the placed block displaces. (This fan
                        // uses `AnyVoxel`, so here it is always air; the predicate
                        // is asserted rather than `!is_occupied` so the invariant
                        // stays true when the same reconstruction is reached
                        // through an edit ray.)
                        assert!(material_is_empty_for_edits(brickmap.get(
                            hit.face_voxel[0],
                            hit.face_voxel[1],
                            hit.face_voxel[2]
                        )));
                        let axis_deltas: i32 = (0..3)
                            .map(|axis| (hit.face_voxel[axis] - hit.voxel[axis]).abs())
                            .sum();
                        assert_eq!(axis_deltas, 1, "face voxel is not face-adjacent");
                        assert_eq!(
                            hit.face_normal
                                .iter()
                                .map(|component| component.abs())
                                .sum::<i32>(),
                            1
                        );
                        hits += 1;
                    }
                    (None, None) => misses += 1,
                    (None, Some(hit)) => {
                        // Allowed: the sampler stepped over a sliver the true ray
                        // clips. The hit must still be real.
                        assert!(
                            brickmap.is_occupied(hit.voxel[0], hit.voxel[1], hit.voxel[2]),
                            "yaw {yaw}, pitch {pitch}: hit an unoccupied voxel {:?}",
                            hit.voxel
                        );
                        hits += 1;
                    }
                    (Some(expected_voxel), None) => panic!(
                        "yaw {yaw}, pitch {pitch}: {expected_voxel:?} is occupied on the ray \
                         but cast reported a miss"
                    ),
                }
            }
        }
        assert!(
            hits > 60 && misses > 30,
            "{hits} hits / {misses} misses — the fan no longer exercises both outcomes"
        );
    }

    /// Straight down onto terrain: the normal must point up, the face voxel must
    /// sit directly above the hit, and the distance must match the surface height.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn a_downward_ray_lands_on_the_surface_with_an_upward_normal() {
        let world = VoxelWorld::generate(1234, 0.0);
        let brickmap = Brickmap::build(&world);
        let hit = cast(
            &brickmap,
            [62.5, 40.0, 62.5],
            [0.0, -1.0, 0.0],
            60.0,
            CastTarget::AnyVoxel,
        )
        .expect("straight down over the island center hits terrain");
        assert_eq!(hit.face_normal, [0, 1, 0]);
        assert_eq!(
            hit.face_voxel,
            [hit.voxel[0], hit.voxel[1] + 1, hit.voxel[2]]
        );
        let surface_y = (0..WORLD_SIZE_Y as i32)
            .rev()
            .find(|y| brickmap.is_occupied(500, *y, 500))
            .expect("the column is occupied");
        assert_eq!(hit.voxel, [500, surface_y, 500]);
        let expected_distance = 40.0 - (surface_y + 1) as f32 * VOXEL_SIZE;
        assert!(
            (hit.distance_meters - expected_distance).abs() < VOXEL_SIZE,
            "distance {} vs expected {expected_distance}",
            hit.distance_meters
        );
    }

    /// Rays that cannot hit: straight up into the sky, a zero-length reach, and an
    /// origin outside the world pointing away from it.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn misses_are_reported_as_misses() {
        let world = VoxelWorld::generate(1234, 0.0);
        let brickmap = Brickmap::build(&world);
        assert!(cast(&brickmap, [62.5, 40.0, 62.5], [0.0, 1.0, 0.0], 100.0, ANY).is_none());
        assert!(cast(&brickmap, [62.5, 40.0, 62.5], [0.0, -1.0, 0.0], 0.0, ANY).is_none());
        assert!(cast(&brickmap, [-50.0, 40.0, 62.5], [-1.0, 0.0, 0.0], 100.0, ANY).is_none());
        assert!(cast(&brickmap, [62.5, 40.0, 62.5], [0.0, 0.0, 0.0], 100.0, ANY).is_none());
        // ...and one that must hit from OUTSIDE the world box (the audio case).
        assert!(cast(&brickmap, [-20.0, 12.0, 62.5], [1.0, 0.0, 0.0], 200.0, ANY).is_some());
    }

    /// The occlusion query: a segment through a hill is blocked, the same segment
    /// high above it is clear, and an edit flips the answer — the audio bridge's
    /// whole contract in one test.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn path_is_clear_answers_the_audio_occlusion_query() {
        let world = VoxelWorld::generate(1234, 0.0);
        let mut brickmap = Brickmap::build(&world);
        let above_left = [30.0, 28.0, 62.5];
        let above_right = [95.0, 28.0, 62.5];
        assert!(
            path_is_clear(&brickmap, above_left, above_right),
            "a segment over the island at 28 m must be clear"
        );
        let surface_y = (0..WORLD_SIZE_Y as i32)
            .rev()
            .find(|y| brickmap.is_occupied(500, *y, 500))
            .expect("the column is occupied");
        let through_hill_left = [30.0, surface_y as f32 * VOXEL_SIZE, 62.5];
        let through_hill_right = [95.0, surface_y as f32 * VOXEL_SIZE, 62.5];
        assert!(
            !path_is_clear(&brickmap, through_hill_left, through_hill_right),
            "a segment through the island's highest column must be blocked"
        );

        // An edit moves the answer: drop one stone voxel into the clear path.
        let blocker = [(62.5 / VOXEL_SIZE) as i32, (28.0 / VOXEL_SIZE) as i32, 500];
        brickmap.set_voxel(
            blocker[0],
            blocker[1],
            blocker[2],
            Voxel::Stone,
            ClearanceUpdate::LocalBox { radius_cells: 8 },
        );
        assert!(
            !path_is_clear(&brickmap, above_left, above_right),
            "the placed voxel must occlude the segment it sits on"
        );
        let hit = cast(
            &brickmap,
            above_left,
            [
                above_right[0] - above_left[0],
                above_right[1] - above_left[1],
                above_right[2] - above_left[2],
            ],
            100.0,
            ANY,
        )
        .expect("the placed voxel is hit");
        assert_eq!(hit.voxel, blocker);
        assert_eq!(hit.material, material_id(Voxel::Stone));
        assert_eq!(hit.face_normal, [-1, 0, 0]);
    }

    /// A ray that starts inside solid geometry reports the voxel it is in, with no
    /// face — the documented degenerate case (a camera buried in terrain must not
    /// place a voxel inside itself).
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn a_ray_starting_inside_solid_geometry_has_no_face() {
        let world = VoxelWorld::generate(1234, 0.0);
        let brickmap = Brickmap::build(&world);
        let surface_y = (0..WORLD_SIZE_Y as i32)
            .rev()
            .find(|y| brickmap.is_occupied(500, *y, 500))
            .expect("the column is occupied");
        let inside = [
            500.5 * VOXEL_SIZE,
            (surface_y as f32 + 0.5) * VOXEL_SIZE,
            500.5 * VOXEL_SIZE,
        ];
        let hit = cast(&brickmap, inside, [0.0, -1.0, 0.0], 10.0, ANY).expect("inside solid = hit");
        assert_eq!(hit.voxel, [500, surface_y, 500]);
        assert_eq!(hit.face_normal, [0, 0, 0]);
        assert_eq!(hit.face_voxel, hit.voxel);
        assert_eq!(hit.distance_meters, 0.0);
    }

    /// **To the editor, water IS air** (E6, the plan's rule). One edit ray, both
    /// directions: through 1.5 m of water it must report the BED, so removing takes
    /// the bed voxel and placing lands in the water cell against it — which is how
    /// a lantern gets into a submerged niche. The audio query over the same column
    /// must still see the water as an obstruction.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn an_edit_ray_treats_water_as_air_in_both_directions() {
        let world = VoxelWorld::generate(1234, 0.0);
        let mut brickmap = Brickmap::build(&world);
        // A column of water standing on the terrain. Both the column position and
        // its depth are DISCOVERED rather than hardcoded — `set_voxel` refuses an
        // out-of-world write, so the loop simply stops at the world ceiling — which
        // keeps this test independent of the world's dimensions.
        let (voxel_x, voxel_z) = (WORLD_SIZE_X as i32 / 2, WORLD_SIZE_Z as i32 / 2);
        let surface_y = (0..WORLD_SIZE_Y as i32)
            .rev()
            .find(|y| brickmap.is_occupied(voxel_x, *y, voxel_z))
            .expect("the column is occupied");
        let mut water_voxels = 0;
        for offset in 1..=12 {
            if brickmap
                .set_voxel(
                    voxel_x,
                    surface_y + offset,
                    voxel_z,
                    Voxel::Water,
                    ClearanceUpdate::LocalBox { radius_cells: 8 },
                )
                .is_none()
            {
                break;
            }
            water_voxels = offset;
        }
        assert!(
            water_voxels >= 4,
            "only {water_voxels} voxels of water fit above the surface — the column is \
             too shallow for this test to prove anything"
        );

        // The eye sits two voxels above the topmost water voxel, looking straight
        // down, with just enough reach to pass through the column and reach the bed.
        let eye_voxel_y = surface_y + water_voxels + 2;
        let above = [
            (voxel_x as f32 + 0.5) * VOXEL_SIZE,
            (eye_voxel_y as f32 + 0.5) * VOXEL_SIZE,
            (voxel_z as f32 + 0.5) * VOXEL_SIZE,
        ];
        let reach_meters = (eye_voxel_y - surface_y + 2) as f32 * VOXEL_SIZE;

        let edit = cast(
            &brickmap,
            above,
            [0.0, -1.0, 0.0],
            reach_meters,
            CastTarget::EditableVoxel,
        )
        .expect("an edit ray must reach the bed under the water");
        assert_eq!(
            edit.voxel,
            [voxel_x, surface_y, voxel_z],
            "the edit ray must pass through every water voxel and land on the bed"
        );
        assert!(
            !material_is_liquid(edit.material),
            "the edit ray stopped on water — water is not a block"
        );
        assert_eq!(edit.face_normal, [0, 1, 0]);
        // The placement cell: the water directly above the bed, NOT the surface and
        // NOT inside the bed. A solid placed here displaces the water.
        assert_eq!(
            edit.face_voxel,
            [voxel_x, surface_y + 1, voxel_z],
            "the placement cell must be the water cell against the bed"
        );
        assert_eq!(
            brickmap.get(edit.face_voxel[0], edit.face_voxel[1], edit.face_voxel[2]),
            material_id(Voxel::Water),
            "the placement cell should still be water before the edit"
        );
        assert!(material_is_empty_for_edits(brickmap.get(
            edit.face_voxel[0],
            edit.face_voxel[1],
            edit.face_voxel[2]
        )));

        // ...and placing there really does displace the water, with a SOLID
        // EMISSIVE of all things — the "put a light in a submerged niche" case
        // that made Pascal ask for this rule, and an E5/CAGI test case.
        brickmap.set_voxel(
            edit.face_voxel[0],
            edit.face_voxel[1],
            edit.face_voxel[2],
            Voxel::GlowBlock,
            ClearanceUpdate::LocalBox { radius_cells: 8 },
        );
        assert_eq!(
            brickmap.get(edit.face_voxel[0], edit.face_voxel[1], edit.face_voxel[2]),
            material_id(Voxel::GlowBlock),
            "a placed solid must displace the water it lands in"
        );
        // The next edit ray now stops on the lantern, one cell higher.
        let onto_lantern = cast(
            &brickmap,
            above,
            [0.0, -1.0, 0.0],
            reach_meters,
            CastTarget::EditableVoxel,
        )
        .expect("the submerged light is the new target");
        assert_eq!(onto_lantern.voxel, [voxel_x, surface_y + 1, voxel_z]);
        assert_eq!(onto_lantern.material, material_id(Voxel::GlowBlock));

        // The water above is untouched by all of this: it is not a block, so no
        // edit ray ever selected it.
        for offset in 2..=water_voxels {
            assert_eq!(
                brickmap.get(voxel_x, surface_y + offset, voxel_z),
                material_id(Voxel::Water),
                "the water at +{offset} was disturbed by an edit"
            );
        }

        // The audio query asks a different question of the same column and must
        // still be occluded by the water.
        assert!(
            cast(
                &brickmap,
                above,
                [0.0, -1.0, 0.0],
                10.0,
                CastTarget::AnyVoxel
            )
            .is_some_and(|hit| material_is_liquid(hit.material)),
            "an occlusion ray must still be stopped by the water surface"
        );

        // Water and nothing else on the ray: an edit ray reports a miss rather than
        // offering the water as a target. One water voxel in mid-air, and a reach
        // short enough that nothing below it is in range.
        let mut floating = Brickmap::build(&world);
        let air_y = surface_y + water_voxels + 1;
        assert!(
            floating
                .set_voxel(
                    voxel_x,
                    air_y,
                    voxel_z,
                    Voxel::Water,
                    ClearanceUpdate::LocalBox { radius_cells: 8 },
                )
                .is_some(),
            "the mid-air water voxel must be inside the world"
        );
        let from_above = [
            (voxel_x as f32 + 0.5) * VOXEL_SIZE,
            (air_y as f32 + 1.5) * VOXEL_SIZE,
            (voxel_z as f32 + 0.5) * VOXEL_SIZE,
        ];
        assert!(
            cast(
                &floating,
                from_above,
                [0.0, -1.0, 0.0],
                VOXEL_SIZE * 2.0,
                CastTarget::EditableVoxel
            )
            .is_none(),
            "an edit ray whose only obstruction is water must report a miss — clicking a \
             pond's surface selects nothing, because there is nothing there"
        );
        assert!(
            cast(
                &floating,
                from_above,
                [0.0, -1.0, 0.0],
                VOXEL_SIZE * 2.0,
                CastTarget::AnyVoxel
            )
            .is_some(),
            "the same ray must still occlude for audio"
        );
    }
}
