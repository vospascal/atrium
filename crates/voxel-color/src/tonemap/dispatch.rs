//! Runtime dispatch for the CPU reference curves.

use super::{bt2390, gt7, hable, hdr_knee, reinhard, reinhard_headroom, TonemapCurve};

/// Apply `curve`, the CPU mirror of `apply_tonemap` in `shaders/tonemap.wgsl`.
///
/// `headroom` is the display's peak as a multiple of SDR reference white (1.0 = no
/// headroom); `content_peak` is read by [`TonemapCurve::Bt2390`] alone and ignored by
/// every other curve. Exposure and the sRGB encode are the caller's, in that order — this
/// function is only the curve, exactly as the shader's is.
pub fn apply(curve: TonemapCurve, color: [f32; 3], headroom: f32, content_peak: f32) -> [f32; 3] {
    let per_channel = |map: &dyn Fn(f32) -> f32| [map(color[0]), map(color[1]), map(color[2])];
    match curve {
        TonemapCurve::Reinhard => per_channel(&reinhard),
        TonemapCurve::ReinhardHeadroom => per_channel(&|c| reinhard_headroom(c, headroom)),
        TonemapCurve::HdrKnee => per_channel(&|c| hdr_knee(c, headroom)),
        TonemapCurve::HableFilmic => per_channel(&hable),
        TonemapCurve::Bt2390 => per_channel(&|c| bt2390(c, headroom, content_peak)),
        // The exception, and the reason `apply` cannot simply be a scalar function mapped
        // across channels: GT7 mixes the channels.
        TonemapCurve::Gt7 => gt7(color, headroom),
    }
}
