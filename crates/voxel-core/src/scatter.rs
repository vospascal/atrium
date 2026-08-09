//! Pass two: deterministic decoration scattered over finished terrain.
//!
//! Pass one owns shape — terrain, caves, fluids. This pass only reads the result
//! and places cover on it, which is what keeps it cheap and re-runnable: it
//! never samples terrain noise, and it asks one question per candidate voxel.
//!
//! Structure comes from a coarse patch field so decoration grows in clumps
//! instead of uniform static; the per-voxel decision is a single coordinate hash
//! compare. Both are functions of the world coordinate alone, so generation
//! order, worker count, and chunk arrival order cannot change the answer.

use serde::{Deserialize, Serialize};

use crate::cave::CaveSurface;
use crate::noise::{coordinate_roll, value_noise_3d};
use crate::world::Voxel;

/// Separates the coarse patch field from the per-voxel roll. Without it a class
/// whose patch field and placement roll shared a seed would correlate the clump
/// shape with the decoration inside it.
const PATCH_FIELD_SALT: u64 = 0x9A7C_4001_5CA7_0000;

/// What a scatter rule attaches to.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScatterTarget {
    /// The open voxel directly above the lit terrain surface.
    SurfaceTop,
    /// An open cave voxel resting on cave floor.
    CaveFloor,
    /// An open cave voxel hanging from a cave ceiling.
    CaveCeiling,
    /// An open cave voxel against a cave wall.
    CaveWall,
}

impl ScatterTarget {
    /// Whether a classified cave surface satisfies this target.
    pub fn accepts(self, surface: CaveSurface) -> bool {
        match self {
            Self::SurfaceTop => false,
            Self::CaveFloor => surface == CaveSurface::Floor,
            Self::CaveCeiling => surface == CaveSurface::Ceiling,
            Self::CaveWall => surface == CaveSurface::Wall,
        }
    }
}

/// One kind of scattered decoration.
///
/// A class is a look, not a rule: the same class can be placed by several rules
/// at different rates. The voxel each one writes is deliberately a dummy row for
/// now — the arc's point is that the placement is real and authored, so the row
/// can be re-authored later without touching generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScatterClass {
    TallGrass,
    Flowers,
    CaveMoss,
    CaveMushroom,
    CaveVine,
    CaveCrystal,
}

impl ScatterClass {
    /// The voxel this class writes.
    pub fn voxel(self, variant_roll: f32) -> Voxel {
        match self {
            Self::TallGrass => Voxel::TallGrass,
            // Four authored flower rows already exist, so the variant roll picks
            // one rather than the table gaining a fifth "generic flower".
            Self::Flowers => match (variant_roll * 4.0) as u32 {
                0 => Voxel::FlowerPink,
                1 => Voxel::FlowerWhite,
                2 => Voxel::FlowerYellow,
                _ => Voxel::FlowerBlue,
            },
            Self::CaveMoss => Voxel::CaveMoss,
            Self::CaveMushroom => Voxel::CaveMushroom,
            Self::CaveVine => Voxel::CaveVine,
            Self::CaveCrystal => Voxel::CaveCrystal,
        }
    }

    /// A stable per-class salt so two classes never share a roll at one
    /// coordinate — without it, every class would decide identically and
    /// decoration would arrive in perfectly co-located stacks.
    const fn salt(self) -> u64 {
        match self {
            Self::TallGrass => 0x5CA7_0001,
            Self::Flowers => 0x5CA7_0002,
            Self::CaveMoss => 0x5CA7_0003,
            Self::CaveMushroom => 0x5CA7_0004,
            Self::CaveVine => 0x5CA7_0005,
            Self::CaveCrystal => 0x5CA7_0006,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::TallGrass => "tall_grass",
            Self::Flowers => "flowers",
            Self::CaveMoss => "cave_moss",
            Self::CaveMushroom => "cave_mushroom",
            Self::CaveVine => "cave_vine",
            Self::CaveCrystal => "cave_crystal",
        }
    }

    /// Every class, in the order rules are evaluated and tallies are reported.
    pub const ALL: [Self; 6] = [
        Self::TallGrass,
        Self::Flowers,
        Self::CaveMoss,
        Self::CaveMushroom,
        Self::CaveVine,
        Self::CaveCrystal,
    ];
}

/// One authored placement rule.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScatterRule {
    pub class: ScatterClass,
    pub target: ScatterTarget,
    pub enabled: bool,
    /// Probability a candidate voxel inside an active patch is decorated.
    pub chance: f32,
    /// World-space frequency of the patch field. Smaller values make broader
    /// clumps; this is what stops decoration reading as uniform noise.
    pub patch_scale: f32,
    /// Fraction of the patch field that is active. 1.0 disables clumping.
    pub patch_coverage: f32,
    pub min_y: i32,
    pub max_y: i32,
}

impl ScatterRule {
    /// Whether this rule can place at all. A rule outside its own height band or
    /// with a zero chance is inert rather than an error: authoring a class off is
    /// a legitimate state.
    pub fn is_active(&self) -> bool {
        self.enabled && self.chance > 0.0 && self.min_y <= self.max_y
    }

    fn places(&self, world_seed: u64, x: i32, y: i32, z: i32) -> bool {
        if !self.is_active() || y < self.min_y || y > self.max_y {
            return false;
        }
        // Cheap tests first: the height band rejects most of the world, the
        // patch field rejects most of the band, and only survivors pay the
        // per-voxel roll. Deliberately the order the worldgen plan's prop flow
        // prescribes.
        if self.patch_coverage < 1.0 {
            let patch = value_noise_3d(
                x as f32 * self.patch_scale,
                0.0,
                z as f32 * self.patch_scale,
                world_seed ^ self.class.salt() ^ PATCH_FIELD_SALT,
            );
            if patch > self.patch_coverage {
                return false;
            }
        }
        coordinate_roll(x, y, z, world_seed, self.class.salt()) < self.chance
    }
}

/// The authored decoration pass.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScatterProgram {
    pub enabled: bool,
    pub rules: Vec<ScatterRule>,
}

impl Default for ScatterProgram {
    fn default() -> Self {
        Self {
            enabled: true,
            rules: vec![
                ScatterRule {
                    class: ScatterClass::TallGrass,
                    target: ScatterTarget::SurfaceTop,
                    enabled: true,
                    chance: 0.28,
                    patch_scale: 0.05,
                    patch_coverage: 0.65,
                    min_y: 0,
                    max_y: 4096,
                },
                ScatterRule {
                    class: ScatterClass::Flowers,
                    target: ScatterTarget::SurfaceTop,
                    enabled: true,
                    chance: 0.05,
                    patch_scale: 0.08,
                    patch_coverage: 0.35,
                    min_y: 0,
                    max_y: 4096,
                },
                ScatterRule {
                    class: ScatterClass::CaveMoss,
                    target: ScatterTarget::CaveFloor,
                    enabled: true,
                    chance: 0.22,
                    patch_scale: 0.06,
                    patch_coverage: 0.55,
                    min_y: 0,
                    max_y: 4096,
                },
                ScatterRule {
                    class: ScatterClass::CaveMushroom,
                    target: ScatterTarget::CaveFloor,
                    enabled: true,
                    chance: 0.08,
                    patch_scale: 0.10,
                    patch_coverage: 0.30,
                    min_y: 0,
                    max_y: 4096,
                },
                ScatterRule {
                    class: ScatterClass::CaveVine,
                    target: ScatterTarget::CaveCeiling,
                    enabled: true,
                    chance: 0.14,
                    patch_scale: 0.07,
                    patch_coverage: 0.45,
                    min_y: 0,
                    max_y: 4096,
                },
                // The readable cave light. Rare on purpose: a crystal every few
                // metres would flatten the darkness that makes a cave a cave.
                ScatterRule {
                    class: ScatterClass::CaveCrystal,
                    target: ScatterTarget::CaveWall,
                    enabled: true,
                    chance: 0.035,
                    patch_scale: 0.09,
                    patch_coverage: 0.25,
                    min_y: 0,
                    max_y: 4096,
                },
            ],
        }
    }
}

impl ScatterProgram {
    /// The class to place at one candidate voxel, if any.
    ///
    /// Rules are evaluated in authored order and the first match wins, so two
    /// rules competing for the same voxel resolve deterministically rather than
    /// by whichever pass ran last.
    pub fn placement(
        &self,
        world_seed: u64,
        x: i32,
        y: i32,
        z: i32,
        target: ScatterTarget,
    ) -> Option<ScatterClass> {
        if !self.enabled {
            return None;
        }
        self.rules
            .iter()
            .find(|rule| rule.target == target && rule.places(world_seed, x, y, z))
            .map(|rule| rule.class)
    }

    /// Whether any rule targets cave surfaces, so the generator can skip the
    /// cave-surface classification entirely when nothing would read it.
    pub fn wants_cave_surfaces(&self) -> bool {
        self.enabled
            && self
                .rules
                .iter()
                .any(|rule| rule.is_active() && !matches!(rule.target, ScatterTarget::SurfaceTop))
    }
}

/// Per-class placement counts from one generation run, for the debug window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScatterTally {
    counts: [u32; ScatterClass::ALL.len()],
}

impl ScatterTally {
    pub fn record(&mut self, class: ScatterClass) {
        let index = ScatterClass::ALL
            .iter()
            .position(|candidate| *candidate == class)
            .unwrap_or(0);
        self.counts[index] = self.counts[index].saturating_add(1);
    }

    pub fn count(&self, class: ScatterClass) -> u32 {
        ScatterClass::ALL
            .iter()
            .position(|candidate| *candidate == class)
            .map(|index| self.counts[index])
            .unwrap_or(0)
    }

    pub fn total(&self) -> u32 {
        self.counts.iter().copied().sum()
    }

    pub fn merge(&mut self, other: &Self) {
        for (own, added) in self.counts.iter_mut().zip(other.counts.iter()) {
            *own = own.saturating_add(*added);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_replays_for_the_same_coordinate_and_seed() {
        let program = ScatterProgram::default();
        let first: Vec<_> = (0..64)
            .map(|x| program.placement(7, x, 33, 11, ScatterTarget::SurfaceTop))
            .collect();
        let second: Vec<_> = (0..64)
            .map(|x| program.placement(7, x, 33, 11, ScatterTarget::SurfaceTop))
            .collect();
        assert_eq!(first, second);
    }

    #[test]
    fn upper_seed_bits_change_where_decoration_lands() {
        let program = ScatterProgram::default();
        let low: Vec<_> = (0..256)
            .map(|x| program.placement(7, x, 33, 11, ScatterTarget::SurfaceTop))
            .collect();
        let high: Vec<_> = (0..256)
            .map(|x| program.placement(7 | (1 << 48), x, 33, 11, ScatterTarget::SurfaceTop))
            .collect();
        assert_ne!(low, high);
    }

    #[test]
    fn a_rule_only_places_on_its_own_target() {
        let program = ScatterProgram::default();
        for x in 0..256 {
            for z in 0..4 {
                if let Some(class) = program.placement(3, x, 20, z, ScatterTarget::CaveCeiling) {
                    assert_eq!(class, ScatterClass::CaveVine);
                }
                if let Some(class) = program.placement(3, x, 20, z, ScatterTarget::SurfaceTop) {
                    assert!(matches!(
                        class,
                        ScatterClass::TallGrass | ScatterClass::Flowers
                    ));
                }
            }
        }
    }

    /// A disabled program must place nothing at all, so the debug toggle and the
    /// "caves without decoration" authoring state are the same code path.
    #[test]
    fn a_disabled_program_places_nothing() {
        let program = ScatterProgram {
            enabled: false,
            ..ScatterProgram::default()
        };
        for x in 0..512 {
            assert_eq!(
                program.placement(1, x, 33, 0, ScatterTarget::SurfaceTop),
                None
            );
        }
        assert!(!program.wants_cave_surfaces());
    }

    #[test]
    fn tallies_accumulate_per_class() {
        let mut tally = ScatterTally::default();
        tally.record(ScatterClass::CaveMoss);
        tally.record(ScatterClass::CaveMoss);
        tally.record(ScatterClass::TallGrass);
        assert_eq!(tally.count(ScatterClass::CaveMoss), 2);
        assert_eq!(tally.count(ScatterClass::TallGrass), 1);
        assert_eq!(tally.count(ScatterClass::CaveCrystal), 0);
        assert_eq!(tally.total(), 3);
        let mut other = ScatterTally::default();
        other.record(ScatterClass::CaveCrystal);
        tally.merge(&other);
        assert_eq!(tally.count(ScatterClass::CaveCrystal), 1);
        assert_eq!(tally.total(), 4);
    }
}
