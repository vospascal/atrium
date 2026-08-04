//! Stable, renderer-facing contracts for the output colour path.
//!
//! The renderer should not decide which surface format, storage format, transfer
//! convention, or default tone curve belongs together.  It supplies a request and the
//! capabilities it probed; a [`ColorAdapter`] returns one coherent answer.  New output
//! backends can replace the adapter without changing the renderer's call sites.

use crate::headroom::DisplayHeadroom;
use crate::{OutputDepth, OutputFormat, OutputSupport, TonemapCurve};

/// What the application would like to use for the next output path.
///
/// `depth` is a request, not a promise: the adapter may resolve it to 8-bit when the
/// device or presentation path cannot support the requested mode.  `tonemap = None`
/// selects the adapter's mode-appropriate default; an explicit curve remains available
/// for comparison and artistic direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorRequest {
    pub depth: OutputDepth,
    pub tonemap: Option<TonemapCurve>,
    pub content_peak: f32,
}

impl Default for ColorRequest {
    fn default() -> Self {
        Self {
            depth: OutputDepth::EightBit,
            tonemap: None,
            content_peak: crate::tonemap::DEFAULT_CONTENT_PEAK,
        }
    }
}

/// GPU and presentation capabilities collected by the renderer at startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorCapabilities {
    pub support: OutputSupport,
    pub srgb_surface_fallback: wgpu::TextureFormat,
}

impl Default for ColorCapabilities {
    fn default() -> Self {
        Self {
            support: OutputSupport::default(),
            srgb_surface_fallback: wgpu::TextureFormat::Bgra8UnormSrgb,
        }
    }
}

impl ColorCapabilities {
    pub fn new(support: OutputSupport, srgb_surface_fallback: wgpu::TextureFormat) -> Self {
        Self {
            support,
            srgb_surface_fallback,
        }
    }
}

/// One resolved colour path.  The selected format and curve are kept together so a
/// renderer cannot accidentally use an HDR surface with an SDR-only mapping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedColorPath {
    format: OutputFormat,
    tonemap: TonemapCurve,
    content_peak: f32,
}

impl ResolvedColorPath {
    pub(crate) fn new(format: OutputFormat, tonemap: TonemapCurve, content_peak: f32) -> Self {
        Self {
            format,
            tonemap,
            content_peak,
        }
    }

    pub fn format(self) -> OutputFormat {
        self.format
    }

    pub fn tonemap(self) -> TonemapCurve {
        self.tonemap
    }

    pub fn content_peak(self) -> f32 {
        self.content_peak
    }

    pub fn map_scene_color(self, color: [f32; 3], headroom: DisplayHeadroom) -> [f32; 3] {
        crate::tonemap::reference::apply(self.tonemap, color, headroom.ratio(), self.content_peak)
    }
}

/// Adapter boundary for output colour policy and conversion.
pub trait ColorAdapter {
    fn resolve(&self, request: ColorRequest, capabilities: ColorCapabilities) -> ResolvedColorPath;
}
