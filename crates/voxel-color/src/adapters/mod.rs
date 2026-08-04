//! Built-in colour adapters.
//!
//! The default adapter is intentionally small: all format/depth mappings remain in
//! [`OutputFormat::resolve`], while this layer owns the policy that joins that answer to
//! a default tone curve.  Platform-specific or experimental policies can implement the
//! public [`ColorAdapter`] trait without changing the facade.

mod hdr_float;
mod sdr;
mod ten_bit;

use crate::api::{ColorAdapter, ColorCapabilities, ColorRequest, ResolvedColorPath};
use crate::tonemap::TonemapCurve;

pub use hdr_float::HdrFloatAdapter;
pub use sdr::SdrAdapter;
pub use ten_bit::TenBitAdapter;

/// The shipped output policy.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultColorAdapter;

impl ColorAdapter for DefaultColorAdapter {
    fn resolve(&self, request: ColorRequest, capabilities: ColorCapabilities) -> ResolvedColorPath {
        let format = crate::OutputFormat::resolve(
            request.depth,
            capabilities.support,
            capabilities.srgb_surface_fallback,
        );
        let tonemap = request
            .tonemap
            .unwrap_or_else(|| TonemapCurve::default_for(format.writes_extended_range()));
        ResolvedColorPath::new(format, tonemap, request.content_peak)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OutputDepth, OutputSupport};

    fn full_support() -> OutputSupport {
        OutputSupport {
            ten_bit_surface: true,
            float_surface: true,
            extended_srgb_presentation: true,
            sixteen_bit_norm_storage: true,
        }
    }

    #[test]
    fn default_policy_pairs_sdr_with_reinhard() {
        let path = DefaultColorAdapter.resolve(
            ColorRequest::default(),
            ColorCapabilities::new(full_support(), wgpu::TextureFormat::Bgra8UnormSrgb),
        );
        assert_eq!(path.format().depth(), OutputDepth::EightBit);
        assert_eq!(path.tonemap(), TonemapCurve::Reinhard);
    }

    #[test]
    fn default_policy_pairs_float_with_headroom_curve() {
        let path = DefaultColorAdapter.resolve(
            ColorRequest {
                depth: OutputDepth::HdrFloat,
                ..ColorRequest::default()
            },
            ColorCapabilities::new(full_support(), wgpu::TextureFormat::Bgra8UnormSrgb),
        );
        assert_eq!(path.format().depth(), OutputDepth::HdrFloat);
        assert_eq!(path.tonemap(), TonemapCurve::ReinhardHeadroom);
    }

    #[test]
    fn unsupported_request_resolves_to_sdr_before_selecting_default_curve() {
        let path = DefaultColorAdapter.resolve(
            ColorRequest {
                depth: OutputDepth::HdrFloat,
                ..ColorRequest::default()
            },
            ColorCapabilities::new(
                OutputSupport::default(),
                wgpu::TextureFormat::Bgra8UnormSrgb,
            ),
        );
        assert_eq!(path.format().depth(), OutputDepth::EightBit);
        assert_eq!(path.tonemap(), TonemapCurve::Reinhard);
    }

    #[test]
    fn resolved_path_owns_cpu_conversion_with_display_headroom() {
        let path =
            DefaultColorAdapter.resolve(ColorRequest::default(), ColorCapabilities::default());
        let mapped = path.map_scene_color([1.0, 2.0, 4.0], crate::DisplayHeadroom::default());
        assert_eq!(mapped, [0.5, 2.0 / 3.0, 0.8]);
    }
}
