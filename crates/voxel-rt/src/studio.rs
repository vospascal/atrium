//! Isolated material and asset preview scene.
//!
//! Studio observes the same scale boundary as the world:
//!
//! - built-in preview geometry is made from indivisible one-metre world voxels;
//! - imported `.vox` subjects are assets, so their cells remain 0.125 m detail;
//! - material patterns are evaluated on the one-metre block surface.

use glam::Vec3;
use voxel_core::world::{
    Voxel, WorldVoxelCoord, DETAIL_CELLS_PER_WORLD_VOXEL, DETAIL_CELL_SIZE_METERS, WORLD_VOXELS_X,
    WORLD_VOXELS_Y, WORLD_VOXELS_Z, WORLD_VOXEL_SIZE_METERS,
};

use crate::brickmap::{Brickmap, ClearanceUpdate};
use crate::camera::CameraPose;
use crate::vox_material::VoxSubject;
use voxel_material::material::material_voxel;

/// Centre of the preview scene, in one-metre world-voxel coordinates.
pub const SAMPLE_VOXEL: [i32; 3] = [
    WORLD_VOXELS_X as i32 / 2,
    WORLD_VOXELS_Y as i32 / 2,
    WORLD_VOXELS_Z as i32 / 2,
];

/// Half-extent of the square shadow plate, in one-metre world voxels.
pub const PLATE_HALF_EXTENT: i32 = 3;

/// Vertical distance from the sample block to the plate, in world voxels.
pub const PLATE_DROP: i32 = 3;

pub const CAMERA_DISTANCE_METERS: f32 = 4.0;
pub const WALL_SIZE: i32 = 4;
pub const CUBE_SIZE: i32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StudioPose {
    #[default]
    Single,
    Wall,
    Cube,
    EmitterWall,
}

impl StudioPose {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Single => "single 1 m voxel",
            Self::Wall => "wall (4x4 m)",
            Self::Cube => "cube (3x3x3 m)",
            Self::EmitterWall => "wall + 1 m glow block",
        }
    }

    pub const ALL: [Self; 4] = [Self::Single, Self::Wall, Self::Cube, Self::EmitterWall];

    /// Extent in one-metre world voxels.
    pub const fn extent(&self) -> [i32; 3] {
        match self {
            Self::Single => [1, 1, 1],
            Self::Wall | Self::EmitterWall => [WALL_SIZE, WALL_SIZE, 1],
            Self::Cube => [CUBE_SIZE, CUBE_SIZE, CUBE_SIZE],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StudioScene {
    pub sample: Voxel,
    pub pose: StudioPose,
    pub plate: Option<Voxel>,
    /// Imported asset geometry. Its cells deliberately remain 0.125 m detail.
    pub subject: Option<VoxSubject>,
}

impl Default for StudioScene {
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
    pub fn build(&self) -> Brickmap {
        let mut brickmap = Brickmap::empty();
        let [x, y, z] = SAMPLE_VOXEL;

        if let Some(plate) = self.plate {
            let plate_y = y - PLATE_DROP;
            let half = PLATE_HALF_EXTENT.max(self.subject_footprint_half_extent());
            for plate_z in (z - half)..=(z + half) {
                for plate_x in (x - half)..=(x + half) {
                    set_world(&mut brickmap, [plate_x, plate_y, plate_z], plate);
                }
            }
        }

        match &self.subject {
            Some(subject) => self.build_asset_subject(&mut brickmap, subject),
            None => {
                let origin = self.pose_origin();
                let [size_x, size_y, size_z] = self.pose.extent();
                for pose_y in 0..size_y {
                    for pose_z in 0..size_z {
                        for pose_x in 0..size_x {
                            set_world(
                                &mut brickmap,
                                [origin[0] + pose_x, origin[1] + pose_y, origin[2] + pose_z],
                                self.sample,
                            );
                        }
                    }
                }
                if self.pose == StudioPose::EmitterWall {
                    set_world(
                        &mut brickmap,
                        self.emitter_block_world_voxel(),
                        Voxel::GlowBlock,
                    );
                }
            }
        }
        brickmap
    }

    fn build_asset_subject(&self, brickmap: &mut Brickmap, subject: &VoxSubject) {
        let [x, y, z] = SAMPLE_VOXEL;
        let detail = DETAIL_CELLS_PER_WORLD_VOXEL as i32;
        let centre_x = x * detail + detail / 2;
        let centre_z = z * detail + detail / 2;
        let base_y = (y - PLATE_DROP + 1) * detail;
        let origin_x = centre_x - subject.size_x / 2;
        let origin_z = centre_z - subject.size_z / 2;

        for model_y in 0..subject.size_y {
            for model_z in 0..subject.size_z {
                for model_x in 0..subject.size_x {
                    let cell =
                        ((model_y * subject.size_z + model_z) * subject.size_x + model_x) as usize;
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

    /// The emitting block as a one-metre world coordinate.
    pub fn emitter_block_world_voxel(&self) -> [i32; 3] {
        let origin = self.pose_origin();
        let [size_x, size_y, _] = StudioPose::EmitterWall.extent();
        [origin[0] + size_x / 2, origin[1] + size_y / 2, origin[2]]
    }

    /// A representative detail cell at the centre of the emitting world block.
    ///
    /// CAGI and the ray tracer operate on detail-grid coordinates internally, so
    /// diagnostics use this conversion rather than treating the block as 0.125 m.
    pub fn emitter_block_voxel(&self) -> [i32; 3] {
        let [x, y, z] = self.emitter_block_world_voxel();
        let world = WorldVoxelCoord::new(x, y, z);
        debug_assert!(world.is_in_bounds());
        let mut detail = world.detail_origin();
        let centre = DETAIL_CELLS_PER_WORLD_VOXEL as i32 / 2;
        detail[0] += centre;
        detail[1] += centre;
        detail[2] += centre;
        detail
    }

    fn pose_origin(&self) -> [i32; 3] {
        let [x, y, z] = SAMPLE_VOXEL;
        let [size_x, _, size_z] = self.pose.extent();
        match self.pose {
            StudioPose::Single => [x, y, z],
            _ => [x - size_x / 2, y - PLATE_DROP + 1, z - size_z / 2],
        }
    }

    /// Required plate radius, measured in one-metre world voxels.
    fn subject_footprint_half_extent(&self) -> i32 {
        match &self.subject {
            Some(subject) => {
                let detail = DETAIL_CELLS_PER_WORLD_VOXEL as i32;
                let width = (subject.size_x.max(subject.size_z) + detail - 1) / detail;
                width / 2 + 2
            }
            None => {
                let [size_x, _, size_z] = self.pose.extent();
                size_x.max(size_z) / 2 + 2
            }
        }
    }

    pub fn sample_center_meters(&self) -> Vec3 {
        match &self.subject {
            Some(subject) => {
                let [x, y, z] = SAMPLE_VOXEL;
                Vec3::new(
                    (x as f32 + 0.5) * WORLD_VOXEL_SIZE_METERS,
                    (y - PLATE_DROP + 1) as f32 * WORLD_VOXEL_SIZE_METERS
                        + subject.size_y as f32 * DETAIL_CELL_SIZE_METERS * 0.5,
                    (z as f32 + 0.5) * WORLD_VOXEL_SIZE_METERS,
                )
            }
            None => {
                let origin = self.pose_origin();
                let [size_x, size_y, size_z] = self.pose.extent();
                Vec3::new(
                    (origin[0] as f32 + size_x as f32 * 0.5) * WORLD_VOXEL_SIZE_METERS,
                    (origin[1] as f32 + size_y as f32 * 0.5) * WORLD_VOXEL_SIZE_METERS,
                    (origin[2] as f32 + size_z as f32 * 0.5) * WORLD_VOXEL_SIZE_METERS,
                )
            }
        }
    }

    pub fn framing_distance_meters(&self) -> f32 {
        match &self.subject {
            Some(subject) => {
                let largest = subject.size_x.max(subject.size_y).max(subject.size_z) as f32;
                (largest * DETAIL_CELL_SIZE_METERS * 2.0).max(CAMERA_DISTANCE_METERS)
            }
            None => {
                let [size_x, size_y, size_z] = self.pose.extent();
                let largest = size_x.max(size_y).max(size_z) as f32;
                (largest * WORLD_VOXEL_SIZE_METERS * 2.0).max(CAMERA_DISTANCE_METERS)
            }
        }
    }
}

const PLATE_CLEARANCE: ClearanceUpdate = ClearanceUpdate::LocalBox { radius_cells: 8 };

fn set_world(brickmap: &mut Brickmap, coordinate: [i32; 3], material: Voxel) {
    let coordinate = WorldVoxelCoord::new(coordinate[0], coordinate[1], coordinate[2]);
    assert!(coordinate.is_in_bounds(), "studio world voxel is in bounds");
    brickmap.set_world_voxel(coordinate, material, PLATE_CLEARANCE);
}

pub fn orbit_pose(scene: &StudioScene, yaw: f32, pitch: f32, distance_meters: f32) -> CameraPose {
    let target = scene.sample_center_meters();
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let (sin_pitch, cos_pitch) = pitch.sin_cos();
    let forward = Vec3::new(cos_yaw * cos_pitch, sin_pitch, sin_yaw * cos_pitch).normalize();
    let position = target - forward * distance_meters.max(WORLD_VOXEL_SIZE_METERS * 2.0);
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
    use voxel_material::material::material_id;

    fn world_material(brickmap: &Brickmap, coordinate: [i32; 3]) -> u8 {
        let coordinate = WorldVoxelCoord::new(coordinate[0], coordinate[1], coordinate[2]);
        assert!(coordinate.is_in_bounds());
        let origin = coordinate.detail_origin();
        brickmap.get(origin[0], origin[1], origin[2])
    }

    #[test]
    fn built_in_single_is_one_uniform_metre_block() {
        let scene = StudioScene::default();
        let brickmap = scene.build();
        let world = WorldVoxelCoord::new(SAMPLE_VOXEL[0], SAMPLE_VOXEL[1], SAMPLE_VOXEL[2]);
        let origin = world.detail_origin();
        let expected = material_id(scene.sample);
        for y in 0..DETAIL_CELLS_PER_WORLD_VOXEL as i32 {
            for z in 0..DETAIL_CELLS_PER_WORLD_VOXEL as i32 {
                for x in 0..DETAIL_CELLS_PER_WORLD_VOXEL as i32 {
                    assert_eq!(
                        brickmap.get(origin[0] + x, origin[1] + y, origin[2] + z),
                        expected
                    );
                }
            }
        }
        assert_eq!(
            world_material(
                &brickmap,
                [SAMPLE_VOXEL[0], SAMPLE_VOXEL[1] + 1, SAMPLE_VOXEL[2]]
            ),
            0
        );
    }

    #[test]
    fn plate_and_gap_are_measured_in_world_voxels() {
        let scene = StudioScene::default();
        let brickmap = scene.build();
        let [x, y, z] = SAMPLE_VOXEL;
        assert_eq!(
            world_material(&brickmap, [x, y - PLATE_DROP, z]),
            material_id(Voxel::Snow)
        );
        for gap in 1..PLATE_DROP {
            assert_eq!(world_material(&brickmap, [x, y - gap, z]), 0);
        }
        assert_eq!(
            world_material(&brickmap, [x + PLATE_HALF_EXTENT + 1, y - PLATE_DROP, z]),
            0
        );
    }

    #[test]
    fn poses_are_built_from_whole_world_voxels() {
        for pose in StudioPose::ALL {
            let scene = StudioScene {
                pose,
                ..StudioScene::default()
            };
            let brickmap = scene.build();
            let origin = scene.pose_origin();
            let extent = pose.extent();
            for y in 0..extent[1] {
                for z in 0..extent[2] {
                    for x in 0..extent[0] {
                        let coordinate = [origin[0] + x, origin[1] + y, origin[2] + z];
                        let expected = if pose == StudioPose::EmitterWall
                            && coordinate == scene.emitter_block_world_voxel()
                        {
                            Voxel::GlowBlock
                        } else {
                            scene.sample
                        };
                        assert_eq!(world_material(&brickmap, coordinate), material_id(expected));
                    }
                }
            }
            assert_eq!(
                world_material(&brickmap, [origin[0], origin[1] + extent[1], origin[2]]),
                0
            );
        }
    }

    #[test]
    fn imported_vox_subject_keeps_detail_cells() {
        let scene = StudioScene {
            subject: Some(VoxSubject {
                size_x: 2,
                size_y: 1,
                size_z: 1,
                cells: vec![
                    Some(material_id(Voxel::Stone)),
                    Some(material_id(Voxel::Sand)),
                ],
            }),
            ..StudioScene::default()
        };
        let brickmap = scene.build();
        let detail = DETAIL_CELLS_PER_WORLD_VOXEL as i32;
        let base_y = (SAMPLE_VOXEL[1] - PLATE_DROP + 1) * detail;
        let centre_x = SAMPLE_VOXEL[0] * detail + detail / 2;
        let centre_z = SAMPLE_VOXEL[2] * detail + detail / 2;
        assert_eq!(
            brickmap.get(centre_x - 1, base_y, centre_z),
            material_id(Voxel::Stone)
        );
        assert_eq!(
            brickmap.get(centre_x, base_y, centre_z),
            material_id(Voxel::Sand)
        );
        assert_eq!(brickmap.get(centre_x + 1, base_y, centre_z), 0);
    }

    #[test]
    fn camera_frames_world_poses_and_detail_assets_in_metres() {
        let single = StudioScene::default();
        assert_eq!(single.framing_distance_meters(), CAMERA_DISTANCE_METERS);
        let pose = orbit_pose(&single, 0.7, 0.2, 0.0);
        assert!((single.sample_center_meters() - pose.position).length() >= 1.999);

        let wall = StudioScene {
            pose: StudioPose::Wall,
            ..StudioScene::default()
        };
        assert_eq!(wall.framing_distance_meters(), WALL_SIZE as f32 * 2.0);
        let origin = wall.pose_origin();
        let expected = Vec3::new(
            (origin[0] as f32 + WALL_SIZE as f32 * 0.5) * WORLD_VOXEL_SIZE_METERS,
            (origin[1] as f32 + WALL_SIZE as f32 * 0.5) * WORLD_VOXEL_SIZE_METERS,
            (origin[2] as f32 + 0.5) * WORLD_VOXEL_SIZE_METERS,
        );
        assert!((wall.sample_center_meters() - expected).length() < 1e-5);
    }
}
