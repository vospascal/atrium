//! Telling the compositor what our pixels MEAN.
//!
//! The fifth consumer of output depth, and the one whose absence is invisible until
//! you look at the picture. The other four are all *format* agreements — surface,
//! storage, WGSL type, bind-group layout — and wgpu validates every one of them, so
//! getting one wrong is a hard error with a message. This one is a *semantic*
//! agreement, nothing validates it, and getting it wrong renders a plausible image
//! that is simply the wrong brightness.
//!
//! **The bug that produced this module.** `HdrFloat` originally wrote scene-linear
//! extended-range radiance, skipping the sRGB encode, and handed it to an untagged
//! `CAMetalLayer`. Apple documents the default:
//!
//! > The default value is `nil`, indicating that the rendered content isn't
//! > color-matched. If you set this to a different color space, Core Animation
//! > performs any necessary color transformations when compositing the view's
//! > contents.
//!
//! Untagged is therefore not "neutral" — it is *pass-through*. The panel receives our
//! values and displays them under its own native transfer function, so linear 0.5
//! landed where encoded 0.5 lands: 0.21 linear. Mid-tones collapsed to roughly a fifth
//! of intended luminance, shadows worse, only near-white surviving. Perceived
//! saturation falls with luminance, so it read as *greyed and shifted* rather than
//! dark — which is exactly how it was reported, and exactly what a missing transfer
//! function looks like.
//!
//! **Tagging it linear then broke the overlay, and that is the more useful lesson.**
//! This surface has TWO writers. egui chooses its own transfer function from
//! `format.is_srgb()` and writes gamma-encoded into any non-sRGB target
//! (`egui-wgpu-0.35.0/src/renderer.rs:406`). So with a linear tag the scene came right
//! and the UI went pale — one tag cannot serve two conventions, and only one of the two
//! writers is ours to change.
//!
//! The resolution is to stop making HDR a *different kind* of value. Every mode now
//! sRGB-encodes, and `HdrFloat` differs only in emitting values above 1.0 — so the tag
//! is `extendedSRGB`, the encoded space, and both writers are correct with no extra
//! pass. **Range survives an encode; it does not survive Reinhard.** Those are separate
//! problems and only the second needed solving. Conflating them cost a round-trip.
//!
//! The general rule worth keeping: when a resource has a writer you do not control,
//! adopt the contract that writer already satisfies.
//!
//! **Both directions were wrong, not just HDR.** The integer depths write
//! sRGB-encoded values into an untagged layer too. Pass-through means a wide-gamut
//! panel interprets sRGB primaries as *its own* primaries, which oversaturates
//! everything. So SDR was subtly too vivid while HDR was badly too dull, and the two
//! could never agree. Tagging both is what makes a depth toggle change *only* the
//! bit depth — the entire point of the toggle.
//!
//! **What wgpu already does, so this module does not.** `wgpu-hal` sets
//! `wantsExtendedDynamicRangeContent` itself whenever the surface format is
//! `Rgba16Float` (`wgpu-hal-29.0.4/src/metal/surface.rs:93`). An earlier note in this
//! crate claimed that flag was the missing piece and needed reaching through
//! `Surface::as_hal`; that was wrong twice over — wgpu sets it automatically, and the
//! flag was never the problem. It is why extended-range speckle was *visible* before
//! any of this landed while everything around it was wrong: the range survived, the
//! encoding did not.
//!
//! Apple's own EDR recipe is three settings, and only the middle one was missing:
//!
//! | setting | who does it |
//! |---|---|
//! | `pixelFormat = .rgba16Float` | [`crate::OutputFormat::surface`] |
//! | `colorspace = extendedSRGB` | **here** |
//! | `wantsExtendedDynamicRangeContent = true` | `wgpu-hal`, automatically |

use crate::OutputFormat;

/// Whether this build has a presentation hook that can establish the encoded
/// extended-sRGB contract used by [`crate::OutputDepth::HdrFloat`].
///
/// A float swapchain format alone is not enough. In particular, Windows interprets an
/// FP16 DXGI swapchain as linear scRGB by default, while this renderer must keep encoded
/// extended sRGB because egui shares the surface. Until a platform arm can establish an
/// equivalent contract, production probing must veto HDR there rather than knowingly
/// present the wrong transfer function.
pub const PLATFORM_SUPPORTS_EXTENDED_SRGB: bool = cfg!(target_vendor = "apple");

/// How the window-system compositor must interpret the values in the swapchain.
///
/// Not a preference — a CONTRACT. The same bits mean different brightnesses under
/// different tags, which is why there is no `Untagged` variant here: leaving the
/// surface unlabelled is the bug this type exists to prevent, not a third option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceColorSpace {
    /// Display-encoded sRGB: sRGB transfer function applied, values in `[0, 1]`,
    /// sRGB/Rec.709 primaries. What both integer depths write.
    Srgb,
    /// The sRGB transfer function CONTINUED past its usual domain: `1.0` is still SDR
    /// reference white ([`crate::SDR_REFERENCE_WHITE_NITS`]), above it is display
    /// headroom, below zero is out of gamut. `kCGColorSpaceExtendedSRGB`, which is also
    /// Vulkan's `VK_COLOR_SPACE_EXTENDED_SRGB_NONLINEAR_EXT`. What
    /// [`crate::OutputDepth::HdrFloat`] writes.
    ///
    /// **Encoded, not linear, and that is a deliberate reversal.** The linear sibling
    /// (`extendedLinearSRGB`) is what every Apple EDR sample reaches for, and it was
    /// tried first. It fails here for a reason those samples do not have: this surface
    /// has TWO writers. egui picks its transfer function from `format.is_srgb()` and
    /// writes gamma-encoded into any non-sRGB target, so tagging linear made the scene
    /// right and the overlay pale. Choosing the encoded space instead puts both writers
    /// on one convention with no extra pass — pick the contract the writer you do not
    /// control already satisfies.
    ///
    /// Deliberately sRGB primaries rather than Display P3 or Rec.2020, because that is
    /// what the content actually IS: albedos are authored sRGB and decoded to linear
    /// sRGB, so tagging wider primaries would claim a saturation the scene never had
    /// and skew every hue. Wide-gamut authoring is a separate capability that starts
    /// at the material, not at the swapchain.
    ExtendedSrgb,
}

impl SurfaceColorSpace {
    pub fn label(self) -> &'static str {
        match self {
            SurfaceColorSpace::Srgb => "sRGB",
            SurfaceColorSpace::ExtendedSrgb => "extended sRGB",
        }
    }
}

/// What actually happened when we tried to tag the surface.
///
/// Reported rather than swallowed because a silent no-op is what made the original
/// bug so hard to see: the picture was wrong and nothing anywhere said why. Every
/// variant is a distinct thing the log can name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpaceOutcome {
    /// The compositor now knows what our pixels mean.
    Tagged(SurfaceColorSpace),
    /// This platform exposes no tagging hook, so the compositor's own convention
    /// applies. SDR may still be well-defined, but HDR is vetoed during
    /// [`crate::OutputSupport::probe`] because its extended transfer contract cannot be
    /// established.
    NoPlatformHook,
    /// The surface is not backed by the platform API we know how to tag (a software
    /// adapter, or a backend other than Metal on an Apple target).
    SurfaceNotTaggable,
    /// The platform rejected the colour-space name. Should not happen — these are
    /// system constants — but returning it beats an `unwrap` in a display path.
    ColorSpaceUnavailable,
}

impl ColorSpaceOutcome {
    /// One line for the startup log, in the shape of
    /// [`crate::OutputSupport::ten_bit_diagnosis`].
    pub fn diagnosis(self) -> &'static str {
        match self {
            ColorSpaceOutcome::Tagged(SurfaceColorSpace::Srgb) => "tagged sRGB",
            ColorSpaceOutcome::Tagged(SurfaceColorSpace::ExtendedSrgb) => "tagged extended sRGB",
            ColorSpaceOutcome::NoPlatformHook => "not tagged (no hook on this platform)",
            ColorSpaceOutcome::SurfaceNotTaggable => "not tagged (surface is not Metal-backed)",
            ColorSpaceOutcome::ColorSpaceUnavailable => "not tagged (system rejected the name)",
        }
    }
}

/// Tag `surface` with the colour space `format` writes into it.
///
/// **Call after every `Surface::configure`.** Reconfiguring does not currently clear
/// the tag — `wgpu-hal`'s `configure` touches device, pixel format, framebuffer-only,
/// the EDR flag, drawable count, drawable size and display sync, and never
/// `colorspace` — but that is an implementation detail of a dependency, and a tag
/// silently lost on resize would reproduce the original bug in a form far harder to
/// find. Re-tagging is idempotent and costs one Objective-C message.
pub fn apply(surface: &wgpu::Surface<'_>, format: &OutputFormat) -> ColorSpaceOutcome {
    tag(surface, format.color_space())
}

#[cfg(target_vendor = "apple")]
fn tag(surface: &wgpu::Surface<'_>, wanted: SurfaceColorSpace) -> ColorSpaceOutcome {
    use objc2_core_foundation::CFString;
    use objc2_core_graphics::{kCGColorSpaceExtendedSRGB, kCGColorSpaceSRGB, CGColorSpace};

    // Extern statics, hence the unsafe block; these are CoreGraphics' own name
    // constants and are valid for the lifetime of the process.
    let name: &CFString = unsafe {
        match wanted {
            SurfaceColorSpace::Srgb => kCGColorSpaceSRGB,
            SurfaceColorSpace::ExtendedSrgb => kCGColorSpaceExtendedSRGB,
        }
    };

    let Some(color_space) = CGColorSpace::with_name(Some(name)) else {
        return ColorSpaceOutcome::ColorSpaceUnavailable;
    };

    // SAFETY: we only read the layer handle and set a property on it. We do not
    // retain it past this call, and we hold wgpu-hal's own lock while doing so, so we
    // cannot race a concurrent `configure` or `acquire_texture`.
    let metal_surface = unsafe { surface.as_hal::<wgpu::hal::api::Metal>() };
    let Some(metal_surface) = metal_surface else {
        return ColorSpaceOutcome::SurfaceNotTaggable;
    };
    metal_surface
        .render_layer()
        .lock()
        .setColorspace(Some(&color_space));
    ColorSpaceOutcome::Tagged(wanted)
}

/// Everywhere else the compositor's own convention applies and there is nothing to
/// set. `surface` and `wanted` are consumed so the signature matches the Apple arm
/// exactly and neither drifts from the other.
#[cfg(not(target_vendor = "apple"))]
fn tag(_surface: &wgpu::Surface<'_>, _wanted: SurfaceColorSpace) -> ColorSpaceOutcome {
    ColorSpaceOutcome::NoPlatformHook
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OutputDepth, OutputSupport};

    fn resolved(depth: OutputDepth) -> OutputFormat {
        OutputFormat::resolve(
            depth,
            OutputSupport {
                ten_bit_surface: true,
                float_surface: true,
                extended_srgb_presentation: true,
                sixteen_bit_norm_storage: true,
            },
            wgpu::TextureFormat::Bgra8UnormSrgb,
        )
    }

    /// The tag has to follow the RANGE, not the bit depth. All three modes share one
    /// transfer function — sRGB — and differ only in whether values may exceed 1.0.
    #[test]
    fn the_tag_follows_the_range_not_the_bit_depth() {
        assert_eq!(
            resolved(OutputDepth::EightBit).color_space(),
            SurfaceColorSpace::Srgb
        );
        assert_eq!(
            resolved(OutputDepth::TenBit).color_space(),
            SurfaceColorSpace::Srgb,
            "ten bits of ENCODED sRGB is still sRGB — the extra bits buy precision, \
             not a different colour space"
        );
        assert_eq!(
            resolved(OutputDepth::HdrFloat).color_space(),
            SurfaceColorSpace::ExtendedSrgb
        );
    }

    /// The one invariant that ties this module to the shader: the depth whose tonemap
    /// can emit above 1.0 is exactly the depth that must be tagged extended. Break
    /// either side and the picture is wrong in the way that started this — so pin them
    /// together rather than trusting two `match` arms to stay in step.
    #[test]
    fn emitting_above_white_and_tagging_extended_are_the_same_decision() {
        for depth in OutputDepth::ALL {
            let format = resolved(depth);
            let extended_tag = format.color_space() == SurfaceColorSpace::ExtendedSrgb;
            assert_eq!(
                format.writes_extended_range(),
                extended_tag,
                "{}: shader range and compositor tag disagree",
                depth.label(),
            );
        }
    }

    /// EVERY mode is sRGB-encoded, and that is the property that keeps egui in step.
    /// egui picks its own transfer function from `format.is_srgb()` and writes
    /// gamma-encoded into any non-sRGB target, so a mode that wrote scene-linear into
    /// the shared surface would leave the overlay pale — which is exactly what
    /// happened. Nothing here can see egui, so the invariant is stated as "no variant
    /// is linear" and enforced by the type having no linear variant at all.
    #[test]
    fn no_mode_asks_the_compositor_for_linear_light() {
        for depth in OutputDepth::ALL {
            assert!(
                matches!(
                    resolved(depth).color_space(),
                    SurfaceColorSpace::Srgb | SurfaceColorSpace::ExtendedSrgb
                ),
                "{}: every mode must stay sRGB-encoded so egui and the blit agree",
                depth.label(),
            );
        }
    }

    /// A vetoed depth must not carry the tag its wish implied. Asking for HDR on a
    /// device without a float surface resolves to eight bits, and eight bits are
    /// encoded — tagging that linear would be the original bug with the arguments
    /// reversed, dark for a reason nobody would think to look for.
    #[test]
    fn a_vetoed_hdr_request_is_tagged_as_the_depth_it_actually_got() {
        let format = OutputFormat::resolve(
            OutputDepth::HdrFloat,
            OutputSupport {
                ten_bit_surface: false,
                float_surface: false,
                extended_srgb_presentation: true,
                sixteen_bit_norm_storage: false,
            },
            wgpu::TextureFormat::Bgra8UnormSrgb,
        );
        assert_eq!(format.depth(), OutputDepth::EightBit);
        assert_eq!(format.color_space(), SurfaceColorSpace::Srgb);
    }

    #[test]
    fn every_outcome_says_something_different() {
        let outcomes = [
            ColorSpaceOutcome::Tagged(SurfaceColorSpace::Srgb),
            ColorSpaceOutcome::Tagged(SurfaceColorSpace::ExtendedSrgb),
            ColorSpaceOutcome::NoPlatformHook,
            ColorSpaceOutcome::SurfaceNotTaggable,
            ColorSpaceOutcome::ColorSpaceUnavailable,
        ];
        for (index, outcome) in outcomes.iter().enumerate() {
            for other in &outcomes[index + 1..] {
                assert_ne!(outcome.diagnosis(), other.diagnosis());
            }
        }
    }

    #[test]
    fn production_probe_exposes_hdr_only_where_a_presentation_hook_exists() {
        let support =
            OutputSupport::probe(&[wgpu::TextureFormat::Rgba16Float], wgpu::Features::empty());
        assert_eq!(
            support.extended_srgb_presentation,
            PLATFORM_SUPPORTS_EXTENDED_SRGB
        );
        assert_eq!(
            support.supports(OutputDepth::HdrFloat),
            PLATFORM_SUPPORTS_EXTENDED_SRGB
        );
    }
}
