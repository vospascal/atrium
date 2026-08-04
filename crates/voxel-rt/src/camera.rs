//! Pure-math camera: no winit, no wgpu, no renderer knowledge.
//!
//! Two cleanly separated layers (plan architecture rule — the camera doubles
//! as the VR head-pose slot later):
//!
//! - [`CameraPose`] — a position + orientation and its conversion to the
//!   per-frame GPU ray basis ([`CameraUniform`]). A future OpenXR backend
//!   builds a `CameraPose` straight from the tracked head pose and never
//!   touches the fly logic.
//! - [`FlyCamera`] — desktop FPS-style movement (WASD + mouse yaw/pitch)
//!   that *produces* a `CameraPose`. The event loop maps raw input into a
//!   [`CameraInput`] and calls [`FlyCamera::update`] once per frame.
//!
//! Coordinate conventions: right-handed, +Y up, positions in world METERS
//! (the shader divides by the brickmap's `voxel_size_meters` to enter voxel
//! space). Yaw is radians around +Y with yaw 0 looking along +X and
//! yaw = -PI/2 looking along -Z; pitch is radians above the horizon,
//! clamped just short of +/-90 degrees.

use glam::Vec3;
use voxel_core::world::{VOXEL_SIZE, WATER_LEVEL, WORLD_SIZE_X, WORLD_SIZE_Z};

/// Vertical field of view used by the default camera, radians (~60 deg).
pub const DEFAULT_VERTICAL_FOV_RADIANS: f32 = 60.0 * std::f32::consts::PI / 180.0;

/// Pitch clamp: just short of straight up/down so the basis never degenerates.
const MAX_PITCH_RADIANS: f32 = std::f32::consts::FRAC_PI_2 - 0.01;

/// World metres one pixel spans at ONE METRE from the eye.
///
/// Multiply by a hit distance in metres and the result is that hit's pixel
/// footprint — the size below which detail cannot be resolved and only aliases.
/// The shading pass carries it in `MaterialParams` so a fractal generator can
/// stop summing octaves it cannot show (`PATTERN_OCTAVE_LOD`).
///
/// Vertical rather than horizontal because the vertical FOV is the authored one;
/// square pixels make the two agree anyway. Guards a zero height so a headless
/// caller with an unset resolution gets a harmless zero footprint (every octave
/// kept) rather than an infinity that would drop them all.
pub(crate) fn pixel_footprint_at_one_meter(vertical_fov_radians: f32, height_pixels: u32) -> f32 {
    if height_pixels == 0 {
        return 0.0;
    }
    2.0 * (vertical_fov_radians * 0.5).tan() / height_pixels as f32
}

/// Per-frame camera data for the DDA compute shader, bindable as a uniform.
///
/// The shader reconstructs each pixel's ray as
/// `ray_direction = normalize(forward + ndc_x * right_scaled + ndc_y * up_scaled)`
/// with `ndc_x` in [-1, 1] left->right and `ndc_y` in [-1, 1] bottom->top
/// (`right_scaled = right * tan(fov_y/2) * aspect`,
/// `up_scaled = up * tan(fov_y/2)`).
///
/// `#[repr(C)]` layout (80 bytes, 16-byte aligned — matches the WGSL
/// `Camera` struct in `shaders/dda.wgsl`; every `vec3<f32>` is padded to 16
/// bytes with an explicit pad float):
///
/// | offset | field          | WGSL type   |
/// |--------|----------------|-------------|
/// | 0      | `position`     | `vec3<f32>` (world meters) |
/// | 12     | `_pad0`        | `f32`       |
/// | 16     | `forward`      | `vec3<f32>` (unit)         |
/// | 28     | `_pad1`        | `f32`       |
/// | 32     | `right_scaled` | `vec3<f32>` |
/// | 44     | `_pad2`        | `f32`       |
/// | 48     | `up_scaled`    | `vec3<f32>` |
/// | 60     | `_pad3`        | `f32`       |
/// | 64     | `resolution`   | `vec2<f32>` (pixels)       |
/// | 72     | `_pad4`        | `vec2<f32>` |
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraUniform {
    pub position: [f32; 3],
    pub _pad0: f32,
    pub forward: [f32; 3],
    pub _pad1: f32,
    pub right_scaled: [f32; 3],
    pub _pad2: f32,
    pub up_scaled: [f32; 3],
    pub _pad3: f32,
    pub resolution: [f32; 2],
    pub _pad4: [f32; 2],
}

// Manual impls instead of derive so we do not depend on bytemuck's `derive`
// feature flag: `#[repr(C)]`, all-f32 fields, no implicit padding (the pads
// are explicit fields).
unsafe impl bytemuck::Zeroable for CameraUniform {}
unsafe impl bytemuck::Pod for CameraUniform {}

/// A position + orientation, renderer- and input-agnostic.
///
/// This is the seam between "where the viewpoint is" and "how it got there":
/// the fly camera produces one per frame; a VR backend will produce one per
/// eye from the tracked head pose instead.
#[derive(Clone, Copy, Debug)]
pub struct CameraPose {
    /// Eye position, world meters.
    pub position: Vec3,
    /// Unit view direction.
    pub forward: Vec3,
    /// Unit right vector (`forward x world_up`, re-orthogonalized).
    pub right: Vec3,
    /// Unit camera up (`right x forward`).
    pub up: Vec3,
}

impl CameraPose {
    /// Build a pose from yaw/pitch Euler angles (the fly-camera path).
    pub fn from_yaw_pitch(position: Vec3, yaw: f32, pitch: f32) -> CameraPose {
        let (sin_yaw, cos_yaw) = yaw.sin_cos();
        let (sin_pitch, cos_pitch) = pitch.sin_cos();
        let forward = Vec3::new(cos_yaw * cos_pitch, sin_pitch, sin_yaw * cos_pitch);
        let right = forward.cross(Vec3::Y).normalize();
        let up = right.cross(forward);
        CameraPose {
            position,
            forward,
            right,
            up,
        }
    }

    /// Convert this pose to the shader's ray basis for one frame.
    ///
    /// `vertical_fov_radians` is the FULL vertical field of view;
    /// `resolution` the output texture size in pixels (drives the aspect
    /// ratio baked into `right_scaled`).
    pub fn gpu_uniform(&self, vertical_fov_radians: f32, resolution: (u32, u32)) -> CameraUniform {
        let aspect = resolution.0 as f32 / resolution.1.max(1) as f32;
        let half_tangent = (vertical_fov_radians * 0.5).tan();
        CameraUniform {
            position: self.position.to_array(),
            _pad0: 0.0,
            forward: self.forward.to_array(),
            _pad1: 0.0,
            right_scaled: (self.right * half_tangent * aspect).to_array(),
            _pad2: 0.0,
            up_scaled: (self.up * half_tangent).to_array(),
            _pad3: 0.0,
            resolution: [resolution.0 as f32, resolution.1 as f32],
            _pad4: [0.0, 0.0],
        }
    }
}

/// One frame of movement intent, already mapped from raw window events by
/// the platform layer (this module never sees key codes or mouse events).
#[derive(Clone, Copy, Debug)]
pub struct CameraInput {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
    /// Mouse motion this frame, pixels (+x right, +y down — raw winit delta).
    pub mouse_delta: (f32, f32),
    /// Speed scale, 1.0 = normal (e.g. shift-to-sprint maps to > 1.0).
    pub speed_multiplier: f32,
}

impl Default for CameraInput {
    fn default() -> CameraInput {
        CameraInput {
            forward: false,
            backward: false,
            left: false,
            right: false,
            up: false,
            down: false,
            mouse_delta: (0.0, 0.0),
            speed_multiplier: 1.0,
        }
    }
}

/// Desktop FPS-style fly camera: mouse steers yaw/pitch, WASD moves in the
/// horizontal plane relative to yaw, up/down move along world +Y.
#[derive(Clone, Copy, Debug)]
pub struct FlyCamera {
    /// Eye position, world meters.
    pub position: Vec3,
    /// Radians around +Y; 0 looks along +X, -PI/2 along -Z.
    pub yaw: f32,
    /// Radians above the horizon, clamped to +/- (PI/2 - 0.01).
    pub pitch: f32,
    /// Base movement speed, meters per second.
    pub movement_speed: f32,
    /// Radians of yaw/pitch per pixel of mouse motion.
    pub mouse_sensitivity: f32,
    /// Full vertical field of view, radians.
    pub vertical_fov_radians: f32,
}

/// Movement-speed band the mouse wheel can reach, meters per second. The floor
/// is slow enough to line up a single 0.125 m voxel; the ceiling crosses the
/// 125 m island in about two seconds.
pub(crate) const MIN_MOVEMENT_SPEED: f32 = 0.25;
pub(crate) const MAX_MOVEMENT_SPEED: f32 = 64.0;

impl Default for FlyCamera {
    /// Spawn above the island's southern rim looking north-and-down at the
    /// island center.
    ///
    /// Derivation (voxel-core constants, 0.125 m/voxel): the world is
    /// 125 m x 32 m x 125 m; the water plane sits at `WATER_LEVEL` (84
    /// voxels) = 10.5 m and procedural peaks reach ~31 m only near the
    /// center, while the rim stays near water level. Spawning at
    /// x = 62.5 m (center), z = 107.5 m (0.86 * 125, above the low rim),
    /// y = 28 m (17.5 m above the water plane) is safely clear of terrain;
    /// yaw = -PI/2 faces -Z toward the island center ~45 m away and
    /// pitch = -0.33 rad (~-19 deg) drops the view onto the terrain.
    fn default() -> FlyCamera {
        let world_x_meters = WORLD_SIZE_X as f32 * VOXEL_SIZE;
        let world_z_meters = WORLD_SIZE_Z as f32 * VOXEL_SIZE;
        let water_meters = WATER_LEVEL as f32 * VOXEL_SIZE;
        FlyCamera {
            position: Vec3::new(
                world_x_meters * 0.5,
                water_meters + 17.5,
                world_z_meters * 0.86,
            ),
            yaw: -std::f32::consts::FRAC_PI_2,
            pitch: -0.33,
            movement_speed: 4.0,
            mouse_sensitivity: 0.0025,
            vertical_fov_radians: DEFAULT_VERTICAL_FOV_RADIANS,
        }
    }
}

impl FlyCamera {
    /// Scale [`FlyCamera::movement_speed`] by `notches` of mouse wheel.
    /// Multiplicative, so one notch feels like the same change whether you are
    /// inching along a wall or crossing the island, and clamped to
    /// [`MIN_MOVEMENT_SPEED`]..=[`MAX_MOVEMENT_SPEED`].
    pub fn adjust_movement_speed(&mut self, notches: f32) {
        self.movement_speed = (self.movement_speed * 1.2f32.powf(notches))
            .clamp(MIN_MOVEMENT_SPEED, MAX_MOVEMENT_SPEED);
    }

    /// Advance the camera one frame: mouse delta -> yaw/pitch (pitch
    /// clamped), then movement keys -> position. Horizontal movement is
    /// relative to yaw only (looking down does not slow forward flight);
    /// up/down are world-axis vertical.
    pub fn update(&mut self, input: &CameraInput, delta_seconds: f32) {
        self.yaw += input.mouse_delta.0 * self.mouse_sensitivity;
        self.yaw = self.yaw.rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch - input.mouse_delta.1 * self.mouse_sensitivity)
            .clamp(-MAX_PITCH_RADIANS, MAX_PITCH_RADIANS);

        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let flat_forward = Vec3::new(cos_yaw, 0.0, sin_yaw);
        let flat_right = Vec3::new(-sin_yaw, 0.0, cos_yaw);

        let mut wish = Vec3::ZERO;
        if input.forward {
            wish += flat_forward;
        }
        if input.backward {
            wish -= flat_forward;
        }
        if input.right {
            wish += flat_right;
        }
        if input.left {
            wish -= flat_right;
        }
        if input.up {
            wish.y += 1.0;
        }
        if input.down {
            wish.y -= 1.0;
        }
        if wish.length_squared() > 0.0 {
            wish = wish.normalize();
            self.position += wish * self.movement_speed * input.speed_multiplier * delta_seconds;
        }
    }

    /// The current pose (the seam a VR backend replaces).
    pub fn pose(&self) -> CameraPose {
        CameraPose::from_yaw_pitch(self.position, self.yaw, self.pitch)
    }

    /// Convenience: this frame's GPU uniform at the camera's own FOV.
    pub fn gpu_uniform(&self, resolution: (u32, u32)) -> CameraUniform {
        self.pose()
            .gpu_uniform(self.vertical_fov_radians, resolution)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_layout_is_gpu_ready() {
        assert_eq!(std::mem::size_of::<CameraUniform>(), 80);
        assert_eq!(std::mem::align_of::<CameraUniform>(), 4);
    }

    #[test]
    fn pose_basis_is_orthonormal() {
        let pose = CameraPose::from_yaw_pitch(Vec3::ZERO, 1.234, -0.5);
        assert!((pose.forward.length() - 1.0).abs() < 1e-5);
        assert!((pose.right.length() - 1.0).abs() < 1e-5);
        assert!((pose.up.length() - 1.0).abs() < 1e-5);
        assert!(pose.forward.dot(pose.right).abs() < 1e-5);
        assert!(pose.forward.dot(pose.up).abs() < 1e-5);
        assert!(pose.right.dot(pose.up).abs() < 1e-5);
        // Right-handedness: right x forward = up (not -up).
        assert!((pose.right.cross(pose.forward) - pose.up).length() < 1e-5);
    }

    #[test]
    fn pitch_clamps_short_of_vertical() {
        let mut camera = FlyCamera::default();
        let mut input = CameraInput {
            mouse_delta: (0.0, -1_000_000.0), // yank the mouse up
            ..CameraInput::default()
        };
        camera.update(&input, 1.0 / 60.0);
        assert!(camera.pitch <= MAX_PITCH_RADIANS);
        input.mouse_delta = (0.0, 1_000_000.0);
        camera.update(&input, 1.0 / 60.0);
        assert!(camera.pitch >= -MAX_PITCH_RADIANS);
    }

    #[test]
    fn ray_basis_matches_fov() {
        let pose = CameraPose::from_yaw_pitch(Vec3::ZERO, 0.0, 0.0);
        let uniform = pose.gpu_uniform(DEFAULT_VERTICAL_FOV_RADIANS, (1600, 900));
        let up_scaled = Vec3::from_array(uniform.up_scaled);
        let right_scaled = Vec3::from_array(uniform.right_scaled);
        let half_tangent = (DEFAULT_VERTICAL_FOV_RADIANS * 0.5).tan();
        assert!((up_scaled.length() - half_tangent).abs() < 1e-5);
        assert!((right_scaled.length() - half_tangent * 1600.0 / 900.0).abs() < 1e-4);
    }

    #[test]
    fn default_spawn_is_inside_world_and_above_water() {
        let camera = FlyCamera::default();
        let world_x = WORLD_SIZE_X as f32 * VOXEL_SIZE;
        let world_z = WORLD_SIZE_Z as f32 * VOXEL_SIZE;
        assert!(camera.position.x > 0.0 && camera.position.x < world_x);
        assert!(camera.position.z > 0.0 && camera.position.z < world_z);
        assert!(camera.position.y > WATER_LEVEL as f32 * VOXEL_SIZE);
    }
}
