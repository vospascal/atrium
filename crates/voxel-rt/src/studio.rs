//! S0 — the material studio: **one voxel, alone, lit controllably**.
//!
//! Why a separate scene rather than tuning in the island. Judging a material in
//! the generated world means judging it through everything else at the same time:
//! the sun angle you happen to be standing under, whatever is bouncing light from
//! next door, whichever face the terrain happens to show you, and a silhouette
//! made of a thousand neighbours. A material row is a small set of numbers, and
//! the only way to see what each number *does* is to look at one voxel with
//! nothing else in frame.
//!
//! Per the plan's isolation rule this is fully excludable: it builds its own
//! [`Brickmap`] and touches nothing the island path uses. Entering it does not
//! generate a world at all, which is also why it starts in well under the island's
//! generation time.
//!
//! ## The poses, and why there are three
//!
//! S0 shipped one voxel, because flat colours are all one voxel can fail at. S2's
//! layer model can fail at things a single voxel cannot show at all, so it brings
//! the two poses that show them ([`StudioPose`]):
//!
//! * **Single** — one voxel. Still the right pose for judging a colour, a face role
//!   or a within-face grain, because nothing else is in frame to distract.
//! * **Wall** — a 16x16 slab. What **cross-voxel continuity** and any period over
//!   one voxel are judged on: a world-framed layer must flow across the whole slab,
//!   and a per-voxel tile is instantly visible as a 16x16 grid. Any period over one
//!   voxel needs it too: a 1 m band spans eight of them, so on a single voxel it is
//!   just a flat colour.
//! * **Cube** — 4x4x4. The pose where a pattern meets a **corner**: three faces at
//!   once, and the only way to see whether a world-framed layer wraps an edge
//!   convincingly or shows a seam along it.
//!
//! The **plate** below the subject is in every pose, because even S0's flat colours
//! need somewhere for the shadow to land and something to bounce a little light
//! back; a voxel floating in a void reads as a sprite, not a surface.

use glam::Vec3;
use voxel_core::world::{Voxel, VOXEL_SIZE, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z};

use crate::brickmap::{Brickmap, ClearanceUpdate};
use crate::camera::CameraPose;
use crate::material::material_voxel;
use crate::vox_material::VoxSubject;

/// Where the sample voxel sits, in world voxel coordinates.
///
/// The middle of the world rather than a corner, so every direction around the
/// sample is in bounds — the shading path's neighbour reads, the AO face frame and
/// a shadow ray in any sun direction all behave exactly as they do in the island
/// rather than clipping against the world edge and making the studio a special
/// case the renderer has to know about.
pub const SAMPLE_VOXEL: [i32; 3] = [
    WORLD_SIZE_X as i32 / 2,
    WORLD_SIZE_Y as i32 / 2,
    WORLD_SIZE_Z as i32 / 2,
];

/// Half-extent of the plate under the sample, in voxels. 12 gives a 25-voxel
/// (~3.1 m) square — wide enough that the sample's shadow lands on it at a low sun
/// and that the plate reads as a ground plane rather than a second object.
pub const PLATE_HALF_EXTENT: i32 = 12;

/// Voxels of clear air between the sample and the plate. Two keeps the sample's
/// own bottom face visible and its shadow legible as a separate shape, instead of
/// the two merging into one blob.
pub const PLATE_DROP: i32 = 3;

/// How far the studio camera sits from the sample, world meters. A voxel is
/// [`VOXEL_SIZE`] = 0.125 m, so this frames one voxel at a comfortable size while
/// still showing the plate.
pub const CAMERA_DISTANCE_METERS: f32 = 0.9;

/// S2 — the shape the sample material is built into.
///
/// Three poses rather than a size slider: each answers a different question, and a
/// continuous size would mean judging continuity at whatever extent happened to be
/// dialled instead of at a known one. See the module docs for what each is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StudioPose {
    /// One voxel. S0's pose, and still the right one for a colour or a face role.
    #[default]
    Single,
    /// A [`WALL_SIZE`]-square slab, one voxel thick, standing on the plate and
    /// facing the camera's starting angle. The continuity and multi-voxel-period pose.
    Wall,
    /// A [`CUBE_SIZE`]-cubed block. The corner pose.
    Cube,
    /// The wall, with a single [`Voxel::GlowBlock`] embedded at its centre.
    ///
    /// A diagnostic prop rather than a material-judging pose. It exists because "one
    /// emitting block in a wall" behaves **arbitrarily** today, and that was invisible
    /// until someone tried it (Pascal, 2026-07-31: *"what happens if we have a wall with
    /// one emiting light source in it?"*).
    ///
    /// The cause is in the CAGI attribute sweep: it writes every occupied voxel's
    /// material into its cell unconditionally, ascending Y outermost, so **one voxel
    /// represents all 64 of a cell** — the highest, then furthest Z, then furthest X. An
    /// embedded emitter that is not that voxel is not in the emitter set at all and
    /// lights *nothing*, while one that is makes the whole half-metre cell blaze at its
    /// full radiance. Move it one voxel and it flips between the two.
    ///
    /// E5 never caught it because its gate placed glow blocks in open air, where the
    /// block is its own cell's only occupant and always wins.
    ///
    /// See [`EmitterWall`](StudioPose::EmitterWall)'s placement in
    /// [`StudioScene::build`]: the block is put where its cell does **not** elect it, so
    /// this pose currently demonstrates the failure. When the sweep is fixed it will
    /// light, which is exactly the before/after this prop is for.
    EmitterWall,
}

impl StudioPose {
    pub const fn label(&self) -> &'static str {
        match self {
            StudioPose::Single => "single voxel",
            StudioPose::Wall => "wall (16x16)",
            StudioPose::Cube => "cube (4x4x4)",
            StudioPose::EmitterWall => "wall + glow block",
        }
    }

    pub const ALL: [StudioPose; 4] = [
        StudioPose::Single,
        StudioPose::Wall,
        StudioPose::Cube,
        StudioPose::EmitterWall,
    ];

    /// The pose's extent in voxels, as `[x, y, z]`.
    ///
    /// One place rather than a match in each of `build`, the framing and the
    /// centring — those three disagreeing is exactly how a pose ends up built
    /// correctly and framed on its corner.
    pub const fn extent(&self) -> [i32; 3] {
        match self {
            StudioPose::Single => [1, 1, 1],
            // One voxel thick: a slab is what a wall's material is seen on, and a
            // solid 16-deep block would hide 15/16 of what was built while costing
            // 4096 edit-path writes to do it.
            StudioPose::Wall | StudioPose::EmitterWall => [WALL_SIZE, WALL_SIZE, 1],
            StudioPose::Cube => [CUBE_SIZE, CUBE_SIZE, CUBE_SIZE],
        }
    }
}

/// Voxels on a side of the wall pose. 16 is 2 m — eight repeats of a 0.25 m period or
/// two of a 1 m one, which is enough of either to tell a flowing field from a tiled
/// one. Fewer repeats and a tile looks like a deliberate motif.
pub const WALL_SIZE: i32 = 16;

/// Voxels on a side of the cube pose.
pub const CUBE_SIZE: i32 = 4;

/// What the studio is showing.
///
/// The plate material is part of the scene rather than fixed, because it is a
/// light source as much as a backdrop: a bright plate bounces onto the sample's
/// underside and a dark one does not, and while judging a material you want to be
/// able to tell which of the two you are looking at.
#[derive(Clone, Debug, PartialEq)]
pub struct StudioScene {
    /// The voxel under the microscope.
    pub sample: Voxel,
    /// S2 — the shape [`Self::sample`] is built into. Ignored while a `.vox`
    /// [`Self::subject`] is loaded, which brings its own geometry.
    pub pose: StudioPose,
    /// The ground plate, or `None` for a sample floating in the void.
    pub plate: Option<Voxel>,
    /// S0b — an imported `.vox` model shown INSTEAD of the single sample.
    ///
    /// Loading a `.vox` and seeing nothing but the sample block is a reasonable
    /// thing to be surprised by (Pascal, 2026-07-31: *"i can load campfire.vox
    /// indeed now but it doesnt yet show it .. just a block"*), so the studio can
    /// display the geometry as well as consume the palette.
    ///
    /// It replaces the sample rather than sitting beside it: the studio frames one
    /// subject, and two would mean neither is centred. The plate stays, because a
    /// model still wants somewhere for its shadow to land.
    pub subject: Option<VoxSubject>,
}

impl Default for StudioScene {
    /// Grass on snow.
    ///
    /// The sample was stone through S0, when every row was one flat colour and the
    /// most-seen surface was the obvious subject. S1 made grass the only row with
    /// something to show: it is the demonstration case for face roles — earth sides,
    /// green top — so it is what the studio should open on. Snow stays the plate: the
    /// most neutral bright row the table has, so it tints the bounce as little as any
    /// authored row can.
    fn default() -> Self {
        Self {
            sample: Voxel::Grass,
            pose: StudioPose::Single,
            plate: Some(Voxel::Snow),
            subject: None,
        }
    }
}

impl StudioScene {
    /// Build the studio's brickmap from scratch.
    ///
    /// Goes through [`Brickmap::set_voxel`] — the same E2 edit path a click uses —
    /// rather than a bespoke filler, so every derived structure (occupancy bits,
    /// material bytes, the clearance field, column and global heights) is repaired
    /// by tested code and the studio cannot end up subtly inconsistent in a way the
    /// island never is.
    pub fn build(&self) -> Brickmap {
        let mut brickmap = Brickmap::empty();
        let [x, y, z] = SAMPLE_VOXEL;

        // The plate sits under whatever the subject is, and a model needs it lower
        // so it is not standing in its own shadow-catcher.
        if let Some(plate) = self.plate {
            let plate_y = y - PLATE_DROP;
            let half = PLATE_HALF_EXTENT.max(self.subject_footprint_half_extent());
            for plate_z in (z - half)..=(z + half) {
                for plate_x in (x - half)..=(x + half) {
                    brickmap.set_voxel(plate_x, plate_y, plate_z, plate, PLATE_CLEARANCE);
                }
            }
        }

        match &self.subject {
            // An imported model, centred on x/z with its base resting on the plate,
            // so it is framed the same way the single sample is.
            Some(subject) => {
                let base_y = y - PLATE_DROP + 1;
                let origin_x = x - subject.size_x / 2;
                let origin_z = z - subject.size_z / 2;
                for model_y in 0..subject.size_y {
                    for model_z in 0..subject.size_z {
                        for model_x in 0..subject.size_x {
                            let cell = ((model_y * subject.size_z + model_z) * subject.size_x
                                + model_x) as usize;
                            let Some(material) = subject.cells[cell] else {
                                continue;
                            };
                            brickmap.set_voxel(
                                origin_x + model_x,
                                base_y + model_y,
                                origin_z + model_z,
                                material_voxel(material),
                                PLATE_CLEARANCE,
                            );
                        }
                    }
                }
            }
            // S2: the pose. `Single` is the S0 case and comes out of the same code
            // as the others — a 1x1x1 extent — rather than being a special arm, so
            // the pose that ships cannot drift from the poses that are tested.
            None => {
                let [size_x, size_y, size_z] = self.pose.extent();
                let origin = self.pose_origin();
                for pose_y in 0..size_y {
                    for pose_z in 0..size_z {
                        for pose_x in 0..size_x {
                            brickmap.set_voxel(
                                origin[0] + pose_x,
                                origin[1] + pose_y,
                                origin[2] + pose_z,
                                self.sample,
                                PLATE_CLEARANCE,
                            );
                        }
                    }
                }
                // The diagnostic prop: one emitter in the middle of the wall.
                if self.pose == StudioPose::EmitterWall {
                    let block = self.emitter_block_voxel();
                    brickmap.set_voxel(
                        block[0],
                        block[1],
                        block[2],
                        Voxel::GlowBlock,
                        PLATE_CLEARANCE,
                    );
                }
                // `Single` keeps S0's exact placement (see `pose_origin`):
                // floating at the sample voxel with air below it, so its own bottom
                // face and its shadow stay two separate shapes. The larger poses
                // STAND on the plate — a 16-voxel wall hovering two voxels up reads
                // as a bug rather than as deliberate framing.
            }
        }
        brickmap
    }

    /// Where [`StudioPose::EmitterWall`] puts its glow block: the wall's centre.
    ///
    /// The centre is not an arbitrary choice — it is the *interesting* one. The CAGI
    /// sweep elects one voxel per cell (highest Y, then Z, then X), and the centre of a
    /// 16-wide wall is not that voxel for its cell, so the block currently injects
    /// nothing. Which is the point: the prop shows the failure now and will show the fix
    /// later, with no edit to the scene.
    pub fn emitter_block_voxel(&self) -> [i32; 3] {
        let origin = self.pose_origin();
        let [size_x, size_y, _] = StudioPose::EmitterWall.extent();
        [origin[0] + size_x / 2, origin[1] + size_y / 2, origin[2]]
    }

    /// Lowest-corner voxel of the current pose.
    fn pose_origin(&self) -> [i32; 3] {
        let [x, y, z] = SAMPLE_VOXEL;
        let [size_x, _, size_z] = self.pose.extent();
        match self.pose {
            StudioPose::Single => [x, y, z],
            // Centred on the sample column, resting on the plate.
            _ => [x - size_x / 2, y - PLATE_DROP + 1, z - size_z / 2],
        }
    }

    /// Half-extent the plate needs to sit under whatever is being shown, in voxels.
    fn subject_footprint_half_extent(&self) -> i32 {
        // A margin of two voxels so the subject never appears to overhang its plate,
        // which reads as the plate being too small rather than as a deliberate frame.
        match &self.subject {
            Some(subject) => subject.size_x.max(subject.size_z) / 2 + 2,
            None => {
                let [size_x, _, size_z] = self.pose.extent();
                size_x.max(size_z) / 2 + 2
            }
        }
    }

    /// World-space point the camera orbits, in meters.
    ///
    /// The sample voxel's centre, or an imported model's own centre — otherwise a
    /// 20-voxel campfire would be framed on its bottom corner and read as
    /// off-centre rather than as large.
    pub fn sample_center_meters(&self) -> Vec3 {
        let [x, y, z] = SAMPLE_VOXEL;
        match &self.subject {
            Some(subject) => Vec3::new(
                (x as f32 + 0.5) * VOXEL_SIZE,
                (y - PLATE_DROP + 1) as f32 * VOXEL_SIZE + subject.size_y as f32 * VOXEL_SIZE * 0.5,
                (z as f32 + 0.5) * VOXEL_SIZE,
            ),
            None => {
                let origin = self.pose_origin();
                let [size_x, size_y, size_z] = self.pose.extent();
                Vec3::new(
                    (origin[0] as f32 + size_x as f32 * 0.5) * VOXEL_SIZE,
                    (origin[1] as f32 + size_y as f32 * 0.5) * VOXEL_SIZE,
                    (origin[2] as f32 + size_z as f32 * 0.5) * VOXEL_SIZE,
                )
            }
        }
    }

    /// A framing distance that fits the current subject, world meters.
    ///
    /// A single voxel and a 20-voxel model want very different camera distances, and
    /// having to scroll out for ten seconds after loading a model is the kind of
    /// friction that makes a tool feel broken.
    pub fn framing_distance_meters(&self) -> f32 {
        match &self.subject {
            Some(subject) => {
                let largest = subject.size_x.max(subject.size_y).max(subject.size_z) as f32;
                (largest * VOXEL_SIZE * 2.0).max(CAMERA_DISTANCE_METERS)
            }
            // Same rule for a pose: two subject-widths back, floored at S0's
            // one-voxel distance so `Single` still frames exactly as it did.
            None => {
                let [size_x, size_y, size_z] = self.pose.extent();
                let largest = size_x.max(size_y).max(size_z) as f32;
                (largest * VOXEL_SIZE * 2.0).max(CAMERA_DISTANCE_METERS)
            }
        }
    }
}

/// The clearance repair the studio's composition uses.
///
/// A bounded local recompute, matching what a click does. The studio writes a few
/// hundred voxels at startup, so the full-rebuild alternative would be correct but
/// pointlessly slower, and the local box is the shipped path — composing through
/// the same one keeps the studio honest about the structure it produces.
const PLATE_CLEARANCE: ClearanceUpdate = ClearanceUpdate::LocalBox { radius_cells: 8 };

/// An orbit camera around the sample.
///
/// `yaw`/`pitch` in radians; the eye is placed on a sphere of
/// [`CAMERA_DISTANCE_METERS`] around the sample centre and always looks straight
/// at it. Deliberately not the fly camera: the whole job here is that the subject
/// stays framed while you turn it, and a free camera makes you fight to keep one
/// voxel on screen.
pub fn orbit_pose(scene: &StudioScene, yaw: f32, pitch: f32, distance_meters: f32) -> CameraPose {
    let target = scene.sample_center_meters();
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let (sin_pitch, cos_pitch) = pitch.sin_cos();
    // The direction the camera LOOKS along, then step backwards from the target.
    let forward = Vec3::new(cos_yaw * cos_pitch, sin_pitch, sin_yaw * cos_pitch).normalize();
    let position = target - forward * distance_meters.max(VOXEL_SIZE * 2.0);
    let right = forward.cross(Vec3::Y).normalize();
    CameraPose {
        position,
        forward,
        right,
        up: right.cross(forward),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::material_id;

    /// An empty brickmap must be genuinely empty and still self-consistent — it is
    /// the base every studio scene is composed onto.
    #[test]
    fn an_empty_brickmap_holds_nothing() {
        let brickmap = Brickmap::empty();
        assert_eq!(brickmap.occupied_brick_count(), 0);
        assert_eq!(
            brickmap.get(SAMPLE_VOXEL[0], SAMPLE_VOXEL[1], SAMPLE_VOXEL[2]),
            0
        );
        // Sampling the far corners must be air, not a panic or a stale byte.
        assert_eq!(brickmap.get(0, 0, 0), 0);
        assert_eq!(
            brickmap.get(
                WORLD_SIZE_X as i32 - 1,
                WORLD_SIZE_Y as i32 - 1,
                WORLD_SIZE_Z as i32 - 1
            ),
            0
        );
    }

    /// The scene must contain exactly what it says: the sample where the camera
    /// looks, the plate under it, and nothing anywhere else.
    #[test]
    fn the_scene_places_the_sample_and_the_plate() {
        let scene = StudioScene::default();
        let brickmap = scene.build();
        let [x, y, z] = SAMPLE_VOXEL;

        assert_eq!(brickmap.get(x, y, z), material_id(scene.sample));
        assert_eq!(
            brickmap.get(x, y - PLATE_DROP, z),
            material_id(scene.plate.expect("the default scene has a plate")),
            "the plate must sit under the sample"
        );
        // The gap between them is air, which is what keeps the shadow legible.
        for gap in 1..PLATE_DROP {
            assert_eq!(brickmap.get(x, y - gap, z), 0, "gap voxel {gap} is not air");
        }
        // Nothing above the sample, and nothing beyond the plate's edge.
        assert_eq!(brickmap.get(x, y + 1, z), 0);
        assert_eq!(
            brickmap.get(x + PLATE_HALF_EXTENT + 1, y - PLATE_DROP, z),
            0
        );
    }

    /// A plateless scene is one voxel and nothing else — the isolation the studio
    /// exists for, with no bounce at all.
    #[test]
    fn a_plateless_scene_is_a_single_voxel() {
        let scene = StudioScene {
            sample: Voxel::Stone,
            pose: StudioPose::Single,
            plate: None,
            subject: None,
        };
        let brickmap = scene.build();
        let [x, y, z] = SAMPLE_VOXEL;
        assert_eq!(brickmap.get(x, y, z), material_id(Voxel::Stone));
        assert_eq!(brickmap.get(x, y - PLATE_DROP, z), 0);
        // One occupied voxel occupies exactly one brick.
        assert_eq!(brickmap.occupied_brick_count(), 1);
    }

    /// The orbit camera must always look AT the sample from
    /// `distance_meters` away, whatever angle it is turned to — that is the only
    /// property that makes it usable for judging one voxel.
    #[test]
    fn the_orbit_camera_always_frames_the_sample() {
        let scene = StudioScene::default();
        let target = scene.sample_center_meters();
        for (yaw, pitch) in [
            (0.0, 0.0),
            (1.3, 0.4),
            (-2.7, -0.9),
            (std::f32::consts::PI, 1.2),
        ] {
            let pose = orbit_pose(&scene, yaw, pitch, CAMERA_DISTANCE_METERS);
            let to_target = target - pose.position;
            assert!(
                (to_target.length() - CAMERA_DISTANCE_METERS).abs() < 1e-4,
                "distance drifted at yaw {yaw} pitch {pitch}"
            );
            assert!(
                to_target.normalize().dot(pose.forward) > 0.9999,
                "the camera is not looking at the sample at yaw {yaw} pitch {pitch}"
            );
            // The basis must stay orthonormal or the ray generation shears.
            assert!(pose.forward.dot(pose.right).abs() < 1e-4);
            assert!(pose.forward.dot(pose.up).abs() < 1e-4);
            assert!(pose.right.dot(pose.up).abs() < 1e-4);
            assert!((pose.right.length() - 1.0).abs() < 1e-4);
            assert!((pose.up.length() - 1.0).abs() < 1e-4);
        }
    }

    /// The prop must be the wall plus exactly one glow block, at the centre.
    #[test]
    fn the_emitter_wall_embeds_exactly_one_glow_block() {
        let scene = StudioScene {
            pose: StudioPose::EmitterWall,
            ..StudioScene::default()
        };
        let brickmap = scene.build();
        let glow = material_id(Voxel::GlowBlock);
        let block = scene.emitter_block_voxel();
        assert_eq!(brickmap.get(block[0], block[1], block[2]), glow);

        // Exactly one, and the rest of the wall is the sample material.
        let origin = scene.pose_origin();
        let [size_x, size_y, size_z] = StudioPose::EmitterWall.extent();
        let mut emitters = 0;
        for y in 0..size_y {
            for z in 0..size_z {
                for x in 0..size_x {
                    let voxel = [origin[0] + x, origin[1] + y, origin[2] + z];
                    let material = brickmap.get(voxel[0], voxel[1], voxel[2]);
                    if material == glow {
                        emitters += 1;
                        assert_eq!(voxel, block, "a glow block turned up off-centre");
                    } else {
                        assert_eq!(material, material_id(scene.sample));
                    }
                }
            }
        }
        assert_eq!(emitters, 1, "the prop must embed exactly one emitter");
    }

    /// The embedded glow block must claim its cell's emitter slot **even though its cell
    /// does not elect it** as the albedo voxel.
    ///
    /// This started life as a characterisation test asserting the opposite — the sweep
    /// took the last voxel visited for every field, so a light embedded in a surface was
    /// outvoted by whichever neighbour sat higher in the cell and injected nothing. It
    /// failed the moment `fold_voxel_attribute` made the emitter index sticky, which is
    /// what it was written to do.
    #[test]
    fn the_embedded_emitter_claims_its_cell() {
        use crate::cagi::{CagiSettings, MaterialAttributes, CELL_EMITTER_MASK};

        let scene = StudioScene {
            pose: StudioPose::EmitterWall,
            ..StudioScene::default()
        };
        let brickmap = scene.build();
        let settings = CagiSettings::default();
        let grid = settings.grid(&brickmap);

        let block = scene.emitter_block_voxel();
        let cell = [
            block[0] as u32 / grid.cell_voxels,
            block[1] as u32 / grid.cell_voxels,
            block[2] as u32 / grid.cell_voxels,
        ];
        let attribute =
            crate::cagi::cell_attribute(&brickmap, &grid, cell, &MaterialAttributes::compiled());

        // The block is NOT the voxel its cell elects for albedo — that is the whole
        // point of where the prop puts it, and what used to make it invisible to the CA.
        let elected = [
            ((cell[0] + 1) * grid.cell_voxels - 1) as i32,
            ((cell[1] + 1) * grid.cell_voxels - 1) as i32,
            ((cell[2] + 1) * grid.cell_voxels - 1) as i32,
        ];
        assert_ne!(
            elected, block,
            "the prop must place the block where its cell does NOT elect it"
        );
        // ...and it claims the emitter slot regardless, because emission is sticky.
        assert_ne!(
            attribute & CELL_EMITTER_MASK,
            0,
            "the embedded emitter lost its cell — an emitter must not be outvoted by a \
             non-emitting neighbour"
        );
        // The albedo still comes from the elected voxel, i.e. the wall material rather
        // than the glow block: the two rules coexist in one word by design.
        let wall = MaterialAttributes::compiled().word(material_id(scene.sample));
        assert_eq!(
            attribute & 0x00ff_ffff,
            wall & 0x00ff_ffff,
            "the bounce tint should still be the wall, not the one embedded block"
        );
    }

    /// S2 — each pose must build exactly the voxels its extent claims, resting on
    /// the plate, and nothing outside it. The extent is what the framing and the
    /// centring are derived from, so a pose that builds something else is framed
    /// wrong in a way that reads as a camera bug.
    #[test]
    fn each_pose_builds_its_own_extent() {
        for pose in StudioPose::ALL {
            let scene = StudioScene {
                pose,
                ..StudioScene::default()
            };
            let brickmap = scene.build();
            let expected = material_id(scene.sample);
            // The diagnostic prop replaces one voxel with an emitter; every other pose
            // is uniformly the sample material.
            let embedded = (pose == StudioPose::EmitterWall).then(|| scene.emitter_block_voxel());
            let origin = scene.pose_origin();
            let [size_x, size_y, size_z] = pose.extent();

            let mut built = 0;
            for y in 0..size_y {
                for z in 0..size_z {
                    for x in 0..size_x {
                        let voxel = [origin[0] + x, origin[1] + y, origin[2] + z];
                        let want = if embedded == Some(voxel) {
                            material_id(Voxel::GlowBlock)
                        } else {
                            expected
                        };
                        assert_eq!(
                            brickmap.get(voxel[0], voxel[1], voxel[2]),
                            want,
                            "{pose:?} left a hole at {x},{y},{z}"
                        );
                        built += 1;
                    }
                }
            }
            assert_eq!(built, size_x * size_y * size_z);

            // One voxel above the top face must be air, or the pose is taller than
            // it claims and the framing is wrong.
            assert_eq!(
                brickmap.get(origin[0], origin[1] + size_y, origin[2]),
                0,
                "{pose:?} built above its extent"
            );
            // And one to the side.
            assert_eq!(
                brickmap.get(origin[0] + size_x, origin[1], origin[2]),
                0,
                "{pose:?} built beside its extent"
            );
        }
    }

    /// The poses that exist to show cross-voxel behaviour must actually span several
    /// voxels in the directions that matter, and the wall must stay a slab.
    ///
    /// Not a tautology over `extent()`: it is the property the poses were added FOR,
    /// and a well-meaning "make the wall thicker" would break the continuity read
    /// without breaking anything else.
    #[test]
    fn the_wall_spans_voxels_and_stays_one_thick() {
        let [width, height, depth] = StudioPose::Wall.extent();
        assert_eq!(depth, 1, "the wall must stay a slab");
        // A 1 m period must span several voxels of it, or a multi-voxel layer has nothing to
        // course across.
        let span_meters = width as f32 * VOXEL_SIZE;
        assert!(span_meters >= 2.0, "the wall is only {span_meters} m wide");
        assert_eq!(width, height, "the wall must be square");

        let [cube_x, cube_y, cube_z] = StudioPose::Cube.extent();
        assert_eq!([cube_x, cube_y, cube_z], [CUBE_SIZE; 3]);
        assert!(cube_x > 1, "the cube must show a multi-voxel corner");
    }

    /// Every pose must be framed on its own centre from a distance that fits it.
    /// The failure this prevents is the one the `.vox` subject already hit: a large
    /// subject framed on the origin voxel reads as off-centre rather than as large.
    #[test]
    fn every_pose_is_centred_and_framed() {
        for pose in StudioPose::ALL {
            let scene = StudioScene {
                pose,
                ..StudioScene::default()
            };
            let [size_x, size_y, size_z] = pose.extent();
            let origin = scene.pose_origin();
            let centre = scene.sample_center_meters();
            // The centre must be the geometric middle of what was built.
            let expected = Vec3::new(
                (origin[0] as f32 + size_x as f32 * 0.5) * VOXEL_SIZE,
                (origin[1] as f32 + size_y as f32 * 0.5) * VOXEL_SIZE,
                (origin[2] as f32 + size_z as f32 * 0.5) * VOXEL_SIZE,
            );
            assert!(
                (centre - expected).length() < 1e-5,
                "{pose:?} is framed at {centre} rather than {expected}"
            );
            // The camera must sit outside the subject's own bounding sphere, or the
            // eye starts inside the wall.
            let radius = size_x.max(size_y).max(size_z) as f32 * VOXEL_SIZE * 0.5;
            assert!(
                scene.framing_distance_meters() > radius,
                "{pose:?} frames from inside itself"
            );
            // And the plate must be wide enough to catch the whole footprint.
            assert!(
                scene.subject_footprint_half_extent() * 2 >= size_x.max(size_z),
                "{pose:?} overhangs its plate"
            );
        }
    }

    /// `Single` must be byte-for-byte the S0/S1 scene: the pose mechanism is an
    /// addition, and the pose that ships must not have moved under it.
    #[test]
    fn the_single_pose_is_unchanged_by_the_pose_mechanism() {
        let scene = StudioScene::default();
        assert_eq!(scene.pose, StudioPose::Single);
        assert_eq!(scene.pose_origin(), SAMPLE_VOXEL);
        assert_eq!(scene.framing_distance_meters(), CAMERA_DISTANCE_METERS);
        let [x, y, z] = SAMPLE_VOXEL;
        assert_eq!(
            scene.sample_center_meters(),
            Vec3::new(
                (x as f32 + 0.5) * VOXEL_SIZE,
                (y as f32 + 0.5) * VOXEL_SIZE,
                (z as f32 + 0.5) * VOXEL_SIZE,
            )
        );
        // Still floating clear of the plate, which is what keeps its own bottom face
        // and its shadow legible as two shapes.
        let brickmap = scene.build();
        for gap in 1..PLATE_DROP {
            assert_eq!(brickmap.get(x, y - gap, z), 0);
        }
    }

    /// A loaded `.vox` model must win over the pose: it brings its own geometry, and
    /// building a wall of grass around a campfire would frame neither.
    #[test]
    fn a_loaded_subject_overrides_the_pose() {
        let scene = StudioScene {
            pose: StudioPose::Wall,
            subject: Some(VoxSubject {
                size_x: 2,
                size_y: 2,
                size_z: 2,
                cells: vec![Some(material_id(Voxel::Stone)); 8],
            }),
            ..StudioScene::default()
        };
        let brickmap = scene.build();
        let [x, y, z] = SAMPLE_VOXEL;
        // The wall's top voxel would be 16 up from the plate; the model is 2 tall.
        assert_eq!(brickmap.get(x, y - PLATE_DROP + 1 + 4, z), 0);
        assert_eq!(
            brickmap.get(x, y - PLATE_DROP + 1, z),
            material_id(Voxel::Stone)
        );
    }

    /// The camera must never be pulled inside the sample voxel, which would put
    /// the eye in solid material and render the inside of a cube.
    #[test]
    fn the_orbit_camera_cannot_enter_the_sample() {
        let scene = StudioScene::default();
        let pose = orbit_pose(&scene, 0.7, 0.2, 0.0);
        let distance = (scene.sample_center_meters() - pose.position).length();
        assert!(
            distance >= VOXEL_SIZE,
            "the camera got within {distance} m of the sample centre"
        );
    }
}
