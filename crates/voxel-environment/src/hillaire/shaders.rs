//! WGSL assembly boundary for the Jolifanto provider.
//!
//! Both modules are assembled from per-implementation fragments so that each has a
//! visible, independently editable source file while the renderer still receives one WGSL
//! module. `concat!` of `include_str!` keeps the result a `&'static str` const with the
//! fragments as the only source of truth — there is no hand-maintained aggregate file to
//! drift, and no build script needed to produce one.

/// The LUT-generation module. One file per lookup table, plus shared helpers.
pub const LUT_WGSL: &str = concat!(
    include_str!("../../shaders/lut/common.wgsl"),
    include_str!("../../shaders/lut/transmittance.wgsl"),
    include_str!("../../shaders/lut/multiple_scattering.wgsl"),
    include_str!("../../shaders/lut/sky_view.wgsl"),
    include_str!("../../shaders/lut/aerial_perspective.wgsl"),
);

/// The sampling module consumed by DDA, water and CAGI. It contains no per-camera ray
/// march; all atmosphere work is a lookup into the generated textures.
///
/// Order is bindings → physical → appearance → dispatch. WGSL permits module-scope
/// declarations in any order, so this is for the reader: the module ends with the four
/// functions consumers actually call, and everything they resolve to appears above them.
pub const WGSL: &str = concat!(
    include_str!("../../shaders/environment/common.wgsl"),
    include_str!("../../shaders/environment/hillaire.wgsl"),
    include_str!("../../shaders/environment/appearance.wgsl"),
    include_str!("../../shaders/environment/dispatch.wgsl"),
);
