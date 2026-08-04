//! The pattern generators' math, one file per generator.
//!
//! Split out of `pattern.rs`, which held the data model, ~30 items of generator math and the
//! evaluation in one 3847-line file. The model and the evaluation stayed; this is the math.
//!
//! Every generator here is deterministic in `(point, salt)` and has a WGSL twin in
//! `voxel-rt/shaders/pattern.wgsl`. That pairing is the reason these are worth isolating: a
//! CPU/GPU mismatch shows up as a preview swatch that lies about the rendered surface, so each
//! function is small enough to read against its shader counterpart side by side.
//!
//! `hash` is the shared floor — every other file interpolates with its easing curves.

pub(crate) mod checker;
pub(crate) mod fractal;
pub(crate) mod hash;
pub(crate) mod perlin;
pub(crate) mod simplex;
pub(crate) mod speckle;
pub(crate) mod tile;
pub(crate) mod value;
pub(crate) mod warp;
pub(crate) mod wave;
pub(crate) mod worley;
