//! What each generation stage did, measured while it did it.
//!
//! The final world tells you what exists; it does not tell you how it came to
//! exist. That gap is where this arc's bugs lived: caves that formed only near the
//! surface, terrain that could not hold water, ramps that opened to the sky. Every
//! one of them was a single stage misbehaving, and every one was invisible in the
//! finished result — the debugging that found them was a throwaway probe printing
//! per-depth histograms.
//!
//! So generation records, per stage: how many voxels it decided, how many it
//! actually changed, and the material composition it left behind. That makes the
//! pipeline readable as a sequence of layers — carve took 13% of the rock, fluids
//! filled 34% of the carved space, scatter dressed 5% of the exposed surfaces —
//! rather than as one opaque number of bricks.

use crate::world::Voxel;

/// One step of the pipeline, in execution order.
///
/// A stage is a decision the generator makes about a voxel, not a pass over
/// memory: `Terrain` and `Carve` both run inside pass one's single loop, but they
/// are separate stages because they answer separate questions and can be wrong
/// separately.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub enum GenerationStage {
    /// Column height, soil layering, and the open-air ocean.
    #[default]
    Terrain,
    /// The cave mask turning solid rock into void.
    Carve,
    /// Ramps overriding the carve so bands stay walkable.
    Ramp,
    /// Underground water and lava filling carved voids.
    Fluids,
    /// Pass two: decoration placed on finished surfaces.
    Scatter,
}

impl GenerationStage {
    pub const ALL: [Self; 5] = [
        Self::Terrain,
        Self::Carve,
        Self::Ramp,
        Self::Fluids,
        Self::Scatter,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Terrain => "terrain",
            Self::Carve => "carve",
            Self::Ramp => "ramp",
            Self::Fluids => "fluids",
            Self::Scatter => "scatter",
        }
    }

    /// Which pass this stage belongs to. Pass one owns shape, pass two decoration.
    pub const fn pass(self) -> u8 {
        match self {
            Self::Terrain | Self::Carve | Self::Ramp | Self::Fluids => 1,
            Self::Scatter => 2,
        }
    }

    /// A stable display colour, so the same stage is the same colour in the layer
    /// view, the stacked bar, and the legend. sRGB bytes.
    pub const fn color(self) -> [u8; 3] {
        match self {
            // Warm earth for the ground, cool blue for what removes it, amber for
            // the walkable overrides, deep teal for fluid, green for growth.
            Self::Terrain => [196, 158, 92],
            Self::Carve => [92, 148, 210],
            Self::Ramp => [232, 172, 74],
            Self::Fluids => [72, 190, 190],
            Self::Scatter => [126, 196, 108],
        }
    }
}

/// How many voxels one stage considered and how many it changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StageCount {
    /// Voxels this stage was asked about.
    pub considered: u32,
    /// Voxels it actually changed. The ratio to `considered` is the stage's real
    /// strength, which is the number an author is tuning even when the slider is
    /// labelled something else.
    pub changed: u32,
}

impl StageCount {
    pub fn consider(&mut self) {
        self.considered = self.considered.saturating_add(1);
    }

    pub fn change(&mut self) {
        self.changed = self.changed.saturating_add(1);
    }

    /// Fraction of considered voxels this stage changed, in `[0, 1]`.
    pub fn hit_rate(&self) -> f32 {
        if self.considered == 0 {
            0.0
        } else {
            self.changed as f32 / self.considered as f32
        }
    }

    pub fn merge(&mut self, other: &Self) {
        self.considered = self.considered.saturating_add(other.considered);
        self.changed = self.changed.saturating_add(other.changed);
    }
}

/// Voxel counts per material, for the "what percentage is applied" readout.
///
/// Keyed by [`Voxel`], not by the renderer's material byte: that mapping lives in
/// `voxel-material`, which depends on THIS crate, so reaching for it here would
/// invert the dependency — and hand-copying the mapping would be a second source
/// of truth for material identity. Callers that need a display name ask
/// `voxel-material` when they render, which is the layer that already knows.
///
/// A plain array indexed by the enum discriminant rather than a map: generation
/// records once per voxel, and a hash lookup in that loop is exactly the kind of
/// cost the review flagged elsewhere in this pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialHistogram {
    counts: [u32; Self::SLOTS],
}

impl Default for MaterialHistogram {
    fn default() -> Self {
        Self {
            counts: [0; Self::SLOTS],
        }
    }
}

impl MaterialHistogram {
    /// One slot per possible discriminant. Deliberately the full byte range rather
    /// than the current variant count: the histogram must not need editing every
    /// time [`Voxel`] gains a row, and 256 words is nothing.
    pub const SLOTS: usize = 256;

    pub fn record(&mut self, voxel: Voxel) {
        let slot = &mut self.counts[voxel as usize];
        *slot = slot.saturating_add(1);
    }

    pub fn count(&self, voxel: Voxel) -> u32 {
        self.counts[voxel as usize]
    }

    /// Every voxel kind present, most common first.
    ///
    /// Sorted because the interesting question is always "what is this world made
    /// of", and an unsorted sweep buries the answer.
    pub fn ranked(&self) -> Vec<(Voxel, u32)> {
        let mut present: Vec<(Voxel, u32)> = Voxel::ALL
            .iter()
            .map(|voxel| (*voxel, self.count(*voxel)))
            .filter(|(_, count)| *count > 0)
            .collect();
        present.sort_by(|left, right| {
            right
                .1
                .cmp(&left.1)
                .then((left.0 as usize).cmp(&(right.0 as usize)))
        });
        present
    }

    /// Total voxels counted, air included.
    pub fn total(&self) -> u32 {
        self.counts.iter().copied().sum()
    }

    /// Share of the world this material occupies, in `[0, 1]`.
    pub fn share(&self, voxel: Voxel) -> f32 {
        let total = self.total();
        if total == 0 {
            0.0
        } else {
            self.count(voxel) as f32 / total as f32
        }
    }

    pub fn merge(&mut self, other: &Self) {
        for (own, added) in self.counts.iter_mut().zip(other.counts.iter()) {
            *own = own.saturating_add(*added);
        }
    }
}

/// Per-stage counts plus the material composition of the finished result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StageReport {
    stages: [StageCount; GenerationStage::ALL.len()],
    /// Material composition of the finished world, one entry per voxel.
    pub materials: MaterialHistogram,
    /// Per-Y-layer counts of what each stage left, so the layer view can show the
    /// pipeline's vertical structure instead of a flat total.
    layers: Vec<LayerSlice>,
}

/// One horizontal layer's stage composition.
///
/// The vertical axis is what makes a cave system legible — where the rooms are,
/// where the water table sits, which depths the scatter reaches. A single total
/// per stage cannot show any of that.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayerSlice {
    pub world_y: i32,
    pub solid: u32,
    pub carved: u32,
    pub water: u32,
    pub lava: u32,
    pub scatter: u32,
}

impl LayerSlice {
    /// Total voxels this layer accounts for.
    pub fn total(&self) -> u32 {
        self.solid + self.carved + self.water + self.lava + self.scatter
    }
}

impl StageReport {
    pub fn new(min_y: i32, height: usize) -> Self {
        Self {
            stages: Default::default(),
            materials: MaterialHistogram::default(),
            layers: (0..height)
                .map(|offset| LayerSlice {
                    world_y: min_y + offset as i32,
                    ..LayerSlice::default()
                })
                .collect(),
        }
    }

    pub fn stage(&self, stage: GenerationStage) -> &StageCount {
        &self.stages[stage as usize]
    }

    pub fn stage_mut(&mut self, stage: GenerationStage) -> &mut StageCount {
        &mut self.stages[stage as usize]
    }

    /// The layers, lowest first. Empty for a report built without a height.
    pub fn layers(&self) -> &[LayerSlice] {
        &self.layers
    }

    pub fn layer_mut(&mut self, world_y: i32) -> Option<&mut LayerSlice> {
        let first = self.layers.first()?.world_y;
        let index = usize::try_from(world_y - first).ok()?;
        self.layers.get_mut(index)
    }

    /// Attribute one coordinate to the stage that decided it, in the layer view.
    ///
    /// Deliberately does NOT touch the material histogram. Pass two replaces
    /// voxels pass one already decided, so recording material here would count
    /// those coordinates twice — the composition is taken once from the finished
    /// chunk by [`Self::record_material`], which is also what "% of the world"
    /// actually means.
    pub fn record_voxel(&mut self, world_y: i32, voxel: Voxel, stage: GenerationStage) {
        let Some(layer) = self.layer_mut(world_y) else {
            return;
        };
        match stage {
            GenerationStage::Scatter => layer.scatter += 1,
            GenerationStage::Fluids => match voxel {
                Voxel::Lava => layer.lava += 1,
                _ => layer.water += 1,
            },
            GenerationStage::Carve | GenerationStage::Ramp => layer.carved += 1,
            GenerationStage::Terrain => {
                if voxel == Voxel::Water {
                    layer.water += 1;
                } else if voxel != Voxel::Air {
                    layer.solid += 1;
                }
            }
        }
    }

    /// Count one voxel of the FINISHED world toward the composition.
    pub fn record_material(&mut self, voxel: Voxel) {
        self.materials.record(voxel);
    }

    pub fn merge(&mut self, other: &Self) {
        for (own, added) in self.stages.iter_mut().zip(other.stages.iter()) {
            own.merge(added);
        }
        self.materials.merge(&other.materials);
        if self.layers.is_empty() {
            self.layers = other.layers.clone();
            return;
        }
        for added in &other.layers {
            if let Some(layer) = self.layer_mut(added.world_y) {
                layer.solid += added.solid;
                layer.carved += added.carved;
                layer.water += added.water;
                layer.lava += added.lava;
                layer.scatter += added.scatter;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stages_hit_rate_is_changed_over_considered() {
        let mut count = StageCount::default();
        for index in 0..10 {
            count.consider();
            if index % 4 == 0 {
                count.change();
            }
        }
        assert_eq!(count.considered, 10);
        assert_eq!(count.changed, 3);
        assert!((count.hit_rate() - 0.3).abs() < 1e-6);
        // An untouched stage reports zero rather than dividing by zero.
        assert_eq!(StageCount::default().hit_rate(), 0.0);
    }

    #[test]
    fn the_histogram_ranks_materials_and_reports_shares() {
        let mut histogram = MaterialHistogram::default();
        for _ in 0..70 {
            histogram.record(Voxel::Stone);
        }
        for _ in 0..30 {
            histogram.record(Voxel::Grass);
        }
        assert_eq!(histogram.total(), 100);
        assert_eq!(
            histogram.ranked(),
            vec![(Voxel::Stone, 70), (Voxel::Grass, 30)]
        );
        assert!((histogram.share(Voxel::Stone) - 0.70).abs() < 1e-6);
        assert_eq!(histogram.share(Voxel::CaveCrystal), 0.0);
    }

    #[test]
    fn layers_are_addressed_by_world_y_and_ignore_out_of_range() {
        let mut report = StageReport::new(-4, 8);
        assert_eq!(report.layers().len(), 8);
        assert_eq!(report.layers()[0].world_y, -4);
        report.record_voxel(-4, Voxel::Stone, GenerationStage::Terrain);
        report.record_voxel(3, Voxel::Water, GenerationStage::Fluids);
        // A coordinate no layer owns is dropped from the layer view rather than
        // panicking or landing in the wrong row.
        report.record_voxel(999, Voxel::Stone, GenerationStage::Terrain);
        assert_eq!(report.layers()[0].solid, 1);
        assert_eq!(report.layers()[7].water, 1);
        assert_eq!(report.layers().iter().map(|l| l.total()).sum::<u32>(), 2);
    }

    #[test]
    fn merging_two_reports_adds_stages_layers_and_materials() {
        let mut left = StageReport::new(0, 4);
        left.stage_mut(GenerationStage::Carve).consider();
        left.stage_mut(GenerationStage::Carve).change();
        left.record_voxel(1, Voxel::Stone, GenerationStage::Terrain);
        left.record_material(Voxel::Stone);
        let mut right = StageReport::new(0, 4);
        right.stage_mut(GenerationStage::Carve).consider();
        right.record_voxel(1, Voxel::Stone, GenerationStage::Terrain);
        right.record_material(Voxel::Stone);
        left.merge(&right);
        assert_eq!(left.stage(GenerationStage::Carve).considered, 2);
        assert_eq!(left.stage(GenerationStage::Carve).changed, 1);
        assert_eq!(left.layers()[1].solid, 2);
        assert_eq!(left.materials.count(Voxel::Stone), 2);
    }

    /// Every stage needs a distinct colour, or the layer view's legend is a lie.
    #[test]
    fn stages_have_distinct_colors_and_labels() {
        let mut seen = Vec::new();
        for stage in GenerationStage::ALL {
            assert!(
                !seen.contains(&stage.color()),
                "{} reuses a colour",
                stage.label()
            );
            seen.push(stage.color());
        }
        assert_eq!(GenerationStage::Scatter.pass(), 2);
        assert_eq!(GenerationStage::Terrain.pass(), 1);
    }
}
