//! S2 — the layer model: **one mechanism for within-face detail, per-voxel tone,
//! multi-voxel patterns and cross-voxel continuity.**
//!
//! S1 gave a voxel three faces. This gives each face structure, and it does so
//! without adding a single byte of per-voxel state: everything here is a small
//! stack of parameters on the *material row*, evaluated as ALU on a hit the
//! renderer has already found.
//!
//! ## Why one mechanism and not four
//!
//! The four scales a material acts on look like four features and are not:
//!
//! | Scale | Example | What it is here |
//! |---|---|---|
//! | within a face | grain, speckle, wear | a layer with a period well under a voxel |
//! | one voxel | per-voxel tone jitter | a [`PatternFrame::Voxel`] layer |
//! | across voxels | ore veins, weathering bands, large mottle | a layer with a period over a voxel |
//! | continuity | no visible per-voxel tiling | [`PatternFrame::World`], which is the default |
//!
//! ## And one thing that is NOT a scale: the texel grid
//!
//! [`PatternLayer::texels_per_voxel`] is orthogonal to all four. It quantises the
//! sample position to an `n x n` grid per face, so every generator becomes blocky at
//! once — square detail on a world made of cubes. It was the gap the S2 gate found:
//! the model had frames and periods, all of them continuous, so `Noise` gave smooth
//! mottle and `Speckle` gave round dots, and neither is what a voxel surface wants.
//!
//! It is deliberately independent of the period, which keeps its own job of setting
//! FEATURE size: 8 texels with a 1 m period is a large soft field rendered in 1.5 cm
//! squares, 8 texels with a 0.125 m period is per-face detail. And the grid is
//! **shared with the rest of the engine** rather than private to this module — see
//! [`TEXEL_RUNGS`], which is where the `.vox` and sub-voxel-model connection is
//! written down.
//!
//! A layer carries a **sampling frame** and a **period in metres**, and that pair
//! is the whole difference between them. `0.02 m` is grain inside one face,
//! `0.125 m` is exactly one voxel, `1.0 m` is a band spanning eight of them. There
//! is no separate "multi-voxel template" subsystem to build, and continuity is a
//! property of the default frame rather than a fix bolted on afterwards: a
//! world-framed field simply does not know where the voxel boundaries are, so it
//! cannot tile against them.
//!
//! ## Why the CPU carries a full evaluator
//!
//! [`PatternLayer::evaluate`] is a complete reference implementation of what
//! `shaders/pattern.wgsl` does, down to the integer hash. Nothing in the renderer
//! calls it — the shading path is the shader. It exists because the WGSL is
//! hand-mirrored and cannot be unit-tested in this crate, so the tests below pin
//! *this* against hand-computed values and the shader against this, one function
//! at a time. A drift between the two is then a readable diff rather than a
//! wrong-looking wall.
//!
//! ## What is deliberately not a target
//!
//! `opacity`. It is a **traversal** input: the DDA decides whether to continue
//! through a voxel before any shading runs, so patterning it would mean evaluating
//! the layer stack inside the innermost traversal loop — precisely the cost this
//! stage is built to avoid (every layer here is paid once per *hit*, not once per
//! *step*). A dissolve/erosion effect wants it and is a named follow-on with its
//! own cost argument. Albedo, roughness and emission are all read after the hit,
//! and are free.

#[cfg(test)]
use voxel_core::world::DETAIL_CELL_SIZE_METERS;
use voxel_core::world::{DETAIL_CELLS_PER_WORLD_VOXEL, WORLD_VOXEL_SIZE_METERS};

// Traversal still reports the 0.125 m detail cell hit. Pattern frames below
// deliberately promote that coordinate to its containing one-metre world voxel.
#[cfg(test)]
const VOXEL_SIZE: f32 = DETAIL_CELL_SIZE_METERS;

/// Layer slots per material row.
///
/// Four, because the authored cases that motivated this stage need at most three — a
/// per-voxel tone, a grain and a speckle — and the fourth is headroom rather than a
/// budget anyone has spent. The shader loops to the row's own active count, so an
/// unused slot costs nothing but the bytes.
///
/// Four is also where the measured slope stops arguing for fewer: the entry cost per
/// hit dominates a single layer, so layers 2-4 are about a third the price of layer 1.
pub const MAX_PATTERN_LAYERS: usize = 4;

/// Default camera distance at which material patterns begin fading. The runtime
/// quality registry exposes start and end directly in metres. An end of zero
/// disables fading.
pub const PATTERN_FADE_START_METERS: f32 = 10.0;
pub const PATTERN_FADE_END_METERS: f32 = 50.0;

/// Every generator bit set — the shipped default, mirroring
/// `MATERIAL_PATTERN_GENERATOR_MASK` in `voxel-rt`'s `shaders/pattern.wgsl`. Fourteen codes.
///
/// Lives here rather than with the renderer's levers because it is DERIVED from the
/// generators below — `every_generator_owns_a_distinct_mask_bit` asserts it is exactly their
/// union, in both directions. The lever that dials it reads this value; it does not define it.
pub const PATTERN_GENERATOR_MASK_ALL: u32 = (1 << 14) - 1;

/// The generator: what shape a layer draws, before any frame or blend.
///
/// Every generator returns a scalar in `0.0..=1.0`, which is what lets the target
/// and blend be independent of it — a mask, a tone and a noise field compose the
/// same way.
///
/// **Built in the order they were asked for** (Pascal, 2026-07-31, on generator
/// priority: *"All three, in that order"*): grain and speckle, then per-voxel tone.
///
/// **Coursing was built and then removed** at Pascal's judgement on the S2 gate
/// (*"only mortor brick like thing we dont need thats meh"*). It was two generators —
/// a mortar mask and a per-brick tone over the same tessellation — and it worked; it
/// was simply not a look this world wants. Recoverable from git if a built structure
/// ever needs brickwork. Note this is NOT a variant-hygiene case: that rule keeps
/// measured *performance* losers as levers because Quest may flip the verdict, and no
/// hardware is going to flip a taste verdict.
///
/// `Streak`, `Cells` (Worley) and `Gradient` from the plan's table were never built,
/// for the reason coursing has now demonstrated: an enum variant that no material
/// uses and no test exercises is a liability rather than a head start. They land when
/// a material wants them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PatternGenerator {
    /// One value per cell of the frame, constant across the cell.
    ///
    /// In [`PatternFrame::Voxel`] this is **per-voxel tone** — the jitter that
    /// stops a stone wall being one flat colour, and the thing the sandbox's mesher
    /// has always had and this table has never had.
    Flat,
    /// Value noise, `octaves` of it. **Grain** at a small period, mottle at a
    /// large one.
    Noise {
        /// 1..=4. Each octave doubles the frequency and halves the amplitude, so
        /// the period always names the *largest* feature.
        octaves: u32,
    },
    /// Scattered round specks: pits in stone, grit in sand, lichen.
    Speckle {
        /// Fraction of cells that carry a speck, `0.0..=1.0`. Not the fraction of
        /// *area* covered — a speck fills a fixed share of its cell, so this knob
        /// controls how crowded the specks are and the period controls how big.
        density: f32,
    },
    /// Classic Perlin gradient noise on the cubic lattice. Eight corners like
    /// [`Self::Noise`], but each contributes a gradient dot rather than a scalar —
    /// more ALU per corner, no branches, and no axis bias in the result.
    Perlin { octaves: u32 },
    /// Gradient noise on the simplex (tetrahedral) lattice: **four** corners
    /// instead of eight, at the cost of a divergent tetrahedron choice. The
    /// half-the-hashes contender — whether it actually wins is a bench question.
    Simplex { octaves: u32 },
    /// Ridged multifractal: `(1 - |2v - 1|)^2` per octave over the value lattice.
    /// Creases at the midline. Veins, erosion channels, rock strata.
    Ridged { octaves: u32 },
    /// Turbulence: `|2v - 1|` per octave. Creases at the ZERO crossing rather than
    /// the midline, which is a different look for the same cost. Smoke, marble.
    Turbulence { octaves: u32 },
    /// Cellular F1 — distance to the nearest jittered feature point. Pebbles,
    /// cells, lichen colonies. The one family here with hard boundaries rather
    /// than gradients, and the dearest: 27 cells walked per sample.
    Worley,
    /// Cellular F2 − F1 — bright exactly on the boundary between two cells.
    /// Cracked mud, dried paint, mortar between irregular stones.
    WorleyEdge,
    /// Cellular F1 through an exponential smooth-minimum, so cell walls swell and
    /// merge instead of meeting at a crease.
    WorleySmooth,
    /// Bands along the sample frame's X, bent by a noise distortion. Wood grain,
    /// geological strata, brushed surfaces.
    Wave {
        /// How far the noise pushes the band coordinate, in periods. `0` rules
        /// perfectly straight bands.
        distortion: f32,
    },
    /// Alternating cells of the sampling lattice. Tiles and boards — and the
    /// bench's cost floor, since it covers the whole frame for two floors and a
    /// bit-and.
    Checker,
    /// Per-tile tone from the tessellation — one flat value per tile, different
    /// from its neighbours. The single most recognisable feature of any masonry
    /// surface, and the thing no other generator produces.
    TileTone,
    /// Distance to the nearest tile edge. Zero inside the joint, rising to one at
    /// the tile's centre — grout, and the bevel that makes a block read as raised.
    TileEdge {
        /// How abruptly the joint gives way to the tile face, `0.0..=1.0`.
        ///
        /// At zero the raw distance ramps all the way to the tile's centre, which
        /// reads as a pillow rather than as masonry — the whole face is a gradient
        /// and none of it is flat. Toward one the transition concentrates at the
        /// joint, leaving a narrow dark line around a flat tile, which is what the
        /// grout in a real wall looks like.
        sharpness: f32,
    },
}

/// How much a generator costs per layer, as a band the authoring UI can show.
///
/// Four bands rather than a number, because the decision this informs is "can I
/// afford this on a surface the player stands on", and that is answered by a band.
/// The measurement behind it is on
/// [`PatternGenerator::measured_reference_milliseconds`], with its conditions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GeneratorCost {
    /// At the measurement floor — it costs what having a layer at all costs.
    Free,
    /// Well under a lattice noise. Affordable anywhere.
    Cheap,
    /// A lattice noise. The workhorse band; fine on surfaces you look at.
    Moderate,
    /// Above a lattice noise. Worth it for what only these produce, but not the
    /// band to reach for on every row.
    Expensive,
}

impl GeneratorCost {
    /// The word the panel shows.
    pub const fn label(&self) -> &'static str {
        match self {
            GeneratorCost::Free => "free",
            GeneratorCost::Cheap => "cheap",
            GeneratorCost::Moderate => "moderate",
            GeneratorCost::Expensive => "expensive",
        }
    }

    /// A four-pip bar, so the band reads at a glance next to the word.
    pub const fn pips(&self) -> &'static str {
        match self {
            GeneratorCost::Free => "▮▯▯▯",
            GeneratorCost::Cheap => "▮▮▯▯",
            GeneratorCost::Moderate => "▮▮▮▯",
            GeneratorCost::Expensive => "▮▮▮▮",
        }
    }

    /// Red-green is the obvious choice for a cost scale and the wrong one — it is
    /// the most common colour-blindness axis, and this bar is the only place the
    /// panel encodes a magnitude in colour alone. The ramp runs from the muted grey
    /// the panel already uses through amber to a warm red, so hue AND lightness both
    /// carry the signal and the pips carry it a third time.
    /// Returned as plain sRGB bytes, not a UI colour type. This crate describes what a
    /// surface is; it does not know what is drawing the panel. The caller wraps it.
    pub const fn rgb(&self) -> [u8; 3] {
        match self {
            GeneratorCost::Free => [0x8b, 0x92, 0x9c],
            GeneratorCost::Cheap => [0xb4, 0xa8, 0x62],
            GeneratorCost::Moderate => [0xd0, 0x93, 0x3f],
            GeneratorCost::Expensive => [0xd2, 0x66, 0x44],
        }
    }
}

impl PatternGenerator {
    /// This generator's bit in a [`generator_mask`], which is simply `1 << code`.
    ///
    /// The mask exists because EVERY generator's body is resident in one function
    /// inlined into the shading pass, and that pass is latency-bound: their
    /// registers cost occupancy whether or not any material reaches them. Measured
    /// 2026-08-03, pruning the nine generators bench section 9's table never uses
    /// took 3.5-6.2% off the pattern path with the frames bit-identical.
    pub const fn mask_bit(&self) -> u32 {
        1u32 << self.code()
    }

    /// The discriminant the GPU row carries. Mirrors the
    /// `PATTERN_GENERATOR_*` consts in `shaders/pattern.wgsl`; a four-bit field,
    /// so 15 is the last usable code.
    pub const fn code(&self) -> u32 {
        match self {
            PatternGenerator::Flat => 0,
            PatternGenerator::Noise { .. } => 1,
            PatternGenerator::Speckle { .. } => 2,
            PatternGenerator::Perlin { .. } => 3,
            PatternGenerator::Simplex { .. } => 4,
            PatternGenerator::Ridged { .. } => 5,
            PatternGenerator::Turbulence { .. } => 6,
            PatternGenerator::Worley => 7,
            PatternGenerator::WorleyEdge => 8,
            PatternGenerator::WorleySmooth => 9,
            PatternGenerator::Wave { .. } => 10,
            PatternGenerator::Checker => 11,
            PatternGenerator::TileTone => 12,
            PatternGenerator::TileEdge { .. } => 13,
        }
    }

    /// The octave count this generator sums, or `1` for the ones that have no
    /// octave structure. One place rather than a `match` at every packing site,
    /// which is what let the fractal family grow from one variant to four without
    /// [`PatternLayer::packed`] changing shape.
    pub const fn octaves(&self) -> u32 {
        match self {
            PatternGenerator::Noise { octaves }
            | PatternGenerator::Perlin { octaves }
            | PatternGenerator::Simplex { octaves }
            | PatternGenerator::Ridged { octaves }
            | PatternGenerator::Turbulence { octaves } => *octaves,
            _ => 1,
        }
    }

    /// This generator's per-layer cost, as a BAND rather than a number.
    ///
    /// The panel shows this and never the milliseconds, deliberately. An absolute
    /// figure in a tooltip reads as a property of the generator when it is really a
    /// property of one machine at one resolution — see
    /// [`Self::measured_reference_milliseconds`] for what the number is actually
    /// conditional on. The BAND survives all of that: simplex is cheaper than value
    /// noise on any hardware that runs this shader, because it reads half the
    /// lattice corners.
    pub const fn cost(&self) -> GeneratorCost {
        // Derived, not hand-assigned, so the band and the measurement cannot drift
        // apart when a generator is re-measured.
        let milliseconds = self.measured_reference_milliseconds();
        if milliseconds < 0.05 {
            GeneratorCost::Free
        } else if milliseconds < 0.5 {
            GeneratorCost::Cheap
        } else if milliseconds < 0.8 {
            GeneratorCost::Moderate
        } else {
            GeneratorCost::Expensive
        }
    }

    /// The reference measurement [`Self::cost`] bands, in milliseconds.
    ///
    /// **Read the conditions before quoting this anywhere.** Bench section 11,
    /// 2026-08-02: ONE layer, world frame, 0.5 m period, 8 texels per voxel,
    /// scenario C (ground level, default sun), 2560x1440, Apple M3 Max — measured
    /// against the `checker` column, which is the cheapest generator the model has
    /// and therefore stands in for everything the layer mechanism costs *around* a
    /// generator. It is a per-layer marginal cost on a saturated table, not a frame
    /// budget, and it does not transfer to another GPU or another resolution.
    ///
    /// **MEDIAN OF THREE RUNS**, which the first version of this table was not, and
    /// the difference mattered. A single run spreads +-0.07 ms on the dear rows and
    /// +-0.02 on the cheap ones — noise that changes no band except at a threshold,
    /// where it changes the answer. `TileEdge` read 0.041 on the first run and
    /// 0.065 on the next, i.e. Free and then Cheap, purely from run-to-run drift.
    /// Take three samples before touching a number here.
    ///
    /// `flat` measures within noise of zero (-0.009 / -0.004 / 0.000 across the
    /// three) and is recorded as zero: it is at the floor, and a negative marginal
    /// cost is noise rather than a generator that makes the frame faster.
    ///
    /// This doubles as the bake-payoff table. A stage evaluated at voxel rate and
    /// cached returns exactly its own number here, which is why there is nothing to
    /// win below `Wave` and about a millisecond on the Worley three.
    pub const fn measured_reference_milliseconds(&self) -> f32 {
        match self {
            PatternGenerator::Flat => 0.000,
            PatternGenerator::Checker => 0.000,
            PatternGenerator::Speckle { .. } => 0.041,
            PatternGenerator::Wave { .. } => 0.210,
            PatternGenerator::Simplex { .. } => 0.383,
            PatternGenerator::Turbulence { .. } => 0.656,
            PatternGenerator::Ridged { .. } => 0.656,
            PatternGenerator::Noise { .. } => 0.661,
            PatternGenerator::Perlin { .. } => 1.012,
            // The headline of the tessellation arc: the entire masonry model is
            // effectively free. The walk is a floor, a hash and four min/max, so the
            // tone lands in the same band as `checker` — and it is what separates a
            // wall of blocks from a painted slab. The edge costs roughly twice the
            // tone and is still a twentieth of one noise layer, the `pow` that
            // sharpens the joint being the whole difference. The edge is the one row
            // whose band is not robust: 0.052 median sits just over the 0.05 Free
            // threshold, so it reads Cheap, but a quiet machine will call it Free.
            PatternGenerator::TileTone => 0.029,
            PatternGenerator::TileEdge { .. } => 0.052,
            PatternGenerator::Worley => 1.003,
            PatternGenerator::WorleySmooth => 1.114,
            PatternGenerator::WorleyEdge => 1.076,
        }
    }

    /// Whether the octave count means anything for this generator — the panel and
    /// the node declarations use it to decide whether to offer the field.
    pub const fn has_octaves(&self) -> bool {
        matches!(
            self,
            PatternGenerator::Noise { .. }
                | PatternGenerator::Perlin { .. }
                | PatternGenerator::Simplex { .. }
                | PatternGenerator::Ridged { .. }
                | PatternGenerator::Turbulence { .. }
        )
    }

    /// Panel label.
    pub const fn label(&self) -> &'static str {
        match self {
            PatternGenerator::Flat => "flat (one value per cell)",
            PatternGenerator::Noise { .. } => "noise (grain / mottle)",
            PatternGenerator::Speckle { .. } => "speckle",
            PatternGenerator::Perlin { .. } => "perlin (gradient, cubic)",
            PatternGenerator::Simplex { .. } => "simplex (gradient, tetrahedral)",
            PatternGenerator::Ridged { .. } => "ridged (veins / strata)",
            PatternGenerator::Turbulence { .. } => "turbulence (marble / smoke)",
            PatternGenerator::Worley => "worley F1 (cells / pebbles)",
            PatternGenerator::WorleyEdge => "worley edge (cracks)",
            PatternGenerator::WorleySmooth => "worley smooth (merged cells)",
            PatternGenerator::Wave { .. } => "wave (bands / wood grain)",
            PatternGenerator::Checker => "checker (tiles)",
            PatternGenerator::TileTone => "tile tone (per-block shade)",
            PatternGenerator::TileEdge { .. } => "tile edge (grout / bevel)",
        }
    }

    /// Every generator, with representative parameters — what the panel offers as
    /// a starting point. "Generate, then hand-tune" is the authoring loop, so these
    /// are seeds and not presets.
    pub const ALL: [PatternGenerator; 14] = [
        PatternGenerator::Flat,
        PatternGenerator::Noise { octaves: 3 },
        PatternGenerator::Speckle { density: 0.25 },
        PatternGenerator::Perlin { octaves: 3 },
        PatternGenerator::Simplex { octaves: 3 },
        PatternGenerator::Ridged { octaves: 3 },
        PatternGenerator::Turbulence { octaves: 3 },
        PatternGenerator::Worley,
        PatternGenerator::WorleyEdge,
        PatternGenerator::WorleySmooth,
        PatternGenerator::Wave { distortion: 0.25 },
        PatternGenerator::Checker,
        PatternGenerator::TileTone,
        PatternGenerator::TileEdge { sharpness: 0.6 },
    ];
}

/// Which space a layer's coordinate lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternFrame {
    /// World space. **The default, and the reason continuity works.** The pattern
    /// is a field the world sits in, so it flows across neighbouring voxels and
    /// *cannot* tile per voxel — a large-period layer is a multi-voxel pattern for
    /// free, with no template mechanism anywhere.
    World,
    /// Restarts at every voxel: the coordinate is the voxel's own centre, so the
    /// generator returns ONE value for the whole voxel. Deliberately per-voxel
    /// motifs — tone jitter being the whole point.
    Voxel,
    /// One-metre-world-voxel-local `u`/`v` within the hit face, so the pattern is
    /// about the authored block face. A period of 1 m spans exactly one face.
    Face,
    /// Subdivides the face into TILES and hands the generator a tile-local
    /// coordinate, so the pattern restarts at every tile edge.
    ///
    /// This is the frame masonry needs and the other three cannot express. A stone
    /// wall is not one field sampled across a surface — it is many small fields, one
    /// per block, each with its own draw. Look at any slate wall: the grain in one
    /// block runs out at the edge and a different grain starts in the next. World and
    /// face frames both run a single continuous field straight across the joint.
    ///
    /// `period_meters` is the TILE SIZE here rather than the feature size, and the
    /// generator's own features are then expressed in tile units — one tile is one
    /// unit of the generator's domain. That is not a limitation so much as the right
    /// default: a grain authored per tile should scale with the tile.
    Tile,
}

impl PatternFrame {
    pub const fn code(&self) -> u32 {
        match self {
            PatternFrame::World => 0,
            PatternFrame::Voxel => 1,
            PatternFrame::Face => 2,
            PatternFrame::Tile => 3,
        }
    }

    pub const fn label(&self) -> &'static str {
        match self {
            PatternFrame::World => "world (continuous)",
            PatternFrame::Voxel => "voxel (per-voxel)",
            PatternFrame::Face => "face (within the face)",
            PatternFrame::Tile => "tile (within a tile)",
        }
    }

    pub const ALL: [PatternFrame; 4] = [
        PatternFrame::World,
        PatternFrame::Voxel,
        PatternFrame::Face,
        PatternFrame::Tile,
    ];
}

/// Which shading input a layer modulates.
///
/// See the module docs on why `opacity` is absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternTarget {
    /// The per-face albedo S1 selected — so a pattern composes with face roles
    /// rather than replacing them.
    Albedo,
    /// The per-face roughness.
    Roughness,
    /// Emitted radiance. Only meaningful on an emitting row, and the panel says so
    /// rather than silently doing nothing.
    Emission,
}

impl PatternTarget {
    pub const fn code(&self) -> u32 {
        match self {
            PatternTarget::Albedo => 0,
            PatternTarget::Roughness => 1,
            PatternTarget::Emission => 2,
        }
    }

    pub const fn label(&self) -> &'static str {
        match self {
            PatternTarget::Albedo => "albedo",
            PatternTarget::Roughness => "roughness",
            PatternTarget::Emission => "emission",
        }
    }

    /// Whether this target is a colour, i.e. whether the layer's `target_color`
    /// means anything beyond its first channel.
    pub const fn is_color(&self) -> bool {
        matches!(self, PatternTarget::Albedo | PatternTarget::Emission)
    }

    pub const ALL: [PatternTarget; 3] = [
        PatternTarget::Albedo,
        PatternTarget::Roughness,
        PatternTarget::Emission,
    ];
}

/// How a layer's value reaches its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternBlend {
    /// Darken toward `1 - amount` where the value is low. The workhorse: grain,
    /// mortar shadow, dirt. Never brightens, so it cannot push an albedo above the
    /// authored colour and out of range.
    Multiply,
    /// Interpolate toward `target_color`. What a two-colour material is — mortar
    /// grey against brick red, lichen green on stone.
    MixToColor,
    /// Add `target_color` scaled by the value. For emission, and for the rare
    /// surface that genuinely gains light rather than losing it.
    Add,
}

impl PatternBlend {
    pub const fn code(&self) -> u32 {
        match self {
            PatternBlend::Multiply => 0,
            PatternBlend::MixToColor => 1,
            PatternBlend::Add => 2,
        }
    }

    pub const fn label(&self) -> &'static str {
        match self {
            PatternBlend::Multiply => "multiply (darken)",
            PatternBlend::MixToColor => "mix to colour",
            PatternBlend::Add => "add",
        }
    }

    /// Whether `target_color` is read at all.
    pub const fn uses_target_color(&self) -> bool {
        matches!(self, PatternBlend::MixToColor | PatternBlend::Add)
    }

    pub const ALL: [PatternBlend; 3] = [
        PatternBlend::Multiply,
        PatternBlend::MixToColor,
        PatternBlend::Add,
    ];
}

/// Which face roles a layer applies to.
///
/// Reuses S1's three roles rather than inventing a second face taxonomy, so "moss
/// on the top only" is one checkbox and not a second copy of the row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatternFaces {
    pub top: bool,
    pub side: bool,
    pub bottom: bool,
}

impl PatternFaces {
    /// Every face — the default, since most layers are a property of the material
    /// rather than of one of its faces.
    pub const ALL: PatternFaces = PatternFaces {
        top: true,
        side: true,
        bottom: true,
    };
    /// The sky-facing face alone: snow settling, moss, sun-bleaching.
    pub const TOP: PatternFaces = PatternFaces {
        top: true,
        side: false,
        bottom: false,
    };
    /// The four sides alone: drips, weathering that runs downward.
    pub const SIDES: PatternFaces = PatternFaces {
        top: false,
        side: true,
        bottom: false,
    };

    pub const fn bits(&self) -> u32 {
        (self.top as u32) | ((self.side as u32) << 1) | ((self.bottom as u32) << 2)
    }

    /// Whether this mask includes the face named by a hit's axis and sign.
    ///
    /// The sign convention is S1's, and it is the one thing here that reads
    /// backwards: `hit_normal` builds its normal as `-axis_sign` along `axis`, so a
    /// `+Y` normal — the TOP face — is `axis == 1` with a *negative* sign.
    pub const fn includes(&self, axis: u32, axis_sign: f32) -> bool {
        if axis != 1 {
            return self.side;
        }
        if axis_sign < 0.0 {
            self.top
        } else {
            self.bottom
        }
    }
}

/// One layer of a material's pattern stack.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PatternLayer {
    pub generator: PatternGenerator,
    pub frame: PatternFrame,
    /// The size of the generator's largest feature, in **metres**. This is
    /// independent of the 0.125 m texel snap: period controls the feature while
    /// `texels_per_voxel` controls material-detail resolution.
    pub period_meters: f32,
    pub target: PatternTarget,
    pub blend: PatternBlend,
    /// How strongly the layer applies, `0.0..=1.0`. Zero is the identity, which is
    /// what makes a layer safe to leave in a row while dialling another one.
    pub amount: f32,
    /// The second value, for [`PatternBlend::MixToColor`] and [`PatternBlend::Add`].
    ///
    /// **In whatever space its target is in**, which is the one thing about this field
    /// worth stating:
    ///
    /// * [`PatternTarget::Albedo`] — sRGB-encoded like
    ///   [`crate::material::Material::albedo`], because the layer is applied *before*
    ///   `srgb_decode` and so mixes in the same space the row's own colour lives in.
    ///   Range 0..1; the panel shows a colour picker.
    /// * [`PatternTarget::Emission`] — **linear radiance, and allowed above 1.0**, like
    ///   [`crate::material::Material::emission`]. A source may be brighter than any
    ///   surface can reflect (`glow_block` authors 3.0). Nothing decodes it.
    /// * [`PatternTarget::Roughness`] — the first channel only, as a scalar 0..1.
    pub target_color: [f32; 3],
    pub faces: PatternFaces,
    /// **Texels per voxel edge**, or `0` for a continuous field.
    ///
    /// The generator is sampled once per texel and held flat across it, so the result
    /// is piecewise constant on an `n x n` grid per one-metre block face — the blocky look, rather
    /// than a smooth field that happens to sit on voxels.
    ///
    /// This is the setting the S2 gate asked for, and it is the one that was missing
    /// (Pascal, 2026-07-31: *"for most cases you want even with spekles and things to
    /// keep to the 8x8 sizing"*). Every frame and period the model had were
    /// **continuous**, so `Noise` gave smooth mottle and `Speckle` gave round dots.
    /// Neither is what a voxel surface wants: the world is made of cubes and its
    /// detail should be made of squares.
    ///
    /// **Why a snap on the coordinate rather than a new generator.** Quantising the
    /// *sample position* makes every generator blocky at once — noise becomes blocky
    /// noise, speckles become square specks — and it stays orthogonal to the period,
    /// which keeps its own job of setting the FEATURE size. 8 texels with a 1 m period
    /// is a large field rendered in 0.125 m squares; changing the period changes the
    /// feature, not the physical detail-cell size. One field, both.
    ///
    /// **The grid is anchored to the world, not to the face**, so in
    /// [`PatternFrame::World`] it lines up across neighbouring voxels: the texel size
    /// divides the 1 m world-voxel edge exactly and world zero is a voxel boundary, so a texel
    /// never straddles a voxel edge. That is what keeps the blocky look and cross-voxel
    /// continuity from being a trade-off.
    ///
    /// Also an **anti-aliasing win**, which is the opposite of the intuition that hard
    /// edges alias worse: a piecewise-constant signal box-filters toward its local
    /// mean, where continuous noise at a sub-pixel period keeps producing new values
    /// per pixel. See [`PatternLayer::fade`] for why the fade still keys off the period
    /// regardless.
    pub texels_per_voxel: u32,
    /// Give every face its own draw of the pattern. **Only affects
    /// [`PatternFrame::Face`]**, and defaults to on.
    ///
    /// The face frame is voxel-local, so without this it draws the *identical* pattern
    /// on every face in the world — which is a visible repeat rather than detail
    /// (Pascal, 2026-07-31: *"the face within the face .. this part should still have a
    /// randomizer so we dont have a repeating patern"*). The other two frames do not
    /// need it: world already varies with position, and voxel already varies per voxel.
    ///
    /// **Implemented as a hash SALT, not a coordinate offset**, and that distinction is
    /// the whole reason it is safe to leave on:
    ///
    /// * An offset would slide the pattern within the face, which breaks the texel grid
    ///   alignment and would break a positional generator's relationship to the edge it
    ///   is supposed to be concentrated at.
    /// * A salt re-rolls the random draw and moves nothing. Hash-driven generators
    ///   (all three of today's) get a completely different pattern per face; a purely
    ///   positional generator would be untouched, because it never hashes.
    ///
    /// Turn it OFF for a deliberate motif — the classic voxel look where every face of
    /// a block type is identical. It does not have to be seamless across faces, which
    /// is what separates this from the world frame's continuity requirement.
    pub vary_per_face: bool,
    /// Domain warp strength, in periods. `0.0` is off and is the default.
    ///
    /// Iñigo Quílez's domain warping: sample a noise field at the point and push
    /// the point by it before the generator ever reads it — `fbm(p + fbm(p))`.
    ///
    /// **A property of the layer rather than a generator of its own**, because it
    /// composes with all twelve: warped [`PatternGenerator::Worley`] is cracked
    /// stone, warped [`PatternGenerator::Wave`] is wood grain, warped
    /// [`PatternGenerator::Checker`] is a rippled tile floor. Twelve generators
    /// times on/off is a far bigger library than twelve plus one.
    ///
    /// It is not cheap — three extra value-noise evaluations, i.e. 24 hashes, which
    /// is THREE octaves and not one. Bench section 11 measures +0.73 ms on a layer
    /// whose whole 3-octave generator costs +0.72 ms, so warping roughly doubles a
    /// layer. The authoring trade is therefore "a warp or a second layer", not "a
    /// warp or one more octave".
    ///
    /// Rides in `GpuPatternLayer::param_b`, which was reserved and unread; the
    /// packed bit 25 only records whether it is on, so a strength of zero folds the
    /// whole path away.
    pub domain_warp: f32,
    /// Tile width over height, for [`PatternFrame::Tile`]. `1.0` is square, `4.0`
    /// is a long brick. Ignored by every other frame.
    ///
    /// The three `tile_*` fields are authored TOGETHER on a `material.tessellation`
    /// node rather than per layer, and the projection copies them onto every layer
    /// downstream of it. That is the whole reason the node exists: a tone layer, a
    /// grout layer and a grain layer on one wall have to agree about where the tiles
    /// are, and three independent copies of the same numbers is a bug waiting for a
    /// slider drag.
    pub tile_aspect: f32,
    /// Fraction of a tile that each successive row shifts by. `0.0` stacks the
    /// courses, `0.5` is a running bond, `1.0 / 3.0` a third bond.
    pub tile_bond: f32,
    /// Grout width as a fraction of the tile's short edge. `0.0` is no gap.
    pub tile_gap: f32,
    /// Brightness multiplier on [`Self::target_color`], for an
    /// [`PatternTarget::Emission`] target only. `0.0..=`[`MAX_EMISSION_INTENSITY`].
    ///
    /// Emission is radiance and belongs above 1.0 — a source may be brighter than any
    /// surface can reflect, and `glow_block` authors 3.0. But a colour picker clamps to
    /// 0..1, and replacing the picker with three raw 0..16 channels was the wrong trade
    /// (Pascal, 2026-07-31: *"why not the picker we had before? why 3 fields?"*): picking
    /// a HUE is what a picker is good at, and being unable to exceed 1 is a separate
    /// problem. So they are separated — picker for the colour, this for the brightness.
    ///
    /// **Costs no bytes and reaches no shader.** [`PatternLayer::to_gpu`] folds it into
    /// the uploaded `target_color`, so this is purely an authoring split: the GPU reads
    /// one pre-multiplied value exactly as it did before the field existed, and the WGSL
    /// needed no change at all. Which is also why it keeps full `f32` precision rather
    /// than being quantised into a spare corner of the packed word.
    pub emission_intensity: f32,
}

/// Ceiling on [`PatternLayer::emission_intensity`], matching the range the row's own
/// `emission` field is authored in.
pub const MAX_EMISSION_INTENSITY: f32 = 16.0;

/// Texel-grid rungs the panel offers, and what the bench sweeps.
///
/// Powers of two only, so the texel size divides the 1 m world-voxel edge exactly and the grid
/// stays aligned across voxels — the property the whole snap rests on. `0` is
/// "continuous", i.e. the pre-snap behaviour.
///
/// ## This is a SHARED lattice, and that is the point
///
/// The grid these rungs describe is not private to the pattern model. It is the
/// natural sub-voxel lattice of the whole engine, which makes three things
/// commensurate that would otherwise each have their own resolution (Pascal,
/// 2026-07-31: *"if you make .vox and assign material it will nicely snap to one of
/// the 8x8"*):
///
/// * **A procedural layer**, snapped to `n` texels per voxel edge.
/// * **A `.vox` model drawn at `n` cells per engine voxel** — the S0b importer already
///   reads one, and an external editor is a far better place to draw an `n³` interior
///   than anything we would build in egui.
/// * **S5's material-owned sub-voxel models**, whose "resolution per material" should
///   be a rung from this list rather than a second, independent number. A hand-drawn
///   mask and a generated field then land on the same cells and compose, instead of
///   one being resampled onto the other and both looking slightly wrong.
///
/// So a change to this list is a change to an engine-wide lattice, not to a pattern
/// setting. Keep it powers of two.
pub const TEXEL_RUNGS: [u32; 6] = [0, 2, 4, 8, 16, 32];

/// The default texel grid: 8 per voxel edge.
///
/// On rather than off, because it is what almost every layer wants (*"for most cases
/// you want … to keep to the 8x8 sizing"*), and a default that has to be switched on
/// to look right is a default in the wrong place.
pub const DEFAULT_TEXELS_PER_VOXEL: u32 = 8;

/// Default feature size for a newly added layer: 2 cm detail, sampled on the
/// default 8×8 face grid. Larger-scale materials can still raise the period in
/// the panel when they want bands or cross-voxel mottle.
pub const DEFAULT_PERIOD_METERS: f32 = 0.02;

/// Ceiling on the texel grid. 32 per voxel edge is a 3.9 mm texel — already finer than
/// a pixel at arm's length, so past it the snap stops being visible and only costs the
/// two floors.
pub const MAX_TEXELS_PER_VOXEL: u32 = 32;

/// Tile aspect bounds. The shader divides by the aspect, so zero is not allowed;
/// past 8:1 a "tile" is a plank and the bond stops reading as masonry.
pub const MINIMUM_TILE_ASPECT: f32 = 0.125;
pub const MAXIMUM_TILE_ASPECT: f32 = 8.0;

/// Grout width as a fraction of the tile's short edge. Beyond a third the gap is
/// wider than the stone and the tessellation reads as a grid of holes.
pub const MAXIMUM_TILE_GAP: f32 = 0.33;

impl PatternLayer {
    /// A layer that changes nothing — the safe baseline for internal composition
    /// and tests.
    pub const IDENTITY: PatternLayer = PatternLayer {
        generator: PatternGenerator::Noise { octaves: 3 },
        frame: PatternFrame::World,
        period_meters: DEFAULT_PERIOD_METERS,
        target: PatternTarget::Albedo,
        blend: PatternBlend::Multiply,
        amount: 0.0,
        target_color: [1.0, 1.0, 1.0],
        domain_warp: 0.0,
        tile_aspect: 1.0,
        tile_bond: 0.5,
        tile_gap: 0.06,
        faces: PatternFaces::ALL,
        texels_per_voxel: DEFAULT_TEXELS_PER_VOXEL,
        vary_per_face: true,
        emission_intensity: 1.0,
    };

    /// The panel's newly-added layer: same shared 8×8/2 cm defaults, but fully
    /// applied so the author sees the pattern immediately.
    pub const DEFAULT: PatternLayer = PatternLayer {
        amount: 1.0,
        ..Self::IDENTITY
    };

    /// The uploaded form.
    pub fn to_gpu(&self) -> GpuPatternLayer {
        let (param_a, param_b) = self.params();
        GpuPatternLayer {
            packed: self.packed(),
            period_meters: self.period_meters.max(MINIMUM_PERIOD_METERS),
            amount: self.amount,
            param_a,
            // The SCALED value, so the shader reads one number and needs no intensity
            // field of its own — and so the WGSL needed no change for this at all.
            target_color: self.target_value(),
            param_b,
            // Clamped on the way to the GPU rather than on the way in, the same way
            // `period_meters` is: the authoring value keeps whatever the slider said
            // and the shader is handed something it can divide by.
            tile_aspect: self
                .tile_aspect
                .clamp(MINIMUM_TILE_ASPECT, MAXIMUM_TILE_ASPECT),
            tile_bond: self.tile_bond.rem_euclid(1.0),
            tile_gap: self.tile_gap.clamp(0.0, MAXIMUM_TILE_GAP),
            _pad_row2: 0.0,
        }
    }

    /// The discriminants and the octave count, in one word.
    ///
    /// Field offsets, mirrored by the accessors at the top of `shaders/pattern.wgsl`
    /// and pinned against them by [`packed_round_trips_every_field`]:
    ///
    /// | bits | field | bits | field |
    /// |---|---|---|---|
    /// | 0-3 | generator (4 bits, 12 of 16 codes used) | 13-15 | octaves |
    /// | 4-5 | frame | 16-23 | texels per voxel |
    /// | 6-7 | target | 24 | vary per face |
    /// | 8-9 | blend | 25 | domain warp |
    /// | 10-12 | face mask | 26-31 | free |
    ///
    /// The generator field was three bits until the library grew past eight codes.
    /// Widening it shifted everything above, which is the kind of change that is
    /// silent on the Rust side and catastrophic on the GPU side — hence the
    /// round-trip test rather than trust.
    fn packed(&self) -> u32 {
        let octaves = self.generator.octaves().clamp(1, MAX_NOISE_OCTAVES);
        self.generator.code()
            | (self.frame.code() << 4)
            | (self.target.code() << 6)
            | (self.blend.code() << 8)
            | (self.faces.bits() << 10)
            | (octaves << 13)
            | (self.texels_per_voxel.min(MAX_TEXELS_PER_VOXEL) << 16)
            | ((self.vary_per_face as u32) << 24)
            | ((self.domain_warp > 0.0) as u32) << 25
    }

    /// The blend's second operand: [`Self::target_color`], scaled by
    /// [`Self::emission_intensity`] on an emission target and left alone otherwise.
    ///
    /// The one place the two authoring fields become the single value everything else
    /// reads — the uploaded row, the CPU reference, and therefore the mean the GI
    /// injects. Nothing downstream knows the split exists.
    pub fn target_value(&self) -> [f32; 3] {
        if self.target != PatternTarget::Emission {
            return self.target_color;
        }
        let intensity = self.emission_intensity.clamp(0.0, MAX_EMISSION_INTENSITY);
        [
            self.target_color[0] * intensity,
            self.target_color[1] * intensity,
            self.target_color[2] * intensity,
        ]
    }

    /// The two free generator parameters. Which generator reads which is the one
    /// place this packing is not self-describing, so it is stated in one function
    /// rather than spread across the shader.
    ///
    /// `param_a` is the generator's own knob and means a different thing per
    /// generator; `param_b` is the domain-warp strength and means the same thing
    /// for all of them, which is why the warp could be added without any generator
    /// giving up a slot.
    fn params(&self) -> (f32, f32) {
        let generator_param = match self.generator {
            PatternGenerator::Speckle { density } => density.clamp(0.0, 1.0),
            PatternGenerator::Wave { distortion } => distortion.max(0.0),
            PatternGenerator::TileEdge { sharpness } => sharpness.clamp(0.0, 1.0),
            _ => 0.0,
        };
        (generator_param, self.domain_warp.max(0.0))
    }
}

/// A period below this is treated as this — the shader divides by it, and a zero
/// dragged out of a slider must not produce an infinity that paints a NaN.
pub const MINIMUM_PERIOD_METERS: f32 = 1e-4;

/// Noise octave ceiling. Four is where the fourth octave's amplitude (1/8) stops
/// being visible against the first, so a fifth costs a lattice fetch for nothing.
pub const MAX_NOISE_OCTAVES: u32 = 4;

/// A material's pattern layers.
///
/// A fixed-size array of `Option`s rather than a `Vec`, because [`crate::material::Material`]
/// is `Copy` and lives in a `const` table: the authored rows are compiled, and
/// [`crate::material_table::MaterialTable`] edits copies of them. A `Vec` here
/// would make every row a heap allocation to buy a fifth layer nobody has asked
/// for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PatternStack {
    pub layers: [Option<PatternLayer>; MAX_PATTERN_LAYERS],
}

/// A row with no patterns at all — what 26 of 27 rows carry until S6.
pub const NO_PATTERNS: PatternStack = PatternStack {
    layers: [None; MAX_PATTERN_LAYERS],
};

impl PatternStack {
    /// Which generators this stack can reach, as bits for
    /// `MATERIAL_PATTERN_GENERATOR_MASK`.
    ///
    /// DERIVED RATHER THAN DIALLED, and that distinction is the point. Set by hand,
    /// the mask is a footgun: clear the bit for a generator some material does use
    /// and that material renders silently flat. Computed from the stack, it cannot
    /// disagree with the stack.
    ///
    /// Errs toward MORE code, never less — an empty stack contributes nothing, and
    /// the table-level [`crate::material::generator_mask`] seeds the flat bit.
    pub fn generator_mask(&self) -> u32 {
        let mut mask = 0u32;
        for layer in self.layers.iter().flatten() {
            mask |= layer.generator.mask_bit();
        }
        mask
    }

    /// A stack holding exactly the layers given, in order.
    ///
    /// `const` so authored rows can build one, and it **compacts**: the shader
    /// walks slots `0..count`, so a `None` with a `Some` behind it would silently
    /// drop the tail. Being unable to express that at all is better than testing
    /// for it.
    pub const fn of(layers: &[PatternLayer]) -> PatternStack {
        let mut stack = NO_PATTERNS;
        let mut index = 0;
        while index < layers.len() && index < MAX_PATTERN_LAYERS {
            stack.layers[index] = Some(layers[index]);
            index += 1;
        }
        stack
    }

    /// How many leading slots hold a layer. The shader's loop bound.
    ///
    /// `const`, and therefore a `while` loop rather than `take_while().count()`:
    /// [`crate::material::Material::flags`] is a const fn, because deriving the
    /// flags at compile time is what stops a row's flags and its data disagreeing.
    pub const fn active_count(&self) -> usize {
        let mut count = 0;
        while count < MAX_PATTERN_LAYERS && self.layers[count].is_some() {
            count += 1;
        }
        count
    }

    pub const fn is_empty(&self) -> bool {
        self.active_count() == 0
    }

    /// The active layers, in order.
    pub fn active(&self) -> impl Iterator<Item = &PatternLayer> {
        self.layers.iter().flatten()
    }

    /// Append a layer, or return it back if the stack is full.
    pub fn push(&mut self, layer: PatternLayer) -> Option<PatternLayer> {
        let count = self.active_count();
        if count >= MAX_PATTERN_LAYERS {
            return Some(layer);
        }
        self.layers[count] = Some(layer);
        None
    }

    /// Remove one layer and close the gap, so the leading-`Some` invariant holds.
    pub fn remove(&mut self, index: usize) {
        if index >= MAX_PATTERN_LAYERS {
            return;
        }
        for slot in index..MAX_PATTERN_LAYERS - 1 {
            self.layers[slot] = self.layers[slot + 1];
        }
        self.layers[MAX_PATTERN_LAYERS - 1] = None;
    }

    /// The uploaded slots, always all [`MAX_PATTERN_LAYERS`] of them: the GPU row
    /// is fixed-size, and an inactive slot is zeroed rather than absent.
    pub fn to_gpu(&self) -> [GpuPatternLayer; MAX_PATTERN_LAYERS] {
        let mut slots = [GpuPatternLayer::INACTIVE; MAX_PATTERN_LAYERS];
        for (slot, layer) in slots.iter_mut().zip(self.active()) {
            *slot = layer.to_gpu();
        }
        slots
    }
}

/// One uploaded layer: 48 bytes, THREE std430 16-byte rows.
///
/// Each row is four scalars, or a `vec3` with the scalar filling its `w` — the same
/// discipline the rest of [`crate::material::GpuMaterial`] follows, so std430
/// inserts no implicit padding and the Rust upload matches the WGSL byte for byte.
///
/// **It was 32 bytes and two rows until the tessellation landed.** The three tile
/// fields would not fit: `period_meters` is the tile size, and `param_a` and
/// `param_b` were already the generator's own knob and the domain-warp strength. The
/// alternative was packing aspect, bond and gap into the six free bits of
/// [`PatternLayer::packed`] as fixed rungs, which would have made them radio rows
/// instead of sliders — a real authoring cost to save bandwidth that section 9
/// already measured as free when the material row went 128 -> 256 bytes.
///
/// `GpuMaterial` is therefore 320 bytes and the 26-row table is 8.3 KB, up from
/// 6.6. That still fits anywhere, which is the same argument the last growth made
/// and the same one section 1's re-verification checked.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuPatternLayer {
    // row 0
    /// Generator, frame, target, blend, face mask and octave count. See
    /// [`PatternLayer::packed`].
    pub packed: u32,
    /// In a tile frame this is the TILE SIZE; in every other frame it is the
    /// generator's feature size. One field, because a tile-framed generator's
    /// features are expressed in tile units and need no second scale.
    pub period_meters: f32,
    pub amount: f32,
    pub param_a: f32,
    // row 1
    pub target_color: [f32; 3],
    pub param_b: f32,
    // row 2 — the tessellation, ignored by every frame but `Tile`.
    /// Tile width over height. 1.0 is square; 4.0 is a long brick.
    pub tile_aspect: f32,
    /// Fraction of a tile that each successive row is shifted by. 0.0 stacks,
    /// 0.5 is a running bond.
    pub tile_bond: f32,
    /// Grout width as a fraction of the tile's short edge.
    pub tile_gap: f32,
    /// Explicit tail padding to close the third row. Named rather than implicit,
    /// the discipline [`crate::world_event::GpuWorldEvent`] documents.
    pub _pad_row2: f32,
}

impl GpuPatternLayer {
    /// An unused slot. `amount` zero makes it the identity even if the shader's
    /// loop bound were ever wrong, which is the cheap belt to the count's braces.
    pub const INACTIVE: GpuPatternLayer = GpuPatternLayer {
        packed: 0,
        period_meters: 1.0,
        amount: 0.0,
        param_a: 0.0,
        target_color: [0.0, 0.0, 0.0],
        param_b: 0.0,
        tile_aspect: 1.0,
        tile_bond: 0.0,
        tile_gap: 0.0,
        _pad_row2: 0.0,
    };
}

unsafe impl bytemuck::Zeroable for GpuPatternLayer {}
unsafe impl bytemuck::Pod for GpuPatternLayer {}

// ---- The reference evaluator -------------------------------------------------
//
// Everything below mirrors `shaders/pattern.wgsl` exactly. See the module docs on
// why it exists at all: nothing in the renderer calls it.

/// Where a hit is, in every form the frames need.
///
/// Built once per hit in the shader from `Hit` and the ray; spelled out as a
/// struct here so the reference evaluator takes the same inputs the WGSL has.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PatternSample {
    /// The hit point in world metres.
    pub world_meters: [f32; 3],
    /// The hit voxel's integer coordinate.
    pub voxel: [i32; 3],
    /// Face axis: 0 = x, 1 = y, 2 = z.
    pub axis: u32,
    /// Sign of the ray along that axis. See [`PatternFaces::includes`] for why the
    /// top face is the negative one.
    pub axis_sign: f32,
    /// How far the camera is from the hit, in metres. Drives the fade only.
    pub distance_meters: f32,
}

/// The per-layer animation values a material graph supplies, as the reference
/// evaluator takes them. Mirrors one slot of `PatternAnimation` in
/// `shaders/world.wgsl` plus the clock the shader reads from its uniform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerAnimationSample {
    /// Multiplies the layer's authored `amount`. Separate from it, so 1.0 is
    /// plainly the identity and the authored number keeps its one meaning.
    pub gain: f32,
    /// Metres per second, world space.
    pub drift_velocity: [f32; 3],
    /// Monotone seconds, from the animation clock.
    pub time_seconds: f32,
}

impl LayerAnimationSample {
    /// The un-animated case: unit gain, no drift. What every pre-S3 call site
    /// means, and what the identity of the whole feature is.
    pub const STILL: Self = Self {
        gain: 1.0,
        drift_velocity: [0.0; 3],
        time_seconds: 0.0,
    };
}

/// The integer hash both sides use: Chris Wellons' `lowbias32`.
///
/// Chosen because it is three multiplies and three shifts with no lookup, has
/// avalanche behaviour good enough that neighbouring lattice cells are visually
/// uncorrelated, and — the deciding property — is expressible identically in Rust
/// and WGSL. Rust's `wrapping_mul` and WGSL's `u32` multiply are both mod 2^32, and
/// `>>` on `u32` is logical in both, so the two implementations agree bit for bit.
fn hash_u32(value: u32) -> u32 {
    let mut hashed = value;
    hashed ^= hashed >> 16;
    hashed = hashed.wrapping_mul(0x7feb_352d);
    hashed ^= hashed >> 15;
    hashed = hashed.wrapping_mul(0x846c_a68b);
    hashed ^= hashed >> 16;
    hashed
}

/// Hash a 3D lattice cell to `0.0..1.0`.
///
/// The `as u32` casts are two's-complement reinterpretations, which is exactly
/// what WGSL's `bitcast<u32>` does — so a negative coordinate (and the world's
/// centre is at +512, but a `Voxel` frame layer on a pattern offset can go
/// negative) hashes the same on both sides.
fn hash_cell(cell: [i32; 3], salt: u32) -> f32 {
    let mixed = (cell[0] as u32).wrapping_mul(0x27d4_eb2d)
        ^ (cell[1] as u32).wrapping_mul(0x9e37_79b9)
        ^ (cell[2] as u32).wrapping_mul(0x85eb_ca6b)
        ^ salt.wrapping_mul(0xc2b2_ae35);
    hash_u32(mixed) as f32 / 4_294_967_296.0
}

/// The classic `3t^2 - 2t^3` ease. WGSL's `smoothstep(0, 1, t)` is the same
/// polynomial, and this is applied to an already-clamped `0..1`.
fn ease(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Value noise on the lattice: hash the eight corners, ease-interpolate.
///
/// Value rather than gradient (Perlin) noise: it needs one hash per corner
/// instead of a hash plus a dot product, has no zero-at-the-lattice artefact to
/// work around, and at the periods this stage uses — a few centimetres to a metre
/// — the difference in character is invisible against the voxel grid it sits on.
fn value_noise(point: [f32; 3], salt: u32) -> f32 {
    let base = [point[0].floor(), point[1].floor(), point[2].floor()];
    let cell = [base[0] as i32, base[1] as i32, base[2] as i32];
    let fraction = [
        ease(point[0] - base[0]),
        ease(point[1] - base[1]),
        ease(point[2] - base[2]),
    ];
    let mut accumulated = 0.0;
    for corner in 0..8 {
        let offset = [corner & 1, (corner >> 1) & 1, (corner >> 2) & 1];
        let weight = (0..3)
            .map(|axis| {
                if offset[axis] == 1 {
                    fraction[axis]
                } else {
                    1.0 - fraction[axis]
                }
            })
            .product::<f32>();
        let corner_cell = [
            cell[0] + offset[0],
            cell[1] + offset[1],
            cell[2] + offset[2],
        ];
        accumulated += weight * hash_cell(corner_cell, salt);
    }
    accumulated
}

/// Fractal value noise, normalised back into `0.0..1.0`.
///
/// Lacunarity 2, gain 0.5, and the sum divided by the amplitude total — so the
/// period always names the largest feature and the octave count changes the
/// texture without changing the contrast, which is what makes the octave slider
/// usable while the amount slider is set.
fn fractal_noise(point: [f32; 3], octaves: u32, salt_base: u32) -> f32 {
    let mut frequency = 1.0;
    let mut amplitude = 1.0;
    let mut total = 0.0;
    let mut normalisation = 0.0;
    for octave in 0..octaves.clamp(1, MAX_NOISE_OCTAVES) {
        let scaled = [
            point[0] * frequency,
            point[1] * frequency,
            point[2] * frequency,
        ];
        total += amplitude * value_noise(scaled, salt_base ^ octave);
        normalisation += amplitude;
        frequency *= 2.0;
        amplitude *= 0.5;
    }
    total / normalisation
}

/// Perlin's quintic fade `6t^5 - 15t^4 + 10t^3`.
///
/// The cubic [`ease`] has a discontinuous second derivative at the lattice, which
/// value noise gets away with and a gradient field does not — it shows as faint
/// creases along the cell planes. Mirrors `pattern_quintic`.
fn quintic(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// One of the 12 edge-midpoint gradients, chosen by hash. Mirrors
/// `pattern_gradient`; shared by [`perlin_noise`] and [`simplex_noise`].
///
/// The `as u32` casts are the same two's-complement reinterpretation
/// [`hash_cell`] documents, and `%` on `u32` is the same operation in both
/// languages, so the chosen index agrees exactly.
fn gradient(cell: [i32; 3], salt: u32) -> [f32; 3] {
    let mixed = (cell[0] as u32).wrapping_mul(0x27d4_eb2d)
        ^ (cell[1] as u32).wrapping_mul(0x9e37_79b9)
        ^ (cell[2] as u32).wrapping_mul(0x85eb_ca6b)
        ^ salt.wrapping_mul(0xc2b2_ae35);
    let index = hash_u32(mixed) % 12;
    let axis = index / 4;
    let first = if index & 1 != 0 { -1.0 } else { 1.0 };
    let second = if index & 2 != 0 { -1.0 } else { 1.0 };
    match axis {
        0 => [first, second, 0.0],
        1 => [first, 0.0, second],
        _ => [0.0, first, second],
    }
}

/// Perlin gradient noise on the cubic lattice. Mirrors `pattern_perlin_noise`.
fn perlin_noise(point: [f32; 3], salt: u32) -> f32 {
    let base = [point[0].floor(), point[1].floor(), point[2].floor()];
    let cell = [base[0] as i32, base[1] as i32, base[2] as i32];
    let local = [point[0] - base[0], point[1] - base[1], point[2] - base[2]];
    let fade = [quintic(local[0]), quintic(local[1]), quintic(local[2])];
    let mut accumulated = 0.0;
    for corner in 0..8 {
        let offset = [corner & 1, (corner >> 1) & 1, (corner >> 2) & 1];
        let mut weight = 1.0;
        for axis in 0..3 {
            weight *= if offset[axis] == 1 {
                fade[axis]
            } else {
                1.0 - fade[axis]
            };
        }
        let corner_cell = [
            cell[0] + offset[0],
            cell[1] + offset[1],
            cell[2] + offset[2],
        ];
        let corner_gradient = gradient(corner_cell, salt);
        let mut dot = 0.0;
        for axis in 0..3 {
            dot += corner_gradient[axis] * (local[axis] - offset[axis] as f32);
        }
        accumulated += weight * dot;
    }
    (0.5 + 0.5 * accumulated).clamp(0.0, 1.0)
}

const SIMPLEX_SKEW: f32 = 0.333_333_33;
const SIMPLEX_UNSKEW: f32 = 0.166_666_67;

/// One simplex corner's contribution: the `(0.6 - r^2)^4` falloff.
/// Mirrors `pattern_simplex_corner`.
fn simplex_corner(offset: [f32; 3], cell: [i32; 3], salt: u32) -> f32 {
    let radius = offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2];
    let falloff = 0.6 - radius;
    if falloff <= 0.0 {
        return 0.0;
    }
    let squared = falloff * falloff;
    let corner_gradient = gradient(cell, salt);
    let dot = corner_gradient[0] * offset[0]
        + corner_gradient[1] * offset[1]
        + corner_gradient[2] * offset[2];
    squared * squared * dot
}

/// Gradient noise on the simplex lattice — four corners rather than eight.
/// Mirrors `pattern_simplex_noise`.
fn simplex_noise(point: [f32; 3], salt: u32) -> f32 {
    let skew = (point[0] + point[1] + point[2]) * SIMPLEX_SKEW;
    let skewed = [
        (point[0] + skew).floor(),
        (point[1] + skew).floor(),
        (point[2] + skew).floor(),
    ];
    let unskew = (skewed[0] + skewed[1] + skewed[2]) * SIMPLEX_UNSKEW;
    let offset0 = [
        point[0] - (skewed[0] - unskew),
        point[1] - (skewed[1] - unskew),
        point[2] - (skewed[2] - unskew),
    ];
    // Which of the six tetrahedra — rank the components, in exactly the branch
    // order the shader uses so the two pick the same one on a tie.
    let (step1, step2): ([f32; 3], [f32; 3]) = if offset0[0] >= offset0[1] {
        if offset0[1] >= offset0[2] {
            ([1.0, 0.0, 0.0], [1.0, 1.0, 0.0])
        } else if offset0[0] >= offset0[2] {
            ([1.0, 0.0, 0.0], [1.0, 0.0, 1.0])
        } else {
            ([0.0, 0.0, 1.0], [1.0, 0.0, 1.0])
        }
    } else if offset0[1] < offset0[2] {
        ([0.0, 0.0, 1.0], [0.0, 1.0, 1.0])
    } else if offset0[0] < offset0[2] {
        ([0.0, 1.0, 0.0], [0.0, 1.0, 1.0])
    } else {
        ([0.0, 1.0, 0.0], [1.0, 1.0, 0.0])
    };
    let cell = [skewed[0] as i32, skewed[1] as i32, skewed[2] as i32];
    let mut offset1 = [0.0f32; 3];
    let mut offset2 = [0.0f32; 3];
    let mut offset3 = [0.0f32; 3];
    for axis in 0..3 {
        offset1[axis] = offset0[axis] - step1[axis] + SIMPLEX_UNSKEW;
        offset2[axis] = offset0[axis] - step2[axis] + 2.0 * SIMPLEX_UNSKEW;
        offset3[axis] = offset0[axis] - 1.0 + 3.0 * SIMPLEX_UNSKEW;
    }
    let cell1 = [
        cell[0] + step1[0] as i32,
        cell[1] + step1[1] as i32,
        cell[2] + step1[2] as i32,
    ];
    let cell2 = [
        cell[0] + step2[0] as i32,
        cell[1] + step2[1] as i32,
        cell[2] + step2[2] as i32,
    ];
    let total = simplex_corner(offset0, cell, salt)
        + simplex_corner(offset1, cell1, salt)
        + simplex_corner(offset2, cell2, salt)
        + simplex_corner(offset3, [cell[0] + 1, cell[1] + 1, cell[2] + 1], salt);
    (0.5 + 16.0 * total).clamp(0.0, 1.0)
}

/// The four fractal families share one octave loop shape; only the per-octave
/// value differs. Mirrors `pattern_fractal_noise` / `_perlin` / `_simplex`,
/// `pattern_ridged_noise` and `pattern_turbulence`.
fn fractal<F: Fn([f32; 3], u32) -> f32>(
    point: [f32; 3],
    octaves: u32,
    salt_base: u32,
    octave_value: F,
) -> f32 {
    let mut frequency = 1.0;
    let mut amplitude = 1.0;
    let mut total = 0.0;
    let mut normalisation = 0.0;
    for octave in 0..octaves.clamp(1, MAX_NOISE_OCTAVES) {
        let scaled = [
            point[0] * frequency,
            point[1] * frequency,
            point[2] * frequency,
        ];
        total += amplitude * octave_value(scaled, salt_base ^ octave);
        normalisation += amplitude;
        frequency *= 2.0;
        amplitude *= 0.5;
    }
    total / normalisation
}

/// Ridged multifractal. Mirrors `pattern_ridged_noise`.
fn ridged_noise(point: [f32; 3], octaves: u32, salt_base: u32) -> f32 {
    fractal(point, octaves, salt_base, |scaled, salt| {
        let folded = 1.0 - (2.0 * value_noise(scaled, salt) - 1.0).abs();
        folded * folded
    })
    .clamp(0.0, 1.0)
}

/// Turbulence. Mirrors `pattern_turbulence`.
fn turbulence(point: [f32; 3], octaves: u32, salt_base: u32) -> f32 {
    fractal(point, octaves, salt_base, |scaled, salt| {
        (2.0 * value_noise(scaled, salt) - 1.0).abs()
    })
    .clamp(0.0, 1.0)
}

const WORLEY_JITTER_X_SALT: u32 = 21;
const WORLEY_JITTER_Y_SALT: u32 = 22;
const WORLEY_JITTER_Z_SALT: u32 = 23;
const WORLEY_SMOOTH_K: f32 = 6.0;
/// The nearest feature point is at most ~1.5 cells away in the worst case.
const WORLEY_RANGE: f32 = 1.5;

/// F1, F2 and the smooth minimum from ONE 27-cell walk, exactly as
/// `pattern_worley_distances` does it — three variants, one loop, one set of
/// jitter salts to get wrong.
fn worley_distances(point: [f32; 3], salt_base: u32) -> [f32; 3] {
    let base = [point[0].floor(), point[1].floor(), point[2].floor()];
    let cell = [base[0] as i32, base[1] as i32, base[2] as i32];
    let local = [point[0] - base[0], point[1] - base[1], point[2] - base[2]];
    let mut nearest = 1e9f32;
    let mut second = 1e9f32;
    let mut smooth_sum = 0.0f32;
    for index in 0..27u32 {
        let neighbour = [
            (index % 3) as i32 - 1,
            ((index / 3) % 3) as i32 - 1,
            (index / 9) as i32 - 1,
        ];
        let neighbour_cell = [
            cell[0] + neighbour[0],
            cell[1] + neighbour[1],
            cell[2] + neighbour[2],
        ];
        let jitter = [
            hash_cell(neighbour_cell, salt_base ^ WORLEY_JITTER_X_SALT),
            hash_cell(neighbour_cell, salt_base ^ WORLEY_JITTER_Y_SALT),
            hash_cell(neighbour_cell, salt_base ^ WORLEY_JITTER_Z_SALT),
        ];
        let mut squared = 0.0;
        for axis in 0..3 {
            let offset = neighbour[axis] as f32 + jitter[axis] - local[axis];
            squared += offset * offset;
        }
        if squared < nearest {
            second = nearest;
            nearest = squared;
        } else if squared < second {
            second = squared;
        }
        smooth_sum += (-WORLEY_SMOOTH_K * squared.sqrt()).exp();
    }
    [
        nearest.sqrt(),
        second.sqrt(),
        -smooth_sum.ln() / WORLEY_SMOOTH_K,
    ]
}

/// Bands along X, bent by noise. Mirrors `pattern_wave`.
fn wave(point: [f32; 3], distortion: f32, salt: u32) -> f32 {
    let mut coordinate = point[0] + point[1] * 0.25;
    if distortion > 0.0 {
        coordinate += distortion * (2.0 * value_noise(point, salt ^ 41) - 1.0);
    }
    let phase = coordinate - coordinate.floor();
    1.0 - (2.0 * phase - 1.0).abs()
}

/// Alternating lattice cells. Mirrors `pattern_checker`.
///
/// `&` on a negative `i32` is the same bitwise operation in both languages, so a
/// cell at a negative coordinate lands on the same colour on both sides.
fn checker(point: [f32; 3]) -> f32 {
    let cell = [
        point[0].floor() as i32,
        point[1].floor() as i32,
        point[2].floor() as i32,
    ];
    if (cell[0] + cell[1] + cell[2]) & 1 == 0 {
        1.0
    } else {
        0.0
    }
}

/// How many octaves are worth summing at a hit this far away — Tier 1b, mirroring
/// `pattern_octave_budget`.
///
/// Octave `k` carries detail at `period / 2^k` metres. Once that is under a pixel it
/// cannot be resolved and only aliases, so dropping it is quality-POSITIVE rather
/// than a trade. `footprint_meters` is the pixel footprint at the hit — the caller
/// multiplies distance by [`crate::camera::pixel_footprint_at_one_meter`] and by the
/// lever's scale.
///
/// Never returns zero: a distant layer softens toward its base frequency rather than
/// vanishing, because vanishing would pop and [`PatternLayer::fade`] already exists
/// for disappearing gracefully.
pub fn octave_budget(authored: u32, period_meters: f32, footprint_meters: f32) -> u32 {
    let footprint = footprint_meters.max(1e-6);
    let mut budget = 1;
    for octave in 1..authored {
        if period_meters / ((1u32 << octave) as f32) < footprint {
            break;
        }
        budget = octave + 1;
    }
    budget
}

const TILE_SALT: u32 = 61;

/// The tessellation: tile-local `u`/`v`, a per-tile hash and the distance to the
/// nearest edge, from one walk. Mirrors `pattern_tessellate`.
///
/// Returns `[u, v, tone, edge]`. See [`PatternFrame::Tile`] for why the courses are
/// bonded and why the gap is taken out of the tile's interior rather than added
/// around it.
fn tessellate(local: [f32; 2], aspect: f32, bond: f32, gap: f32) -> [f32; 4] {
    let scaled = [local[0] / aspect.max(1e-4), local[1]];
    let row = scaled[1].floor();
    let shifted_x = scaled[0] + row * bond;
    let column = shifted_x.floor();
    let cell = [shifted_x - column, scaled[1] - row];

    let tone = hash_cell([column as i32, row as i32, 0], TILE_SALT);

    let to_edge = cell[0].min(1.0 - cell[0]).min(cell[1].min(1.0 - cell[1]));
    let interior = (0.5 - gap).max(1e-4);
    let edge = ((to_edge - gap) / interior).clamp(0.0, 1.0);

    let span = (1.0 - 2.0 * gap).max(1e-4);
    [
        ((cell[0] - gap) / span).clamp(0.0, 1.0),
        ((cell[1] - gap) / span).clamp(0.0, 1.0),
        tone,
        edge,
    ]
}

/// The edge distance shaped from a bevel into a joint. Mirrors
/// `pattern_tile_edge_shaped`.
fn tile_edge_shaped(edge: f32, sharpness: f32) -> f32 {
    let amount = sharpness.clamp(0.0, 1.0);
    if amount <= 0.0 {
        return edge;
    }
    edge.powf(1.0 / (1.0 + 15.0 * amount))
}

/// The two world axes lying in a face. Mirrors `pattern_face_uv`.
fn face_uv(meters: [f32; 3], axis: u32) -> [f32; 2] {
    match axis {
        0 => [meters[2], meters[1]],
        1 => [meters[0], meters[2]],
        _ => [meters[0], meters[1]],
    }
}

const WARP_OFFSET_Y: [f32; 3] = [31.416, 7.913, 19.264];
const WARP_OFFSET_Z: [f32; 3] = [-13.077, 41.502, 5.731];
const WARP_SALT: u32 = 51;

/// Domain warping — displace the sample point by a noise field before the
/// generator reads it. Mirrors `pattern_warp`.
fn domain_warp(point: [f32; 3], strength: f32, salt: u32) -> [f32; 3] {
    if strength == 0.0 {
        return point;
    }
    let warp_salt = salt ^ WARP_SALT;
    let offset_y = [
        point[0] + WARP_OFFSET_Y[0],
        point[1] + WARP_OFFSET_Y[1],
        point[2] + WARP_OFFSET_Y[2],
    ];
    let offset_z = [
        point[0] + WARP_OFFSET_Z[0],
        point[1] + WARP_OFFSET_Z[1],
        point[2] + WARP_OFFSET_Z[2],
    ];
    let displacement = [
        value_noise(point, warp_salt),
        value_noise(offset_y, warp_salt),
        value_noise(offset_z, warp_salt),
    ];
    let mut warped = [0.0f32; 3];
    for axis in 0..3 {
        warped[axis] = point[axis] + (displacement[axis] * 2.0 - 1.0) * strength;
    }
    warped
}

/// Scattered round specks. See [`PatternGenerator::Speckle`].
fn speckle(point: [f32; 3], density: f32, salt_base: u32) -> f32 {
    let base = [point[0].floor(), point[1].floor(), point[2].floor()];
    let cell = [base[0] as i32, base[1] as i32, base[2] as i32];
    if hash_cell(cell, salt_base ^ SPECKLE_PRESENCE_SALT) >= density {
        return 0.0;
    }
    // The speck sits somewhere inside its cell rather than at the centre, or the
    // specks line up on the lattice and read as a grid.
    let centre = [
        0.25 + 0.5 * hash_cell(cell, salt_base ^ SPECKLE_JITTER_X_SALT),
        0.25 + 0.5 * hash_cell(cell, salt_base ^ SPECKLE_JITTER_Y_SALT),
        0.25 + 0.5 * hash_cell(cell, salt_base ^ SPECKLE_JITTER_Z_SALT),
    ];
    let offset = [
        point[0] - base[0] - centre[0],
        point[1] - base[1] - centre[1],
        point[2] - base[2] - centre[2],
    ];
    let distance = (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt();
    // Smooth rather than a hard disc, so a speck does not alias into a flickering
    // dot the moment it approaches a pixel in size.
    let edge = (1.0 - distance / SPECKLE_RADIUS_CELLS).clamp(0.0, 1.0);
    ease(edge)
}

const SPECKLE_PRESENCE_SALT: u32 = 11;
const SPECKLE_JITTER_X_SALT: u32 = 12;
const SPECKLE_JITTER_Y_SALT: u32 = 13;
const SPECKLE_JITTER_Z_SALT: u32 = 14;
/// A speck's radius as a fraction of its cell. Big enough to read, small enough
/// that neighbouring cells' specks stay separate at full density.
const SPECKLE_RADIUS_CELLS: f32 = 0.32;

impl PatternLayer {
    /// This layer's sample coordinate, in period units.
    ///
    /// The whole frame mechanism: put the position in the right space, snap it to the
    /// texel grid, divide by the period.
    ///
    /// `Voxel` quantises to the voxel centre, which is what makes the generator return
    /// one value for the whole voxel without any generator knowing about voxels — and
    /// is also why the texel snap is a no-op there, a centre already being one point.
    fn coordinate_animated(&self, sample: &PatternSample, drift_meters: [f32; 3]) -> [f32; 3] {
        let period = self.period_meters.max(MINIMUM_PERIOD_METERS);
        let detail_per_world = DETAIL_CELLS_PER_WORLD_VOXEL as i32;
        let world_voxel = [
            sample.voxel[0].div_euclid(detail_per_world),
            sample.voxel[1].div_euclid(detail_per_world),
            sample.voxel[2].div_euclid(detail_per_world),
        ];
        let drifted = [
            sample.world_meters[0] - drift_meters[0],
            sample.world_meters[1] - drift_meters[1],
            sample.world_meters[2] - drift_meters[2],
        ];
        let meters = match self.frame {
            PatternFrame::World => drifted,
            PatternFrame::Voxel => [
                (world_voxel[0] as f32 + 0.5) * WORLD_VOXEL_SIZE_METERS,
                (world_voxel[1] as f32 + 0.5) * WORLD_VOXEL_SIZE_METERS,
                (world_voxel[2] as f32 + 0.5) * WORLD_VOXEL_SIZE_METERS,
            ],
            // Local to the containing one-metre block. DDA's 0.125 m cell is an
            // implementation detail here, not a material-pattern boundary.
            // Drift applies HERE too, and it has to: lava is face-framed, so a
            // face frame that ignored drift would leave the one material this
            // feature exists for perfectly still. The block origin is not
            // re-derived from the drifted position, so the coordinate simply
            // scrolls out of the 0..1 tile rather than wrapping at the edge.
            PatternFrame::Face => [
                drifted[0] - world_voxel[0] as f32 * WORLD_VOXEL_SIZE_METERS,
                drifted[1] - world_voxel[1] as f32 * WORLD_VOXEL_SIZE_METERS,
                drifted[2] - world_voxel[2] as f32 * WORLD_VOXEL_SIZE_METERS,
            ],
            // Returns EARLY: the tile frame produces its coordinate from the
            // tessellation rather than from a remap-then-snap, and the texel snap
            // does not apply — the tile edge is already the quantisation.
            PatternFrame::Tile => {
                let tile = self.tile_at(sample, drift_meters);
                // The per-tile hash as the third coordinate, so every tile samples
                // its own slice of a 3D generator. See `pattern_coordinate`.
                return [tile[0], tile[1], tile[2] * 64.0];
            }
        };
        let snapped = self.snap_to_texels(meters);
        [
            snapped[0] / period,
            snapped[1] / period,
            snapped[2] / period,
        ]
    }

    /// Quantise a position in metres to the centre of its texel.
    ///
    /// The **centre**, not the corner: one sample per texel is the whole point, and
    /// the centre is the representative point of the cell. Sampling the corner would
    /// make an interpolating generator read the same lattice values that its
    /// neighbours read, correlating adjacent texels for no reason.
    ///
    /// The grid is anchored at world zero and divides one 1 m world voxel exactly
    /// (see [`TEXEL_RUNGS`]), which is what makes a texel never straddle a voxel edge
    /// in the world frame.
    fn snap_to_texels(&self, meters: [f32; 3]) -> [f32; 3] {
        if self.texels_per_voxel == 0 {
            return meters;
        }
        let texel =
            WORLD_VOXEL_SIZE_METERS / self.texels_per_voxel.min(MAX_TEXELS_PER_VOXEL) as f32;
        [
            (meters[0] / texel).floor() * texel + texel * 0.5,
            (meters[1] / texel).floor() * texel + texel * 0.5,
            (meters[2] / texel).floor() * texel + texel * 0.5,
        ]
    }

    /// How far this layer's pattern has drifted, in metres.
    ///
    /// Mirrors `pattern_drift_meters` in `shaders/pattern.wgsl`.
    ///
    /// The offset is QUANTISED TO THE TEXEL GRID, and there is deliberately no
    /// opt-out. The snap is applied to the coordinate AFTER the drift is
    /// subtracted, so the sampled value steps a whole texel at a time whether
    /// or not the offset itself was quantised — an un-quantised offset produces
    /// byte-identical output, and only costs the grid its alignment to world
    /// voxel boundaries. Genuinely continuous motion is what
    /// `texels_per_voxel = 0` already means: no texel grid, no snap, and drift
    /// falls through untouched by the branch above.
    pub fn drift_meters(&self, animation: LayerAnimationSample) -> [f32; 3] {
        if animation.drift_velocity == [0.0; 3] {
            return [0.0; 3];
        }
        let offset = animation
            .drift_velocity
            .map(|axis| axis * animation.time_seconds);
        if self.texels_per_voxel == 0 {
            return offset;
        }
        let texel = WORLD_VOXEL_SIZE_METERS / self.texels_per_voxel as f32;
        // Quantise toward zero so a newly-started negative drift does not
        // jump backwards by one whole texel while an equally small positive
        // drift remains at rest.
        offset.map(|axis| (axis / texel).trunc() * texel)
    }

    /// The generator's raw value at this sample, `0.0..=1.0`, before fade, amount,
    /// face mask or blend.
    pub fn generator_value(&self, sample: &PatternSample) -> f32 {
        self.generator_value_animated(sample, LayerAnimationSample::STILL)
    }

    /// The same, with the layer's pattern drifted by an animation sample.
    pub fn generator_value_animated(
        &self,
        sample: &PatternSample,
        animation: LayerAnimationSample,
    ) -> f32 {
        let drift_meters = self.drift_meters(animation);
        // The two tessellation readouts need the SAMPLE, not a mapped coordinate —
        // they report where the hit sits in the wall's tiling rather than evaluating
        // a field at a point. Mirrors the same early branch in
        // `pattern_generator_value`.
        match self.generator {
            PatternGenerator::TileTone => return self.tile_at(sample, drift_meters)[2],
            PatternGenerator::TileEdge { sharpness } => {
                return tile_edge_shaped(self.tile_at(sample, drift_meters)[3], sharpness)
            }
            _ => {}
        }
        let raw_point = self.coordinate_animated(sample, drift_meters);
        let salt = self.variation_salt(sample);
        self.raw_generator_value(raw_point, salt)
    }

    /// This layer's tessellation at this sample. Mirrors `pattern_tile_of`.
    ///
    /// World coordinates projected onto the hit face, not voxel-local: the
    /// tessellation is a property of the WALL, so courses continue across block
    /// boundaries and a tile may be larger than a voxel.
    pub fn tile_at(&self, sample: &PatternSample, drift_meters: [f32; 3]) -> [f32; 4] {
        let period = self.period_meters.max(MINIMUM_PERIOD_METERS);
        let drifted = [
            sample.world_meters[0] - drift_meters[0],
            sample.world_meters[1] - drift_meters[1],
            sample.world_meters[2] - drift_meters[2],
        ];
        let uv = face_uv(drifted, sample.axis);
        tessellate(
            [uv[0] / period, uv[1] / period],
            self.tile_aspect,
            self.tile_bond,
            self.tile_gap,
        )
    }

    /// The generator evaluated at an ALREADY-MAPPED coordinate — the warp and the
    /// generator dispatch, with the frame, drift and salt already resolved.
    ///
    /// Split out of [`Self::generator_value_animated`] so a test can exercise all
    /// twelve generators over chosen points without constructing a
    /// [`PatternSample`] per case, and so the mirror against
    /// `pattern_generator_value` is one function against one function.
    pub fn raw_generator_value(&self, raw_point: [f32; 3], salt: u32) -> f32 {
        let point = domain_warp(raw_point, self.domain_warp.max(0.0), salt);
        match self.generator {
            PatternGenerator::Flat => hash_cell(
                [
                    point[0].floor() as i32,
                    point[1].floor() as i32,
                    point[2].floor() as i32,
                ],
                salt ^ FLAT_SALT,
            ),
            PatternGenerator::Noise { octaves } => fractal_noise(point, octaves, salt),
            PatternGenerator::Speckle { density } => speckle(point, density.clamp(0.0, 1.0), salt),
            PatternGenerator::Perlin { octaves } => {
                fractal(point, octaves, salt, perlin_noise).clamp(0.0, 1.0)
            }
            PatternGenerator::Simplex { octaves } => {
                fractal(point, octaves, salt, simplex_noise).clamp(0.0, 1.0)
            }
            PatternGenerator::Ridged { octaves } => ridged_noise(point, octaves, salt),
            PatternGenerator::Turbulence { octaves } => turbulence(point, octaves, salt),
            PatternGenerator::Worley => {
                (worley_distances(point, salt)[0] / WORLEY_RANGE).clamp(0.0, 1.0)
            }
            PatternGenerator::WorleyEdge => {
                let distances = worley_distances(point, salt);
                (1.0 - (distances[1] - distances[0]) / WORLEY_RANGE).clamp(0.0, 1.0)
            }
            PatternGenerator::WorleySmooth => {
                (worley_distances(point, salt)[2].max(0.0) / WORLEY_RANGE).clamp(0.0, 1.0)
            }
            PatternGenerator::Wave { distortion } => wave(point, distortion.max(0.0), salt),
            PatternGenerator::Checker => checker(point),
            // Handled by `generator_value_animated`, which has the sample the
            // tessellation needs. Reaching here means a caller evaluated a tile
            // generator through the raw path, where there is no wall to tile.
            PatternGenerator::TileTone | PatternGenerator::TileEdge { .. } => 0.0,
        }
    }

    /// A per-face hash salt, or `0` for no variation.
    ///
    /// `0` is exactly the pre-variation behaviour, because every generator mixes this
    /// with `^` — which is what makes "variation off" provably identical rather than
    /// approximately so.
    ///
    /// Only the face frame gets one. The world frame must NOT (a per-face salt would
    /// destroy the continuity that is the entire point of it), and the voxel frame does
    /// not need one (it already returns a different value per voxel).
    fn variation_salt(&self, sample: &PatternSample) -> u32 {
        if !self.vary_per_face || self.frame != PatternFrame::Face {
            return 0;
        }
        let detail_per_world = DETAIL_CELLS_PER_WORLD_VOXEL as i32;
        let world_voxel = [
            sample.voxel[0].div_euclid(detail_per_world),
            sample.voxel[1].div_euclid(detail_per_world),
            sample.voxel[2].div_euclid(detail_per_world),
        ];
        // The face index, so the top and bottom of one world voxel differ as well
        // as neighbouring world voxels: 0..5 over (axis, sign).
        let face = sample.axis * 2 + u32::from(sample.axis_sign >= 0.0);
        hash_u32(
            (world_voxel[0] as u32).wrapping_mul(0x9e37_79b9)
                ^ (world_voxel[1] as u32).wrapping_mul(0x85eb_ca6b)
                ^ (world_voxel[2] as u32).wrapping_mul(0xc2b2_ae35)
                ^ face.wrapping_mul(0x27d4_eb2d),
        )
    }

    /// How much of this layer survives at this distance, `0.0..=1.0`.
    ///
    /// See [`PATTERN_FADE_START_METERS`]. The fade is applied to the *amount*, so a
    /// faded layer converges on the material's unpatterned base rather than on
    /// black or on grey.
    pub fn fade(&self, distance_meters: f32, fade_start_meters: f32, fade_end_meters: f32) -> f32 {
        if fade_end_meters <= 0.0 {
            return 1.0;
        }
        let start = fade_start_meters;
        let end = fade_end_meters.max(start);
        if distance_meters <= start {
            return 1.0;
        }
        if distance_meters >= end {
            return 0.0;
        }
        1.0 - ease((distance_meters - start) / (end - start))
    }

    /// The layer's effective strength at this sample: `amount`, faded, and zero on
    /// a face the mask excludes.
    pub fn strength(
        &self,
        sample: &PatternSample,
        fade_start_meters: f32,
        fade_end_meters: f32,
    ) -> f32 {
        self.strength_animated(
            sample,
            fade_start_meters,
            fade_end_meters,
            LayerAnimationSample::STILL,
        )
    }

    /// The same, scaled by a graph's animation gain.
    ///
    /// The gain MULTIPLIES the authored amount rather than replacing it, and is
    /// a separate value from it: an unconnected socket is 1.0, so nothing is
    /// applied twice and the authored number keeps its single meaning.
    pub fn strength_animated(
        &self,
        sample: &PatternSample,
        fade_start_meters: f32,
        fade_end_meters: f32,
        animation: LayerAnimationSample,
    ) -> f32 {
        if !self.faces.includes(sample.axis, sample.axis_sign) {
            return 0.0;
        }
        self.amount.clamp(0.0, 1.0)
            * animation.gain.max(0.0)
            * self.fade(sample.distance_meters, fade_start_meters, fade_end_meters)
    }

    /// Apply this layer to a colour target.
    pub fn apply_color(
        &self,
        base: [f32; 3],
        sample: &PatternSample,
        fade_start_meters: f32,
        fade_end_meters: f32,
    ) -> [f32; 3] {
        self.apply_color_animated(
            base,
            sample,
            fade_start_meters,
            fade_end_meters,
            LayerAnimationSample::STILL,
        )
    }

    /// The same, with a graph's animation gain and drift.
    pub fn apply_color_animated(
        &self,
        base: [f32; 3],
        sample: &PatternSample,
        fade_start_meters: f32,
        fade_end_meters: f32,
        animation: LayerAnimationSample,
    ) -> [f32; 3] {
        let strength =
            self.strength_animated(sample, fade_start_meters, fade_end_meters, animation);
        if strength <= 0.0 {
            return base;
        }
        let value = self.generator_value_animated(sample, animation);
        let target = self.target_value();
        let mut out = base;
        for channel in 0..3 {
            out[channel] = match self.blend {
                // `1 - strength` at value 0, `1` at value 1: the layer darkens where
                // its value is low and leaves the base alone where it is high, so
                // turning `amount` down converges on the base.
                PatternBlend::Multiply => base[channel] * (1.0 - strength * (1.0 - value)),
                PatternBlend::MixToColor => {
                    base[channel] + (target[channel] - base[channel]) * strength * value
                }
                PatternBlend::Add => base[channel] + target[channel] * strength * value,
            };
        }
        out
    }

    /// Apply this layer to a scalar target. `target_color`'s first channel is the
    /// target value, which is why the panel shows a single slider there instead of
    /// a colour picker.
    pub fn apply_scalar(
        &self,
        base: f32,
        sample: &PatternSample,
        fade_start_meters: f32,
        fade_end_meters: f32,
    ) -> f32 {
        self.apply_scalar_animated(
            base,
            sample,
            fade_start_meters,
            fade_end_meters,
            LayerAnimationSample::STILL,
        )
    }

    /// The same, with a graph's animation gain and drift.
    pub fn apply_scalar_animated(
        &self,
        base: f32,
        sample: &PatternSample,
        fade_start_meters: f32,
        fade_end_meters: f32,
        animation: LayerAnimationSample,
    ) -> f32 {
        let strength =
            self.strength_animated(sample, fade_start_meters, fade_end_meters, animation);
        if strength <= 0.0 {
            return base;
        }
        let value = self.generator_value_animated(sample, animation);
        let target = self.target_value();
        match self.blend {
            PatternBlend::Multiply => base * (1.0 - strength * (1.0 - value)),
            PatternBlend::MixToColor => base + (target[0] - base) * strength * value,
            PatternBlend::Add => base + target[0] * strength * value,
        }
    }
}

const FLAT_SALT: u32 = 31;

/// The whole stack applied to one target — the reference for what the shader's
/// loop does, including the order.
///
/// Layers apply **in slot order**, each on the previous one's output. So a mortar
/// mask followed by a grain layer grains the mortar too, and the reverse order does
/// not; that is a real authoring difference and the panel lets you reorder rather
/// than hiding it.
pub fn apply_stack_color(
    stack: &PatternStack,
    base: [f32; 3],
    target: PatternTarget,
    sample: &PatternSample,
    fade_start_meters: f32,
    fade_end_meters: f32,
    max_layers: usize,
) -> [f32; 3] {
    let mut out = base;
    for layer in stack.active().take(max_layers) {
        if layer.target == target {
            out = layer.apply_color(out, sample, fade_start_meters, fade_end_meters);
        }
    }
    out
}

/// The stack applied to a scalar target.
pub fn apply_stack_scalar(
    stack: &PatternStack,
    base: f32,
    target: PatternTarget,
    sample: &PatternSample,
    fade_start_meters: f32,
    fade_end_meters: f32,
    max_layers: usize,
) -> f32 {
    let mut out = base;
    for layer in stack.active().take(max_layers) {
        if layer.target == target {
            out = layer.apply_scalar(out, sample, fade_start_meters, fade_end_meters);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- S3: animation -------------------------------------------------------

    fn drift_sample(world_meters: [f32; 3]) -> PatternSample {
        PatternSample {
            world_meters,
            voxel: [0, 0, 0],
            axis: 1,
            axis_sign: -1.0,
            distance_meters: 0.0,
        }
    }

    /// The parity claim, stated as an equality rather than an eyeball: after one
    /// second at one texel per second, the drifted pattern equals the un-drifted
    /// pattern sampled one texel back.
    #[test]
    fn drifting_one_texel_per_second_shifts_the_pattern_by_exactly_one_texel() {
        let layer = PatternLayer {
            texels_per_voxel: 8,
            frame: PatternFrame::World,
            ..PatternLayer::IDENTITY
        };
        let texel = WORLD_VOXEL_SIZE_METERS / 8.0;
        let animation = LayerAnimationSample {
            gain: 1.0,
            drift_velocity: [texel, 0.0, 0.0],
            time_seconds: 1.0,
        };
        for step in 0..12 {
            let point = [0.3 + step as f32 * texel * 0.37, 0.5, 0.75];
            let drifted = layer.generator_value_animated(&drift_sample(point), animation);
            let shifted =
                layer.generator_value(&drift_sample([point[0] - texel, point[1], point[2]]));
            assert!(
                (drifted - shifted).abs() < 1e-6,
                "drifted {drifted} != shifted {shifted} at {point:?}"
            );
        }
    }

    /// Drift MARCHES in whole texels, and `texels_per_voxel = 0` is how you ask
    /// for continuous motion instead.
    ///
    /// There is no `smooth_drift` flag, and there was one until this test was
    /// written: `pattern_coordinate` snaps AFTER subtracting the offset, so an
    /// un-quantised offset produced byte-identical output. The flag cost a
    /// packed bit, a serde field and a shader branch to change nothing.
    #[test]
    fn drift_steps_by_whole_texels_and_a_continuous_field_moves_smoothly() {
        let texel = WORLD_VOXEL_SIZE_METERS / 8.0;
        let sample = drift_sample([12.3, 4.5, 6.7]);
        let velocity = [texel, 0.0, 0.0];
        let over_two_seconds = |layer: &PatternLayer| {
            let values: Vec<f32> = (0..=40)
                .map(|step| {
                    layer.generator_value_animated(
                        &sample,
                        LayerAnimationSample {
                            gain: 1.0,
                            drift_velocity: velocity,
                            time_seconds: step as f32 * 0.05,
                        },
                    )
                })
                .collect();
            let mut distinct = values.clone();
            distinct.dedup();
            distinct.len()
        };

        let snapped = PatternLayer {
            texels_per_voxel: 8,
            frame: PatternFrame::World,
            ..PatternLayer::IDENTITY
        };
        // Two seconds at one texel per second: a handful of steps, not a ramp.
        let steps = over_two_seconds(&snapped);
        assert!(
            (2..=4).contains(&steps),
            "expected a few whole-texel steps, got {steps} distinct values"
        );

        // The continuous field has no grid to snap to, so it moves every frame.
        let continuous = PatternLayer {
            texels_per_voxel: 0,
            ..snapped
        };
        assert_eq!(
            over_two_seconds(&continuous),
            41,
            "a continuous field must move on every sample"
        );

        // And the quantised offset is exactly a texel multiple.
        let quarter = LayerAnimationSample {
            gain: 1.0,
            drift_velocity: velocity,
            time_seconds: 0.25,
        };
        assert_eq!(snapped.drift_meters(quarter), [0.0; 3]);
        let full = LayerAnimationSample {
            time_seconds: 1.0,
            ..quarter
        };
        assert!((snapped.drift_meters(full)[0] - texel).abs() < 1e-7);
    }

    /// The lava contract, as an equality rather than an eyeball: after one
    /// texel of downward drift, every row holds the value the row ABOVE it held
    /// before. The texture is not regenerated, it is translated — which is what
    /// "keeping the same texture, just getting a new row" means.
    #[test]
    fn downward_drift_shifts_every_row_down_by_exactly_one() {
        let texel = WORLD_VOXEL_SIZE_METERS / 8.0;
        let layer = PatternLayer {
            frame: PatternFrame::World,
            texels_per_voxel: 8,
            period_meters: 0.25,
            ..PatternLayer::IDENTITY
        };
        let at = |height: f32, seconds: f32| {
            layer.generator_value_animated(
                &PatternSample {
                    world_meters: [3.3, height, 7.7],
                    voxel: [26, (height / WORLD_VOXEL_SIZE_METERS * 8.0) as i32, 61],
                    axis: 0,
                    axis_sign: 1.0,
                    distance_meters: 0.0,
                },
                LayerAnimationSample {
                    gain: 1.0,
                    // 0.25 m/s downward: exactly two texel rows per second, so
                    // one row per half second.
                    drift_velocity: [0.0, -0.25, 0.0],
                    time_seconds: seconds,
                },
            )
        };
        // A column of rows, one step apart in time and one texel apart in space.
        let mut moved = 0;
        for row in 0..12 {
            let height = 4.0 + row as f32 * texel;
            assert!(
                (at(height, 0.5) - at(height + texel, 0.0)).abs() < 1e-6,
                "row at {height} did not inherit the row above it"
            );
            if (at(height, 0.5) - at(height, 0.0)).abs() > 1e-6 {
                moved += 1;
            }
        }
        // ...and it genuinely moved. Without this the equality above would hold
        // just as well for a pattern that never changes.
        assert!(
            moved >= 8,
            "only {moved} of 12 rows changed — the pattern is barely moving"
        );
    }

    /// ...and the row scrolling in comes from the NEIGHBOURING block, not from
    /// a fresh per-block tile.
    ///
    /// This is what the world frame buys and the face frame cannot: the face
    /// frame is voxel-local and salts every face differently, so each block
    /// draws an unrelated pattern and a flow crossing a block boundary would
    /// visibly reset.
    #[test]
    fn the_world_frame_is_continuous_across_a_block_boundary() {
        let texel = WORLD_VOXEL_SIZE_METERS / 8.0;
        let world = PatternLayer {
            frame: PatternFrame::World,
            texels_per_voxel: 8,
            period_meters: 0.25,
            ..PatternLayer::IDENTITY
        };
        let face = PatternLayer {
            frame: PatternFrame::Face,
            vary_per_face: true,
            ..world
        };
        // The same world position, reached as the bottom row of the upper block
        // and as the top row of the lower one.
        let boundary = 5.0;
        let sample_in_block = |layer: &PatternLayer, block_y: i32, height: f32| {
            layer.generator_value(&PatternSample {
                world_meters: [3.3, height, 7.7],
                voxel: [26, block_y * 8, 61],
                axis: 0,
                axis_sign: 1.0,
                distance_meters: 0.0,
            })
        };
        // World frame: the block index does not enter the coordinate at all, so
        // the field is one field.
        assert_eq!(
            sample_in_block(&world, 4, boundary - texel * 0.5),
            sample_in_block(&world, 5, boundary - texel * 0.5),
        );
        // Face frame: the same position reads differently depending on which
        // block claims it — which is exactly why lava moved off it.
        assert_ne!(
            sample_in_block(&face, 4, boundary - texel * 0.5),
            sample_in_block(&face, 5, boundary - texel * 0.5),
        );
    }

    /// What a drift does to a face depends on how it lies RELATIVE to that
    /// face, and this is the table that says so.
    ///
    /// The drift translates a 3D field, so on any given face the component
    /// lying IN the face slides the pattern across it, while the component
    /// along the normal walks to a different slice of the field — which reads
    /// as the surface churning, not flowing. One world vector therefore cannot
    /// be a clean flow on every face at once.
    #[test]
    fn a_drift_slides_a_face_it_lies_in_and_churns_one_it_points_through() {
        let layer = PatternLayer {
            frame: PatternFrame::World,
            texels_per_voxel: 0, // continuous, so the check is about direction
            period_meters: 0.25,
            ..PatternLayer::IDENTITY
        };
        let value = |point: [f32; 3], velocity: [f32; 3], seconds: f32| {
            layer.generator_value_animated(
                &PatternSample {
                    world_meters: point,
                    voxel: [
                        (point[0] * 8.0) as i32,
                        (point[1] * 8.0) as i32,
                        (point[2] * 8.0) as i32,
                    ],
                    axis: 0,
                    axis_sign: 1.0,
                    distance_meters: 0.0,
                },
                LayerAnimationSample {
                    gain: 1.0,
                    drift_velocity: velocity,
                    time_seconds: seconds,
                },
            )
        };

        // Is the change over `dt` reproducible as a pure slide WITHIN the face
        // whose normal is `normal_axis`? It is exactly when the velocity has no
        // component along that normal.
        let slides_cleanly = |velocity: [f32; 3], normal_axis: usize| {
            let dt = 0.7;
            let mut in_plane = velocity;
            in_plane[normal_axis] = 0.0;
            (0..8).all(|step| {
                let mut point = [3.3, 4.4, 5.5];
                // Walk across the face, in one of its two in-plane axes.
                let across = (normal_axis + 1) % 3;
                point[across] += step as f32 * 0.11;
                let moved = [
                    point[0] - in_plane[0] * dt,
                    point[1] - in_plane[1] * dt,
                    point[2] - in_plane[2] * dt,
                ];
                (value(point, velocity, dt) - value(moved, velocity, 0.0)).abs() < 1e-6
            })
        };

        // Level flow along +Z: slides the top face and the X-facing walls,
        // churns the Z-facing walls it points straight through.
        let level_z = [0.0, 0.0, 0.25];
        assert!(slides_cleanly(level_z, 1), "top face should slide");
        assert!(slides_cleanly(level_z, 0), "X-facing wall should slide");
        assert!(!slides_cleanly(level_z, 2), "Z-facing wall should churn");

        // Straight down: slides both wall orientations, churns the top face —
        // which is why a lava LAKE wants a level flow and a lava FALL does not.
        let straight_down = [0.0, -0.25, 0.0];
        assert!(slides_cleanly(straight_down, 0));
        assert!(slides_cleanly(straight_down, 2));
        assert!(!slides_cleanly(straight_down, 1), "top face should churn");

        // The authored lava: down and across +Z. It lies entirely in the
        // X-facing walls, so those get a clean diagonal; the other two faces
        // each get a component through them.
        let diagonal = [0.0, -0.177, 0.177];
        assert!(
            slides_cleanly(diagonal, 0),
            "the X-facing wall is the face this setting is clean on"
        );
        assert!(!slides_cleanly(diagonal, 1));
        assert!(!slides_cleanly(diagonal, 2));
    }

    /// The double-apply regression. `animation_gain` is a SEPARATE value from
    /// the authored `amount`; an unconnected socket is 1.0, so the authored
    /// number must survive end to end rather than being squared.
    #[test]
    fn an_identity_animation_gain_leaves_the_authored_amount_untouched() {
        let layer = PatternLayer {
            amount: 0.5,
            target: PatternTarget::Albedo,
            blend: PatternBlend::Multiply,
            ..PatternLayer::IDENTITY
        };
        let sample = drift_sample([0.3, 0.5, 0.75]);
        assert_eq!(
            layer.strength_animated(&sample, 0.0, 0.0, LayerAnimationSample::STILL),
            layer.strength(&sample, 0.0, 0.0)
        );
        assert_eq!(layer.strength(&sample, 0.0, 0.0), 0.5);
        // Half the gain halves the strength — once, not twice.
        let halved = LayerAnimationSample {
            gain: 0.5,
            ..LayerAnimationSample::STILL
        };
        assert_eq!(layer.strength_animated(&sample, 0.0, 0.0, halved), 0.25);
    }

    /// Drift must work in the FACE frame, which is where it originally did not:
    /// the Rust mirror applied it in the world frame only, and lava — the one
    /// material this feature exists for — is face-framed, so the authored asset
    /// sat perfectly still while every synthetic test passed.
    #[test]
    fn drift_moves_a_face_framed_pattern() {
        let layer = PatternLayer {
            frame: PatternFrame::Face,
            texels_per_voxel: 8,
            ..PatternLayer::IDENTITY
        };
        let sample = PatternSample {
            world_meters: [12.3, 4.5, 6.7],
            voxel: [98, 36, 53],
            axis: 1,
            axis_sign: -1.0,
            distance_meters: 0.0,
        };
        let still = layer.generator_value(&sample);
        let drifted = layer.generator_value_animated(
            &sample,
            LayerAnimationSample {
                gain: 1.0,
                drift_velocity: [0.06, 0.0, 0.02],
                time_seconds: 8.0,
            },
        );
        assert_ne!(still, drifted, "a face-framed pattern did not drift");
    }

    /// The voxel frame quantises to one point per voxel, so there is no
    /// coordinate for drift to move. Documented as ignored rather than silently
    /// doing something surprising, and pinned so it stays that way.
    #[test]
    fn drift_is_inert_in_the_voxel_frame() {
        let layer = PatternLayer {
            frame: PatternFrame::Voxel,
            texels_per_voxel: 8,
            ..PatternLayer::IDENTITY
        };
        let sample = drift_sample([0.3, 0.5, 0.75]);
        let moving = LayerAnimationSample {
            gain: 1.0,
            drift_velocity: [1.0, 0.0, 0.0],
            time_seconds: 3.0,
        };
        assert_eq!(
            layer.generator_value_animated(&sample, moving),
            layer.generator_value(&sample)
        );
    }

    /// A sample looking at the top face of the voxel at the origin-ish, from 1 m.
    fn sample_at(world_meters: [f32; 3], voxel: [i32; 3]) -> PatternSample {
        PatternSample {
            world_meters,
            voxel,
            axis: 1,
            axis_sign: -1.0,
            distance_meters: 1.0,
        }
    }

    /// The GPU layer must be exactly two std430 rows with no implicit padding, or
    /// the whole uploaded material row shifts under it.
    #[test]
    fn the_gpu_layer_is_two_std430_rows() {
        assert_eq!(std::mem::size_of::<GpuPatternLayer>(), 48);
        assert_eq!(std::mem::align_of::<GpuPatternLayer>(), 4);
    }

    /// Every discriminant must survive the round trip into the packed word — the
    /// shader unpacks by the same shifts, so an overlap here would silently make one
    /// generator behave like another.
    #[test]
    fn the_packed_word_separates_every_field() {
        for generator in PatternGenerator::ALL {
            for frame in PatternFrame::ALL {
                for target in PatternTarget::ALL {
                    for blend in PatternBlend::ALL {
                        for faces in [PatternFaces::ALL, PatternFaces::TOP, PatternFaces::SIDES] {
                            let layer = PatternLayer {
                                generator,
                                frame,
                                target,
                                blend,
                                faces,
                                ..PatternLayer::IDENTITY
                            };
                            let packed = layer.packed();
                            assert_eq!(packed & 0xf, generator.code(), "generator");
                            assert_eq!((packed >> 4) & 0x3, frame.code(), "frame");
                            assert_eq!((packed >> 6) & 0x3, target.code(), "target");
                            assert_eq!((packed >> 8) & 0x3, blend.code(), "blend");
                            assert_eq!((packed >> 10) & 0x7, faces.bits(), "faces");
                        }
                    }
                }
            }
        }
    }

    /// The octave field must carry the count and must clamp, since the shader loops
    /// on it and an authored 99 would be 99 lattice fetches per hit.
    #[test]
    fn the_octave_field_carries_and_clamps() {
        for octaves in 1..=MAX_NOISE_OCTAVES {
            let layer = PatternLayer {
                generator: PatternGenerator::Noise { octaves },
                ..PatternLayer::IDENTITY
            };
            assert_eq!((layer.packed() >> 13) & 0x7, octaves);
        }
        let absurd = PatternLayer {
            generator: PatternGenerator::Noise { octaves: 99 },
            ..PatternLayer::IDENTITY
        };
        assert_eq!((absurd.packed() >> 13) & 0x7, MAX_NOISE_OCTAVES);
        // A non-noise generator must still write a usable octave count, because the
        // shader reads the field unconditionally.
        let flat = PatternLayer {
            generator: PatternGenerator::Flat,
            ..PatternLayer::IDENTITY
        };
        assert_eq!((flat.packed() >> 13) & 0x7, 1);
    }

    /// The texel snap must make the generator PIECEWISE CONSTANT on the grid — one
    /// value per texel, held flat across it. That is the whole feature: a voxel world
    /// should have square detail, not a smooth field that happens to sit on cubes.
    #[test]
    fn the_texel_snap_holds_one_value_across_each_texel() {
        let texels = 8;
        let texel = WORLD_VOXEL_SIZE_METERS / texels as f32;
        let layer = PatternLayer {
            generator: PatternGenerator::Noise { octaves: 3 },
            frame: PatternFrame::World,
            period_meters: 0.5,
            texels_per_voxel: texels,
            ..PatternLayer::IDENTITY
        };
        // Several points well inside the SAME texel must all read alike.
        let base = 64.0_f32;
        let inside: Vec<f32> = [0.05, 0.3, 0.5, 0.7, 0.95]
            .iter()
            .map(|offset| {
                let x = base + offset * texel;
                layer.generator_value(&sample_at([x, 32.0 + texel * 0.5, 64.0], [512, 256, 512]))
            })
            .collect();
        for value in &inside {
            assert_eq!(*value, inside[0], "the value varied within one texel");
        }
        // The NEXT texel must differ, or it is not a grid, it is a constant.
        let neighbour = layer.generator_value(&sample_at(
            [base + texel * 1.5, 32.0 + texel * 0.5, 64.0],
            [512, 256, 512],
        ));
        assert_ne!(neighbour, inside[0], "adjacent texels share a value");
    }

    /// The alignment property the whole snap rests on: a texel must never STRADDLE a
    /// voxel edge, so the blocky look and cross-voxel continuity are not a trade-off.
    ///
    /// It holds because every rung divides the voxel exactly and the grid is anchored
    /// at world zero. A rung of 3 or 5 would break it silently — hence the test over
    /// `TEXEL_RUNGS` rather than over one value.
    #[test]
    fn every_texel_rung_tiles_a_voxel_exactly() {
        for texels in TEXEL_RUNGS {
            if texels == 0 {
                continue;
            }
            assert!(
                texels.is_power_of_two(),
                "{texels} is not a power of two, so its texels drift off the voxel grid"
            );
            assert!(texels <= MAX_TEXELS_PER_VOXEL);
            // The texel size must divide one world voxel with no remainder.
            let texel = WORLD_VOXEL_SIZE_METERS / texels as f32;
            assert_eq!(
                texel * texels as f32,
                WORLD_VOXEL_SIZE_METERS,
                "{texels} texels do not reconstruct a voxel"
            );
            // A sample either side of a voxel boundary must land in DIFFERENT texels,
            // and the boundary must be a texel edge rather than an interior point.
            let layer = PatternLayer {
                texels_per_voxel: texels,
                ..PatternLayer::IDENTITY
            };
            let boundary = 64.0 * WORLD_VOXEL_SIZE_METERS;
            let below = layer.snap_to_texels([boundary - texel * 0.5, 0.0, 0.0])[0];
            let above = layer.snap_to_texels([boundary + texel * 0.5, 0.0, 0.0])[0];
            assert!(
                below < boundary && above > boundary,
                "a texel straddles the voxel edge at {texels} texels"
            );
        }
    }

    /// Zero texels must be exactly the continuous field — the pre-snap behaviour has
    /// to remain reachable, and it is what the fine-period layers still want.
    #[test]
    fn zero_texels_is_the_continuous_field() {
        let layer = PatternLayer {
            texels_per_voxel: 0,
            ..PatternLayer::IDENTITY
        };
        let meters = [64.321, 32.777, 64.999];
        assert_eq!(layer.snap_to_texels(meters), meters);
        // And a continuous generator must actually vary within a texel there.
        let noisy = PatternLayer {
            generator: PatternGenerator::Noise { octaves: 2 },
            period_meters: 0.5,
            texels_per_voxel: 0,
            ..PatternLayer::IDENTITY
        };
        let first = noisy.generator_value(&sample_at([64.30, 32.0, 64.0], [512, 256, 512]));
        let second = noisy.generator_value(&sample_at([64.31, 32.0, 64.0], [512, 256, 512]));
        assert_ne!(first, second, "the continuous field is not continuous");
    }

    /// The snap must survive the round trip into the packed word, and must clamp — the
    /// shader reads 8 bits and divides by the value.
    #[test]
    fn the_texel_count_packs_and_clamps() {
        for texels in TEXEL_RUNGS {
            let layer = PatternLayer {
                texels_per_voxel: texels,
                ..PatternLayer::IDENTITY
            };
            assert_eq!((layer.packed() >> 16) & 0xff, texels);
            // It must not disturb any other field: mask the texel bits off both sides
            // and everything else must be identical.
            const TEXEL_FIELD: u32 = 0xff << 16;
            let continuous = PatternLayer {
                texels_per_voxel: 0,
                ..layer
            };
            assert_eq!(
                layer.packed() & !TEXEL_FIELD,
                continuous.packed() & !TEXEL_FIELD
            );
        }
        let absurd = PatternLayer {
            texels_per_voxel: 9999,
            ..PatternLayer::IDENTITY
        };
        assert_eq!((absurd.packed() >> 16) & 0xff, MAX_TEXELS_PER_VOXEL);
    }

    #[test]
    fn the_default_layer_uses_two_centimetre_features() {
        assert_eq!(PatternLayer::IDENTITY.period_meters, DEFAULT_PERIOD_METERS);
        assert_eq!(DEFAULT_PERIOD_METERS, 0.02);
        assert_eq!(PatternLayer::DEFAULT.amount, 1.0);
        assert_eq!(
            PatternLayer::DEFAULT.texels_per_voxel,
            DEFAULT_TEXELS_PER_VOXEL
        );
    }

    /// A new layer must default to the texel grid, because that is what almost every
    /// layer wants and a default you have to switch on to look right is in the wrong
    /// place.
    #[test]
    fn a_new_layer_starts_on_the_texel_grid() {
        assert_eq!(
            PatternLayer::IDENTITY.texels_per_voxel,
            DEFAULT_TEXELS_PER_VOXEL
        );
        assert_eq!(DEFAULT_TEXELS_PER_VOXEL, 8);
        assert!(TEXEL_RUNGS.contains(&DEFAULT_TEXELS_PER_VOXEL));
        // Still the identity at amount zero, which is the other half of "safe to add".
        let sample = sample_at([64.3, 32.7, 64.9], [514, 262, 519]);
        assert_eq!(
            PatternLayer::IDENTITY.apply_color(
                [0.4, 0.5, 0.3],
                &sample,
                PATTERN_FADE_START_METERS,
                PATTERN_FADE_END_METERS,
            ),
            [0.4, 0.5, 0.3]
        );
    }

    /// **The face frame must not repeat across faces.** Without per-face variation it
    /// draws the identical pattern on every face in the world, which reads as a repeat
    /// rather than as detail — the hole the S2 gate found.
    #[test]
    fn the_face_frame_varies_per_face() {
        let layer = PatternLayer {
            generator: PatternGenerator::Noise { octaves: 3 },
            frame: PatternFrame::Face,
            period_meters: VOXEL_SIZE,
            vary_per_face: true,
            ..PatternLayer::IDENTITY
        };
        // The SAME point within the face of several different voxels.
        let at_voxel = |voxel: [i32; 3]| {
            layer.generator_value(&PatternSample {
                world_meters: [
                    (voxel[0] as f32 + 0.5) * VOXEL_SIZE,
                    (voxel[1] as f32 + 1.0) * VOXEL_SIZE,
                    (voxel[2] as f32 + 0.5) * VOXEL_SIZE,
                ],
                voxel,
                axis: 1,
                axis_sign: -1.0,
                distance_meters: 1.0,
            })
        };
        let first = at_voxel([512, 256, 512]);
        for neighbour in [
            [513, 256, 512],
            [512, 256, 513],
            [512, 257, 512],
            [520, 260, 530],
        ] {
            assert_ne!(
                at_voxel(neighbour),
                first,
                "{neighbour:?} repeats voxel [512,256,512]'s face"
            );
        }

        // The top and bottom of the SAME voxel must differ too, or a stack of blocks
        // shows the same face twice at every joint.
        let face_of = |axis: u32, sign: f32| {
            layer.generator_value(&PatternSample {
                world_meters: [64.03, 32.05, 64.07],
                voxel: [512, 256, 512],
                axis,
                axis_sign: sign,
                distance_meters: 1.0,
            })
        };
        let mut seen = Vec::new();
        for (axis, sign) in [
            (0, -1.0),
            (0, 1.0),
            (1, -1.0),
            (1, 1.0),
            (2, -1.0),
            (2, 1.0),
        ] {
            let value = face_of(axis, sign);
            assert!(
                !seen.contains(&value.to_bits()),
                "axis {axis} sign {sign} repeats another face of the same voxel"
            );
            seen.push(value.to_bits());
        }
        assert_eq!(seen.len(), 6, "the six faces are not six distinct draws");
    }

    /// Variation OFF must be exactly the unvaried pattern — the deliberate-motif case,
    /// the classic voxel look where every face of a block type is identical.
    ///
    /// `0` mixed with `^` is why this is exact rather than approximate.
    #[test]
    fn variation_off_repeats_deliberately() {
        let layer = PatternLayer {
            generator: PatternGenerator::Speckle { density: 0.4 },
            frame: PatternFrame::Face,
            period_meters: VOXEL_SIZE / 4.0,
            vary_per_face: false,
            ..PatternLayer::IDENTITY
        };
        let at_voxel = |voxel: [i32; 3]| {
            layer.generator_value(&PatternSample {
                world_meters: [
                    (voxel[0] as f32 + 0.37) * VOXEL_SIZE,
                    (voxel[1] as f32 + 1.0) * VOXEL_SIZE,
                    (voxel[2] as f32 + 0.62) * VOXEL_SIZE,
                ],
                voxel,
                axis: 1,
                axis_sign: -1.0,
                distance_meters: 1.0,
            })
        };
        let first = at_voxel([512, 256, 512]);
        for neighbour in [[513, 256, 512], [512, 256, 513], [700, 300, 700]] {
            assert_eq!(
                at_voxel(neighbour),
                first,
                "variation is off, so {neighbour:?} must show the same face"
            );
        }
        assert_eq!(layer.variation_salt(&sample_at([0.0; 3], [0; 3])), 0);
    }

    /// The variation must NOT touch the other two frames. World continuity is the whole
    /// point of that frame and a per-face salt would destroy it; the voxel frame already
    /// varies per voxel.
    #[test]
    fn variation_leaves_the_world_and_voxel_frames_alone() {
        for frame in [PatternFrame::World, PatternFrame::Voxel] {
            let varied = PatternLayer {
                generator: PatternGenerator::Noise { octaves: 2 },
                frame,
                period_meters: 0.5,
                vary_per_face: true,
                ..PatternLayer::IDENTITY
            };
            let unvaried = PatternLayer {
                vary_per_face: false,
                ..varied
            };
            for (axis, sign) in [(0, 1.0), (1, -1.0), (2, -1.0)] {
                let sample = PatternSample {
                    world_meters: [64.3, 32.7, 64.9],
                    voxel: [514, 262, 519],
                    axis,
                    axis_sign: sign,
                    distance_meters: 1.0,
                };
                assert_eq!(varied.variation_salt(&sample), 0, "{frame:?} got a salt");
                assert_eq!(
                    varied.generator_value(&sample),
                    unvaried.generator_value(&sample),
                    "{frame:?} changed with per-face variation"
                );
            }
        }
    }

    /// The emission split: a picker-friendly 0..1 colour and a separate brightness that
    /// can exceed 1, multiplied into the one value everything downstream reads.
    #[test]
    fn emission_intensity_scales_the_colour_and_nothing_else_does() {
        let orange = [1.0, 0.45, 0.1];
        let layer = PatternLayer {
            target: PatternTarget::Emission,
            blend: PatternBlend::Add,
            target_color: orange,
            emission_intensity: 8.0,
            amount: 1.0,
            ..PatternLayer::IDENTITY
        };
        // The product is what the row uploads and what the CPU reference reads.
        assert_eq!(layer.target_value(), [8.0, 3.6, 0.8]);
        assert_eq!(layer.to_gpu().target_color, [8.0, 3.6, 0.8]);
        // Which means it CAN exceed 1 — the whole point, since a picker cannot.
        assert!(layer.target_value()[0] > 1.0);
        // Intensity 1 is the identity, so the field is invisible until used.
        let plain = PatternLayer {
            emission_intensity: 1.0,
            ..layer
        };
        assert_eq!(plain.target_value(), orange);
        // Zero intensity emits nothing at all.
        let dark = PatternLayer {
            emission_intensity: 0.0,
            ..layer
        };
        assert_eq!(dark.target_value(), [0.0; 3]);
        // Clamped at the ceiling rather than trusted.
        let absurd = PatternLayer {
            emission_intensity: 1e6,
            ..layer
        };
        assert_eq!(absurd.target_value()[0], MAX_EMISSION_INTENSITY);
    }

    /// A NON-emission target must ignore the intensity entirely — an albedo above 1 is
    /// not a thing, and silently scaling one would blow out a colour the picker says is
    /// safe.
    #[test]
    fn only_an_emission_target_reads_the_intensity() {
        for target in [PatternTarget::Albedo, PatternTarget::Roughness] {
            let layer = PatternLayer {
                target,
                target_color: [0.5, 0.4, 0.3],
                emission_intensity: 16.0,
                ..PatternLayer::IDENTITY
            };
            assert_eq!(
                layer.target_value(),
                [0.5, 0.4, 0.3],
                "{target:?} scaled its target by the emission intensity"
            );
        }
    }

    /// It must not have taken a bit of the packed word — the whole reason it is folded
    /// into `target_color` on upload is that the shader needs no new field.
    #[test]
    fn the_emission_intensity_costs_no_packed_bits() {
        let dim = PatternLayer {
            target: PatternTarget::Emission,
            emission_intensity: 0.0,
            ..PatternLayer::IDENTITY
        };
        let bright = PatternLayer {
            emission_intensity: MAX_EMISSION_INTENSITY,
            ..dim
        };
        assert_eq!(dim.packed(), bright.packed());
        assert_eq!(std::mem::size_of::<GpuPatternLayer>(), 48);
    }

    /// The face mask must name the faces S1 names, including the inverted Y sign.
    #[test]
    fn the_face_mask_follows_s1s_sign_convention() {
        // axis 1 with a NEGATIVE sign is the +Y normal, i.e. the top.
        assert!(PatternFaces::TOP.includes(1, -1.0));
        assert!(!PatternFaces::TOP.includes(1, 1.0));
        assert!(!PatternFaces::TOP.includes(0, -1.0));
        assert!(!PatternFaces::TOP.includes(2, 1.0));

        assert!(PatternFaces::SIDES.includes(0, 1.0));
        assert!(PatternFaces::SIDES.includes(2, -1.0));
        assert!(!PatternFaces::SIDES.includes(1, -1.0));
        assert!(!PatternFaces::SIDES.includes(1, 1.0));

        for axis in 0..3 {
            for sign in [-1.0, 1.0] {
                assert!(PatternFaces::ALL.includes(axis, sign));
            }
        }
    }

    /// The stack's leading-`Some` invariant is what the shader's loop bound assumes.
    #[test]
    fn the_stack_stays_compacted() {
        let mut stack = PatternStack::of(&[
            PatternLayer {
                amount: 0.1,
                ..PatternLayer::IDENTITY
            },
            PatternLayer {
                amount: 0.2,
                ..PatternLayer::IDENTITY
            },
            PatternLayer {
                amount: 0.3,
                ..PatternLayer::IDENTITY
            },
        ]);
        assert_eq!(stack.active_count(), 3);

        // Removing from the middle must close the gap, not leave a hole.
        stack.remove(1);
        assert_eq!(stack.active_count(), 2);
        let amounts: Vec<f32> = stack.active().map(|layer| layer.amount).collect();
        assert_eq!(amounts, vec![0.1, 0.3]);

        // Filling up and pushing once more must hand the layer back rather than
        // dropping it silently.
        stack.push(PatternLayer::IDENTITY);
        stack.push(PatternLayer::IDENTITY);
        assert_eq!(stack.active_count(), MAX_PATTERN_LAYERS);
        assert!(stack.push(PatternLayer::IDENTITY).is_some());
        assert_eq!(stack.active_count(), MAX_PATTERN_LAYERS);
    }

    /// An empty stack must upload four inactive slots, and an inactive slot must be
    /// the identity — the belt to the layer count's braces.
    #[test]
    fn an_empty_stack_uploads_inactive_slots() {
        let slots = NO_PATTERNS.to_gpu();
        assert_eq!(slots.len(), MAX_PATTERN_LAYERS);
        for slot in slots {
            assert_eq!(slot, GpuPatternLayer::INACTIVE);
            assert_eq!(slot.amount, 0.0);
            // Never zero: the shader divides by it.
            assert!(slot.period_meters > 0.0);
        }
    }

    /// A zero period must not reach the shader, which divides by it.
    #[test]
    fn a_zero_period_is_floored_on_upload() {
        let layer = PatternLayer {
            period_meters: 0.0,
            ..PatternLayer::IDENTITY
        };
        assert_eq!(layer.to_gpu().period_meters, MINIMUM_PERIOD_METERS);
        // And the CPU evaluator must not produce a NaN either.
        let value = layer.generator_value(&sample_at([1.0, 2.0, 3.0], [8, 16, 24]));
        assert!(value.is_finite(), "a zero period produced {value}");
    }

    /// `amount: 0.0` must be the exact identity on every generator, blend and
    /// target. This is the property that makes a layer safe to leave in a row while
    /// dialling another, and it is also the CPU half of "the lever off is
    /// bit-identical".
    #[test]
    fn a_zero_amount_layer_changes_nothing() {
        let sample = sample_at([64.3, 32.7, 64.9], [514, 262, 519]);
        let base = [0.4, 0.5, 0.3];
        for generator in PatternGenerator::ALL {
            for blend in PatternBlend::ALL {
                for frame in PatternFrame::ALL {
                    let layer = PatternLayer {
                        generator,
                        blend,
                        frame,
                        amount: 0.0,
                        target_color: [1.0, 0.2, 0.7],
                        ..PatternLayer::IDENTITY
                    };
                    assert_eq!(
                        layer.apply_color(
                            base,
                            &sample,
                            PATTERN_FADE_START_METERS,
                            PATTERN_FADE_END_METERS
                        ),
                        base
                    );
                    assert_eq!(
                        layer.apply_scalar(
                            0.6,
                            &sample,
                            PATTERN_FADE_START_METERS,
                            PATTERN_FADE_END_METERS
                        ),
                        0.6
                    );
                }
            }
        }
    }

    /// Every generator must stay inside `0..=1` — the targets and blends are all
    /// written assuming it, and a generator that overshot would push an albedo out
    /// of range in a way that only shows up as a blown highlight.
    #[test]
    fn every_generator_stays_in_the_unit_range() {
        for generator in PatternGenerator::ALL {
            for frame in PatternFrame::ALL {
                let layer = PatternLayer {
                    generator,
                    frame,
                    period_meters: 0.2,
                    ..PatternLayer::IDENTITY
                };
                for step in 0..400 {
                    let t = step as f32 * 0.031;
                    let sample = PatternSample {
                        world_meters: [t, t * 0.7 + 0.3, t * 1.3 - 0.6],
                        voxel: [(t * 8.0) as i32, (t * 3.0) as i32 - 40, (t * 5.0) as i32],
                        axis: step % 3,
                        axis_sign: if step % 2 == 0 { -1.0 } else { 1.0 },
                        distance_meters: 1.0,
                    };
                    let value = layer.generator_value(&sample);
                    assert!(
                        (0.0..=1.0).contains(&value),
                        "{generator:?} in {frame:?} returned {value} at {t}"
                    );
                }
            }
        }
    }

    /// The whole point of the voxel frame: one value for the whole voxel, so a
    /// generator that varies continuously in world space becomes a flat tone.
    #[test]
    fn the_voxel_frame_is_constant_within_a_voxel() {
        let layer = PatternLayer {
            generator: PatternGenerator::Noise { octaves: 3 },
            frame: PatternFrame::Voxel,
            period_meters: 0.125,
            ..PatternLayer::IDENTITY
        };
        let world_voxel = [64, 32, 64];
        let detail = DETAIL_CELLS_PER_WORLD_VOXEL as i32;
        let detail_origin = [
            world_voxel[0] * detail,
            world_voxel[1] * detail,
            world_voxel[2] * detail,
        ];
        let mut seen: Option<f32> = None;
        // Every corner and the centre of the same one-metre voxel.
        for offset in [0.01, 0.5, 0.99] {
            let sample = sample_at(
                [
                    (world_voxel[0] as f32 + offset) * WORLD_VOXEL_SIZE_METERS,
                    (world_voxel[1] as f32 + offset) * WORLD_VOXEL_SIZE_METERS,
                    (world_voxel[2] as f32 + offset) * WORLD_VOXEL_SIZE_METERS,
                ],
                [
                    detail_origin[0] + (offset * detail as f32).floor() as i32,
                    detail_origin[1] + (offset * detail as f32).floor() as i32,
                    detail_origin[2] + (offset * detail as f32).floor() as i32,
                ],
            );
            let value = layer.generator_value(&sample);
            match seen {
                None => seen = Some(value),
                Some(first) => assert_eq!(value, first, "the voxel frame varied within one voxel"),
            }
        }
        // And the NEIGHBOUR must differ, or it is not a tone, it is a constant.
        let neighbour = sample_at(
            [
                (world_voxel[0] as f32 + 1.5) * WORLD_VOXEL_SIZE_METERS,
                (world_voxel[1] as f32 + 0.5) * WORLD_VOXEL_SIZE_METERS,
                (world_voxel[2] as f32 + 0.5) * WORLD_VOXEL_SIZE_METERS,
            ],
            [
                detail_origin[0] + detail,
                detail_origin[1],
                detail_origin[2],
            ],
        );
        assert_ne!(
            layer.generator_value(&neighbour),
            seen.expect("a value"),
            "neighbouring voxels got the same tone"
        );
    }

    /// The world frame's defining property for a CONTINUOUS layer: the value must not
    /// jump at a voxel boundary.
    ///
    /// Explicitly `texels_per_voxel: 0`, because the snap is on by default and a
    /// snapped layer is piecewise constant — it jumps at every texel edge, one of which
    /// sits exactly on the voxel boundary. That is not a continuity failure, and
    /// `the_world_frame_does_not_tile_per_voxel` is the property that survives the
    /// snap.
    #[test]
    fn a_continuous_world_layer_does_not_jump_at_a_voxel_boundary() {
        let layer = PatternLayer {
            generator: PatternGenerator::Noise { octaves: 2 },
            frame: PatternFrame::World,
            period_meters: 0.5,
            texels_per_voxel: 0,
            ..PatternLayer::IDENTITY
        };
        let boundary = 65.0 * WORLD_VOXEL_SIZE_METERS;
        let epsilon = 1e-4;
        let inside = sample_at([boundary - epsilon, 32.0, 64.0], [519, 256, 512]);
        let outside = sample_at([boundary + epsilon, 32.0, 64.0], [520, 256, 512]);
        let difference = (layer.generator_value(&inside) - layer.generator_value(&outside)).abs();
        assert!(
            difference < 1e-3,
            "the world frame jumped by {difference} at a voxel boundary"
        );
    }

    /// **The continuity property that survives the texel snap**, and the one the wall
    /// pose exists to show: the same offset WITHIN two different voxels must read
    /// differently.
    ///
    /// This is the sharp statement of "does not tile per voxel". Once a layer is
    /// quantised, smoothness across the boundary is gone by construction, so the thing
    /// that distinguishes a flowing field from a repeating tile is whether voxel N and
    /// voxel N+1 show the same arrangement. A tiled design would; a world-anchored grid
    /// does not, because the texel index keeps counting.
    #[test]
    fn the_world_frame_does_not_tile_per_voxel() {
        let layer = PatternLayer {
            generator: PatternGenerator::Noise { octaves: 2 },
            frame: PatternFrame::World,
            period_meters: 0.5,
            texels_per_voxel: DEFAULT_TEXELS_PER_VOXEL,
            ..PatternLayer::IDENTITY
        };
        // Walk the same intra-voxel offsets across eight neighbouring voxels and
        // collect the pattern each one shows.
        let texel = WORLD_VOXEL_SIZE_METERS / DEFAULT_TEXELS_PER_VOXEL as f32;
        let arrangement = |world_x: i32| -> Vec<f32> {
            (0..DEFAULT_TEXELS_PER_VOXEL)
                .map(|step| {
                    let x = world_x as f32 * WORLD_VOXEL_SIZE_METERS + (step as f32 + 0.5) * texel;
                    layer.generator_value(&sample_at(
                        [x, 32.0, 64.0],
                        [
                            world_x * DETAIL_CELLS_PER_WORLD_VOXEL as i32 + step as i32,
                            256,
                            512,
                        ],
                    ))
                })
                .collect()
        };
        let first = arrangement(64);
        // Every texel of a voxel must be a distinct sample, or the grid is coarser
        // than it claims.
        assert_eq!(first.len(), DEFAULT_TEXELS_PER_VOXEL as usize);
        for neighbour in 65..72 {
            assert_ne!(
                arrangement(neighbour),
                first,
                "voxel {neighbour} repeats voxel 64's arrangement — the layer tiles"
            );
        }
    }

    /// Noise must be hand-checkable, or the WGSL has nothing to be checked against.
    /// At an exact lattice point the eight-corner interpolation collapses to that
    /// corner's hash, which is the one value in the whole generator that can be
    /// computed by hand.
    #[test]
    fn value_noise_at_a_lattice_point_is_that_cells_hash() {
        for cell in [[0, 0, 0], [3, -7, 11], [512, 256, 512]] {
            let point = [cell[0] as f32, cell[1] as f32, cell[2] as f32];
            assert_eq!(value_noise(point, 0), hash_cell(cell, 0));
        }
        // One octave of fractal noise is therefore the same value.
        assert_eq!(
            fractal_noise([3.0, -7.0, 11.0], 1, 0),
            hash_cell([3, -7, 11], 0)
        );
    }

    /// EVERY generator must stay inside `0..=1`, because everything downstream —
    /// `pattern_apply_color`'s three blends, the CPU reference, the emission mean
    /// the GI injects — assumes it without checking. A generator that returned
    /// 1.4 would brighten an albedo past white rather than fail visibly.
    ///
    /// The sample points deliberately include negatives and lattice-exact values:
    /// negatives exercise the two's-complement hash reinterpretation, and exact
    /// lattice points are where the gradient generators evaluate to zero and the
    /// interpolations collapse to a corner.
    #[test]
    fn every_generator_stays_within_the_unit_range() {
        let points = [
            [0.0, 0.0, 0.0],
            [0.5, 0.5, 0.5],
            [3.0, -7.0, 11.0],
            [-0.25, -13.75, 4.5],
            [512.125, 256.0, 512.875],
            [1.0e3, -1.0e3, 7.5],
        ];
        for generator in PatternGenerator::ALL {
            for point in points {
                for salt in [0u32, 1, 0xdead_beef] {
                    let layer = PatternLayer {
                        generator,
                        ..PatternLayer::IDENTITY
                    };
                    let value = layer.raw_generator_value(point, salt);
                    assert!(
                        (0.0..=1.0).contains(&value),
                        "{generator:?} at {point:?} salt {salt} produced {value}"
                    );
                }
            }
        }
    }

    /// The cost band must never contradict the measurement it is derived from.
    ///
    /// Trivially true today because [`PatternGenerator::cost`] computes the band
    /// from the number — and that is the point of the test. It fails the moment
    /// someone hand-assigns a band, which is the shape this would rot into: a
    /// generator gets added, nobody re-runs section 11, and the panel starts
    /// claiming a cost nothing measured.
    #[test]
    fn every_cost_band_agrees_with_its_measurement() {
        let mut by_cost: Vec<PatternGenerator> = PatternGenerator::ALL.to_vec();
        by_cost.sort_by(|a, b| {
            a.measured_reference_milliseconds()
                .partial_cmp(&b.measured_reference_milliseconds())
                .expect("no NaN in the reference table")
        });
        // Bands are monotone in the measurement: sorted by milliseconds, the band
        // never goes backwards.
        for pair in by_cost.windows(2) {
            assert!(
                pair[0].cost() <= pair[1].cost(),
                "{:?} ({} ms, {:?}) bands above {:?} ({} ms, {:?})",
                pair[0],
                pair[0].measured_reference_milliseconds(),
                pair[0].cost(),
                pair[1],
                pair[1].measured_reference_milliseconds(),
                pair[1].cost(),
            );
        }
        // Every band is actually used, or the scale is finer than the data and one
        // of the four words is decoration.
        for band in [
            GeneratorCost::Free,
            GeneratorCost::Cheap,
            GeneratorCost::Moderate,
            GeneratorCost::Expensive,
        ] {
            assert!(
                PatternGenerator::ALL.iter().any(|g| g.cost() == band),
                "no generator lands in the {:?} band",
                band
            );
        }
        // And the anchors the UI's comparison text leans on.
        assert_eq!(PatternGenerator::Checker.cost(), GeneratorCost::Free);
        assert_eq!(
            PatternGenerator::Simplex { octaves: 3 }.cost(),
            GeneratorCost::Cheap
        );
        assert_eq!(
            PatternGenerator::Noise { octaves: 3 }.cost(),
            GeneratorCost::Moderate
        );
        assert_eq!(
            PatternGenerator::WorleyEdge.cost(),
            GeneratorCost::Expensive
        );
    }

    /// The bond is what separates masonry from a grid, so it gets its own test:
    /// with a half bond, the vertical joints of one course must NOT line up with
    /// the course above it.
    #[test]
    fn a_bonded_course_staggers_its_joints() {
        let stacked = tessellate([0.0, 0.5], 1.0, 0.0, 0.0);
        let above = tessellate([0.0, 1.5], 1.0, 0.0, 0.0);
        // No bond: the same u at the same offset in the next course up.
        assert!((stacked[0] - above[0]).abs() < 1e-6);

        let bonded = tessellate([0.0, 0.5], 1.0, 0.5, 0.0);
        let bonded_above = tessellate([0.0, 1.5], 1.0, 0.5, 0.0);
        // Half bond: the next course is offset by half a tile.
        assert!(
            (bonded[0] - bonded_above[0]).abs() > 0.4,
            "a half bond left the joints aligned: {} vs {}",
            bonded[0],
            bonded_above[0]
        );
    }

    /// The gap comes out of the tile's INTERIOR, so widening it must open the
    /// joints without moving the tiles. If the tiles slid, dragging the grout
    /// slider would shift the whole wall.
    #[test]
    fn widening_the_gap_does_not_move_the_tiles() {
        // A point just inside a tile's left edge falls in the joint once the gap
        // grows past it, but the TILE it belongs to must not change.
        for gap in [0.0, 0.05, 0.15, 0.3] {
            let inside = tessellate([2.5, 3.5], 1.0, 0.0, gap);
            // Dead centre of a tile is centre of the interior at every gap.
            assert!(
                (inside[0] - 0.5).abs() < 1e-5 && (inside[1] - 0.5).abs() < 1e-5,
                "tile centre moved at gap {gap}: {inside:?}"
            );
            // And the centre is maximally far from the edge, whatever the gap.
            assert!(
                (inside[3] - 1.0).abs() < 1e-5,
                "centre not at edge=1 at {gap}"
            );
        }
        // Inside the joint the edge output is zero.
        assert_eq!(tessellate([2.01, 3.5], 1.0, 0.0, 0.1)[3], 0.0);
    }

    /// Neighbouring tiles must draw different tones, or the whole point of the
    /// generator is lost — and the tone must be STABLE across a tile, or it is
    /// noise rather than masonry.
    #[test]
    fn tile_tone_is_constant_within_a_tile_and_differs_between_them() {
        let a = tessellate([2.3, 3.3], 1.0, 0.0, 0.0)[2];
        let same = tessellate([2.7, 3.7], 1.0, 0.0, 0.0)[2];
        assert_eq!(a, same, "tone varies within one tile");
        let right = tessellate([3.3, 3.3], 1.0, 0.0, 0.0)[2];
        let above = tessellate([2.3, 4.3], 1.0, 0.0, 0.0)[2];
        assert_ne!(a, right, "neighbouring tiles share a tone");
        assert_ne!(a, above, "stacked tiles share a tone");
    }

    /// Every output stays in range — the same guarantee every generator owes, and
    /// the tessellation feeds two of them.
    #[test]
    fn the_tessellation_stays_within_the_unit_range() {
        for aspect in [0.125f32, 1.0, 2.5, 8.0] {
            for bond in [0.0f32, 0.33, 0.5, 0.99] {
                for gap in [0.0f32, 0.06, 0.33] {
                    for step in 0..40 {
                        let coordinate = step as f32 * 0.37 - 7.0;
                        let tile = tessellate([coordinate, coordinate * 1.3], aspect, bond, gap);
                        for (index, value) in tile.iter().enumerate() {
                            assert!(
                                (0.0..=1.0).contains(value),
                                "output {index} = {value} outside 0..1 \
                                 at aspect {aspect} bond {bond} gap {gap}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Two properties of the twelve-code table that a new generator can silently
    /// break: a duplicate code makes two generators the same pattern on the GPU,
    /// and a code past 15 is truncated by the four-bit field into a DIFFERENT
    /// generator rather than into an error.
    #[test]
    fn generator_codes_are_unique_and_fit_the_packed_field() {
        let mut seen = Vec::new();
        for generator in PatternGenerator::ALL {
            let code = generator.code();
            assert!(code <= 0xf, "{generator:?} code {code} overflows four bits");
            assert!(!seen.contains(&code), "{generator:?} reuses code {code}");
            seen.push(code);
        }
        assert_eq!(seen.len(), PatternGenerator::ALL.len());
    }

    /// The generator field must survive the widening in both directions: a code
    /// above 7 has to come back intact (the three-bit field would have truncated
    /// it), and it must not disturb the frame sitting immediately above it.
    #[test]
    fn the_widened_generator_field_carries_every_code() {
        for generator in PatternGenerator::ALL {
            for frame in PatternFrame::ALL {
                let layer = PatternLayer {
                    generator,
                    frame,
                    ..PatternLayer::IDENTITY
                };
                let packed = layer.packed();
                assert_eq!(packed & 0xf, generator.code(), "{generator:?}");
                assert_eq!(
                    (packed >> 4) & 0x3,
                    frame.code(),
                    "{generator:?} / {frame:?}"
                );
            }
        }
    }

    /// Checker is the one generator with an exactly predictable value, so it is the
    /// one that can pin the lattice convention itself: cells alternate by the parity
    /// of their summed coordinates, and a NEGATIVE coordinate has to keep alternating
    /// rather than mirroring at the origin.
    #[test]
    fn checker_alternates_by_cell_parity_including_negatives() {
        assert_eq!(checker([0.5, 0.5, 0.5]), 1.0);
        assert_eq!(checker([1.5, 0.5, 0.5]), 0.0);
        assert_eq!(checker([-0.5, 0.5, 0.5]), 0.0);
        assert_eq!(checker([-1.5, 0.5, 0.5]), 1.0);
        assert_eq!(checker([-0.5, -0.5, -0.5]), 0.0);
    }

    /// A warp strength of zero must be the exact identity, not merely close to it.
    ///
    /// This is what makes the warp safe to ship on every generator node: the field
    /// defaults to `0.0`, so every existing material and every project saved before
    /// the warp existed produces byte-identical output.
    #[test]
    fn a_zero_domain_warp_is_the_exact_identity() {
        for point in [[0.0, 0.0, 0.0], [3.25, -7.5, 11.125], [-0.5, 0.5, -0.5]] {
            assert_eq!(domain_warp(point, 0.0, 0), point);
            assert_eq!(domain_warp(point, 0.0, 0xdead_beef), point);
        }
        // And through the full layer path, for every generator.
        for generator in PatternGenerator::ALL {
            let unwarped = PatternLayer {
                generator,
                domain_warp: 0.0,
                ..PatternLayer::IDENTITY
            };
            assert!(!bit(unwarped.packed(), 25), "{generator:?} warp bit set");
        }
        let warped = PatternLayer {
            domain_warp: 0.5,
            ..PatternLayer::IDENTITY
        };
        assert!(bit(warped.packed(), 25), "warp bit not set");
    }

    /// A non-zero warp must actually move the point, and must move it BOTH ways
    /// across the sample set — an uncentred warp would slide every point along the
    /// same diagonal, which reads as the pattern drifting rather than distorting.
    #[test]
    fn a_domain_warp_displaces_in_both_directions() {
        let points: Vec<[f32; 3]> = (0..64)
            .map(|index| {
                let step = index as f32 * 0.37;
                [step, step * 1.7 - 5.0, 11.0 - step * 0.9]
            })
            .collect();
        let mut moved_positive = false;
        let mut moved_negative = false;
        for point in &points {
            let warped = domain_warp(*point, 0.5, 0);
            for axis in 0..3 {
                let delta = warped[axis] - point[axis];
                assert!(delta.abs() <= 0.5 + 1e-6, "warp exceeded its strength");
                if delta > 1e-4 {
                    moved_positive = true;
                }
                if delta < -1e-4 {
                    moved_negative = true;
                }
            }
        }
        assert!(
            moved_positive && moved_negative,
            "warp is not centred on zero"
        );
    }

    /// The octave budget must be the identity when nothing is sub-pixel, must never
    /// drop below one octave, and must shed octaves as the hit recedes. The first
    /// property is the one that matters most: it is what makes the lever's OFF
    /// behaviour and its near-field behaviour identical.
    #[test]
    fn the_octave_budget_sheds_octaves_only_with_distance() {
        // A 0.25 m period at a 0.5 mm footprint: every octave is far above a pixel.
        assert_eq!(octave_budget(4, 0.25, 0.0005), 4);
        // Push the footprint up and the fine octaves go first, never below one.
        let near = octave_budget(4, 0.25, 0.01);
        let far = octave_budget(4, 0.25, 0.2);
        assert!(near >= far, "a further hit must not keep MORE octaves");
        assert_eq!(octave_budget(4, 0.25, 100.0), 1);
        assert_eq!(octave_budget(1, 0.25, 100.0), 1);
    }

    fn bit(word: u32, index: u32) -> bool {
        word & (1 << index) != 0
    }

    /// The hash's exact outputs, so a change to it is a failing test rather than a
    /// world that quietly looks different. These are the numbers the WGSL is
    /// checked against by hand.
    #[test]
    fn the_hash_is_pinned() {
        // Zero hashes to zero: `lowbias32` is a bijection with no additive term, so
        // it has a fixed point at 0. Harmless — a single lattice cell at the world
        // origin reads black-ish for one salt — and worth stating so it is not
        // mistaken for the hash failing to run.
        assert_eq!(hash_u32(0), 0);
        assert_eq!(hash_u32(1), 0x6889_90c0);
        assert_eq!(hash_u32(2), 0xd113_2181);
        assert_eq!(hash_u32(0xffff_ffff), 0x6768_824a);
        // Negative coordinates must reinterpret, not saturate — WGSL's
        // `bitcast<u32>(-1)` is 0xffffffff and Rust's `-1i32 as u32` must agree.
        assert_eq!((-1_i32) as u32, 0xffff_ffff);
        // And the cell hash must land in the unit range for wildly separated cells.
        for cell in [[0, 0, 0], [-1, -1, -1], [i32::MIN, 0, i32::MAX]] {
            let value = hash_cell(cell, 0);
            assert!((0.0..1.0).contains(&value), "{cell:?} hashed to {value}");
        }
    }

    /// Speckle's density knob must actually control how many cells carry a speck,
    /// and the extremes must be the extremes.
    #[test]
    fn speckle_density_controls_how_many_cells_carry_a_speck() {
        let hits_at = |density: f32| {
            let mut hits = 0;
            for x in 0..24 {
                for z in 0..24 {
                    // The jittered centre of each cell is the most likely point to
                    // be inside its speck, so sampling the cell centre undercounts;
                    // sample a small grid within the cell instead.
                    let mut cell_hit = false;
                    for sub_x in 0..4 {
                        for sub_z in 0..4 {
                            let point = [
                                x as f32 + sub_x as f32 * 0.25 + 0.125,
                                0.5,
                                z as f32 + sub_z as f32 * 0.25 + 0.125,
                            ];
                            if speckle(point, density, 0) > 0.0 {
                                cell_hit = true;
                            }
                        }
                    }
                    if cell_hit {
                        hits += 1;
                    }
                }
            }
            hits
        };
        assert_eq!(hits_at(0.0), 0, "density 0 must produce no specks at all");
        let sparse = hits_at(0.15);
        let dense = hits_at(0.85);
        assert!(
            sparse < dense,
            "density did not increase the speck count ({sparse} vs {dense})"
        );
        assert!(
            dense > sparse * 2,
            "density barely moved: {sparse} -> {dense}"
        );
    }

    /// The fade is one predictable world-space distance for every layer: identity
    /// up close, gone at twice the configured start, and monotone between.
    #[test]
    fn the_fade_uses_absolute_metres() {
        let grain = PatternLayer {
            period_meters: 0.02,
            ..PatternLayer::IDENTITY
        };
        let band = PatternLayer {
            period_meters: 1.0,
            ..PatternLayer::IDENTITY
        };
        // Both fine grain and broad bands obey the same camera distance.
        assert_eq!(
            grain.fade(0.5, PATTERN_FADE_START_METERS, PATTERN_FADE_END_METERS),
            1.0
        );
        assert_eq!(
            grain.fade(250.0, PATTERN_FADE_START_METERS, PATTERN_FADE_END_METERS),
            0.0
        );
        assert_eq!(
            band.fade(250.0, PATTERN_FADE_START_METERS, PATTERN_FADE_END_METERS),
            0.0
        );
        assert!((0.0..1.0).contains(&grain.fade(
            30.0,
            PATTERN_FADE_START_METERS,
            PATTERN_FADE_END_METERS
        )));

        // Monotone, with no step.
        let mut previous = 1.0;
        for step in 0..1600 {
            let distance = step as f32 * 0.15;
            let fade = grain.fade(distance, PATTERN_FADE_START_METERS, PATTERN_FADE_END_METERS);
            assert!(fade <= previous + 1e-6, "the fade rose at {distance} m");
            assert!((0.0..=1.0).contains(&fade));
            previous = fade;
        }

        // Zero metres disables the fade entirely, which is what the lever's
        // off position has to mean.
        assert_eq!(grain.fade(1e6, 0.0, 0.0), 1.0);
    }

    /// Multiply must only ever darken, and must converge on the base as the amount
    /// falls — the property that makes it the workhorse blend.
    #[test]
    fn multiply_only_darkens() {
        let sample = sample_at([64.3, 32.7, 64.9], [514, 262, 519]);
        let base = [0.5, 0.5, 0.5];
        for amount in [0.1, 0.5, 1.0] {
            let layer = PatternLayer {
                generator: PatternGenerator::Noise { octaves: 3 },
                amount,
                blend: PatternBlend::Multiply,
                ..PatternLayer::IDENTITY
            };
            let out = layer.apply_color(
                base,
                &sample,
                PATTERN_FADE_START_METERS,
                PATTERN_FADE_END_METERS,
            );
            for channel in 0..3 {
                assert!(
                    out[channel] <= base[channel] + 1e-6,
                    "multiply brightened at amount {amount}: {out:?}"
                );
                assert!(out[channel] >= 0.0);
            }
        }
    }

    /// The stack must apply in slot order and skip layers aimed at another target —
    /// the two things the shader's loop has to get right.
    #[test]
    fn the_stack_applies_in_order_and_only_to_its_target() {
        let sample = sample_at([64.3, 32.7, 64.9], [514, 262, 519]);
        let to_red = PatternLayer {
            generator: PatternGenerator::Flat,
            frame: PatternFrame::Voxel,
            target: PatternTarget::Albedo,
            blend: PatternBlend::MixToColor,
            amount: 1.0,
            target_color: [1.0, 0.0, 0.0],
            ..PatternLayer::IDENTITY
        };
        let to_blue = PatternLayer {
            target_color: [0.0, 0.0, 1.0],
            ..to_red
        };
        let roughness_layer = PatternLayer {
            target: PatternTarget::Roughness,
            ..to_red
        };

        let red_then_blue = PatternStack::of(&[to_red, to_blue]);
        let blue_then_red = PatternStack::of(&[to_blue, to_red]);
        let base = [0.5, 0.5, 0.5];
        let first = apply_stack_color(
            &red_then_blue,
            base,
            PatternTarget::Albedo,
            &sample,
            PATTERN_FADE_START_METERS,
            PATTERN_FADE_END_METERS,
            MAX_PATTERN_LAYERS,
        );
        let second = apply_stack_color(
            &blue_then_red,
            base,
            PatternTarget::Albedo,
            &sample,
            PATTERN_FADE_START_METERS,
            PATTERN_FADE_END_METERS,
            MAX_PATTERN_LAYERS,
        );
        assert_ne!(
            first, second,
            "the stack is order-independent, so it is wrong"
        );
        // The LAST mix dominates, since each mixes toward its own colour.
        assert!(first[2] > first[0], "blue did not land last");
        assert!(second[0] > second[2], "red did not land last");

        // A roughness layer must not touch albedo at all.
        let mixed_targets = PatternStack::of(&[roughness_layer]);
        assert_eq!(
            apply_stack_color(
                &mixed_targets,
                base,
                PatternTarget::Albedo,
                &sample,
                PATTERN_FADE_START_METERS,
                PATTERN_FADE_END_METERS,
                MAX_PATTERN_LAYERS,
            ),
            base
        );
    }

    /// The max-layer clamp — the Quest lever — must drop layers from the END, so
    /// lowering it degrades a material rather than changing which layers it has.
    #[test]
    fn the_max_layer_clamp_drops_the_tail() {
        let sample = sample_at([64.3, 32.7, 64.9], [514, 262, 519]);
        let mix = |color: [f32; 3]| PatternLayer {
            generator: PatternGenerator::Flat,
            frame: PatternFrame::Voxel,
            blend: PatternBlend::MixToColor,
            amount: 1.0,
            target_color: color,
            ..PatternLayer::IDENTITY
        };
        let stack = PatternStack::of(&[mix([1.0, 0.0, 0.0]), mix([0.0, 0.0, 1.0])]);
        let base = [0.5, 0.5, 0.5];
        let both = apply_stack_color(
            &stack,
            base,
            PatternTarget::Albedo,
            &sample,
            PATTERN_FADE_START_METERS,
            PATTERN_FADE_END_METERS,
            2,
        );
        let first_only = apply_stack_color(
            &stack,
            base,
            PatternTarget::Albedo,
            &sample,
            PATTERN_FADE_START_METERS,
            PATTERN_FADE_END_METERS,
            1,
        );
        let none = apply_stack_color(
            &stack,
            base,
            PatternTarget::Albedo,
            &sample,
            PATTERN_FADE_START_METERS,
            PATTERN_FADE_END_METERS,
            0,
        );
        assert_eq!(none, base, "clamping to zero must be the identity");
        assert!(
            first_only[0] > first_only[2],
            "the first layer was not kept"
        );
        assert!(both[2] > both[0], "the second layer was not kept");
    }

    /// A face-masked layer must apply to its faces and to no others — "moss on top"
    /// is the case, and getting it wrong paints the underside of an overhang.
    #[test]
    fn a_face_masked_layer_skips_the_other_faces() {
        let layer = PatternLayer {
            generator: PatternGenerator::Flat,
            frame: PatternFrame::Voxel,
            blend: PatternBlend::MixToColor,
            amount: 1.0,
            target_color: [0.0, 1.0, 0.0],
            faces: PatternFaces::TOP,
            ..PatternLayer::IDENTITY
        };
        let base = [0.5, 0.5, 0.5];
        let mut top = sample_at([64.3, 32.7, 64.9], [514, 262, 519]);
        top.axis = 1;
        top.axis_sign = -1.0;
        assert_ne!(
            layer.apply_color(
                base,
                &top,
                PATTERN_FADE_START_METERS,
                PATTERN_FADE_END_METERS
            ),
            base
        );

        for (axis, sign) in [(0, 1.0), (0, -1.0), (2, 1.0), (2, -1.0), (1, 1.0)] {
            let sample = PatternSample {
                axis,
                axis_sign: sign,
                ..top
            };
            assert_eq!(
                layer.apply_color(
                    base,
                    &sample,
                    PATTERN_FADE_START_METERS,
                    PATTERN_FADE_END_METERS
                ),
                base,
                "the top-only layer applied to axis {axis} sign {sign}"
            );
        }
    }
}
