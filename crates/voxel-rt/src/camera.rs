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
use voxel_core::world::{VOXEL_SIZE, WATER_LEVEL, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z};

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
/// Maximum independently visible flares. This stays fixed because the DDA
/// camera uniform is a fixed-size GPU contract, not a growable scene buffer.
pub const MAX_FLYING_FLARES: usize = 16;

/// One transient flare slot in [`CameraUniform`]. The two vec3/f32 pairs are
/// 16-byte aligned in both Rust and WGSL.
///
/// Everything here is in VOXEL UNITS, unlike the metres the rest of
/// [`CameraUniform`] carries, and both sphere radii are stated rather than one
/// being derived from the other. That is deliberate: the shader intersects flares
/// in the traversal space it is already in, and the metre->voxel conversion is a
/// frame constant. Converting on the CPU costs a handful of divides per frame;
/// leaving it in the loop cost one divide per radius per flare per PIXEL, which at
/// 1440p with a full slot table is a few hundred million divides a frame for a
/// value that never varies across the image.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuFlare {
    pub head_voxels: [f32; 3],
    pub head_radius_voxels: f32,
    pub tail_voxels: [f32; 3],
    /// Also the slot's occupancy flag, which is why the shader tests it instead of
    /// carrying a separate `enabled` float: a flare with no ember to draw is not a
    /// flare, so a zero radius and an unused slot are the same state.
    pub tail_radius_voxels: f32,
}

impl GpuFlare {
    pub const INACTIVE: GpuFlare = GpuFlare {
        head_voxels: [0.0; 3],
        head_radius_voxels: 0.0,
        tail_voxels: [0.0; 3],
        tail_radius_voxels: 0.0,
    };
}

unsafe impl bytemuck::Zeroable for GpuFlare {}
unsafe impl bytemuck::Pod for GpuFlare {}

/// `#[repr(C)]` layout (608 bytes, 16-byte aligned — matches the WGSL
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
/// | 72     | `debug_no_clip`| `f32`       |
/// | 76     | `_pad4`        | `f32`       |
/// | 80     | `flare_count`  | `u32`       |
/// | 84     | `_flare_pad[0]`| `u32`       |
/// | 88     | `_flare_pad[1]`| `u32`       |
/// | 92     | `_flare_pad[2]`| `u32`       |
/// | 96     | `flares`       | `array<GpuFlare, 16>` |
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
    /// Debug no-clip flight asks primary rays that begin in solid material to
    /// skip that enclosing volume and resume at its first air cell.
    pub debug_no_clip: f32,
    pub _pad4: f32,
    /// Prefix of [`Self::flares`] occupied this frame.
    pub flare_count: u32,
    pub _flare_pad: [u32; 3],
    /// Renderer-only moving shots, in voxel units (see [`GpuFlare`]). Attached
    /// flares become regular brickmap geometry after impact.
    pub flares: [GpuFlare; MAX_FLYING_FLARES],
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
            debug_no_clip: 0.0,
            _pad4: 0.0,
            flare_count: 0,
            _flare_pad: [0; 3],
            flares: [GpuFlare::INACTIVE; MAX_FLYING_FLARES],
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
    /// Grounded slow-movement intent. The fly camera ignores it; the character
    /// uses it to enter its sneak speed while keeping `down` available to dive.
    pub sneak: bool,
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
            sneak: false,
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

/// Vertical gap the ejected eye keeps above the first free cell it finds, world
/// meters. Four voxels — enough that the eye is not resting on the surface it was
/// buried in, and small enough that the view is still of that surface.
pub const EJECT_CLEARANCE_METERS: f32 = 0.5;

/// Lift an eye that solid material has closed around until it is in free space.
///
/// A regenerated world is a NEW world: nothing says the terrain under the eye is
/// where it was before, and fly collision is always on, so an eye that a rebuild
/// left inside rock cannot cross a single cell boundary to get out — it is trapped
/// for the rest of the session while every primary ray starts inside solid
/// material, which reads as a dead screen rather than as a camera that needs
/// moving. Every path that replaces the brickmap owes the eye this.
///
/// Straight up rather than "back to where it came from": the previous pose is just
/// as likely to be inside the new terrain, whereas up always terminates — worst
/// case at the world ceiling, which is open sky. Both the landing cell and the
/// clearance above it must be free, so a one-voxel seam inside the rock is not
/// mistaken for the surface.
///
/// `cell_is_solid` answers for a point in world METERS and must answer FALSE for
/// liquids: a submerged camera renders correctly (E6's water medium march), and
/// being spat out of a lake by a world rebuild would be the surprising behaviour.
pub fn eject_eye_from_solid(eye_position: Vec3, cell_is_solid: impl Fn(Vec3) -> bool) -> Vec3 {
    if !cell_is_solid(eye_position) {
        return eye_position;
    }
    let world_ceiling_meters = WORLD_SIZE_Y as f32 * VOXEL_SIZE;
    let clearance = Vec3::Y * EJECT_CLEARANCE_METERS;
    // One voxel per step: the coarsest stride that cannot step over a free cell.
    let mut candidate = eye_position;
    while candidate.y < world_ceiling_meters {
        candidate.y += VOXEL_SIZE;
        if !cell_is_solid(candidate) && !cell_is_solid(candidate + clearance) {
            return candidate + clearance;
        }
    }
    // A column solid to the ceiling has no surface to stand above. Above the world
    // is always free, and the eye can fly back down wherever it likes from there.
    Vec3::new(
        eye_position.x,
        world_ceiling_meters + EJECT_CLEARANCE_METERS,
        eye_position.z,
    )
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
    /// 125 m x 64 m x 125 m; the water plane sits at `WATER_LEVEL` (263
    /// detail cells) = 32.875 m and procedural peaks reach roughly 52 m,
    /// while the rim stays near water level. Spawning at x = 62.5 m (center),
    /// z = 107.5 m (0.86 * 125, above the low rim), y = 52.9 m (20 m above
    /// the water plane) is safely clear of terrain;
    /// yaw = -PI/2 faces -Z toward the island center ~45 m away and
    /// pitch = -0.33 rad (~-19 deg) drops the view onto the terrain.
    fn default() -> FlyCamera {
        let world_x_meters = WORLD_SIZE_X as f32 * VOXEL_SIZE;
        let world_z_meters = WORLD_SIZE_Z as f32 * VOXEL_SIZE;
        let water_meters = WATER_LEVEL as f32 * VOXEL_SIZE;
        FlyCamera {
            position: Vec3::new(
                world_x_meters * 0.5,
                water_meters + 20.0,
                world_z_meters * 0.86,
            ),
            yaw: -std::f32::consts::FRAC_PI_2,
            pitch: -0.33,
            movement_speed: 10.79,
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
        assert_eq!(std::mem::size_of::<GpuFlare>(), 32);
        assert_eq!(std::mem::size_of::<CameraUniform>(), 608);
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

    /// A buried eye comes out just above the surface it was buried in, and an eye
    /// that is already free is left exactly where it is (a rebuild must not nudge a
    /// camera that was fine).
    #[test]
    fn ejection_lands_just_above_the_surface() {
        let terrain_top_meters = 10.0;
        let solid_below_ten = |point: Vec3| point.y < terrain_top_meters;

        let ejected = eject_eye_from_solid(Vec3::new(3.0, 5.0, 7.0), solid_below_ten);
        assert_eq!(ejected.x, 3.0);
        assert_eq!(ejected.z, 7.0);
        assert_eq!(ejected.y, terrain_top_meters + EJECT_CLEARANCE_METERS);
        assert!(!solid_below_ten(ejected));

        let free = Vec3::new(3.0, 20.0, 7.0);
        assert_eq!(eject_eye_from_solid(free, solid_below_ten), free);
    }

    /// A column that is solid all the way up has no surface to sit above, so the
    /// only free space left is over the world. The search must terminate there
    /// rather than scan forever.
    #[test]
    fn ejection_escapes_a_fully_solid_column() {
        let world_ceiling_meters = WORLD_SIZE_Y as f32 * VOXEL_SIZE;
        let ejected = eject_eye_from_solid(Vec3::new(1.0, 4.0, 2.0), |_| true);
        assert_eq!(ejected.y, world_ceiling_meters + EJECT_CLEARANCE_METERS);
        assert_eq!(ejected.x, 1.0);
        assert_eq!(ejected.z, 2.0);
    }

    /// A one-voxel void inside the rock is not the surface. The eye must keep
    /// climbing until the clearance above the landing cell is free too, otherwise
    /// the ejection would leave it re-buried by its own clearance offset.
    #[test]
    fn ejection_skips_a_pocket_too_thin_to_stand_in() {
        let pocket_y = 6.0;
        let terrain_top_meters = 10.0;
        let ejected = eject_eye_from_solid(Vec3::new(0.5, 2.0, 0.5), |point| {
            if (point.y - pocket_y).abs() < VOXEL_SIZE * 0.5 {
                return false;
            }
            point.y < terrain_top_meters
        });
        assert_eq!(ejected.y, terrain_top_meters + EJECT_CLEARANCE_METERS);
    }

    #[test]
    fn default_fly_speed_matches_the_minecraft_target() {
        assert!((FlyCamera::default().movement_speed - 10.79).abs() < f32::EPSILON);
    }
}
