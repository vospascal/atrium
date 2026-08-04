//! E2b — the walk-mode character controller: a *body* in the voxel world.
//!
//! Pure math, exactly like [`crate::camera`]: no winit, no wgpu, no renderer
//! type. Its only world input is `&`[`Brickmap`] (an `Arc<RwLock<Brickmap>>`
//! read guard from [`crate::world_host`] derefs straight into it), and it speaks
//! world METERS on the way in and out. That purity is not tidiness, it is the
//! seam three later experiments plug into:
//!
//! - **E8 (audio bridge): the listener becomes a body.** [`Self::eye_position`]
//!   at head height — not the fly camera's free-floating point — is the pose
//!   atrium's HRTF listener should take, and the same `&Brickmap` that resolves
//!   collision here resolves occlusion there ([`crate::voxel_dda`]). A listener
//!   that stands on the ground, wades and submerges is the difference between
//!   "sound in a scene" and "being there"; [`Self::head_submerged`] is the
//!   submerged-listener flag that costs nothing to carry now.
//! - **E9 (Quest): this body IS the VR player.** The platform layer will feed
//!   yaw/pitch from a tracked head pose instead of a mouse
//!   ([`CameraPose`] is already that slot), while gravity, collision, step-up
//!   and the water states stay exactly this code.
//! - **E6 (water rendering)** consumes [`Self::head_submerged`]; this module
//!   deliberately does no rendering decision of its own.
//!
//! ## The body
//!
//! An axis-aligned box, `body_width_meters` square in plan and
//! `body_height_meters` tall, whose ORIGIN is the centre of its base
//! ("feet"), with the eye at `eye_height_meters` above that. Collision is a
//! **swept, per-axis** resolve: move X and resolve it, then Y, then Z, testing
//! **every voxel layer the leading face crosses** rather than only the layer it
//! lands in. That is what makes the guarantee below hold at any speed.
//!
//! ## Solidity: `Voxel::is_solid()`, deliberately
//!
//! Blocking is [`material_blocks_movement`], i.e. `voxel-core`'s
//! `Voxel::is_solid()`. It excludes **water** and **thin cover** (tall grass,
//! flowers, reeds, lily pads, weeds), so you walk *through* vegetation and
//! *into* water, which preserves the intended feel. Note the
//! consequence that leaves ARE solid (`is_solid` counts them): you can stand on
//! a canopy here where sandbox lets you fall through it. That is one predicate
//! away if it ever reads wrong, and it keeps tree trunks solid without a
//! special case.
//!
//! ## Water: buoyancy is a spring to the SURFACE, not a lift
//!
//! Per-voxel, no global water plane. Feet wet = [`Submersion::Wading`], water
//! over the shoulders = [`Submersion::Swimming`]. A swimmer's vertical model is
//! **one restoring force toward the float line whose strength fades with
//! depth** ([`CharacterController::buoyant_acceleration`]) — near the surface
//! the body is pinned head-out and has to actively dive; past
//! [`SWIM_SURFACE_BAND_METERS`] it is neutral and drifts down slowly, so you can
//! hold a depth instead of being corked back up (Pascal's requirement, and
//! the original controller feel, whose swimmer is a spring toward `water_surface + 0.1`).
//! A *constant* buoyant acceleration — the first version of this module — pushes
//! the body to the surface from any depth, which is the behaviour this shape
//! exists to remove.
//!
//! ## Anti-tunneling guarantee
//!
//! Two independent mechanisms, both required, because a 40 ms hitch at sprint
//! speed is 0.5 m and a stalled second at terminal velocity is 55 m:
//!
//! 1. **The step is clamped and substepped**: `delta_seconds` is capped at
//!    [`MAX_STEP_SECONDS`] and the remaining motion is split so no substep moves
//!    the body more than [`MAX_SUBSTEP_METERS`] — strictly less than half the
//!    body's smallest dimension, so a substep can never straddle a wall it did
//!    not touch.
//! 2. **Every sweep tests every layer it crosses**, and refuses to move past a
//!    layer it has not tested ([`MAX_SWEEP_LAYERS`]). So even a call that
//!    escaped (1) cannot pass through geometry — it stops at the last verified
//!    voxel boundary instead.
//!
//! `absurd_frame_deltas_never_end_inside_solid` in this module's tests is the
//! evidence: a body fired in 96 directions at maximum speed with a 1000 ms
//! delta ends outside solid geometry every time.

use glam::Vec3;
use voxel_core::world::{VOXEL_SIZE, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z};

use crate::brickmap::Brickmap;
use crate::camera::{CameraInput, CameraPose, DEFAULT_VERTICAL_FOV_RADIANS};
use voxel_material::material::{material_blocks_movement, material_is_liquid};

/// Body width and depth, meters (0.6 m = 4.8 voxels). Wide enough to feel like
/// a person, narrow enough to walk a 1 m gap between boulders.
pub(crate) const BODY_WIDTH_METERS: f32 = 0.6;
/// Body height, meters (1.8 m = 14.4 voxels).
pub(crate) const BODY_HEIGHT_METERS: f32 = 1.8;
/// Eye height above the feet, meters — the listener/head pose (E8/E9).
pub const EYE_HEIGHT_METERS: f32 = 1.65;

/// Downward acceleration, m/s^2. Deliberately above real gravity (the same 22.0
/// the original controller settled on): 9.81 makes a jump feel like a moon hop, and the
/// snappier value is what makes a 1.2 m jump land in ~0.47 s.
pub(crate) const GRAVITY_METERS_PER_SECOND_SQUARED: f32 = 22.0;
/// Fall-speed ceiling, m/s. Roughly a real skydiver's terminal velocity; here it
/// also *bounds the substep count* — the worst fall a frame can integrate is
/// `TERMINAL_VELOCITY * MAX_STEP_SECONDS` = 5.5 m = 22 substeps.
pub(crate) const TERMINAL_VELOCITY_METERS_PER_SECOND: f32 = 55.0;
/// How high a jump rises, meters. The jump *speed* is derived from this and
/// gravity ([`CharacterSettings::jump_speed`]) so the tunable is the thing you
/// can see rather than an impulse you have to convert in your head.
pub(crate) const JUMP_APEX_METERS: f32 = 1.2;

/// Auto-step height, meters. **Exactly 3 voxels** (0.375 m), not the round
/// 0.35 m: voxels are 0.125 m, natural terrain steps are 1-3 voxels, and 0.35
/// would fail the 3-voxel step by 2.5 cm — which is precisely the case that
/// makes walking an island miserable.
pub(crate) const STEP_UP_METERS: f32 = 3.0 * VOXEL_SIZE;

/// Walk speed, m/s.
pub(crate) const WALK_SPEED_METERS_PER_SECOND: f32 = 4.5;
/// Cap on the platform layer's speed multiplier while walking. The fly camera's
/// boost is 4x, which at walk speed is 18 m/s — a sprint the collision feel
/// cannot support; 2.6x = 11.7 m/s matches sandbox's 12 m/s sprint.
pub(crate) const SPRINT_MULTIPLIER: f32 = 2.6;
/// Wheel-tunable walk-speed band, m/s: slow enough to line up a single voxel,
/// fast enough to cross the island without switching to fly.
pub(crate) const MIN_WALK_SPEED: f32 = 0.5;
pub(crate) const MAX_WALK_SPEED: f32 = 16.0;

/// Horizontal speed scale while the feet are in water — the "wading is heavy"
/// term. Cheap (one multiply) and the whole of water v0's horizontal model.
pub(crate) const WADE_SPEED_SCALE: f32 = 0.55;
/// Horizontal speed while swimming, m/s.
pub(crate) const SWIM_SPEED_METERS_PER_SECOND: f32 = 2.2;
/// Fraction of the body height at which submersion becomes *swimming*. 0.8 =
/// 1.44 m, i.e. water over the shoulders. The float line sits
/// [`SWIM_FLOAT_SHOULDER_DEPTH_METERS`] *below* this threshold, so the
/// equilibrium and the state boundary differ by a margin — which is what keeps
/// the state from dithering at the surface.
pub(crate) const SWIM_SUBMERSION_FRACTION: f32 = 0.8;
/// How far under the local water surface the shoulder probe rests at the float
/// line, meters. Half a voxel: the probe sits in the middle of the topmost water
/// voxel, so the `Swimming` test keeps 0.06 m of slack either way, and the eye
/// ends up 0.15 m clear of the surface.
pub(crate) const SWIM_FLOAT_SHOULDER_DEPTH_METERS: f32 = 0.5 * VOXEL_SIZE;
/// Depth band below the float line over which buoyancy still lifts, meters.
/// Deeper than this the body is *neutral* — the point of the whole model: you
/// float at the SURFACE, you are not shoved up from depth, so a swimmer can hold
/// a depth and explore instead of being corked out of the water.
pub(crate) const SWIM_SURFACE_BAND_METERS: f32 = 0.75;
/// Restoring stiffness at the float line, 1/s^2 (acceleration per meter of
/// displacement). With [`WATER_VERTICAL_DRAG_PER_SECOND`] this is a damped
/// spring: 0.2 m under the line lifts at 0.6 m/s, and the dive thrust
/// ([`SWIM_THRUST_METERS_PER_SECOND_SQUARED`]) beats it several times over, so
/// pressing dive always wins.
pub(crate) const SWIM_BUOYANCY_STIFFNESS_PER_SECOND_SQUARED: f32 = 12.0;
/// Residual sink acceleration once the body is deeper than
/// [`SWIM_SURFACE_BAND_METERS`], m/s^2. Not zero, so releasing every key at
/// depth drifts *down* (0.125 m/s terminal against the drag) rather than
/// hovering forever — "hover or sink very slowly", never rise.
pub(crate) const SWIM_DEEP_SINK_METERS_PER_SECOND_SQUARED: f32 = 0.5;
/// How far up the surface probe looks for air, meters. Only the band matters, so
/// the probe is bounded just past it: beyond this the body is neutral by
/// definition and the exact depth is irrelevant, which keeps the probe ~10 voxel
/// reads instead of the whole column.
pub(crate) const SWIM_SURFACE_PROBE_METERS: f32 = SWIM_SURFACE_BAND_METERS + 0.5;
/// Vertical velocity damping per second while any part of the body is in water.
/// Applied while wading too, not only while swimming: it is what turns every
/// vertical term in water into a terminal speed instead of an accumulating one.
pub(crate) const WATER_VERTICAL_DRAG_PER_SECOND: f32 = 4.0;
/// Deliberate swim up/down acceleration (jump = up, crouch = dive), m/s^2.
pub(crate) const SWIM_THRUST_METERS_PER_SECOND_SQUARED: f32 = 12.0;
/// Vertical speed ceiling while swimming, m/s — no cannonballing out of a pool.
pub(crate) const SWIM_MAX_VERTICAL_SPEED: f32 = 2.0;

/// Gap kept between the body and the surface it rests against, meters. Large
/// enough that float error cannot re-enter the voxel, small enough to be
/// invisible (1 mm = 1/125 of a voxel).
pub(crate) const COLLISION_SKIN_METERS: f32 = 1.0e-3;
/// How far below the feet the ground test looks, meters. Must exceed
/// [`COLLISION_SKIN_METERS`] (or a resting body reads as airborne) and stay well
/// under a voxel (or it grabs ground that is not there).
pub(crate) const GROUND_PROBE_METERS: f32 = 0.02;
/// Slack subtracted from an interval's upper bound before deriving its voxel
/// layer, so a body exactly touching a voxel plane does not count the voxel
/// beyond it, meters.
const CONTACT_EPSILON_METERS: f32 = 1.0e-4;

/// Largest displacement one substep may apply, meters — strictly less than half
/// the body's smallest dimension (0.6 / 2 = 0.3), which is the condition that
/// makes per-axis resolution safe against corner tunneling.
pub(crate) const MAX_SUBSTEP_METERS: f32 = 0.25;
/// Longest `delta_seconds` the controller integrates. A longer stall (a shader
/// compile, a breakpoint, a laptop lid) is simulated as this much: the body
/// stays somewhere physics can defend instead of teleporting.
pub(crate) const MAX_STEP_SECONDS: f32 = 0.1;
/// Substep ceiling, so a pathological delta can never turn into an unbounded
/// loop. 64 substeps x 0.25 m = 16 m of motion in one call.
pub(crate) const MAX_SUBSTEPS: u32 = 64;
/// Voxel layers one sweep may test. Reaching it stops the body at the last
/// verified boundary — the second half of the anti-tunneling guarantee.
/// 32 layers = 4 m, far past anything [`MAX_SUBSTEP_METERS`] can produce.
pub(crate) const MAX_SWEEP_LAYERS: u32 = 32;

const AXIS_X: usize = 0;
const AXIS_Y: usize = 1;
const AXIS_Z: usize = 2;

/// How wet the body is. Ordered by how much of it is under water.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Submersion {
    /// No part of the body is in a liquid.
    Dry,
    /// Feet in water, shoulders out: walking, damped, still needs ground.
    Wading,
    /// Water over the shoulders: buoyant, needs no ground.
    Swimming,
}

impl Submersion {
    /// Short label for the overlay readout.
    pub fn label(self) -> &'static str {
        match self {
            Submersion::Dry => "dry",
            Submersion::Wading => "wading",
            Submersion::Swimming => "swimming",
        }
    }
}

/// The character's tunable feel — body size, gravity, jump, step-up, water.
///
/// Deliberately NOT part of [`crate::variants::RenderQuality`]: that struct is
/// the *render quality* lever surface (its registry rows carry measured
/// ms-per-frame verdicts and drive shader permutations), and movement feel has
/// neither a shader const nor a frame-time verdict to record. Keeping it here
/// also keeps this module standalone for E8/E9, which want the body without the
/// renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterSettings {
    /// Body width AND depth, meters.
    pub body_width_meters: f32,
    /// Body height, meters.
    pub body_height_meters: f32,
    /// Eye height above the feet, meters.
    pub eye_height_meters: f32,
    /// Base walk speed, m/s (the mouse wheel tunes this one).
    pub walk_speed: f32,
    /// Ceiling applied to [`CameraInput::speed_multiplier`].
    pub sprint_multiplier: f32,
    /// Downward acceleration, m/s^2.
    pub gravity: f32,
    /// Fall-speed ceiling, m/s.
    pub terminal_velocity: f32,
    /// How high a jump rises, meters.
    pub jump_apex_meters: f32,
    /// Auto-step height, meters. Zero disables both step-up and step-down snap.
    pub step_up_meters: f32,
    /// Horizontal speed scale while wading.
    pub wade_speed_scale: f32,
    /// Horizontal speed while swimming, m/s.
    pub swim_speed: f32,
}

impl Default for CharacterSettings {
    fn default() -> CharacterSettings {
        CharacterSettings {
            body_width_meters: BODY_WIDTH_METERS,
            body_height_meters: BODY_HEIGHT_METERS,
            eye_height_meters: EYE_HEIGHT_METERS,
            walk_speed: WALK_SPEED_METERS_PER_SECOND,
            sprint_multiplier: SPRINT_MULTIPLIER,
            gravity: GRAVITY_METERS_PER_SECOND_SQUARED,
            terminal_velocity: TERMINAL_VELOCITY_METERS_PER_SECOND,
            jump_apex_meters: JUMP_APEX_METERS,
            step_up_meters: STEP_UP_METERS,
            wade_speed_scale: WADE_SPEED_SCALE,
            swim_speed: SWIM_SPEED_METERS_PER_SECOND,
        }
    }
}

impl CharacterSettings {
    /// Launch speed that reaches [`Self::jump_apex_meters`] under
    /// [`Self::gravity`]: `v = sqrt(2 g h)`.
    pub fn jump_speed(&self) -> f32 {
        (2.0 * self.gravity * self.jump_apex_meters).sqrt()
    }
}

/// A body that walks, falls, climbs steps and swims through the voxel world.
///
/// Mirror of [`crate::camera::FlyCamera`]'s role — it consumes the same
/// [`CameraInput`] the platform layer already builds (`up` = jump / swim up,
/// `down` = dive, `speed_multiplier` = sprint) and produces the same
/// [`CameraPose`] — so switching modes changes the movement model and nothing
/// else in the frame.
#[derive(Clone, Copy, Debug)]
pub struct CharacterController {
    /// Centre of the body's base, world meters. The eye is
    /// `settings.eye_height_meters` above this.
    pub feet_position: Vec3,
    /// Radians around +Y; 0 looks along +X, -PI/2 along -Z (camera convention).
    pub yaw: f32,
    /// Radians above the horizon, clamped by [`CameraPose`]'s convention.
    pub pitch: f32,
    /// Radians of yaw/pitch per pixel of mouse motion.
    pub mouse_sensitivity: f32,
    /// Full vertical field of view, radians.
    pub vertical_fov_radians: f32,
    pub settings: CharacterSettings,
    vertical_velocity: f32,
    grounded: bool,
    submersion: Submersion,
    head_submerged: bool,
}

/// Pitch clamp, mirroring [`crate::camera`]'s: just short of straight up/down so
/// the view basis never degenerates.
const MAX_PITCH_RADIANS: f32 = std::f32::consts::FRAC_PI_2 - 0.01;

impl CharacterController {
    /// A body whose EYE is at `eye_position` — the fly->walk handover, since the
    /// fly camera's position *is* an eye. Call [`Self::snap_to_ground`] next to
    /// put its feet on the terrain below.
    pub fn from_eye(eye_position: Vec3, yaw: f32, pitch: f32) -> CharacterController {
        let settings = CharacterSettings::default();
        CharacterController {
            feet_position: eye_position - Vec3::Y * settings.eye_height_meters,
            yaw,
            pitch,
            mouse_sensitivity: 0.0025,
            vertical_fov_radians: DEFAULT_VERTICAL_FOV_RADIANS,
            settings,
            vertical_velocity: 0.0,
            grounded: false,
            submersion: Submersion::Dry,
            head_submerged: false,
        }
    }

    /// Eye position, world meters — **the listener pose E8 should use** and the
    /// head position E9 replaces with a tracked one.
    pub fn eye_position(&self) -> Vec3 {
        self.feet_position + Vec3::Y * self.settings.eye_height_meters
    }

    /// The view pose (the seam a VR backend replaces).
    pub fn pose(&self) -> CameraPose {
        CameraPose::from_yaw_pitch(self.eye_position(), self.yaw, self.pitch)
    }

    /// Convenience: this frame's GPU uniform at the character's own FOV.
    pub fn gpu_uniform(&self, resolution: (u32, u32)) -> crate::camera::CameraUniform {
        self.pose()
            .gpu_uniform(self.vertical_fov_radians, resolution)
    }

    /// Whether the body is resting on (or within [`GROUND_PROBE_METERS`] of)
    /// solid ground.
    pub fn grounded(&self) -> bool {
        self.grounded
    }

    /// How wet the body is.
    pub fn submersion(&self) -> Submersion {
        self.submersion
    }

    /// Whether the EYE is inside a liquid — E6's underwater-view flag and E8's
    /// submerged-listener flag. Computed here because the body already knows.
    pub fn head_submerged(&self) -> bool {
        self.head_submerged
    }

    /// Current vertical velocity, m/s (positive up). Overlay/diagnostics.
    pub fn vertical_velocity(&self) -> f32 {
        self.vertical_velocity
    }

    /// Scale the walk speed by `notches` of mouse wheel — multiplicative and
    /// clamped exactly like [`crate::camera::FlyCamera::adjust_movement_speed`],
    /// so the wheel feels the same in both modes.
    pub fn adjust_walk_speed(&mut self, notches: f32) {
        self.settings.walk_speed =
            (self.settings.walk_speed * 1.2f32.powf(notches)).clamp(MIN_WALK_SPEED, MAX_WALK_SPEED);
    }

    /// Advance the body one frame.
    ///
    /// The step is clamped to [`MAX_STEP_SECONDS`] and split into substeps of at
    /// most [`MAX_SUBSTEP_METERS`] of motion (the anti-tunneling clamp); each
    /// substep integrates gravity/buoyancy, then resolves X, Y and Z in turn.
    pub fn step(&mut self, brickmap: &Brickmap, input: &CameraInput, delta_seconds: f32) {
        self.apply_look(input);
        let delta_seconds = if delta_seconds.is_finite() {
            delta_seconds.clamp(0.0, MAX_STEP_SECONDS)
        } else {
            0.0
        };
        if delta_seconds <= 0.0 {
            // Keep the water readout (and E6/E8's submerged flag) honest even on a
            // zero-length frame.
            self.update_submersion(brickmap);
            return;
        }
        let substeps = self.substep_count(delta_seconds);
        let substep_seconds = delta_seconds / substeps as f32;
        for _ in 0..substeps {
            self.integrate_substep(brickmap, input, substep_seconds);
        }
        // Re-sample AFTER the last substep (E6, 2026-07-31). Each substep samples
        // at its START — it has to, because the submersion state drives that
        // substep's speed and buoyancy — which left the PUBLISHED flags describing
        // the pose the last substep began at rather than the pose the frame is
        // rendered from. E6 tests the primary ray's own origin, so a one-substep
        // lag makes `head_submerged` disagree with the picture at the surface
        // crossing; `the_two_underwater_predicates_agree` catches it. Three voxel
        // reads per frame.
        self.update_submersion(brickmap);
    }

    /// Mouse delta -> yaw/pitch. Identical convention to the fly camera, so a
    /// mode switch never turns the view.
    fn apply_look(&mut self, input: &CameraInput) {
        self.yaw += input.mouse_delta.0 * self.mouse_sensitivity;
        self.yaw = self.yaw.rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch - input.mouse_delta.1 * self.mouse_sensitivity)
            .clamp(-MAX_PITCH_RADIANS, MAX_PITCH_RADIANS);
    }

    /// How many substeps this frame needs so no substep moves further than
    /// [`MAX_SUBSTEP_METERS`].
    ///
    /// Deliberately computed from the body's WORST case — the fastest horizontal
    /// speed any submersion state allows, and the fastest vertical speed a jump or
    /// the current fall can reach inside the step — not from the motion this
    /// frame's input happens to request. The count is fixed for the whole step
    /// while the submersion state can change *inside* it, so a state-dependent
    /// count could under-substep the substeps that follow a state flip; a
    /// state-independent bound cannot.
    fn substep_count(&self, delta_seconds: f32) -> u32 {
        let horizontal = (self.settings.walk_speed * self.settings.sprint_multiplier)
            .max(self.settings.swim_speed);
        let vertical = self.vertical_velocity.abs().max(self.settings.jump_speed())
            + self.settings.gravity * delta_seconds;
        let motion = horizontal.max(vertical) * delta_seconds;
        ((motion / MAX_SUBSTEP_METERS).ceil() as u32).clamp(1, MAX_SUBSTEPS)
    }

    /// Horizontal speed this frame: walk, sprint-capped, damped by water.
    fn planar_speed(&self, input: &CameraInput) -> f32 {
        let sprint = if input.speed_multiplier.is_finite() {
            input
                .speed_multiplier
                .clamp(1.0, self.settings.sprint_multiplier)
        } else {
            1.0
        };
        match self.submersion {
            Submersion::Swimming => self.settings.swim_speed,
            Submersion::Wading => {
                self.settings.walk_speed * sprint * self.settings.wade_speed_scale
            }
            Submersion::Dry => self.settings.walk_speed * sprint,
        }
    }

    /// One substep: velocities, then X, then Y, then Z, then grounding.
    fn integrate_substep(&mut self, brickmap: &Brickmap, input: &CameraInput, seconds: f32) {
        self.update_submersion(brickmap);
        self.integrate_vertical_velocity(brickmap, input, seconds);

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
        let horizontal = if wish.length_squared() > 0.0 {
            wish.normalize() * self.planar_speed(input) * seconds
        } else {
            Vec3::ZERO
        };

        let was_grounded = self.grounded;
        // A swimmer floats with their feet well below the surface.  Without a
        // special exit lift, a solid whose top is level with the water looks
        // like a 1.5 m wall, even though it is a perfectly ordinary shore.
        // Only offer that extra lift while a local surface is in reach; it
        // therefore cannot be used to climb a wall above the waterline.
        let step_up_meters = if was_grounded {
            self.settings.step_up_meters
        } else if self.submersion == Submersion::Swimming {
            self.swim_exit_step_up(brickmap).unwrap_or(0.0)
        } else {
            0.0
        };
        self.sweep_with_step_up(brickmap, AXIS_X, horizontal.x, step_up_meters);

        let mut feet = self.feet_position.to_array();
        let vertical_delta = self.vertical_velocity * seconds;
        let blocked_vertically =
            sweep_axis(brickmap, &mut feet, &self.settings, AXIS_Y, vertical_delta);
        self.feet_position = Vec3::from_array(feet);
        if blocked_vertically {
            self.vertical_velocity = 0.0;
        }

        self.sweep_with_step_up(brickmap, AXIS_Z, horizontal.z, step_up_meters);

        self.grounded = self.ground_is_below(brickmap);
        if !self.grounded
            && was_grounded
            && self.vertical_velocity <= 0.0
            && self.submersion != Submersion::Swimming
        {
            self.snap_down_a_step(brickmap);
        }
        self.clamp_to_world();
    }

    /// Gravity, jump, buoyancy and water drag — the only place vertical velocity
    /// changes other than a collision.
    fn integrate_vertical_velocity(
        &mut self,
        brickmap: &Brickmap,
        input: &CameraInput,
        seconds: f32,
    ) {
        match self.submersion {
            Submersion::Swimming => {
                self.vertical_velocity += self.buoyant_acceleration(brickmap) * seconds;
                if input.up {
                    self.vertical_velocity += SWIM_THRUST_METERS_PER_SECOND_SQUARED * seconds;
                }
                if input.down {
                    self.vertical_velocity -= SWIM_THRUST_METERS_PER_SECOND_SQUARED * seconds;
                }
            }
            Submersion::Wading | Submersion::Dry => {
                if input.up && self.grounded {
                    self.vertical_velocity = self.settings.jump_speed();
                    self.grounded = false;
                }
                self.vertical_velocity -= self.settings.gravity * seconds;
            }
        }
        if self.submersion != Submersion::Dry {
            // Applied while wading too: it is what turns every vertical term in
            // water into a terminal speed, and it is why the swim/wade boundary
            // cannot dither at the surface.
            self.vertical_velocity *= (1.0 - WATER_VERTICAL_DRAG_PER_SECOND * seconds).max(0.0);
        }
        self.vertical_velocity = match self.submersion {
            Submersion::Swimming => self
                .vertical_velocity
                .clamp(-SWIM_MAX_VERTICAL_SPEED, SWIM_MAX_VERTICAL_SPEED),
            _ => self.vertical_velocity.max(-self.settings.terminal_velocity),
        };
    }

    /// A swimmer's whole vertical model, m/s^2 — gravity included, because while
    /// swimming the water carries the body's weight and what is left is exactly
    /// this residual.
    ///
    /// One expression covers both regimes. Writing `displacement` for how far the
    /// shoulders are below the float line and `t = min(displacement / band, 1)`:
    ///
    /// ```text
    /// a = stiffness * displacement * (1 - t)  -  deep_sink * t
    /// ```
    ///
    /// - at the float line (`displacement = 0`) it is zero — a stable
    ///   equilibrium: shallower pulls down, deeper lifts up, so a resting swimmer
    ///   bobs head-out and must hold dive to go under;
    /// - at or past the band (`t = 1`) the lift is gone and only the slow sink is
    ///   left, so a swimmer released at depth stays at depth (drifting down at
    ///   0.125 m/s against the drag) instead of rocketing to the surface;
    /// - in between the two terms cross at ~0.71 m below the line, which is the
    ///   watershed: closer than that you surface, deeper than that you sink.
    ///
    /// Continuous by construction (both terms are linear in `t`), so there is no
    /// step in acceleration for the drag to ring on.
    fn buoyant_acceleration(&self, brickmap: &Brickmap) -> f32 {
        let Some(depth) = self.shoulder_depth_below_surface(brickmap) else {
            // No surface within the probe: deep, so neutral bar the slow sink.
            return -SWIM_DEEP_SINK_METERS_PER_SECOND_SQUARED;
        };
        let displacement = depth - SWIM_FLOAT_SHOULDER_DEPTH_METERS;
        let band_fraction = (displacement / SWIM_SURFACE_BAND_METERS).clamp(0.0, 1.0);
        SWIM_BUOYANCY_STIFFNESS_PER_SECOND_SQUARED * displacement * (1.0 - band_fraction)
            - SWIM_DEEP_SINK_METERS_PER_SECOND_SQUARED * band_fraction
    }

    /// How far the body's shoulders (the point the [`Submersion::Swimming`] test
    /// uses) sit below the local water surface, meters.
    ///
    /// Probes UP voxel by voxel and stops at [`SWIM_SURFACE_PROBE_METERS`], which
    /// is what keeps this ~10 reads instead of a whole column: past the band the
    /// exact depth cannot change the acceleration. `None` = no surface in reach,
    /// i.e. the deep regime.
    fn shoulder_depth_below_surface(&self, brickmap: &Brickmap) -> Option<f32> {
        let shoulder =
            self.feet_position.y + self.settings.body_height_meters * SWIM_SUBMERSION_FRACTION;
        let voxel_x = (self.feet_position.x / VOXEL_SIZE).floor() as i32;
        let voxel_z = (self.feet_position.z / VOXEL_SIZE).floor() as i32;
        let shoulder_layer = (shoulder / VOXEL_SIZE).floor() as i32;
        let probe_layers = (SWIM_SURFACE_PROBE_METERS / VOXEL_SIZE).ceil() as i32;
        (shoulder_layer..=shoulder_layer + probe_layers)
            .find(|layer| !material_is_liquid(brickmap.get(voxel_x, *layer, voxel_z)))
            // The surface is the floor of the first non-liquid layer.
            .map(|layer| layer as f32 * VOXEL_SIZE - shoulder)
    }

    /// Height to lift a floating swimmer in order to try stepping onto a shore
    /// whose top is flush with the local water surface.  The normal sweep and
    /// settle still validate the move, so this is not a general swim climb.
    fn swim_exit_step_up(&self, brickmap: &Brickmap) -> Option<f32> {
        let shoulder =
            self.feet_position.y + self.settings.body_height_meters * SWIM_SUBMERSION_FRACTION;
        let depth = self.shoulder_depth_below_surface(brickmap)?;
        let surface = shoulder + depth;
        Some((surface - self.feet_position.y + COLLISION_SKIN_METERS).max(0.0))
    }

    /// Move along a horizontal axis; if a wall stops it, retry the move lifted
    /// by up to `step_up_meters` and settle back down onto whatever that found.
    /// Returns whether the body ended up blocked.
    fn sweep_with_step_up(
        &mut self,
        brickmap: &Brickmap,
        axis: usize,
        delta: f32,
        step_up_meters: f32,
    ) -> bool {
        let before = self.feet_position.to_array();
        let mut feet = before;
        if !sweep_axis(brickmap, &mut feet, &self.settings, axis, delta) {
            self.feet_position = Vec3::from_array(feet);
            return false;
        }
        let lift = step_up_meters;
        if lift <= 0.0 {
            self.feet_position = Vec3::from_array(feet);
            return true;
        }
        // Retry from the pre-move position: lift, move, settle.
        let mut trial = before;
        if sweep_axis(brickmap, &mut trial, &self.settings, AXIS_Y, lift) {
            // Not enough headroom to lift (a low overhang): keep the slide.
            self.feet_position = Vec3::from_array(feet);
            return true;
        }
        if sweep_axis(brickmap, &mut trial, &self.settings, axis, delta) {
            // A real wall, not a step.
            self.feet_position = Vec3::from_array(feet);
            return true;
        }
        let landed = sweep_axis(brickmap, &mut trial, &self.settings, AXIS_Y, -lift);
        self.feet_position = Vec3::from_array(trial);
        if landed {
            self.grounded = true;
            self.vertical_velocity = 0.0;
        }
        false
    }

    /// Walking off a small drop should follow the ground rather than launch the
    /// body into a fall: while grounded and not rising, look up to
    /// `step_up_meters` down for ground and settle onto it.
    fn snap_down_a_step(&mut self, brickmap: &Brickmap) {
        let mut feet = self.feet_position.to_array();
        if sweep_axis(
            brickmap,
            &mut feet,
            &self.settings,
            AXIS_Y,
            -self.settings.step_up_meters,
        ) {
            self.feet_position = Vec3::from_array(feet);
            self.vertical_velocity = 0.0;
            self.grounded = true;
        }
    }

    /// Whether solid ground sits within [`GROUND_PROBE_METERS`] under the feet.
    fn ground_is_below(&self, brickmap: &Brickmap) -> bool {
        let feet = self.feet_position.to_array();
        let mut spans = body_spans(feet, &self.settings);
        let layer = ((feet[AXIS_Y] - GROUND_PROBE_METERS) / VOXEL_SIZE).floor() as i32;
        spans[AXIS_Y] = (layer, layer);
        any_blocking_voxel(brickmap, &spans)
    }

    /// Sample the world's liquids at the three heights the water model needs.
    fn update_submersion(&mut self, brickmap: &Brickmap) {
        let feet = self.feet_position;
        let liquid_at = |offset: f32| {
            let point = feet + Vec3::Y * offset;
            material_is_liquid(brickmap.get(
                (point.x / VOXEL_SIZE).floor() as i32,
                (point.y / VOXEL_SIZE).floor() as i32,
                (point.z / VOXEL_SIZE).floor() as i32,
            ))
        };
        self.head_submerged = liquid_at(self.settings.eye_height_meters);
        self.submersion = if liquid_at(self.settings.body_height_meters * SWIM_SUBMERSION_FRACTION)
        {
            Submersion::Swimming
        } else if liquid_at(GROUND_PROBE_METERS) {
            Submersion::Wading
        } else {
            Submersion::Dry
        };
    }

    /// Keep the body inside the world box. The world's floor is a hard floor:
    /// there is no geometry below y = 0, so without this a walk off the island's
    /// edge falls forever and the velocity clamp is the only thing bounding it.
    fn clamp_to_world(&mut self) {
        let half_width = self.settings.body_width_meters * 0.5;
        let maximum_x = WORLD_SIZE_X as f32 * VOXEL_SIZE - half_width;
        let maximum_z = WORLD_SIZE_Z as f32 * VOXEL_SIZE - half_width;
        self.feet_position.x = self.feet_position.x.clamp(half_width, maximum_x);
        self.feet_position.z = self.feet_position.z.clamp(half_width, maximum_z);
        let maximum_y = WORLD_SIZE_Y as f32 * VOXEL_SIZE - self.settings.body_height_meters;
        if self.feet_position.y <= 0.0 {
            self.feet_position.y = 0.0;
            self.vertical_velocity = 0.0;
            self.grounded = true;
        } else if self.feet_position.y > maximum_y {
            self.feet_position.y = maximum_y;
            self.vertical_velocity = self.vertical_velocity.min(0.0);
        }
    }

    /// Put the feet on the ground below the current position — what entering
    /// walk mode does with the fly camera's eye. Searches down to
    /// `max_drop_meters`, then lifts the body clear of anything it is standing
    /// inside (toggling to walk from inside a hill must not leave you stuck).
    ///
    /// Returns false when no ground was found within the search distance; the
    /// body is then left where it is and gravity takes over.
    pub fn snap_to_ground(&mut self, brickmap: &Brickmap, max_drop_meters: f32) -> bool {
        let found = ground_height_below(
            brickmap,
            self.feet_position,
            &self.settings,
            max_drop_meters,
        );
        if let Some(ground_meters) = found {
            self.feet_position.y = ground_meters + COLLISION_SKIN_METERS;
        }
        // Whether or not ground was found, do not leave the body inside solid.
        let mut lifted = 0.0;
        while lifted < self.settings.body_height_meters * 2.0
            && any_blocking_voxel(
                brickmap,
                &body_spans(self.feet_position.to_array(), &self.settings),
            )
        {
            self.feet_position.y += VOXEL_SIZE;
            lifted += VOXEL_SIZE;
        }
        self.vertical_velocity = 0.0;
        self.clamp_to_world();
        self.update_submersion(brickmap);
        self.grounded = self.ground_is_below(brickmap);
        found.is_some()
    }
}

/// Height of the highest solid surface under a body at `feet_position`, world
/// meters — the "find ground at XZ" query, using the body's FOOTPRINT rather
/// than a single column so a body straddling a ledge rests on the higher side.
///
/// Returns `None` when nothing solid is within `max_drop_meters`.
pub(crate) fn ground_height_below(
    brickmap: &Brickmap,
    feet_position: Vec3,
    settings: &CharacterSettings,
    max_drop_meters: f32,
) -> Option<f32> {
    let feet = feet_position.to_array();
    let mut spans = body_spans(feet, settings);
    let highest_layer = (feet[AXIS_Y] / VOXEL_SIZE).floor() as i32;
    let lowest_layer = (((feet[AXIS_Y] - max_drop_meters.max(0.0)) / VOXEL_SIZE).floor() as i32)
        .max(0)
        .min(highest_layer);
    for layer in (lowest_layer..=highest_layer.min(WORLD_SIZE_Y as i32 - 1)).rev() {
        spans[AXIS_Y] = (layer, layer);
        if any_blocking_voxel(brickmap, &spans) {
            return Some((layer + 1) as f32 * VOXEL_SIZE);
        }
    }
    None
}

/// Whether a body standing at `feet_position` overlaps any blocking voxel — the
/// invariant every test in this module asserts, and the reason the sweep exists.
pub fn body_overlaps_solid(
    brickmap: &Brickmap,
    feet_position: Vec3,
    settings: &CharacterSettings,
) -> bool {
    any_blocking_voxel(brickmap, &body_spans(feet_position.to_array(), settings))
}

/// Lower/upper offsets of the body box from the feet position, per axis. Y is
/// asymmetric (the origin is the base), X and Z are half-widths.
fn extent_offsets(settings: &CharacterSettings) -> ([f32; 3], [f32; 3]) {
    let half_width = settings.body_width_meters * 0.5;
    (
        [-half_width, 0.0, -half_width],
        [half_width, settings.body_height_meters, half_width],
    )
}

/// Voxel layer indices the interval `[lower, upper)` overlaps. The upper bound
/// is open, so a body whose face rests exactly on a voxel plane does not claim
/// the voxel beyond it.
fn voxel_span(lower: f32, upper: f32) -> (i32, i32) {
    let first = (lower / VOXEL_SIZE).floor() as i32;
    let last = ((upper - CONTACT_EPSILON_METERS) / VOXEL_SIZE).floor() as i32;
    (first, last.max(first))
}

/// The body box's voxel layer spans, per axis.
fn body_spans(feet: [f32; 3], settings: &CharacterSettings) -> [(i32, i32); 3] {
    let (lower, upper) = extent_offsets(settings);
    [AXIS_X, AXIS_Y, AXIS_Z]
        .map(|axis| voxel_span(feet[axis] + lower[axis], feet[axis] + upper[axis]))
}

/// Whether any voxel in the given box of layers blocks movement. Out-of-world
/// coordinates read as air ([`Brickmap::get`]), which is what lets the body walk
/// off the island and fall.
fn any_blocking_voxel(brickmap: &Brickmap, spans: &[(i32, i32); 3]) -> bool {
    for z in spans[AXIS_Z].0..=spans[AXIS_Z].1 {
        for y in spans[AXIS_Y].0..=spans[AXIS_Y].1 {
            for x in spans[AXIS_X].0..=spans[AXIS_X].1 {
                if material_blocks_movement(brickmap.get(x, y, z)) {
                    return true;
                }
            }
        }
    }
    false
}

/// Move the body along one axis by `delta`, testing the body's cross-section
/// against EVERY voxel layer the leading face crosses, and stopping it
/// [`COLLISION_SKIN_METERS`] short of the first blocked one. Returns whether the
/// motion was blocked.
///
/// This is the whole collision resolver: the caller applies it per axis (X, then
/// Y, then Z), which is what keeps a diagonal move from cutting a corner and
/// makes each resolution a one-dimensional problem.
fn sweep_axis(
    brickmap: &Brickmap,
    feet: &mut [f32; 3],
    settings: &CharacterSettings,
    axis: usize,
    delta: f32,
) -> bool {
    if delta == 0.0 || !delta.is_finite() {
        return false;
    }
    let (lower, upper) = extent_offsets(settings);
    let forward = delta > 0.0;
    let direction: i32 = if forward { 1 } else { -1 };
    let leading_offset = if forward { upper[axis] } else { lower[axis] };
    // A leading face is an OPEN bound when moving up/right and a CLOSED one when
    // moving down/left, so the layer it currently occupies differs by direction.
    let layer_of = |face: f32| {
        if forward {
            ((face - CONTACT_EPSILON_METERS) / VOXEL_SIZE).floor() as i32
        } else {
            (face / VOXEL_SIZE).floor() as i32
        }
    };
    // Where the body must stop so its leading face sits just short of `boundary`.
    let stop_at = |boundary_layer: i32, near: bool| {
        let boundary = if forward == near {
            boundary_layer as f32
        } else {
            (boundary_layer + 1) as f32
        } * VOXEL_SIZE;
        boundary - leading_offset - direction as f32 * COLLISION_SKIN_METERS
    };
    let clamp_forward = |candidate: f32, current: f32| {
        if forward {
            candidate.max(current)
        } else {
            candidate.min(current)
        }
    };

    let first_layer = layer_of(feet[axis] + leading_offset);
    let last_layer = layer_of(feet[axis] + leading_offset + delta);
    let mut spans = body_spans(*feet, settings);
    let mut layer = first_layer;
    let mut tested = 0_u32;
    while layer != last_layer {
        if tested == MAX_SWEEP_LAYERS {
            // Refuse to move past voxels we have not tested: stop at the far
            // edge of the last cleared layer (the anti-tunneling backstop).
            feet[axis] = clamp_forward(stop_at(layer, false), feet[axis]);
            return true;
        }
        layer += direction;
        tested += 1;
        spans[axis] = (layer, layer);
        if any_blocking_voxel(brickmap, &spans) {
            feet[axis] = clamp_forward(stop_at(layer, true), feet[axis]);
            return true;
        }
    }
    feet[axis] += delta;
    false
}

#[cfg(test)]
mod tests {
    use std::f32::consts::PI;

    use super::*;
    use crate::brickmap::ClearanceUpdate;
    use voxel_core::world::{
        Voxel, VoxelWorld, WorldVoxelCoord, DETAIL_CELLS_PER_WORLD_VOXEL, WATER_LEVEL,
        WORLD_VOXELS_Y,
    };

    /// The island every test walks on. NOTE: generates the full world — run the
    /// suite with `--release`.
    fn island() -> Brickmap {
        Brickmap::build(&VoxelWorld::generate(1234, 0.0))
    }

    /// Cheap clearance strategy for test edits: nothing in this module reads the
    /// chebyshev field (the body queries `Brickmap::get`), so the smallest
    /// radius keeps a few hundred edits fast.
    const TEST_CLEARANCE: ClearanceUpdate = ClearanceUpdate::LocalBox { radius_cells: 2 };

    fn surface_voxel_y(brickmap: &Brickmap, x: i32, z: i32) -> i32 {
        (0..WORLD_SIZE_Y as i32)
            .rev()
            .find(|y| material_blocks_movement(brickmap.get(x, *y, z)))
            .expect("every island column has a surface")
    }

    /// A body standing on the surface at a voxel column, facing `yaw`.
    fn standing_body(
        brickmap: &Brickmap,
        voxel_x: i32,
        voxel_z: i32,
        yaw: f32,
    ) -> CharacterController {
        let surface_y = surface_voxel_y(brickmap, voxel_x, voxel_z);
        let eye = Vec3::new(
            (voxel_x as f32 + 0.5) * VOXEL_SIZE,
            (surface_y + 1) as f32 * VOXEL_SIZE + EYE_HEIGHT_METERS,
            (voxel_z as f32 + 0.5) * VOXEL_SIZE,
        );
        let mut body = CharacterController::from_eye(eye, yaw, 0.0);
        body.snap_to_ground(brickmap, 8.0);
        body
    }

    fn walking_forward() -> CameraInput {
        CameraInput {
            forward: true,
            ..CameraInput::default()
        }
    }

    #[derive(Clone, Copy)]
    struct WaterPool {
        centre_voxel_x: i32,
        centre_voxel_z: i32,
    }

    impl WaterPool {
        fn surface_centre(self) -> Vec3 {
            Vec3::new(
                (self.centre_voxel_x as f32 + 0.5) * VOXEL_SIZE,
                (WATER_LEVEL + 1) as f32 * VOXEL_SIZE,
                (self.centre_voxel_z as f32 + 0.5) * VOXEL_SIZE,
            )
        }
    }

    /// Build a stepped swimming basin from aligned one-metre blocks. This is a
    /// test fixture, not an alternate fine-cell world-edit path.
    fn carve_test_pool(brickmap: &mut Brickmap) -> WaterPool {
        let detail = DETAIL_CELLS_PER_WORLD_VOXEL as i32;
        let centre_world = [62, 62];
        let pool = WaterPool {
            centre_voxel_x: centre_world[0] * detail + detail / 2,
            centre_voxel_z: centre_world[1] * detail + detail / 2,
        };
        for dz in -4_i32..=4 {
            for dx in -4_i32..=4 {
                let radius = dx.abs().max(dz.abs());
                let bed_y = match radius {
                    4 => 10,
                    3 => 9,
                    2 => 8,
                    _ => 5,
                };
                let x = centre_world[0] + dx;
                let z = centre_world[1] + dz;
                brickmap.set_world_voxel(
                    WorldVoxelCoord::new(x, bed_y, z),
                    Voxel::Stone,
                    TEST_CLEARANCE,
                );
                for y in (bed_y + 1)..WORLD_VOXELS_Y as i32 {
                    brickmap.set_world_voxel(
                        WorldVoxelCoord::new(x, y, z),
                        Voxel::Air,
                        TEST_CLEARANCE,
                    );
                }
                for y in (bed_y + 1)..=10 {
                    brickmap.set_world_voxel(
                        WorldVoxelCoord::new(x, y, z),
                        Voxel::Water,
                        TEST_CLEARANCE,
                    );
                }
                brickmap.set_world_voxel(
                    WorldVoxelCoord::new(x, 11, z),
                    Voxel::Air,
                    TEST_CLEARANCE,
                );
            }
        }
        pool
    }

    /// Fill a solid box of voxels (inclusive ranges) with one material.
    fn fill_box(brickmap: &mut Brickmap, from: [i32; 3], to: [i32; 3], voxel: Voxel) {
        for z in from[2]..=to[2] {
            for y in from[1]..=to[1] {
                for x in from[0]..=to[0] {
                    brickmap.set_voxel(x, y, z, voxel, TEST_CLEARANCE);
                }
            }
        }
    }

    /// Carve a deterministic flat walkway running along +X: one solid floor layer
    /// with 3 m of air above it, cut just into the terrain.
    ///
    /// The island's natural slope is 1-3 voxels per 0.2 m in places, which is the
    /// whole point of the step-up feature and therefore *noise* for a test that
    /// measures how a specific step is handled (the first version of the four tests
    /// below measured the terrain instead of the controller). Returns the floor's
    /// voxel Y and the top surface in world meters.
    fn flat_walkway(
        brickmap: &mut Brickmap,
        voxel_x_range: (i32, i32),
        centre_voxel_z: i32,
    ) -> (i32, f32) {
        let floor_y = (voxel_x_range.0..=voxel_x_range.1)
            .map(|x| surface_voxel_y(brickmap, x, centre_voxel_z))
            .min()
            .expect("the walkway spans at least one column")
            - 1;
        for z in centre_voxel_z - 8..=centre_voxel_z + 8 {
            for x in voxel_x_range.0..=voxel_x_range.1 {
                brickmap.set_voxel(x, floor_y, z, Voxel::Stone, TEST_CLEARANCE);
                for y in floor_y + 1..=floor_y + 24 {
                    brickmap.set_voxel(x, y, z, Voxel::Air, TEST_CLEARANCE);
                }
            }
        }
        (floor_y, (floor_y + 1) as f32 * VOXEL_SIZE)
    }

    /// The derived jump impulse must actually reach the configured apex, and the
    /// body must land back on the ground it left.
    #[test]
    fn a_jump_reaches_the_configured_apex() {
        let mut brickmap = island();
        flat_walkway(&mut brickmap, (480, 520), 500);
        let mut body = standing_body(&brickmap, 500, 500, 0.0);
        let ground_y = body.feet_position.y;
        assert!(body.grounded(), "the body must start grounded");
        let jump = CameraInput {
            up: true,
            ..CameraInput::default()
        };
        let mut highest = ground_y;
        // One frame with jump held, then release: holding it would re-jump on
        // landing, which is correct behaviour but not what this measures.
        body.step(&brickmap, &jump, 1.0 / 240.0);
        for _ in 0..480 {
            body.step(&brickmap, &CameraInput::default(), 1.0 / 240.0);
            highest = highest.max(body.feet_position.y);
        }
        let apex = highest - ground_y;
        assert!(
            (apex - JUMP_APEX_METERS).abs() < 0.05,
            "jump apex {apex:.3} m, expected {JUMP_APEX_METERS} m"
        );
        assert!(body.grounded(), "the body must have landed");
        assert!(
            (body.feet_position.y - ground_y).abs() < VOXEL_SIZE,
            "landed at {:.3} m, left from {ground_y:.3} m",
            body.feet_position.y
        );
        assert!(!body_overlaps_solid(
            &brickmap,
            body.feet_position,
            &body.settings
        ));
    }

    /// A wall taller than the step height stops the body dead, outside the wall,
    /// and does not lift it.
    #[test]
    fn a_wall_stops_the_body_without_being_entered() {
        let mut brickmap = island();
        let (voxel_x, voxel_z) = (500, 500);
        let (floor_y, _) = flat_walkway(&mut brickmap, (496, 544), voxel_z);
        // A 2 m high wall across the walkway, 1.5 m ahead of the body.
        let wall_x = voxel_x + 12;
        fill_box(
            &mut brickmap,
            [wall_x, floor_y + 1, voxel_z - 8],
            [wall_x, floor_y + 16, voxel_z + 8],
            Voxel::Stone,
        );
        let mut body = standing_body(&brickmap, voxel_x, voxel_z, 0.0); // +X, into the wall
        let start_y = body.feet_position.y;
        for _ in 0..90 {
            body.step(&brickmap, &walking_forward(), 1.0 / 60.0);
            assert!(
                !body_overlaps_solid(&brickmap, body.feet_position, &body.settings),
                "the body entered the wall at {:?}",
                body.feet_position
            );
        }
        let wall_face = wall_x as f32 * VOXEL_SIZE;
        let body_front = body.feet_position.x + body.settings.body_width_meters * 0.5;
        assert!(
            body_front <= wall_face && wall_face - body_front < 0.01,
            "stopped with the front face at {body_front:.4} m against a wall at {wall_face:.4} m"
        );
        assert!(
            (body.feet_position.y - start_y).abs() < VOXEL_SIZE,
            "a 2 m wall must not be climbed: y went {start_y:.3} -> {:.3}",
            body.feet_position.y
        );
    }

    /// The case that decides whether walking the island is bearable: a 3-voxel
    /// (0.375 m) step is climbed without touching the jump key.
    #[test]
    fn a_three_voxel_step_is_climbed_without_jumping() {
        let mut brickmap = island();
        let (voxel_x, voxel_z) = (500, 496);
        let (floor_y, _) = flat_walkway(&mut brickmap, (496, 544), voxel_z);
        // A 3-voxel-high plateau filling the rest of the walkway.
        let step_x = voxel_x + 12;
        fill_box(
            &mut brickmap,
            [step_x, floor_y + 1, voxel_z - 8],
            [544, floor_y + 3, voxel_z + 8],
            Voxel::Stone,
        );
        let mut body = standing_body(&brickmap, voxel_x, voxel_z, 0.0);
        let start_y = body.feet_position.y;
        for _ in 0..60 {
            body.step(&brickmap, &walking_forward(), 1.0 / 60.0);
            assert!(!body_overlaps_solid(
                &brickmap,
                body.feet_position,
                &body.settings
            ));
        }
        let climbed = body.feet_position.y - start_y;
        assert!(
            (climbed - 3.0 * VOXEL_SIZE).abs() < 0.02,
            "climbed {climbed:.3} m, expected {:.3} m",
            3.0 * VOXEL_SIZE
        );
        assert!(
            body.feet_position.x > step_x as f32 * VOXEL_SIZE,
            "the body is still in front of the step at x = {:.3}",
            body.feet_position.x
        );
        assert!(body.grounded(), "the body must stand on the step");
    }

    /// A step one voxel taller than the step-up height must NOT be climbed —
    /// the auto-step is a convenience, not a free wall-scale.
    #[test]
    fn a_step_above_the_step_height_is_not_climbed() {
        let mut brickmap = island();
        let (voxel_x, voxel_z) = (500, 504);
        let (floor_y, _) = flat_walkway(&mut brickmap, (496, 544), voxel_z);
        let step_x = voxel_x + 12;
        fill_box(
            &mut brickmap,
            [step_x, floor_y + 1, voxel_z - 8],
            [544, floor_y + 4, voxel_z + 8],
            Voxel::Stone,
        );
        let mut body = standing_body(&brickmap, voxel_x, voxel_z, 0.0);
        let start_y = body.feet_position.y;
        for _ in 0..60 {
            body.step(&brickmap, &walking_forward(), 1.0 / 60.0);
        }
        assert!(
            (body.feet_position.y - start_y).abs() < VOXEL_SIZE,
            "a 4-voxel step must not be climbed: y went {start_y:.3} -> {:.3}",
            body.feet_position.y
        );
        assert!(!body_overlaps_solid(
            &brickmap,
            body.feet_position,
            &body.settings
        ));
    }

    /// THE ANTI-TUNNELING TEST. A body fired in every direction at maximum
    /// speed, with frame deltas from "bad hitch" to "absurd", must never end up
    /// inside solid geometry — and must actually hit the island often enough for
    /// that to mean something.
    #[test]
    fn absurd_frame_deltas_never_end_inside_solid() {
        let brickmap = island();
        let mut blocked_runs = 0_usize;
        let mut total_runs = 0_usize;
        for hitch_seconds in [0.04_f32, 0.25, 1.0] {
            for yaw_step in 0..24 {
                for start_index in 0..4 {
                    let yaw = yaw_step as f32 / 24.0 * std::f32::consts::TAU;
                    // Four starts: island centre, two rim columns, and high air.
                    let (voxel_x, voxel_z, extra_height) = match start_index {
                        0 => (500, 500, 0.5),
                        1 => (420, 560, 0.5),
                        2 => (560, 440, 0.5),
                        _ => (500, 500, 30.0),
                    };
                    let surface_y = surface_voxel_y(&brickmap, voxel_x, voxel_z);
                    let eye = Vec3::new(
                        (voxel_x as f32 + 0.5) * VOXEL_SIZE,
                        (surface_y + 1) as f32 * VOXEL_SIZE + EYE_HEIGHT_METERS + extra_height,
                        (voxel_z as f32 + 0.5) * VOXEL_SIZE,
                    );
                    let mut body = CharacterController::from_eye(eye, yaw, 0.0);
                    body.settings.walk_speed = MAX_WALK_SPEED;
                    let sprinting = CameraInput {
                        forward: true,
                        speed_multiplier: 100.0, // clamped to the sprint ceiling
                        ..CameraInput::default()
                    };
                    let start_position = body.feet_position;
                    for _ in 0..8 {
                        body.step(&brickmap, &sprinting, hitch_seconds);
                        assert!(
                            !body_overlaps_solid(&brickmap, body.feet_position, &body.settings),
                            "tunneled: {hitch_seconds} s steps from {start_position:?} at yaw \
                             {yaw} put the body inside solid at {:?}",
                            body.feet_position
                        );
                    }
                    // Reaching less far than an unobstructed run would is the
                    // evidence that geometry actually stopped it.
                    let travelled = (body.feet_position - start_position).with_y(0.0).length();
                    let unobstructed = MAX_WALK_SPEED * SPRINT_MULTIPLIER * MAX_STEP_SECONDS * 8.0;
                    if travelled < unobstructed * 0.95 {
                        blocked_runs += 1;
                    }
                    total_runs += 1;
                }
            }
        }
        assert!(
            blocked_runs * 4 > total_runs,
            "only {blocked_runs} of {total_runs} runs were obstructed — the fan no longer \
             exercises collision"
        );
    }

    /// Water v0: falling into deep water damps, switches to swimming, and
    /// settles at a stable float line with the head out — no chatter.
    #[test]
    fn deep_water_wades_then_swims_and_floats_without_chatter() {
        let mut brickmap = island();
        let pool = carve_test_pool(&mut brickmap);
        let water_surface = (WATER_LEVEL + 1) as f32 * VOXEL_SIZE;
        let voxel_x = pool.centre_voxel_x;
        let voxel_z = pool.centre_voxel_z;
        let floor_y = (0..WATER_LEVEL)
            .rev()
            .find(|y| material_blocks_movement(brickmap.get(voxel_x, *y, voxel_z)))
            .expect("the basin has a floor");
        let depth = (WATER_LEVEL - floor_y) as f32 * VOXEL_SIZE;
        assert!(
            depth > BODY_HEIGHT_METERS * SWIM_SUBMERSION_FRACTION,
            "deepest water found is only {depth:.2} m — too shallow to swim in"
        );

        let mut body = CharacterController::from_eye(
            Vec3::new(
                (voxel_x as f32 + 0.5) * VOXEL_SIZE,
                water_surface + 3.0,
                (voxel_z as f32 + 0.5) * VOXEL_SIZE,
            ),
            0.0,
            0.0,
        );
        let mut saw_swimming = false;
        for _ in 0..600 {
            body.step(&brickmap, &CameraInput::default(), 1.0 / 60.0);
            saw_swimming |= body.submersion() == Submersion::Swimming;
            assert!(!body_overlaps_solid(
                &brickmap,
                body.feet_position,
                &body.settings
            ));
        }
        assert!(
            saw_swimming,
            "falling into deep water never entered swim mode"
        );
        // The last second must be a stable float, not a bob between states.
        let mut lowest = f32::INFINITY;
        let mut highest = f32::NEG_INFINITY;
        for _ in 0..60 {
            body.step(&brickmap, &CameraInput::default(), 1.0 / 60.0);
            lowest = lowest.min(body.feet_position.y);
            highest = highest.max(body.feet_position.y);
        }
        assert!(
            highest - lowest < 0.1,
            "the float line moved {:.3} m over the last second",
            highest - lowest
        );
        assert!(
            !body.head_submerged(),
            "a resting swimmer's eye must be above the water"
        );
        assert!(
            body.eye_position().y > water_surface,
            "eye at {:.3} m, water surface {water_surface:.3} m",
            body.eye_position().y
        );

        // ...and swimming must not require ground: diving holds the body under.
        let diving = CameraInput {
            down: true,
            ..CameraInput::default()
        };
        for _ in 0..120 {
            body.step(&brickmap, &diving, 1.0 / 60.0);
        }
        assert!(
            body.head_submerged(),
            "holding dive must submerge the head; eye at {:.3} m",
            body.eye_position().y
        );
    }

    /// A block-aligned basin must make swimming reachable ON FOOT — wade in from the
    /// shallows, cross the threshold, then float head-out without chatter. A pool
    /// that does not do this is a broken tool, and this is the test that says so.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn the_test_pool_makes_swimming_reachable() {
        let mut brickmap = island();
        let pool = carve_test_pool(&mut brickmap);
        let water_surface = (WATER_LEVEL + 1) as f32 * VOXEL_SIZE;

        // Start standing on the bed in the shallows, 3.5 m out, facing the centre.
        let shallow_x = pool.centre_voxel_x + 28;
        let mut body = standing_body(&brickmap, shallow_x, pool.centre_voxel_z, PI);
        assert_ne!(
            body.submersion(),
            Submersion::Swimming,
            "the shallows must not already be swimming depth"
        );
        let mut saw_wading = false;
        let mut saw_swimming = false;
        for _ in 0..300 {
            body.step(&brickmap, &walking_forward(), 1.0 / 60.0);
            saw_wading |= body.submersion() == Submersion::Wading;
            saw_swimming |= body.submersion() == Submersion::Swimming;
            // Reaching swimming is the behavior under test. Continuing to
            // hold forward would now correctly let the swimmer leave over the
            // opposite water-level rim.
            if saw_swimming {
                break;
            }
            assert!(!body_overlaps_solid(
                &brickmap,
                body.feet_position,
                &body.settings
            ));
        }
        assert!(saw_wading, "walking into the pool never read as wading");
        assert!(
            saw_swimming,
            "walking into the pool never reached swimming: feet at {:?}",
            body.feet_position
        );

        // Resting at the surface: a stable float line, head out (regime a).
        for _ in 0..120 {
            body.step(&brickmap, &CameraInput::default(), 1.0 / 60.0);
        }
        assert_eq!(body.submersion(), Submersion::Swimming);
        let mut lowest = f32::INFINITY;
        let mut highest = f32::NEG_INFINITY;
        for _ in 0..60 {
            body.step(&brickmap, &CameraInput::default(), 1.0 / 60.0);
            lowest = lowest.min(body.feet_position.y);
            highest = highest.max(body.feet_position.y);
        }
        assert!(
            highest - lowest < 0.1,
            "the float line moved {:.3} m over a second in the pool",
            highest - lowest
        );
        assert!(
            !body.head_submerged() && body.eye_position().y > water_surface,
            "a resting swimmer's eye must be clear of the water: eye {:.3} m, surface {:.3} m",
            body.eye_position().y,
            water_surface
        );
    }

    /// A shore block whose top meets the water surface must be usable without
    /// switching to fly mode.  At the float line the body's feet are roughly
    /// 1.5 m below that top, so ordinary walk step-up cannot handle this case.
    #[test]
    fn a_swimmer_can_exit_onto_a_water_level_ledge() {
        let mut brickmap = island();
        let pool = carve_test_pool(&mut brickmap);
        let water_surface = (WATER_LEVEL + 1) as f32 * VOXEL_SIZE;
        let ledge_x = pool.centre_voxel_x + 12;
        fill_box(
            &mut brickmap,
            [ledge_x, WATER_LEVEL - 24, pool.centre_voxel_z - 12],
            [ledge_x + 9, WATER_LEVEL, pool.centre_voxel_z + 12],
            Voxel::Stone,
        );

        let mut body = CharacterController::from_eye(
            Vec3::new(
                (pool.centre_voxel_x as f32 + 0.5) * VOXEL_SIZE,
                water_surface + 1.0,
                (pool.centre_voxel_z as f32 + 0.5) * VOXEL_SIZE,
            ),
            0.0,
            0.0,
        );
        for _ in 0..240 {
            body.step(&brickmap, &CameraInput::default(), 1.0 / 60.0);
        }
        assert_eq!(body.submersion(), Submersion::Swimming);

        for _ in 0..45 {
            body.step(&brickmap, &walking_forward(), 1.0 / 60.0);
        }
        assert!(
            body.feet_position.x
                > (ledge_x as f32 * VOXEL_SIZE) + body.settings.body_width_meters * 0.5,
            "the swimmer did not get onto the ledge: feet at {:?}",
            body.feet_position
        );
        assert!(body.grounded(), "the swimmer must stand on the ledge");
        assert!(
            (body.feet_position.y - water_surface).abs() < 0.02,
            "feet at {:.3} m, expected water-level ledge at {water_surface:.3} m",
            body.feet_position.y
        );
    }

    /// E6 — the two underwater predicates must agree wherever both apply.
    ///
    /// [`CharacterController::head_submerged`] is E2b's body-state flag (sampled
    /// from the body's own eye height during the movement step);
    /// [`crate::water::eye_is_submerged`] is E6's view-state test, asked of an
    /// arbitrary eye position so it also answers for the fly camera. They are
    /// different code reading the same world, and the underwater view would be
    /// wrong in walk mode if they disagreed — so: dive in the test pool and assert
    /// they track each other frame by frame, through the surface crossing in both
    /// directions.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn the_two_underwater_predicates_agree() {
        let mut brickmap = island();
        let pool = carve_test_pool(&mut brickmap);
        let mut body = standing_body(&brickmap, pool.centre_voxel_x + 28, pool.centre_voxel_z, PI);

        let diving = CameraInput {
            forward: true,
            down: true,
            ..CameraInput::default()
        };
        let mut saw_submerged = false;
        let mut saw_dry = false;
        for step in 0..600 {
            // Walk in and dive for the first half, swim back up for the second, so
            // the surface is crossed in both directions.
            let input = if step < 400 {
                diving
            } else {
                CameraInput {
                    up: true,
                    ..CameraInput::default()
                }
            };
            body.step(&brickmap, &input, 1.0 / 60.0);
            let by_body = body.head_submerged();
            let by_view = crate::water::eye_is_submerged(&brickmap, body.eye_position());
            assert_eq!(
                by_body,
                by_view,
                "step {step}: the body says head_submerged = {by_body} but the view says \
                 {by_view}, eye at {:?}",
                body.eye_position()
            );
            saw_submerged |= by_body;
            saw_dry |= !by_body;
        }
        assert!(
            saw_submerged && saw_dry,
            "the dive never crossed the surface: submerged {saw_submerged}, dry {saw_dry}"
        );

        // The view predicate must also answer for an eye the body could never have
        // — the fly-mode case, which is the whole reason it takes a position.
        let deep = pool.surface_centre() - Vec3::Y * 2.0;
        assert!(
            crate::water::eye_is_submerged(&brickmap, deep),
            "a fly camera 2 m under the pool's surface must read as underwater"
        );
        assert!(
            !crate::water::eye_is_submerged(&brickmap, pool.surface_centre() + Vec3::Y * 2.0),
            "a fly camera 2 m above the pool must not"
        );
    }

    /// The buoyancy shape: you float at the
    /// surface and you are neutral at depth. Dive must go under and STAY under
    /// when released — a constant buoyant acceleration, which is what this
    /// controller shipped with first, corks the body back up from any depth and
    /// would fail every assertion below.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn a_swimmer_holds_its_depth_instead_of_being_pushed_back_to_the_surface() {
        let mut brickmap = island();
        let pool = carve_test_pool(&mut brickmap);
        let water_surface = (WATER_LEVEL + 1) as f32 * VOXEL_SIZE;
        let centre = Vec3::new(
            (pool.centre_voxel_x as f32 + 0.5) * VOXEL_SIZE,
            water_surface + 1.0,
            (pool.centre_voxel_z as f32 + 0.5) * VOXEL_SIZE,
        );

        // Dropped into the middle, then left alone: it floats at the surface.
        let mut body = CharacterController::from_eye(centre, 0.0, 0.0);
        for _ in 0..240 {
            body.step(&brickmap, &CameraInput::default(), 1.0 / 60.0);
        }
        assert_eq!(body.submersion(), Submersion::Swimming);
        assert!(!body.head_submerged(), "the body did not surface");

        // Holding dive submerges the eye and keeps going down (regime c).
        let diving = CameraInput {
            down: true,
            ..CameraInput::default()
        };
        for _ in 0..180 {
            body.step(&brickmap, &diving, 1.0 / 60.0);
        }
        let dived_eye_y = body.eye_position().y;
        assert!(
            body.head_submerged() && dived_eye_y < water_surface - 1.5,
            "holding dive only reached {:.2} m below the surface",
            water_surface - dived_eye_y
        );
        assert_eq!(
            body.submersion(),
            Submersion::Swimming,
            "the body left swim mode while diving — it hit the bed"
        );

        // Releasing at depth leaves it roughly where it is (regime d): no rise,
        // and at most a slow sink.
        let released_y = body.feet_position.y;
        for _ in 0..60 {
            body.step(&brickmap, &CameraInput::default(), 1.0 / 60.0);
        }
        let drift = body.feet_position.y - released_y;
        assert!(
            drift < 0.05,
            "releasing dive at depth lifted the body {drift:.3} m in one second"
        );
        assert!(
            drift > -0.3,
            "releasing dive at depth dropped the body {drift:.3} m in one second"
        );

        // ...and it STAYS deep: three more seconds of nothing must not surface it
        // (regime b — the one a constant buoyancy would silently break).
        let settled_y = body.feet_position.y;
        for _ in 0..180 {
            body.step(&brickmap, &CameraInput::default(), 1.0 / 60.0);
        }
        assert!(
            body.feet_position.y <= settled_y + 0.05,
            "the body rose {:.3} m over three idle seconds at depth",
            body.feet_position.y - settled_y
        );
        assert!(
            body.head_submerged(),
            "three idle seconds at depth surfaced the head: eye {:.3} m, surface {:.3} m",
            body.eye_position().y,
            water_surface
        );

        // Deliberate up input is how you come back — buoyancy is not a lift.
        let rising = CameraInput {
            up: true,
            ..CameraInput::default()
        };
        for _ in 0..240 {
            body.step(&brickmap, &rising, 1.0 / 60.0);
        }
        assert!(
            !body.head_submerged(),
            "holding swim-up never surfaced the head: eye {:.3} m",
            body.eye_position().y
        );
    }

    /// Wading damps horizontal speed without needing a swim state.
    #[test]
    fn shallow_water_damps_walking_speed() {
        let mut brickmap = island();
        let pool = carve_test_pool(&mut brickmap);
        // The basin's radius-three ring has one complete metre of water over its bed.
        let voxel_x = pool.centre_voxel_x + 3 * DETAIL_CELLS_PER_WORLD_VOXEL as i32;
        let voxel_z = pool.centre_voxel_z;
        let mut body = standing_body(&brickmap, voxel_x, voxel_z, PI);
        body.settings.walk_speed = 4.0;
        let mut wading_distance = 0.0;
        let before = body.feet_position;
        for _ in 0..30 {
            body.step(&brickmap, &walking_forward(), 1.0 / 60.0);
        }
        wading_distance += (body.feet_position - before).with_y(0.0).length();
        assert_eq!(
            body.submersion(),
            Submersion::Wading,
            "standing in one metre of water must read as wading"
        );
        let dry_distance = 4.0 * 0.5; // 0.5 s at 4 m/s
        assert!(
            wading_distance < dry_distance * 0.8,
            "wading covered {wading_distance:.3} m against {dry_distance:.3} m dry"
        );
    }

    /// Entering walk mode: the body lands on the ground under the camera, is not
    /// inside geometry, and reads grounded — including when the camera was
    /// buried inside a hill.
    #[test]
    fn snapping_to_ground_lands_on_the_surface() {
        let brickmap = island();
        let (voxel_x, voxel_z) = (500, 500);
        let surface_y = surface_voxel_y(&brickmap, voxel_x, voxel_z);
        let column_x = (voxel_x as f32 + 0.5) * VOXEL_SIZE;
        let column_z = (voxel_z as f32 + 0.5) * VOXEL_SIZE;

        let mut from_the_air =
            CharacterController::from_eye(Vec3::new(column_x, 30.0, column_z), 0.0, 0.0);
        assert!(from_the_air.snap_to_ground(&brickmap, 40.0));
        assert!(from_the_air.grounded());
        assert!(!body_overlaps_solid(
            &brickmap,
            from_the_air.feet_position,
            &from_the_air.settings
        ));
        assert!(
            from_the_air.feet_position.y >= (surface_y + 1) as f32 * VOXEL_SIZE - VOXEL_SIZE,
            "feet at {:.3} m, surface top at {:.3} m",
            from_the_air.feet_position.y,
            (surface_y + 1) as f32 * VOXEL_SIZE
        );

        // Buried: the eye inside the hill must still produce a standable body.
        let mut buried = CharacterController::from_eye(
            Vec3::new(column_x, (surface_y - 8) as f32 * VOXEL_SIZE, column_z),
            0.0,
            0.0,
        );
        buried.snap_to_ground(&brickmap, 60.0);
        assert!(
            !body_overlaps_solid(&brickmap, buried.feet_position, &buried.settings),
            "a buried camera left the body inside solid at {:?}",
            buried.feet_position
        );

        // No ground at all (over the void, looking down from the sky).
        let mut over_the_void = CharacterController::from_eye(Vec3::new(2.0, 30.0, 2.0), 0.0, 0.0);
        let before = over_the_void.feet_position;
        assert!(!over_the_void.snap_to_ground(&brickmap, 4.0));
        assert_eq!(over_the_void.feet_position, before);
    }

    /// The body never leaves the world box, in any direction.
    #[test]
    fn the_body_stays_inside_the_world_box() {
        let brickmap = island();
        let world_x = WORLD_SIZE_X as f32 * VOXEL_SIZE;
        let world_z = WORLD_SIZE_Z as f32 * VOXEL_SIZE;
        for (yaw, start) in [
            (0.0_f32, Vec3::new(world_x - 1.0, 20.0, world_z * 0.5)),
            (std::f32::consts::PI, Vec3::new(1.0, 20.0, world_z * 0.5)),
            (
                std::f32::consts::FRAC_PI_2,
                Vec3::new(world_x * 0.5, 20.0, world_z - 1.0),
            ),
            (
                -std::f32::consts::FRAC_PI_2,
                Vec3::new(world_x * 0.5, 20.0, 1.0),
            ),
        ] {
            let mut body = CharacterController::from_eye(start, yaw, 0.0);
            for _ in 0..600 {
                body.step(&brickmap, &walking_forward(), 1.0 / 60.0);
            }
            let half_width = body.settings.body_width_meters * 0.5;
            assert!(
                body.feet_position.x >= half_width - 1e-3
                    && body.feet_position.x <= world_x - half_width + 1e-3
                    && body.feet_position.z >= half_width - 1e-3
                    && body.feet_position.z <= world_z - half_width + 1e-3
                    && body.feet_position.y >= 0.0,
                "the body left the world at {:?}",
                body.feet_position
            );
        }
    }

    /// Mode-switch invariants: the eye/pose the renderer and the audio listener
    /// read must be the body's head, and a fly->walk->fly round trip must not
    /// turn the view.
    #[test]
    fn the_eye_pose_is_the_body_head_and_survives_a_mode_round_trip() {
        let eye = Vec3::new(10.0, 20.0, 30.0);
        let body = CharacterController::from_eye(eye, 1.25, -0.4);
        assert!((body.eye_position() - eye).length() < 1e-5);
        assert!((body.pose().position - eye).length() < 1e-5);
        assert_eq!(
            body.pose().forward,
            CameraPose::from_yaw_pitch(eye, 1.25, -0.4).forward
        );
        assert!(
            (body.feet_position.y - (eye.y - EYE_HEIGHT_METERS)).abs() < 1e-5,
            "the feet must sit one eye height below the head"
        );
    }

    /// The substep clamp is not a suggestion: every substep must move the body
    /// less than half its smallest dimension.
    #[test]
    fn substeps_bound_the_per_step_motion() {
        let mut body = CharacterController::from_eye(Vec3::new(10.0, 20.0, 30.0), 0.0, 0.0);
        body.settings.walk_speed = MAX_WALK_SPEED;
        for delta_seconds in [1.0 / 240.0_f32, 1.0 / 60.0, 0.04, 0.25, 1.0, 10.0] {
            let clamped = delta_seconds.min(MAX_STEP_SECONDS);
            let substeps = body.substep_count(clamped);
            let horizontal = MAX_WALK_SPEED * SPRINT_MULTIPLIER * clamped / substeps as f32;
            assert!(
                horizontal <= MAX_SUBSTEP_METERS + 1e-6,
                "{delta_seconds} s -> {substeps} substeps of {horizontal:.4} m"
            );
            assert!(substeps <= MAX_SUBSTEPS);
        }
        // A terminal-velocity fall must be substepped just as tightly.
        body.vertical_velocity = -TERMINAL_VELOCITY_METERS_PER_SECOND;
        let substeps = body.substep_count(MAX_STEP_SECONDS);
        let vertical = TERMINAL_VELOCITY_METERS_PER_SECOND * MAX_STEP_SECONDS / substeps as f32;
        assert!(
            vertical <= MAX_SUBSTEP_METERS + 1e-6,
            "a terminal fall moved {vertical:.4} m per substep"
        );
    }

    /// Thin cover and water must not block movement, and the auto-step must not
    /// treat a grass tuft as a step. This is the `is_solid` contract, seen from
    /// the body's side.
    #[test]
    fn vegetation_and_water_are_walked_through() {
        let mut brickmap = island();
        let (voxel_x, voxel_z) = (500, 508);
        let (floor_y, _) = flat_walkway(&mut brickmap, (496, 544), voxel_z);
        for (offset, voxel) in [
            (8, Voxel::TallGrass),
            (12, Voxel::FlowerBlue),
            (16, Voxel::Reed),
            (20, Voxel::Water),
        ] {
            fill_box(
                &mut brickmap,
                [voxel_x + offset, floor_y + 1, voxel_z - 8],
                [voxel_x + offset, floor_y + 3, voxel_z + 8],
                voxel,
            );
        }
        let mut body = standing_body(&brickmap, voxel_x, voxel_z, 0.0);
        let start = body.feet_position;
        for _ in 0..60 {
            body.step(&brickmap, &walking_forward(), 1.0 / 60.0);
        }
        let travelled = body.feet_position.x - start.x;
        assert!(
            travelled > 22.0 * VOXEL_SIZE,
            "cover blocked the walk: travelled only {travelled:.3} m"
        );
        assert!(
            (body.feet_position.y - start.y).abs() < VOXEL_SIZE,
            "cover was treated as a step: y went {:.3} -> {:.3}",
            start.y,
            body.feet_position.y
        );
    }
}
