//! GPU-facing colour adapter seam.
//!
//! The concrete `wgpu` formats remain part of [`crate::OutputFormat`] because they are
//! needed by the renderer's pipeline and surface setup.  This module is kept as the
//! home for future backend-specific probing and pipeline-resource adapters; the stable
//! policy boundary itself is [`crate::ColorAdapter`].

use crate::{
    ColorAdapter, ColorCapabilities, ColorRequest, DefaultColorAdapter, OutputSupport,
    ResolvedColorPath,
};

/// `wgpu`-specific adapter that owns capability probing and the platform's 8-bit fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuColorAdapter {
    srgb_surface_fallback: wgpu::TextureFormat,
}

impl GpuColorAdapter {
    pub fn new(srgb_surface_fallback: wgpu::TextureFormat) -> Self {
        Self {
            srgb_surface_fallback,
        }
    }

    pub fn capabilities(
        &self,
        surface_formats: &[wgpu::TextureFormat],
        device_features: wgpu::Features,
    ) -> ColorCapabilities {
        ColorCapabilities::new(
            OutputSupport::probe(surface_formats, device_features),
            self.srgb_surface_fallback,
        )
    }
}

impl Default for GpuColorAdapter {
    fn default() -> Self {
        Self::new(wgpu::TextureFormat::Bgra8UnormSrgb)
    }
}

impl ColorAdapter for GpuColorAdapter {
    fn resolve(
        &self,
        request: ColorRequest,
        mut capabilities: ColorCapabilities,
    ) -> ResolvedColorPath {
        capabilities.srgb_surface_fallback = self.srgb_surface_fallback;
        DefaultColorAdapter.resolve(request, capabilities)
    }
}
