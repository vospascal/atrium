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

use voxel_core::world::VOXEL_SIZE;

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

/// How many periods away a layer has faded out completely.
///
/// The aliasing story, and the reason it is derived rather than authored. A layer
/// crawls when its period shrinks below a pixel, and *when* that happens depends
/// entirely on the period: 2 cm grain is sub-pixel at a few metres while a 1 m band
/// never is. Authoring a fade distance per layer would mean re-deriving this by hand
/// for every layer and getting it wrong on most of them.
///
/// Note this keys off the PERIOD even when [`PatternLayer::texels_per_voxel`] snaps the
/// layer to a finer grid. Deliberate: the texels are hard edges, but a
/// piecewise-constant signal box-filters toward its local mean, so it is better
/// behaved under minification than the continuous field, not worse. Fading on texel
/// size would erase a 1 m band because its texels are small, which is the wrong
/// answer.
///
/// So the fade is expressed in **periods**, and the metric distance follows from
/// the layer. At 1080p with the shipped field of view a pixel covers roughly one
/// thousandth of its distance, so 250 periods puts the fade where a period spans
/// about four pixels — the point at which detail stops reading as detail and starts
/// reading as noise. That estimate is what the app run at the S2 gate checks; the
/// number is a lever precisely because a screen-space threshold cannot be settled
/// from a source file.
pub const PATTERN_FADE_PERIODS: f32 = 250.0;

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
}

impl PatternGenerator {
    /// The discriminant the GPU row carries.
    pub const fn code(&self) -> u32 {
        match self {
            PatternGenerator::Flat => 0,
            PatternGenerator::Noise { .. } => 1,
            PatternGenerator::Speckle { .. } => 2,
        }
    }

    /// Panel label.
    pub const fn label(&self) -> &'static str {
        match self {
            PatternGenerator::Flat => "flat (one value per cell)",
            PatternGenerator::Noise { .. } => "noise (grain / mottle)",
            PatternGenerator::Speckle { .. } => "speckle",
        }
    }

    /// Every generator, with representative parameters — what the panel offers as
    /// a starting point. "Generate, then hand-tune" is the authoring loop, so these
    /// are seeds and not presets.
    pub const ALL: [PatternGenerator; 3] = [
        PatternGenerator::Flat,
        PatternGenerator::Noise { octaves: 3 },
        PatternGenerator::Speckle { density: 0.25 },
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
    /// Voxel-local `u`/`v` within the hit face, so the pattern is *about* the face:
    /// wear concentrated toward an edge, a drip running down a side. Period `0.125`
    /// (one [`VOXEL_SIZE`]) spans exactly one face.
    Face,
}

impl PatternFrame {
    pub const fn code(&self) -> u32 {
        match self {
            PatternFrame::World => 0,
            PatternFrame::Voxel => 1,
            PatternFrame::Face => 2,
        }
    }

    pub const fn label(&self) -> &'static str {
        match self {
            PatternFrame::World => "world (continuous)",
            PatternFrame::Voxel => "voxel (per-voxel)",
            PatternFrame::Face => "face (within the face)",
        }
    }

    pub const ALL: [PatternFrame; 3] =
        [PatternFrame::World, PatternFrame::Voxel, PatternFrame::Face];
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
    /// The size of the generator's largest feature, in **metres**. One
    /// [`VOXEL_SIZE`] is 0.125, so this is the field that decides which of the four
    /// scales the layer is acting on.
    pub period_meters: f32,
    pub target: PatternTarget,
    pub blend: PatternBlend,
    /// How strongly the layer applies, `0.0..=1.0`. Zero is the identity, which is
    /// what makes a layer safe to leave in a row while dialling another one.
    pub amount: f32,
    /// The second colour, for [`PatternBlend::MixToColor`] and
    /// [`PatternBlend::Add`]. sRGB-encoded like [`crate::material::Material::albedo`],
    /// so the panel's picker and the row's own colour are the same kind of value.
    /// Only the first channel is read for a scalar target.
    pub target_color: [f32; 3],
    pub faces: PatternFaces,
    /// **Texels per voxel edge**, or `0` for a continuous field.
    ///
    /// The generator is sampled once per texel and held flat across it, so the result
    /// is piecewise constant on an `n x n` grid per face — the blocky look, rather
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
    /// is a large soft field rendered in 1.5 cm squares; 8 texels with a 0.125 m period
    /// is per-face detail. One field, both.
    ///
    /// **The grid is anchored to the world, not to the face**, so in
    /// [`PatternFrame::World`] it lines up across neighbouring voxels: the texel size
    /// divides [`VOXEL_SIZE`] exactly and world zero is a voxel boundary, so a texel
    /// never straddles a voxel edge. That is what keeps the blocky look and cross-voxel
    /// continuity from being a trade-off.
    ///
    /// Also an **anti-aliasing win**, which is the opposite of the intuition that hard
    /// edges alias worse: a piecewise-constant signal box-filters toward its local
    /// mean, where continuous noise at a sub-pixel period keeps producing new values
    /// per pixel. See [`PatternLayer::fade`] for why the fade still keys off the period
    /// regardless.
    pub texels_per_voxel: u32,
}

/// Texel-grid rungs the panel offers, and what the bench sweeps.
///
/// Powers of two only, so the texel size divides [`VOXEL_SIZE`] exactly and the grid
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

/// Ceiling on the texel grid. 32 per voxel edge is a 3.9 mm texel — already finer than
/// a pixel at arm's length, so past it the snap stops being visible and only costs the
/// two floors.
pub const MAX_TEXELS_PER_VOXEL: u32 = 32;

impl PatternLayer {
    /// A layer that changes nothing — the starting point the panel adds.
    pub const IDENTITY: PatternLayer = PatternLayer {
        generator: PatternGenerator::Noise { octaves: 3 },
        frame: PatternFrame::World,
        period_meters: 0.25,
        target: PatternTarget::Albedo,
        blend: PatternBlend::Multiply,
        amount: 0.0,
        target_color: [1.0, 1.0, 1.0],
        faces: PatternFaces::ALL,
        texels_per_voxel: DEFAULT_TEXELS_PER_VOXEL,
    };

    /// The uploaded form.
    pub fn to_gpu(&self) -> GpuPatternLayer {
        let (param_a, param_b) = self.params();
        GpuPatternLayer {
            packed: self.packed(),
            period_meters: self.period_meters.max(MINIMUM_PERIOD_METERS),
            amount: self.amount,
            param_a,
            target_color: self.target_color,
            param_b,
        }
    }

    /// The discriminants and the octave count, in one word.
    fn packed(&self) -> u32 {
        let octaves = match self.generator {
            PatternGenerator::Noise { octaves } => octaves.clamp(1, MAX_NOISE_OCTAVES),
            _ => 1,
        };
        self.generator.code()
            | (self.frame.code() << 3)
            | (self.target.code() << 5)
            | (self.blend.code() << 7)
            | (self.faces.bits() << 9)
            | (octaves << 12)
            | (self.texels_per_voxel.min(MAX_TEXELS_PER_VOXEL) << 15)
    }

    /// The two free generator parameters. Which generator reads which is the one
    /// place this packing is not self-describing, so it is stated in one function
    /// rather than spread across the shader.
    fn params(&self) -> (f32, f32) {
        match self.generator {
            PatternGenerator::Flat | PatternGenerator::Noise { .. } => (0.0, 0.0),
            PatternGenerator::Speckle { density } => (density.clamp(0.0, 1.0), 0.0),
        }
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

/// A row with no patterns at all — what 26 of 26 rows carry until S6.
pub const NO_PATTERNS: PatternStack = PatternStack {
    layers: [None; MAX_PATTERN_LAYERS],
};

impl PatternStack {
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

/// One uploaded layer: 32 bytes, two std430 16-byte rows.
///
/// The first row is four scalars, the second a `vec3` and the scalar filling its
/// `w` — the same discipline the rest of [`crate::material::GpuMaterial`] follows,
/// so std430 inserts no implicit padding and the Rust upload matches the WGSL byte
/// for byte.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuPatternLayer {
    /// Generator, frame, target, blend, face mask and octave count. See
    /// [`PatternLayer::packed`].
    pub packed: u32,
    pub period_meters: f32,
    pub amount: f32,
    pub param_a: f32,
    pub target_color: [f32; 3],
    pub param_b: f32,
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
fn fractal_noise(point: [f32; 3], octaves: u32) -> f32 {
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
        total += amplitude * value_noise(scaled, octave);
        normalisation += amplitude;
        frequency *= 2.0;
        amplitude *= 0.5;
    }
    total / normalisation
}

/// Scattered round specks. See [`PatternGenerator::Speckle`].
fn speckle(point: [f32; 3], density: f32) -> f32 {
    let base = [point[0].floor(), point[1].floor(), point[2].floor()];
    let cell = [base[0] as i32, base[1] as i32, base[2] as i32];
    if hash_cell(cell, SPECKLE_PRESENCE_SALT) >= density {
        return 0.0;
    }
    // The speck sits somewhere inside its cell rather than at the centre, or the
    // specks line up on the lattice and read as a grid.
    let centre = [
        0.25 + 0.5 * hash_cell(cell, SPECKLE_JITTER_X_SALT),
        0.25 + 0.5 * hash_cell(cell, SPECKLE_JITTER_Y_SALT),
        0.25 + 0.5 * hash_cell(cell, SPECKLE_JITTER_Z_SALT),
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
    fn coordinate(&self, sample: &PatternSample) -> [f32; 3] {
        let period = self.period_meters.max(MINIMUM_PERIOD_METERS);
        let meters = match self.frame {
            PatternFrame::World => sample.world_meters,
            PatternFrame::Voxel => [
                (sample.voxel[0] as f32 + 0.5) * VOXEL_SIZE,
                (sample.voxel[1] as f32 + 0.5) * VOXEL_SIZE,
                (sample.voxel[2] as f32 + 0.5) * VOXEL_SIZE,
            ],
            // Voxel-local, so the pattern repeats identically on every face — which
            // is what "about the face" means. The face axis keeps its own local
            // value rather than being zeroed, so a 3D generator still sees three
            // varying inputs on a face that happens to be flat in one of them.
            PatternFrame::Face => [
                (sample.world_meters[0] / VOXEL_SIZE - sample.voxel[0] as f32) * VOXEL_SIZE,
                (sample.world_meters[1] / VOXEL_SIZE - sample.voxel[1] as f32) * VOXEL_SIZE,
                (sample.world_meters[2] / VOXEL_SIZE - sample.voxel[2] as f32) * VOXEL_SIZE,
            ],
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
    /// The grid is anchored at world zero and its size divides [`VOXEL_SIZE`] exactly
    /// (see [`TEXEL_RUNGS`]), which is what makes a texel never straddle a voxel edge
    /// in the world frame.
    fn snap_to_texels(&self, meters: [f32; 3]) -> [f32; 3] {
        if self.texels_per_voxel == 0 {
            return meters;
        }
        let texel = VOXEL_SIZE / self.texels_per_voxel.min(MAX_TEXELS_PER_VOXEL) as f32;
        [
            (meters[0] / texel).floor() * texel + texel * 0.5,
            (meters[1] / texel).floor() * texel + texel * 0.5,
            (meters[2] / texel).floor() * texel + texel * 0.5,
        ]
    }

    /// The generator's raw value at this sample, `0.0..=1.0`, before fade, amount,
    /// face mask or blend.
    pub fn generator_value(&self, sample: &PatternSample) -> f32 {
        let point = self.coordinate(sample);
        match self.generator {
            PatternGenerator::Flat => hash_cell(
                [
                    point[0].floor() as i32,
                    point[1].floor() as i32,
                    point[2].floor() as i32,
                ],
                FLAT_SALT,
            ),
            PatternGenerator::Noise { octaves } => fractal_noise(point, octaves),
            PatternGenerator::Speckle { density } => speckle(point, density.clamp(0.0, 1.0)),
        }
    }

    /// How much of this layer survives at this distance, `0.0..=1.0`.
    ///
    /// See [`PATTERN_FADE_PERIODS`]. The fade is applied to the *amount*, so a
    /// faded layer converges on the material's unpatterned base rather than on
    /// black or on grey.
    pub fn fade(&self, distance_meters: f32, fade_periods: f32) -> f32 {
        if fade_periods <= 0.0 {
            return 1.0;
        }
        let period = self.period_meters.max(MINIMUM_PERIOD_METERS);
        let start = period * fade_periods;
        let end = start * 2.0;
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
    pub fn strength(&self, sample: &PatternSample, fade_periods: f32) -> f32 {
        if !self.faces.includes(sample.axis, sample.axis_sign) {
            return 0.0;
        }
        self.amount.clamp(0.0, 1.0) * self.fade(sample.distance_meters, fade_periods)
    }

    /// Apply this layer to a colour target.
    pub fn apply_color(
        &self,
        base: [f32; 3],
        sample: &PatternSample,
        fade_periods: f32,
    ) -> [f32; 3] {
        let strength = self.strength(sample, fade_periods);
        if strength <= 0.0 {
            return base;
        }
        let value = self.generator_value(sample);
        let mut out = base;
        for channel in 0..3 {
            out[channel] = match self.blend {
                // `1 - strength` at value 0, `1` at value 1: the layer darkens where
                // its value is low and leaves the base alone where it is high, so
                // turning `amount` down converges on the base.
                PatternBlend::Multiply => base[channel] * (1.0 - strength * (1.0 - value)),
                PatternBlend::MixToColor => {
                    base[channel] + (self.target_color[channel] - base[channel]) * strength * value
                }
                PatternBlend::Add => base[channel] + self.target_color[channel] * strength * value,
            };
        }
        out
    }

    /// Apply this layer to a scalar target. `target_color`'s first channel is the
    /// target value, which is why the panel shows a single slider there instead of
    /// a colour picker.
    pub fn apply_scalar(&self, base: f32, sample: &PatternSample, fade_periods: f32) -> f32 {
        let strength = self.strength(sample, fade_periods);
        if strength <= 0.0 {
            return base;
        }
        let value = self.generator_value(sample);
        match self.blend {
            PatternBlend::Multiply => base * (1.0 - strength * (1.0 - value)),
            PatternBlend::MixToColor => base + (self.target_color[0] - base) * strength * value,
            PatternBlend::Add => base + self.target_color[0] * strength * value,
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
    fade_periods: f32,
    max_layers: usize,
) -> [f32; 3] {
    let mut out = base;
    for layer in stack.active().take(max_layers) {
        if layer.target == target {
            out = layer.apply_color(out, sample, fade_periods);
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
    fade_periods: f32,
    max_layers: usize,
) -> f32 {
    let mut out = base;
    for layer in stack.active().take(max_layers) {
        if layer.target == target {
            out = layer.apply_scalar(out, sample, fade_periods);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(std::mem::size_of::<GpuPatternLayer>(), 32);
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
                            assert_eq!(packed & 0x7, generator.code(), "generator");
                            assert_eq!((packed >> 3) & 0x3, frame.code(), "frame");
                            assert_eq!((packed >> 5) & 0x3, target.code(), "target");
                            assert_eq!((packed >> 7) & 0x3, blend.code(), "blend");
                            assert_eq!((packed >> 9) & 0x7, faces.bits(), "faces");
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
            assert_eq!((layer.packed() >> 12) & 0x7, octaves);
        }
        let absurd = PatternLayer {
            generator: PatternGenerator::Noise { octaves: 99 },
            ..PatternLayer::IDENTITY
        };
        assert_eq!((absurd.packed() >> 12) & 0x7, MAX_NOISE_OCTAVES);
        // A non-noise generator must still write a usable octave count, because the
        // shader reads the field unconditionally.
        let flat = PatternLayer {
            generator: PatternGenerator::Flat,
            ..PatternLayer::IDENTITY
        };
        assert_eq!((flat.packed() >> 12) & 0x7, 1);
    }

    /// The texel snap must make the generator PIECEWISE CONSTANT on the grid — one
    /// value per texel, held flat across it. That is the whole feature: a voxel world
    /// should have square detail, not a smooth field that happens to sit on cubes.
    #[test]
    fn the_texel_snap_holds_one_value_across_each_texel() {
        let texels = 8;
        let texel = VOXEL_SIZE / texels as f32;
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
            // The texel size must divide VOXEL_SIZE with no remainder, exactly in f32.
            let texel = VOXEL_SIZE / texels as f32;
            assert_eq!(
                texel * texels as f32,
                VOXEL_SIZE,
                "{texels} texels do not reconstruct a voxel"
            );
            // A sample either side of a voxel boundary must land in DIFFERENT texels,
            // and the boundary must be a texel edge rather than an interior point.
            let layer = PatternLayer {
                texels_per_voxel: texels,
                ..PatternLayer::IDENTITY
            };
            let boundary = 512.0 * VOXEL_SIZE;
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
            assert_eq!((layer.packed() >> 15) & 0xff, texels);
            // It must not collide with any field below it.
            assert_eq!(layer.packed() & 0x7fff, {
                let continuous = PatternLayer {
                    texels_per_voxel: 0,
                    ..layer
                };
                continuous.packed()
            });
        }
        let absurd = PatternLayer {
            texels_per_voxel: 9999,
            ..PatternLayer::IDENTITY
        };
        assert_eq!((absurd.packed() >> 15) & 0xff, MAX_TEXELS_PER_VOXEL);
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
            PatternLayer::IDENTITY.apply_color([0.4, 0.5, 0.3], &sample, PATTERN_FADE_PERIODS),
            [0.4, 0.5, 0.3]
        );
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
                    assert_eq!(layer.apply_color(base, &sample, PATTERN_FADE_PERIODS), base);
                    assert_eq!(layer.apply_scalar(0.6, &sample, PATTERN_FADE_PERIODS), 0.6);
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
        let voxel = [514, 260, 519];
        let mut seen: Option<f32> = None;
        // Every corner and the centre of the same voxel.
        for offset in [0.01, 0.5, 0.99] {
            let sample = sample_at(
                [
                    (voxel[0] as f32 + offset) * VOXEL_SIZE,
                    (voxel[1] as f32 + offset) * VOXEL_SIZE,
                    (voxel[2] as f32 + offset) * VOXEL_SIZE,
                ],
                voxel,
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
                (voxel[0] as f32 + 1.5) * VOXEL_SIZE,
                (voxel[1] as f32 + 0.5) * VOXEL_SIZE,
                (voxel[2] as f32 + 0.5) * VOXEL_SIZE,
            ],
            [voxel[0] + 1, voxel[1], voxel[2]],
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
        let boundary = 515.0 * VOXEL_SIZE;
        let epsilon = 1e-4;
        let inside = sample_at([boundary - epsilon, 32.0, 64.0], [514, 256, 512]);
        let outside = sample_at([boundary + epsilon, 32.0, 64.0], [515, 256, 512]);
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
        let texel = VOXEL_SIZE / DEFAULT_TEXELS_PER_VOXEL as f32;
        let arrangement = |voxel_x: i32| -> Vec<f32> {
            (0..DEFAULT_TEXELS_PER_VOXEL)
                .map(|step| {
                    let x = voxel_x as f32 * VOXEL_SIZE + (step as f32 + 0.5) * texel;
                    layer.generator_value(&sample_at([x, 32.0, 64.0], [voxel_x, 256, 512]))
                })
                .collect()
        };
        let first = arrangement(512);
        // Every texel of a voxel must be a distinct sample, or the grid is coarser
        // than it claims.
        assert_eq!(first.len(), DEFAULT_TEXELS_PER_VOXEL as usize);
        for neighbour in 513..520 {
            assert_ne!(
                arrangement(neighbour),
                first,
                "voxel {neighbour} repeats voxel 512's arrangement — the layer tiles"
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
            fractal_noise([3.0, -7.0, 11.0], 1),
            hash_cell([3, -7, 11], 0)
        );
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
                            if speckle(point, density) > 0.0 {
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

    /// The fade must be the identity up close, gone at range, and monotone between —
    /// and it must scale with the period, which is the reason it is derived.
    #[test]
    fn the_fade_scales_with_the_period() {
        let grain = PatternLayer {
            period_meters: 0.02,
            ..PatternLayer::IDENTITY
        };
        let band = PatternLayer {
            period_meters: 1.0,
            ..PatternLayer::IDENTITY
        };
        // 2 cm grain: full strength at arm's length, gone by ~10 m.
        assert_eq!(grain.fade(0.5, PATTERN_FADE_PERIODS), 1.0);
        assert_eq!(grain.fade(100.0, PATTERN_FADE_PERIODS), 0.0);
        // A 1 m band at the same 100 m is untouched — the whole point.
        assert_eq!(band.fade(100.0, PATTERN_FADE_PERIODS), 1.0);

        // Monotone, with no step.
        let mut previous = 1.0;
        for step in 0..200 {
            let distance = step as f32 * 0.15;
            let fade = grain.fade(distance, PATTERN_FADE_PERIODS);
            assert!(fade <= previous + 1e-6, "the fade rose at {distance} m");
            assert!((0.0..=1.0).contains(&fade));
            previous = fade;
        }

        // Zero periods disables the fade entirely, which is what the lever's
        // off position has to mean.
        assert_eq!(grain.fade(1e6, 0.0), 1.0);
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
            let out = layer.apply_color(base, &sample, PATTERN_FADE_PERIODS);
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
            PATTERN_FADE_PERIODS,
            MAX_PATTERN_LAYERS,
        );
        let second = apply_stack_color(
            &blue_then_red,
            base,
            PatternTarget::Albedo,
            &sample,
            PATTERN_FADE_PERIODS,
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
                PATTERN_FADE_PERIODS,
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
            PATTERN_FADE_PERIODS,
            2,
        );
        let first_only = apply_stack_color(
            &stack,
            base,
            PatternTarget::Albedo,
            &sample,
            PATTERN_FADE_PERIODS,
            1,
        );
        let none = apply_stack_color(
            &stack,
            base,
            PatternTarget::Albedo,
            &sample,
            PATTERN_FADE_PERIODS,
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
        assert_ne!(layer.apply_color(base, &top, PATTERN_FADE_PERIODS), base);

        for (axis, sign) in [(0, 1.0), (0, -1.0), (2, 1.0), (2, -1.0), (1, 1.0)] {
            let sample = PatternSample {
                axis,
                axis_sign: sign,
                ..top
            };
            assert_eq!(
                layer.apply_color(base, &sample, PATTERN_FADE_PERIODS),
                base,
                "the top-only layer applied to axis {axis} sign {sign}"
            );
        }
    }
}
