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
//! ## What is deliberately NOT here yet
//!
//! A **wall pose**. Cross-voxel continuity and any pattern whose period exceeds
//! one voxel are invisible on a single voxel, so S2 (the layer model) is where a
//! wall and a cube earn their place — that is the first stage with something to
//! show on them. Adding them now would be scenery with nothing to judge.
//!
//! The **plate** below the sample is here, though, because even S0's flat colours
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
            None => {
                brickmap.set_voxel(x, y, z, self.sample, PLATE_CLEARANCE);
            }
        }
        brickmap
    }

    /// Half-extent the plate needs to sit under the current subject, in voxels.
    fn subject_footprint_half_extent(&self) -> i32 {
        match &self.subject {
            // A margin of two voxels so the model never appears to overhang its
            // plate, which reads as the plate being too small rather than as a
            // deliberate frame.
            Some(subject) => subject.size_x.max(subject.size_z) / 2 + 2,
            None => 0,
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
            None => Vec3::new(
                (x as f32 + 0.5) * VOXEL_SIZE,
                (y as f32 + 0.5) * VOXEL_SIZE,
                (z as f32 + 0.5) * VOXEL_SIZE,
            ),
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
            None => CAMERA_DISTANCE_METERS,
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
