//! L0 — authored light-transport validation rooms.
//!
//! These are not scenery and not a scene format. They are **fixtures**: worlds
//! whose geometry is chosen so that a single lighting property is legible in one
//! screenshot, and whose ground truth is known by construction rather than by
//! comparison against a reference renderer we do not have.
//!
//! The first one is the *rainbow corridor*, reconstructed from x1m4's
//! 2022-11-24 ray-tracing demo (`05itZEDaj1A`). That demo is worth copying
//! because of what it is: it is the shot he posted three weeks after finding a
//! bug where the GI had silently not been bouncing at all —
//!
//! > *"shocking news! in the last videos the lighting actually didn't have
//! > global illumination, as I messed up a parameter regarding albedo. now as
//! > you can see, it correctly bounces"* (2022-11-01)
//!
//! — which is exactly the failure this fixture is built to catch. A GI system
//! that has lost its bounce term still renders a plausible picture. It does not
//! render a plausible picture *of this room*.
//!
//! # What the corridor tests, surface by surface
//!
//! * **Saturated side walls and floor, changing colour every
//!   [`SEGMENT_LENGTH`] voxels along the corridor** — the bounce term carries
//!   material colour. A transport that drops albedo (x1m4's bug) leaves the
//!   ceiling grey; a transport that carries it stains the ceiling in bands.
//! * **A neutral white ceiling** ([`CEILING_MATERIAL`]) — it receives no direct
//!   light at all, so *every* photon reaching it arrived via at least one
//!   bounce. Any colour up there is by definition indirect. This is the read.
//! * **The six band colours are the corners of the RGB cube** — red, yellow,
//!   green, cyan, blue, magenta, in spectral order, so adjacent segments differ
//!   in exactly one channel. The corpus records that CAGI's integer channels
//!   *"diverge visibly as they darken"*, and a per-channel transport bug shows
//!   up here as a hue shift along the corridor rather than as a brightness
//!   error, which is far easier to see.
//! * **One ceiling slot as the only light entrance**, [`SLOT_WIDTH`] voxels wide
//!   and running the corridor's FULL LENGTH along the `-X` side, with the sun
//!   crossing it at right angles — light arrives collimated from a known
//!   direction and lands on the floor of the same band it entered over, so every
//!   one of the six bands is directly lit and they can be compared against each
//!   other in a single frame. Orienting the slot across the near end instead
//!   lights one band and starves the rest; see [`SLOT_WIDTH`] for why that
//!   distinction is the difference between a fixture and a decoration. It also
//!   makes the room usable *today*: CAGI v0 injects sun and sky only, and
//!   emissive voxels are E5, so a sun-through-a-slot room is testable before the
//!   emitter work lands.
//! * **A two-voxel shell** ([`SHELL_THICKNESS`]) — thick enough that sub-voxel
//!   leaking through the walls cannot be confused with a transport error. Wall
//!   leaking is a *separate* question (L2, per-face opacity) and wants its own
//!   thin-walled fixture; conflating the two would make both unreadable.
//!
//! # Deliberate deviation from the reference
//!
//! x1m4's corridor has ten colour bands. This one has six, one per corner of the
//! RGB cube ([`BAND_MATERIALS`]). Six already covers every full/zero combination
//! of R, G and B, which is the entire diagnostic; four more would add material
//! rows and no information.
//!
//! # The trap this fixture already fell into once
//!
//! The bands must be **reflectors**. `Voxel::HdrRed` and its five siblings look
//! like the right palette — they are the saturated primaries and secondaries —
//! and they are pure emitters at `albedo: [0, 0, 0]`. Built from those, the
//! corridor lights itself, the ceiling shows convincing colour bands, and the
//! bounce term is never exercised. It renders as a *pass* while measuring
//! nothing. What caught it was the `gi-no-emissive` lever driving the frame
//! difference to 0.0009% in a room where every pixel is supposed to be indirect.
//!
//! So: the fixture's own diagnostic value depends on a material property that is
//! invisible in the picture, and a screenshot could not have told us. That is
//! worth remembering before trusting any later reading taken from this room.
//!
//! # Why the bounding box is cleared first
//!
//! [`RainbowCorridor::carve`] writes air across the entire outer box before it
//! writes a single wall. The fixture must be a *sealed room*, and that property
//! has to hold no matter what terrain happened to be generated where it lands —
//! a stray stone voxel poking into the interior would add an unaccounted
//! reflector, and a hole punched into terrain would add an unaccounted light
//! path. Clearing first makes the geometry a function of the constants in this
//! module and nothing else.

use voxel_core::world::{Voxel, WorldVoxelCoord, WORLD_VOXELS_X, WORLD_VOXELS_Y, WORLD_VOXELS_Z};

use crate::brickmap::{Brickmap, ClearanceUpdate};

/// Interior cross-section, in one-metre world voxels: 12 wide by 12 high, the
/// dimensions Pascal read off the reference shot.
pub const INTERIOR_WIDTH: i32 = 12;
/// Interior height, in one-metre world voxels.
pub const INTERIOR_HEIGHT: i32 = 12;

/// Corridor length covered by one colour, in world voxels. Three is the
/// reference shot's band width.
pub const SEGMENT_LENGTH: i32 = 3;

/// The band palette, in spectral order. Adjacent entries differ in exactly one
/// RGB channel — see the module note on why that ordering is the diagnostic.
/// These MUST be reflectors, not emitters. The `Voxel::Hdr*` rows are the
/// obvious-looking choice and are exactly wrong: they carry `albedo: [0, 0, 0]`
/// with `emission: Some(..)`, so a corridor built from them is lit by its own
/// walls and has no bounce to measure at all. The first version of this fixture
/// did that, rendered a perfectly convincing striped ceiling, and the only thing
/// that caught it was the `gi-no-emissive` lever collapsing the frame difference
/// to 0.0009%. Hence [`Voxel::AlbedoRed`] and friends, whose reflectance is the
/// whole point.
pub const BAND_MATERIALS: [Voxel; 6] = [
    Voxel::AlbedoRed,
    Voxel::AlbedoYellow,
    Voxel::AlbedoGreen,
    Voxel::AlbedoCyan,
    Voxel::AlbedoBlue,
    Voxel::AlbedoMagenta,
];

/// Interior length: one [`SEGMENT_LENGTH`] run per band.
pub const INTERIOR_LENGTH: i32 = SEGMENT_LENGTH * BAND_MATERIALS.len() as i32;

/// Wall/floor/ceiling thickness. Two, so wall leaking cannot be mistaken for a
/// transport bug — see the module note.
pub(crate) const SHELL_THICKNESS: i32 = 2;

/// The shell material. Neutral and dark enough that the shell itself
/// contributes nothing interesting to the bounce budget.
pub(crate) const SHELL_MATERIAL: Voxel = Voxel::Stone;

/// The ceiling material: the palette's whitest row. The ceiling is the readout
/// surface, so it must not tint what lands on it.
pub(crate) const CEILING_MATERIAL: Voxel = Voxel::Snow;

/// Far-end cap material. Same as the ceiling, for the same reason: it is a
/// second neutral surface that only ever sees indirect light.
pub(crate) const END_CAP_MATERIAL: Voxel = Voxel::Snow;

/// Ceiling-slot width across the corridor, in world voxels. The slot runs the
/// corridor's **full length**, so this is its only finite dimension.
///
/// It is oriented ALONG the corridor rather than across it, and that is the
/// difference between a working fixture and a decorative one. Across the near
/// end, sunlight enters at one Z and lands on the floor a fixed distance further
/// along — about 4.4 voxels at 70 degrees — so it strikes ONE band. The other
/// five are then lit only by light that has already bounced, contribute almost
/// nothing back, and the corridor reads as a bright end fading to black. Six
/// RGB-cube corners are worth nothing if four of them never reflect anything.
///
/// Run lengthwise with the sun crossing it ([`RainbowCorridor::sun`], azimuth
/// 180), every Z gets the same treatment: light enters above the `-X` side and
/// lands on the floor of the SAME band it entered over. All six bands get a
/// directly lit floor, so all six can bleed onto the ceiling and be compared
/// against each other in one frame.
pub(crate) const SLOT_WIDTH: i32 = 2;

/// Height of the far-end notch through the `+X` wall, in world voxels — the
/// "two blocks taken off" of the reference shot.
pub(crate) const NOTCH_HEIGHT: i32 = 2;
/// Corridor length spanned by that notch.
pub(crate) const NOTCH_LENGTH: i32 = 4;

/// Interior minimum corner, in one-metre world voxels.
///
/// Placed high and toward a corner of the lattice on purpose: the ceiling slot
/// has to see open sky for the sun to reach the interior, so the room must sit
/// above the island's terrain rather than inside it. A slot that opens into
/// stone would make the fixture render black and the reason would not be
/// obvious.
pub(crate) const INTERIOR_MIN: [i32; 3] = [18, 14, 14];

/// How the far-end notch is treated.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NotchState {
    /// Fully sealed: the ceiling slot is the only opening. This is the
    /// configuration to read colour bleed from, because it has exactly one
    /// light path.
    Sealed,
    /// Two voxels cut through the `+X` wall near the far end, reproducing the
    /// reference shot. Adds a second light path and a hard corner, which is the
    /// corner-seal occlusion case: TooManyLimits' kernel attenuates a diagonal
    /// in three tiers depending on how sealed the corner is, and both
    /// bracketing neighbours being solid must transmit *nothing*. This is the
    /// configuration that exercises that.
    Open,
}

/// One authored corridor. Construct with [`RainbowCorridor::new`] and stamp it
/// into a brickmap with [`RainbowCorridor::carve`].
#[derive(Clone, Copy, Debug)]
pub struct RainbowCorridor {
    /// Interior minimum corner, one-metre world voxels.
    pub interior_min: [i32; 3],
    pub notch: NotchState,
}

impl RainbowCorridor {
    pub fn new(notch: NotchState) -> RainbowCorridor {
        RainbowCorridor {
            interior_min: INTERIOR_MIN,
            notch,
        }
    }

    /// Interior maximum corner, inclusive.
    pub(crate) fn interior_max(&self) -> [i32; 3] {
        [
            self.interior_min[0] + INTERIOR_WIDTH - 1,
            self.interior_min[1] + INTERIOR_HEIGHT - 1,
            self.interior_min[2] + INTERIOR_LENGTH - 1,
        ]
    }

    /// Outer bounding box of the whole structure, inclusive, shell included.
    pub fn outer_bounds(&self) -> ([i32; 3], [i32; 3]) {
        let interior_max = self.interior_max();
        (
            [
                self.interior_min[0] - SHELL_THICKNESS,
                self.interior_min[1] - SHELL_THICKNESS,
                self.interior_min[2] - SHELL_THICKNESS,
            ],
            [
                interior_max[0] + SHELL_THICKNESS,
                interior_max[1] + SHELL_THICKNESS,
                interior_max[2] + SHELL_THICKNESS,
            ],
        )
    }

    /// Whether the structure fits the world lattice. Checked by
    /// [`Self::carve`], because a fixture that silently clips at the world edge
    /// is a fixture with an unaccounted hole in it.
    pub(crate) fn fits_world(&self) -> bool {
        let (min, max) = self.outer_bounds();
        min[0] >= 0
            && min[1] >= 0
            && min[2] >= 0
            && max[0] < WORLD_VOXELS_X as i32
            && max[1] < WORLD_VOXELS_Y as i32
            && max[2] < WORLD_VOXELS_Z as i32
    }

    /// The band material for a corridor position, or `None` outside the
    /// interior's Z range.
    pub(crate) fn band_material(&self, z: i32) -> Option<Voxel> {
        let offset = z - self.interior_min[2];
        if !(0..INTERIOR_LENGTH).contains(&offset) {
            return None;
        }
        Some(BAND_MATERIALS[(offset / SEGMENT_LENGTH) as usize])
    }

    /// Whether a voxel is inside the interior air volume.
    pub(crate) fn is_interior(&self, coordinate: WorldVoxelCoord) -> bool {
        let interior_max = self.interior_max();
        (self.interior_min[0]..=interior_max[0]).contains(&coordinate.x)
            && (self.interior_min[1]..=interior_max[1]).contains(&coordinate.y)
            && (self.interior_min[2]..=interior_max[2]).contains(&coordinate.z)
    }

    /// Ceiling-slot X range, inclusive: [`SLOT_WIDTH`] voxels against the `-X`
    /// side, which is the side the sun crosses from.
    ///
    /// Flush with the interior's `-X` edge rather than inset. The sun travels
    /// `+X`, so light entering here moves away from the `-X` wall immediately and
    /// the full width of the floor is available to it.
    pub(crate) fn slot_x_range(&self) -> (i32, i32) {
        (self.interior_min[0], self.interior_min[0] + SLOT_WIDTH - 1)
    }

    /// Ceiling-slot Z range, inclusive: the corridor's whole length, so every
    /// colour band gets its own directly lit floor.
    pub(crate) fn slot_z_range(&self) -> (i32, i32) {
        (self.interior_min[2], self.interior_max()[2])
    }

    /// Far-end notch extent, inclusive, as `(y_range, z_range)`. The notch cuts
    /// the top [`NOTCH_HEIGHT`] interior rows of the `+X` wall.
    pub(crate) fn notch_extent(&self) -> ([i32; 2], [i32; 2]) {
        let interior_max = self.interior_max();
        (
            [interior_max[1] - NOTCH_HEIGHT + 1, interior_max[1]],
            [interior_max[2] - NOTCH_LENGTH + 1, interior_max[2]],
        )
    }

    /// Whether a voxel is part of the ceiling slot's carved-out volume.
    fn is_slot(&self, coordinate: WorldVoxelCoord) -> bool {
        let interior_max = self.interior_max();
        let (slot_min_x, slot_max_x) = self.slot_x_range();
        let (slot_min_z, slot_max_z) = self.slot_z_range();
        coordinate.y > interior_max[1]
            && (slot_min_x..=slot_max_x).contains(&coordinate.x)
            && (slot_min_z..=slot_max_z).contains(&coordinate.z)
    }

    /// Whether a voxel is part of the far-end notch's carved-out volume.
    fn is_notch(&self, coordinate: WorldVoxelCoord) -> bool {
        if self.notch != NotchState::Open {
            return false;
        }
        let interior_max = self.interior_max();
        let ([notch_min_y, notch_max_y], [notch_min_z, notch_max_z]) = self.notch_extent();
        coordinate.x > interior_max[0]
            && (notch_min_y..=notch_max_y).contains(&coordinate.y)
            && (notch_min_z..=notch_max_z).contains(&coordinate.z)
    }

    /// Stamp the corridor into `brickmap`, returning the number of one-metre
    /// voxels written.
    ///
    /// The order is load-bearing: clear the whole outer box to air, then write
    /// the shell, then paint the interior surfaces, then cut the openings. Every
    /// later step overrides the earlier one, so the final geometry depends only
    /// on this module's constants and never on the terrain underneath.
    pub fn carve(&self, brickmap: &mut Brickmap) -> usize {
        assert!(
            self.fits_world(),
            "rainbow corridor at {:?} does not fit the {WORLD_VOXELS_X}x{WORLD_VOXELS_Y}x\
             {WORLD_VOXELS_Z} world lattice",
            self.interior_min,
        );

        // A local box is the right clearance mode for the same reason the water
        // pool uses one: the edit is a compact region, and the distance field
        // only has to be exact within a neighbourhood of it.
        let clearance = ClearanceUpdate::LocalBox { radius_cells: 8 };
        let (outer_min, outer_max) = self.outer_bounds();
        let interior_max = self.interior_max();
        let mut written = 0_usize;

        for z in outer_min[2]..=outer_max[2] {
            for y in outer_min[1]..=outer_max[1] {
                for x in outer_min[0]..=outer_max[0] {
                    let coordinate = WorldVoxelCoord::new(x, y, z);
                    let material = self.material_at(coordinate, interior_max);
                    brickmap.set_world_voxel(coordinate, material, clearance);
                    if material != Voxel::Air {
                        written += 1;
                    }
                }
            }
        }

        written
    }

    /// The authored material for one voxel of the outer box. Air everywhere the
    /// fixture wants empty space, including the two openings.
    fn material_at(&self, coordinate: WorldVoxelCoord, interior_max: [i32; 3]) -> Voxel {
        if self.is_interior(coordinate) {
            return Voxel::Air;
        }
        if self.is_slot(coordinate) || self.is_notch(coordinate) {
            return Voxel::Air;
        }

        // Shell. The three surfaces that face the interior get their own
        // materials; everything else is structural.
        let faces_interior_side = (self.interior_min[0]..=interior_max[0]).contains(&coordinate.x)
            && (self.interior_min[2]..=interior_max[2]).contains(&coordinate.z);
        let within_cross_section = (self.interior_min[0]..=interior_max[0]).contains(&coordinate.x)
            && (self.interior_min[1]..=interior_max[1]).contains(&coordinate.y);

        // Ceiling: the readout surface. Only the layer actually touching the
        // interior needs to be white; the layer above it is never visible.
        if faces_interior_side && coordinate.y == interior_max[1] + 1 {
            return CEILING_MATERIAL;
        }
        // Floor: takes the band colour, so the lit patch is a coloured
        // reflector.
        if faces_interior_side && coordinate.y == self.interior_min[1] - 1 {
            return self.band_material(coordinate.z).unwrap_or(SHELL_MATERIAL);
        }
        // Side walls: the other two coloured reflectors.
        let is_side_wall_layer =
            coordinate.x == self.interior_min[0] - 1 || coordinate.x == interior_max[0] + 1;
        let within_height = (self.interior_min[1]..=interior_max[1]).contains(&coordinate.y);
        if is_side_wall_layer
            && within_height
            && (self.interior_min[2]..=interior_max[2]).contains(&coordinate.z)
        {
            return self.band_material(coordinate.z).unwrap_or(SHELL_MATERIAL);
        }
        // Far end cap: neutral, like the ceiling.
        if within_cross_section && coordinate.z == interior_max[2] + 1 {
            return END_CAP_MATERIAL;
        }

        SHELL_MATERIAL
    }

    /// The fixture's sun: high, aimed down the corridor, ambient floor removed.
    ///
    /// Part of the fixture rather than of whatever renders it, because the room
    /// only means anything under this light. Both the bench section and the app
    /// flag read it from here so the two cannot drift apart.
    ///
    /// Three things are load-bearing:
    ///
    /// * `ambient_scale: 0.0` — with the shipped 1.0 the whole interior sits at
    ///   the ambient floor and the indirect contribution is a fraction nobody can
    ///   see. At zero the room is genuinely dark and every lit pixel came through
    ///   the slot. `gi-off` renders pure black, which is the proof.
    /// * `day_night_enabled: false` — it defaults to TRUE, in which case
    ///   `day_phase` drives the direction and the angles below are ignored
    ///   entirely. That would leave the slot pointing away from the sun and the
    ///   fixture rendering black for a reason that looks exactly like a GI bug.
    /// * Azimuth 180 puts the travel direction at pure `+X` — **across** the
    ///   corridor, square to the lengthwise slot. This is the pairing that makes
    ///   the fixture work. Any azimuth with a `Z` component shifts where light
    ///   lands along the corridor, which reintroduces exactly the problem the
    ///   rotated slot fixes: some bands lit, others not. Crossing the slot at a
    ///   right angle makes every Z identical, so all six bands are lit alike and
    ///   differ only in colour — which is the one variable the room is testing.
    /// * 60 degrees of elevation — light travels 0.58 voxels of `+X` per voxel of
    ///   drop, so over the 12-voxel interior height it crosses about 6.9 of the 12
    ///   voxels of width. It lands on the floor a little past the middle, well
    ///   clear of both side walls, so the lit strip is floor rather than a wall
    ///   graze.
    pub fn sun() -> voxel_environment::SunSettings {
        voxel_environment::SunSettings {
            azimuth_degrees: 180.0,
            elevation_degrees: 60.0,
            intensity_scale: 1.0,
            ambient_scale: 0.0,
            day_night_enabled: false,
            cycle_running: false,
            ..voxel_environment::SunSettings::default()
        }
    }

    /// Yaw that looks down the corridor, for [`crate::camera::FlyCamera`] and
    /// [`crate::camera::CameraPose`] alike — both use `forward.z = sin(yaw)`.
    pub fn yaw_down_corridor() -> f32 {
        std::f32::consts::FRAC_PI_2
    }

    /// Yaw that looks back up the corridor, toward its near end.
    pub fn yaw_up_corridor() -> f32 {
        -std::f32::consts::FRAC_PI_2
    }

    /// Eye position at the corridor's near end, for a pose looking down its
    /// length. World metres, which are one-metre world voxels one-for-one.
    ///
    /// Now that the slot runs the whole length there is no "past the slot" to
    /// stand beyond — every Z is lit alike, and the useful composition is simply
    /// the one that puts all six bands in frame at once, receding. Held 1.5
    /// voxels inside the end cap so the near wall is behind the eye rather than
    /// filling the frame.
    pub fn viewer_eye_meters(&self) -> [f32; 3] {
        [
            self.interior_min[0] as f32 + INTERIOR_WIDTH as f32 * 0.5,
            self.interior_min[1] as f32 + INTERIOR_HEIGHT as f32 * 0.35,
            self.interior_min[2] as f32 + 1.5,
        ]
    }

    /// Eye position at the far end, looking back up the corridor — the same six
    /// bands in reverse order.
    ///
    /// Worth having as its own pose rather than as a mirror image: the bands are
    /// asymmetric in colour, so reading them from both ends is what shows whether
    /// a falloff is a property of the light or of the palette.
    pub fn far_eye_meters(&self) -> [f32; 3] {
        [
            self.interior_min[0] as f32 + INTERIOR_WIDTH as f32 * 0.5,
            self.interior_min[1] as f32 + INTERIOR_HEIGHT as f32 * 0.5,
            self.interior_max()[2] as f32 - 0.5,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::world::DETAIL_CELLS_PER_WORLD_VOXEL;

    /// Material id read back through the brickmap at a one-metre coordinate.
    /// `Brickmap::get` works in detail cells, so this goes through the voxel's
    /// detail origin.
    fn material_at(brickmap: &Brickmap, coordinate: WorldVoxelCoord) -> Voxel {
        let origin = coordinate.detail_origin();
        voxel_material::material::material_voxel(brickmap.get(origin[0], origin[1], origin[2]))
    }

    fn carved(notch: NotchState) -> (RainbowCorridor, Brickmap) {
        let world = voxel_core::world::VoxelWorld::generate(1, 0.0);
        let mut brickmap = Brickmap::build(&world);
        let corridor = RainbowCorridor::new(notch);
        corridor.carve(&mut brickmap);
        (corridor, brickmap)
    }

    #[test]
    fn geometry_fits_the_world_lattice() {
        let corridor = RainbowCorridor::new(NotchState::Sealed);
        assert!(corridor.fits_world());
        let (min, max) = corridor.outer_bounds();
        assert_eq!(min, [16, 12, 12]);
        assert_eq!(max, [31, 27, 33]);
        // The detail grid the renderer traverses must contain it too.
        assert!(
            (max[1] + 1) * DETAIL_CELLS_PER_WORLD_VOXEL as i32
                <= voxel_core::world::DETAIL_GRID_SIZE_Y as i32
        );
    }

    #[test]
    fn interior_dimensions_match_the_reference_shot() {
        let corridor = RainbowCorridor::new(NotchState::Sealed);
        let interior_max = corridor.interior_max();
        assert_eq!(
            interior_max[0] - corridor.interior_min[0] + 1,
            INTERIOR_WIDTH
        );
        assert_eq!(
            interior_max[1] - corridor.interior_min[1] + 1,
            INTERIOR_HEIGHT
        );
        assert_eq!(
            interior_max[2] - corridor.interior_min[2] + 1,
            INTERIOR_LENGTH
        );
        // 12 x 12 cross-section, six three-voxel bands.
        assert_eq!(INTERIOR_WIDTH, 12);
        assert_eq!(INTERIOR_HEIGHT, 12);
        assert_eq!(INTERIOR_LENGTH, 18);
    }

    #[test]
    fn every_band_is_three_voxels_of_its_own_colour() {
        let corridor = RainbowCorridor::new(NotchState::Sealed);
        for (index, expected) in BAND_MATERIALS.iter().enumerate() {
            for offset in 0..SEGMENT_LENGTH {
                let z = corridor.interior_min[2] + index as i32 * SEGMENT_LENGTH + offset;
                assert_eq!(
                    corridor.band_material(z),
                    Some(*expected),
                    "band {index} at z={z}"
                );
            }
        }
        // One past each end is outside the interior.
        assert_eq!(corridor.band_material(corridor.interior_min[2] - 1), None);
        assert_eq!(
            corridor.band_material(corridor.interior_min[2] + INTERIOR_LENGTH),
            None
        );
    }

    /// The test that would have caught the original bug. Every band material has
    /// to be a REFLECTOR — non-trivial albedo and no emission — or the fixture
    /// measures its own walls instead of the bounce term. See the module docs.
    #[test]
    fn every_band_material_is_a_reflector_and_not_an_emitter() {
        for band in BAND_MATERIALS {
            let material = &voxel_material::material::MATERIALS
                [voxel_material::material::material_id(band) as usize];
            assert!(
                material.emission.is_none(),
                "band material `{}` emits — a corridor of emitters lights itself and \
                 exercises no bounce",
                material.name
            );
            let brightest = material
                .albedo
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max);
            assert!(
                brightest > 0.5,
                "band material `{}` has peak albedo {brightest}, too dark to bleed a \
                 readable amount of colour onto the ceiling",
                material.name
            );
            // Energy conservation: at albedo >= 1 a closed room is a divergent
            // feedback loop under bounce resampling.
            assert!(
                brightest < 1.0,
                "band material `{}` has peak albedo {brightest} >= 1.0, which makes the \
                 infinite-bounce series diverge",
                material.name
            );
        }
    }

    /// The ceiling is the readout surface: it must be bright, neutral and dark in
    /// no channel, so a tint on it can only have come from a band.
    #[test]
    fn ceiling_is_a_neutral_reflector() {
        for readout in [CEILING_MATERIAL, END_CAP_MATERIAL] {
            let material = &voxel_material::material::MATERIALS
                [voxel_material::material::material_id(readout) as usize];
            assert!(
                material.emission.is_none(),
                "readout surface `{}` emits, so colour on it proves nothing",
                material.name
            );
            let [red, green, blue] = material.albedo;
            let brightest = red.max(green).max(blue);
            let darkest = red.min(green).min(blue);
            assert!(
                brightest > 0.7,
                "readout surface `{}` is too dark to show a bleed",
                material.name
            );
            assert!(
                brightest - darkest < 0.1,
                "readout surface `{}` is tinted ({:?}) — it would bias the hue it is \
                 supposed to be measuring",
                material.name,
                material.albedo,
            );
        }
    }

    /// The slot must run the corridor's full length, so every colour band sits
    /// under open sky and gets a directly lit floor.
    ///
    /// This is the property the first version got wrong: a slot across the near
    /// end lit band 2 and left bands 3-6 with nothing to reflect, which made four
    /// of the six palette entries dead weight. Asserted per band rather than as a
    /// single length comparison so a failure says which band went dark.
    #[test]
    fn the_slot_spans_every_colour_band() {
        let corridor = RainbowCorridor::new(NotchState::Sealed);
        let (slot_min_z, slot_max_z) = corridor.slot_z_range();
        for (index, _) in BAND_MATERIALS.iter().enumerate() {
            for offset in 0..SEGMENT_LENGTH {
                let z = corridor.interior_min[2] + index as i32 * SEGMENT_LENGTH + offset;
                assert!(
                    (slot_min_z..=slot_max_z).contains(&z),
                    "band {index} at z={z} is not under the slot ({slot_min_z}..={slot_max_z}) \
                     — it would only ever see bounced light and could not bleed"
                );
            }
        }
        // And it is narrow across the corridor, against the -X side: a slot the
        // full width would be an open roof, not a slot.
        let (slot_min_x, slot_max_x) = corridor.slot_x_range();
        assert_eq!(slot_max_x - slot_min_x + 1, SLOT_WIDTH);
        assert!(SLOT_WIDTH < INTERIOR_WIDTH / 2);
        assert_eq!(slot_min_x, corridor.interior_min[0]);
    }

    /// The sun must cross the slot square-on. An azimuth with a `Z` component
    /// would slide the lit strip along the corridor and undo the rotation.
    #[test]
    fn the_sun_crosses_the_corridor_rather_than_running_along_it() {
        let direction = RainbowCorridor::sun().sun_direction();
        assert!(
            direction.z.abs() < 1e-5,
            "sun has a Z component ({}) — the lit strip would land on different \
             bands at different depths",
            direction.z
        );
        assert!(direction.x.abs() > 0.1, "sun does not cross the corridor");
        assert!(direction.y > 0.0, "sun is below the horizon");

        // It must clear the side walls: over the interior height the light has to
        // travel less than the interior width, or the "lit floor strip" is
        // actually a wall graze.
        let crossing = (direction.x / direction.y).abs() * INTERIOR_HEIGHT as f32;
        assert!(
            crossing < INTERIOR_WIDTH as f32,
            "light crosses {crossing:.1} voxels over a {INTERIOR_HEIGHT}-voxel drop, \
             wider than the {INTERIOR_WIDTH}-voxel interior — it hits the far wall, \
             not the floor"
        );
    }

    /// Ambient must be off and the day/night cycle disabled, or the fixture's
    /// premise fails silently. See [`RainbowCorridor::sun`].
    #[test]
    fn the_fixture_sun_makes_the_room_dark() {
        let sun = RainbowCorridor::sun();
        assert_eq!(sun.ambient_scale, 0.0, "ambient floor would mask the GI");
        assert!(
            !sun.day_night_enabled,
            "with the cycle enabled `day_phase` overrides the azimuth/elevation \
             and the slot stops facing the sun"
        );
        assert!(sun.intensity_scale > 0.0, "no light at all");
    }

    #[test]
    fn band_palette_covers_every_rgb_cube_corner() {
        // The diagnostic claim in the module docs: six saturated rows, all
        // distinct, adjacent entries one channel apart. If someone reorders the
        // palette for looks, this is what says no.
        let mut seen = BAND_MATERIALS.to_vec();
        seen.dedup();
        assert_eq!(seen.len(), BAND_MATERIALS.len(), "palette has a duplicate");
        assert_eq!(
            BAND_MATERIALS,
            [
                Voxel::AlbedoRed,
                Voxel::AlbedoYellow,
                Voxel::AlbedoGreen,
                Voxel::AlbedoCyan,
                Voxel::AlbedoBlue,
                Voxel::AlbedoMagenta,
            ],
            "spectral order is the diagnostic — see the module docs"
        );
    }

    #[test]
    fn carve_paints_walls_floor_and_ceiling_as_specified() {
        let (corridor, brickmap) = carved(NotchState::Sealed);
        let interior_max = corridor.interior_max();
        let centre_x = corridor.interior_min[0] + INTERIOR_WIDTH / 2;

        for z in corridor.interior_min[2]..=interior_max[2] {
            let band = corridor.band_material(z).expect("z is inside the interior");

            // Floor and both side walls carry the band colour.
            assert_eq!(
                material_at(
                    &brickmap,
                    WorldVoxelCoord::new(centre_x, corridor.interior_min[1] - 1, z)
                ),
                band,
                "floor at z={z}"
            );
            assert_eq!(
                material_at(
                    &brickmap,
                    WorldVoxelCoord::new(
                        corridor.interior_min[0] - 1,
                        corridor.interior_min[1] + 1,
                        z
                    )
                ),
                band,
                "-X wall at z={z}"
            );
            assert_eq!(
                material_at(
                    &brickmap,
                    WorldVoxelCoord::new(interior_max[0] + 1, corridor.interior_min[1] + 1, z)
                ),
                band,
                "+X wall at z={z}"
            );
        }

        // The ceiling is neutral along the WHOLE length. The slot runs lengthwise
        // now, so the intact ceiling is what lies to `+X` of it rather than what
        // lies beyond it in Z — checking at centre_x covers every band.
        let (_, slot_max_x) = corridor.slot_x_range();
        assert!(
            centre_x > slot_max_x,
            "the readout column at x={centre_x} must be clear of the slot ({slot_max_x})"
        );
        for z in corridor.interior_min[2]..=interior_max[2] {
            assert_eq!(
                material_at(
                    &brickmap,
                    WorldVoxelCoord::new(centre_x, interior_max[1] + 1, z)
                ),
                CEILING_MATERIAL,
                "ceiling at z={z}"
            );
        }

        // Far cap is neutral too.
        assert_eq!(
            material_at(
                &brickmap,
                WorldVoxelCoord::new(centre_x, corridor.interior_min[1] + 1, interior_max[2] + 1)
            ),
            END_CAP_MATERIAL,
        );
    }

    #[test]
    fn interior_volume_is_empty() {
        let (corridor, brickmap) = carved(NotchState::Sealed);
        let interior_max = corridor.interior_max();
        for z in corridor.interior_min[2]..=interior_max[2] {
            for y in corridor.interior_min[1]..=interior_max[1] {
                for x in corridor.interior_min[0]..=interior_max[0] {
                    let coordinate = WorldVoxelCoord::new(x, y, z);
                    assert_eq!(
                        material_at(&brickmap, coordinate),
                        Voxel::Air,
                        "interior voxel {coordinate:?} is not air — a stray reflector would \
                         invalidate every reading taken from this fixture"
                    );
                }
            }
        }
    }

    /// The load-bearing test. Flood-fills air from the middle of the corridor
    /// and checks the set of escapes. Every conclusion drawn from this fixture
    /// rests on the room having exactly the light paths it claims, and this is
    /// the only test that proves it — the per-surface assertions above would
    /// all still pass with a hole in a corner.
    fn escape_columns(corridor: &RainbowCorridor, brickmap: &Brickmap) -> Vec<[i32; 3]> {
        let (outer_min, outer_max) = corridor.outer_bounds();
        // Search one voxel beyond the shell so an escape is observable as
        // reaching the boundary layer.
        let search_min = [outer_min[0] - 1, outer_min[1] - 1, outer_min[2] - 1];
        let search_max = [outer_max[0] + 1, outer_max[1] + 1, outer_max[2] + 1];

        let interior_max = corridor.interior_max();
        let start = WorldVoxelCoord::new(
            corridor.interior_min[0] + INTERIOR_WIDTH / 2,
            corridor.interior_min[1] + INTERIOR_HEIGHT / 2,
            interior_max[2] - 1,
        );
        assert_eq!(material_at(brickmap, start), Voxel::Air, "seed must be air");

        let mut visited = std::collections::HashSet::new();
        let mut stack = vec![start];
        let mut escapes = Vec::new();
        visited.insert([start.x, start.y, start.z]);

        while let Some(current) = stack.pop() {
            let outside = current.x <= search_min[0]
                || current.y <= search_min[1]
                || current.z <= search_min[2]
                || current.x >= search_max[0]
                || current.y >= search_max[1]
                || current.z >= search_max[2];
            if outside {
                escapes.push([current.x, current.y, current.z]);
                continue;
            }
            for (dx, dy, dz) in [
                (1, 0, 0),
                (-1, 0, 0),
                (0, 1, 0),
                (0, -1, 0),
                (0, 0, 1),
                (0, 0, -1),
            ] {
                let next = WorldVoxelCoord::new(current.x + dx, current.y + dy, current.z + dz);
                if !visited.insert([next.x, next.y, next.z]) {
                    continue;
                }
                if material_at(brickmap, next) == Voxel::Air {
                    stack.push(next);
                }
            }
        }
        escapes
    }

    #[test]
    fn sealed_corridor_leaks_only_through_the_ceiling_slot() {
        let (corridor, brickmap) = carved(NotchState::Sealed);
        let escapes = escape_columns(&corridor, &brickmap);
        assert!(
            !escapes.is_empty(),
            "the slot must actually reach open air, or the fixture renders black"
        );

        let interior_max = corridor.interior_max();
        let (slot_min_x, slot_max_x) = corridor.slot_x_range();
        let (slot_min_z, slot_max_z) = corridor.slot_z_range();
        for escape in &escapes {
            assert!(
                escape[1] > interior_max[1],
                "air escaped at {escape:?}, below the ceiling — the shell has a hole"
            );
            assert!(
                (slot_min_x..=slot_max_x).contains(&escape[0]),
                "air escaped at {escape:?}, outside the slot's X range \
                 {slot_min_x}..={slot_max_x}"
            );
            assert!(
                (slot_min_z..=slot_max_z).contains(&escape[2]),
                "air escaped at {escape:?}, outside the slot's Z range \
                 {slot_min_z}..={slot_max_z}"
            );
        }
    }

    #[test]
    fn open_corridor_adds_exactly_the_far_end_notch() {
        let (corridor, brickmap) = carved(NotchState::Open);
        let escapes = escape_columns(&corridor, &brickmap);
        let interior_max = corridor.interior_max();
        let (slot_min_z, slot_max_z) = corridor.slot_z_range();
        let ([notch_min_y, notch_max_y], [notch_min_z, notch_max_z]) = corridor.notch_extent();

        let mut saw_notch_escape = false;
        for escape in &escapes {
            let through_slot = escape[1] > interior_max[1]
                && (slot_min_z..=slot_max_z).contains(&escape[2])
                && (corridor.interior_min[0]..=interior_max[0]).contains(&escape[0]);
            let through_notch = escape[0] > interior_max[0]
                && (notch_min_y..=notch_max_y).contains(&escape[1])
                && (notch_min_z..=notch_max_z).contains(&escape[2]);
            saw_notch_escape |= through_notch;
            assert!(
                through_slot || through_notch,
                "air escaped at {escape:?}, which is neither the ceiling slot nor the notch"
            );
        }
        assert!(saw_notch_escape, "the notch never reached open air");

        // And the notch is the documented size.
        assert_eq!(notch_max_y - notch_min_y + 1, NOTCH_HEIGHT);
        assert_eq!(notch_max_z - notch_min_z + 1, NOTCH_LENGTH);
    }

    #[test]
    fn sealing_the_notch_removes_its_light_path() {
        // The A/B the fixture exists to support: the only difference between
        // the two configurations is the notch, so a lighting change between
        // them is attributable to it and to nothing else.
        let (sealed, sealed_map) = carved(NotchState::Sealed);
        let (open, open_map) = carved(NotchState::Open);
        let interior_max = sealed.interior_max();
        let ([notch_min_y, notch_max_y], [notch_min_z, notch_max_z]) = open.notch_extent();

        for z in notch_min_z..=notch_max_z {
            for y in notch_min_y..=notch_max_y {
                let coordinate = WorldVoxelCoord::new(interior_max[0] + 1, y, z);
                assert_eq!(
                    material_at(&open_map, coordinate),
                    Voxel::Air,
                    "notch not carved at {coordinate:?}"
                );
                assert_ne!(
                    material_at(&sealed_map, coordinate),
                    Voxel::Air,
                    "sealed configuration has the notch open at {coordinate:?}"
                );
            }
        }
    }

    #[test]
    fn viewer_poses_stand_inside_the_interior() {
        let corridor = RainbowCorridor::new(NotchState::Sealed);
        for eye in [corridor.viewer_eye_meters(), corridor.far_eye_meters()] {
            let coordinate = WorldVoxelCoord::new(
                eye[0].floor() as i32,
                eye[1].floor() as i32,
                eye[2].floor() as i32,
            );
            assert!(
                corridor.is_interior(coordinate),
                "eye {eye:?} -> {coordinate:?} is not inside the corridor"
            );
        }
    }
}
