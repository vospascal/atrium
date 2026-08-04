//! WGSL assembly boundary for the Jolifanto provider.

/// The LUT-generation module. Fragments are concatenated so each LUT has a visible,
/// independently editable source file while the renderer still receives one WGSL module.
pub const LUT_WGSL: &str = concat!(
    include_str!("../../shaders/lut/common.wgsl"),
    include_str!("../../shaders/lut/transmittance.wgsl"),
    include_str!("../../shaders/lut/multiple_scattering.wgsl"),
    include_str!("../../shaders/lut/sky_view.wgsl"),
    include_str!("../../shaders/lut/aerial_perspective.wgsl"),
);

/// The sampling module consumed by DDA and CAGI. It contains no per-camera ray march;
/// all atmosphere work is a lookup into the generated textures.
pub const WGSL: &str = include_str!("../../shaders/environment.wgsl");
