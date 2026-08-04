//! Material graphs: the **recipe** half of the node system.
//!
//! ```text
//! picture:  [noise] ──┐                  recipe:  1. noise
//!                     ├─→ [mix] → [out]           2. red
//!           [red]  ───┘                           3. mix step 1 and step 2
//!                                                 4. output step 3
//!    ^ voxel-graph                                   ^ this crate
//! ```
//!
//! An editable [`GraphAsset`](voxel_graph::GraphAsset) is never evaluated directly. [`compile`]
//! turns it into a flat, ordered instruction list, and **two backends read that one list** —
//! the WGSL emitter and the CPU preview. That is the entire reason the list exists: if each
//! read the picture itself they would drift, and the editor's swatch would lie about the frame.
//!
//! # Layout
//!
//! | module | holds |
//! |---|---|
//! | [`operation`] | what a material node does, as a typed value |
//! | [`contract`] | what a material graph must contain |
//! | [`nodes`] | one file per node — 50 declarations |
//! | [`declare`] | the `socket!`/`node!` builders and shared field atoms |
//! | [`lowering`] | picture → instructions, then WGSL and the CPU evaluator |
//! | [`layers`] | projecting a graph onto the pattern-layer stack |
//! | [`cacheability`] | which layers can be evaluated once instead of per pixel per frame |
//!
//! # No `wgpu`
//!
//! This crate *generates* WGSL text and never compiles or binds any. That is the boundary, and
//! it is checkable: the day it needs a device, the split is in the wrong place.

// `#[macro_use]` for this crate's own node files (textual scope), `#[macro_export]` inside
// for other crates' catalogues. Both are needed; neither implies the other.
#[macro_use]
pub mod declare;

pub mod cacheability;
pub mod contract;
pub mod layers;
pub mod lowering;
pub mod nodes;
pub mod operation;

#[doc(hidden)]
pub mod test_support;

pub use contract::CONTRACTS;
pub use lowering::compile;

/// This crate's own catalogue: its 50 nodes and the material contract.
///
/// Enough to compile and validate a material graph standalone, which is what lets this crate
/// be tested without a renderer. `voxel-rt` builds a wider one that also carries its world
/// family — a graph validated against that one is validated against a superset, so nothing
/// here needs to know it exists.
pub const CATALOGUE: voxel_graph::NodeRegistry =
    voxel_graph::NodeRegistry::new(FAMILIES, CONTRACT_SETS);

/// Named `static`s because `NodeRegistry::new` borrows for `'static` and a `&[..]` literal
/// inside a `const` initialiser is a temporary.
static FAMILIES: &[&[voxel_graph::NodeDeclaration]] = &[NODES];
static CONTRACT_SETS: &[&[voxel_graph::GraphContractStatic]] = &[CONTRACTS];
pub use nodes::NODES;
pub use operation::MaterialNodeOperation;

/// The graph ABI a generated material function is injected into: shared helpers, then the
/// dispatch. `voxel-rt` splices these into its shading shader.
///
/// Exposed as consts rather than files so no consumer reaches across the crate boundary by
/// relative path — that mistake broke the CAGI shader once already when `voxel-environment`
/// reorganised its fragments.
pub const WGSL_PRELUDE: &str = include_str!("../shaders/graph_prelude.wgsl");
/// The dispatch the generated per-material functions are injected into.
pub const WGSL_DISPATCH: &str = include_str!("../shaders/material_graph.wgsl");
