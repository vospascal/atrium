//! Which curve maps unbounded scene radiance onto a display.
//!
//! Selectable at runtime rather than compiled in, because the choice is a **look
//! decision nobody can make from a description**. Switching `HdrFloat` on made the whole
//! room brighter, and that was not the colour space or the headroom — it was swapping
//! Reinhard for a curve that leaves mid-tones alone. Whether that is an improvement or a
//! regression is a judgement about the image, so the curves have to be comparable
//! side by side, in the app, on the actual display.
//!
//! ## What each one is, and whose it is
//!
//! | curve | provenance | mid-tones | above white |
//! |---|---|---|---|
//! | [`TonemapCurve::Reinhard`] | Reinhard et al. 2002, eq. 3 | compressed | impossible |
//! | [`TonemapCurve::ReinhardHeadroom`] | ours, Reinhard plus a bounded continuation | **exactly Reinhard through white** | approaches headroom |
//! | [`TonemapCurve::HdrKnee`] | ours, not a standard | untouched | rolls into headroom |
//! | [`TonemapCurve::HableFilmic`] | Hable, Uncharted 2 (disowned by him) | toe + shoulder | clipped at white |
//! | [`TonemapCurve::Bt2390`] | **ITU-R BT.2390-4** | untouched below the knee | shoulder to display peak |
//! | [`TonemapCurve::Gt7`] | **Gran Turismo 7, SIGGRAPH 2025** | toe + linear | shoulder to display peak |
//!
//! **Reinhard** is `L/(1+L)`. Its ceiling is 1.0 by construction, so it cannot serve an
//! HDR surface at all — a highlight at linear 100 arrives as 0.99. It is also why the SDR
//! image looks flat in the mid-tones: linear 1.0 maps to display 0.5.
//!
//! **Reinhard+HDR** keeps `L/(1+L)` exactly through scene white, then adds a C¹ continuation
//! bounded by the measured display headroom. It is ours, not Reinhard et al. eq. 4. The
//! earlier implementation passed display headroom as that equation's input white point;
//! at headroom 1.0 the equation simplifies to identity, not Reinhard, and at high input it
//! is unbounded. The replacement makes the SDR fallback exact and keeps every output
//! inside the range the display reported.
//!
//! **HdrKnee** is ours: identity up to white, then a hyperbolic shoulder asymptotic to
//! `headroom`, with the denominator arranged so the derivative is 1 at white (C¹ — the
//! obvious form kinks there). It is structurally a naive cousin of **ITU-R BT.2390's
//! EETF**, which is the actual standard: black boost, linear mid-tones, Hermite-spline
//! shoulder, C¹, all computed in **PQ** rather than linear light.
//!
//! **Hable** is the filmic one — a toe and a shoulder instead of a single hyperbola, which
//! is what makes the SDR image read like a game rather than a render. Two things about it
//! are routinely got wrong and are pinned by tests in [`mod@reference`]: the `/ f(W)`
//! normalisation (drop
//! it and the curve peaks near 0.8 and looks washed) and the 2.0 exposure bias (drop it and
//! it looks far too dark, which then gets blamed on the curve). A third only shows up on a
//! float surface: the normalised ratio does not stop at 1.0, it climbs to about 1.17,
//! because `f` is bounded by `1 - E/F` = 0.933 while `f(W)` is smaller still. The original
//! relied on an 8-bit framebuffer clipping that away, so we clip it explicitly — it is a
//! fixed artifact that does not scale with headroom, not usable range.
//!
//! **BT.2390** is the standards-body answer and the only curve here whose knee is
//! *computed*: `KS = 1.5 · maxLum − 0.5`, where `maxLum` is the display's peak as a
//! fraction of the content's peak, both in PQ. So the knee MOVES with the display — a
//! brighter panel pushes it later and compresses less. Below it the signal is untouched;
//! above it a cubic Hermite spline carries it to the display peak, C¹ at the join. All of
//! it in PQ rather than linear light, which is the substantive difference from our knee: a
//! knee placed in perceptually-uniform space lands where the eye expects it.
//!
//! Two simplifications against the spec, both because their inputs do not exist here:
//!
//! - **Black level is taken as zero at both ends.** The spec carries `minLum` and a
//!   matching `E3 = E2 + minLum·(1−E2)⁴` lift. No platform we probe reports a black level —
//!   macOS and Android give a ratio only, DXGI's `MinLuminance` is widely reported as 0 —
//!   so carrying the term would mean inventing its input.
//! - **Applied per channel, not on luminance.** What most implementations do, and it keeps
//!   each channel monotonic. The cost is that a saturated highlight desaturates as it rolls
//!   off, because the brightest channel compresses hardest. A luminance-preserving variant
//!   is the next refinement, not a correction.
//!
//! And one real assumption: it needs a **content peak**, which nothing measures. See
//! [`DEFAULT_CONTENT_PEAK`] — that is the weak point of this curve, not the maths.

mod bt2390;
mod dispatch;
mod gt7;
mod hable;
mod hdr_knee;
mod reinhard;

pub mod reference;

pub use bt2390::{bt2390, pq_from_relative, relative_from_pq};
pub use dispatch::apply;
pub use gt7::{gt7, gt7_curve, ictcp_to_rgb, rgb_to_ictcp};
pub use hable::hable;
pub use hdr_knee::hdr_knee;
pub use reinhard::{reinhard, reinhard_headroom};

/// The GPU implementation of every curve, as WGSL. Its CPU counterpart is [`mod@reference`].
///
/// **This crate owns both halves of the tonemap**, and that is the point of putting the
/// shader text here rather than in the renderer. [`mod@reference`] and this string are two
/// implementations of one piece of mathematics; the danger is not that either is wrong on
/// its own but that they stop agreeing. In one crate a test can hold them together —
/// `wgsl_curve_indices_match_the_rust_enum` and
/// `every_curve_has_a_wgsl_implementation_and_a_dispatch_arm` do exactly that,
/// and neither could have been written while the halves sat in different crates.
///
/// It declares no bindings and reads no uniforms, so a consumer concatenates it into a
/// module and calls `apply_tonemap(color, headroom, curve, content_peak)` — the mirror of
/// [`reference::apply`]. Exposure and the sRGB encode are the caller's, in that order; see
/// the shader file's header.
pub const WGSL: &str = include_str!(concat!(env!("OUT_DIR"), "/tonemap.wgsl"));

/// The tonemap curves the shading pass can apply.
///
/// Carried in `Lighting.output_params.y` as [`TonemapCurve::shader_index`] rather than
/// patched in as a shader const. A const would fold the branch away, and the worry was
/// that five resident curves would cost occupancy in a pass that is residency-bound —
/// GT7's ICtCp matrices and BT.2390's PQ constants are live in one function, and
/// register allocation is decided by a kernel's worst path.
///
/// **Measured instead of assumed** (voxel-rt bench section 14, M3 Max): against a shader
/// with the dispatch collapsed to its Reinhard return, the six-curve build lands within
/// ±0.3% across three runs — zero. The unselected curves are free, so the runtime uniform
/// costs nothing and buys the one thing a const cannot: flipping between curves while
/// looking at the same frame, which is the entire point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TonemapCurve {
    /// `L/(1+L)` — Reinhard et al. 2002 eq. 3. The shipped SDR curve. Ceiling 1.0, so on
    /// an HDR surface it wastes the headroom entirely; useful as the baseline to compare
    /// against, and correct for the integer depths.
    #[default]
    Reinhard,
    /// Plain Reinhard through scene white, followed by a C¹ shoulder that approaches the
    /// measured display headroom. Ours, not Reinhard et al. eq. 4. At headroom 1.0 it is
    /// exactly plain Reinhard for every nonnegative input, so the conservative fallback
    /// really is SDR.
    ReinhardHeadroom,
    /// Identity to white, then a C¹ hyperbolic shoulder into the headroom. Ours. Leaves
    /// mid-tones exactly as authored, which is why it reads brighter than SDR.
    HdrKnee,
    /// Hable's Uncharted 2 filmic curve — a toe and a shoulder rather than Reinhard's
    /// single hyperbola, so blacks crush slightly and highlights roll off with more
    /// contrast. An **SDR** operator: it normalises by `f(W)`, so its ceiling is 1.0 and it
    /// can no more reach HDR headroom than plain Reinhard can.
    ///
    /// Worth having anyway, because the SDR path is the DEFAULT and plain Reinhard is the
    /// flattest curve in common use. This is the one that makes the 8-bit image look like a
    /// game rather than a render.
    ///
    /// **Provenance caveat, since this is exactly what becomes folklore:** Hable has
    /// publicly disowned it. He has said the constants were tuned for one game's look and
    /// that the curve crushes blacks more than he would now recommend, publishing a simpler
    /// piecewise replacement on Filmic Worlds. Kept under his name because that is what
    /// everyone calls it, and labelled so nobody mistakes it for a current recommendation.
    HableFilmic,
    /// **ITU-R BT.2390 EETF** — the standards-body display-mapping curve, and the only one
    /// here whose knee point is COMPUTED rather than pinned by hand.
    ///
    /// Black boost, linear mid-tones, cubic-Hermite shoulder, C¹ at the join, all in **PQ**
    /// (SMPTE ST.2084) rather than linear light — which is the substantive difference from
    /// [`TonemapCurve::HdrKnee`]. A knee placed in perceptually-uniform space lands where
    /// the eye expects it; the same knee in linear light does not.
    ///
    /// `KS = 1.5 · maxLum − 0.5`, where `maxLum` is the display's peak expressed as a
    /// fraction of the content's peak in PQ. So the knee MOVES with the display: a brighter
    /// panel pushes it later and compresses less. That is the property a hand-placed knee
    /// cannot have.
    ///
    /// Needs a **content peak** as well as a display peak — see
    /// [`DEFAULT_CONTENT_PEAK`].
    Bt2390,
    /// **GT7** — Yasutomi, Suzuki and Uchimura's Gran Turismo 7 operator, SIGGRAPH 2025.
    /// Transcribed from the course's own supplemental source, not reconstructed.
    ///
    /// The one that actually fits a renderer. Every other curve here is either SDR-only or,
    /// like [`TonemapCurve::Bt2390`], a *display-mapping* operator for content already
    /// graded to a known peak — which is why that one needs a content peak we cannot
    /// measure. GT7 goes scene-referred straight to the display, parameterised by the
    /// display alone, so **the content-peak assumption disappears entirely**.
    ///
    /// It is also the only COLOUR-VOLUME operator here: brightness and chroma together,
    /// not a per-channel curve. A per-channel pass gives a camera-like hue shift, a
    /// chroma-preserving pass through ICtCp holds hue exactly, and the two blend 60/40
    /// toward the second. That is what keeps saturated highlights from desaturating as
    /// they roll off — the failure every per-channel curve above shares.
    ///
    /// Their framebuffer convention is `physical / 100`, identical to
    /// [`crate::SDR_REFERENCE_WHITE_NITS`], so `peakIntensity` is exactly our headroom
    /// ratio with nothing to convert.
    ///
    /// **The most expensive curve here by some margin** — roughly two dozen `pow` calls
    /// per pixel for the ICtCp round trips, against two for Reinhard. Worth measuring
    /// before it becomes a default.
    Gt7,
}

impl TonemapCurve {
    pub const ALL: [TonemapCurve; 6] = [
        TonemapCurve::Reinhard,
        TonemapCurve::ReinhardHeadroom,
        TonemapCurve::HdrKnee,
        TonemapCurve::HableFilmic,
        TonemapCurve::Bt2390,
        TonemapCurve::Gt7,
    ];

    /// The default for a mode, so switching output depth picks a curve that suits it
    /// without the user having to know to change both.
    ///
    /// Reinhard+HDR rather than [`TonemapCurve::HdrKnee`]: it preserves the SDR curve
    /// through scene white, uses headroom only above it, and becomes exact SDR when the
    /// platform reports no headroom.
    pub fn default_for(writes_extended_range: bool) -> TonemapCurve {
        if writes_extended_range {
            TonemapCurve::ReinhardHeadroom
        } else {
            TonemapCurve::Reinhard
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TonemapCurve::Reinhard => "Reinhard",
            TonemapCurve::ReinhardHeadroom => "Reinhard+HDR",
            TonemapCurve::HdrKnee => "Knee",
            TonemapCurve::HableFilmic => "Hable",
            TonemapCurve::Bt2390 => "BT.2390",
            TonemapCurve::Gt7 => "GT7",
        }
    }

    /// A longer line for the overlay, naming provenance — the difference between a
    /// published operator and one of ours is exactly the sort of thing that becomes
    /// folklore once it is only in a commit message.
    pub fn description(self) -> &'static str {
        match self {
            TonemapCurve::Reinhard => {
                "L/(1+L), Reinhard 2002 eq.3. Ceiling 1.0 — cannot use HDR headroom."
            }
            TonemapCurve::ReinhardHeadroom => {
                "Reinhard through scene white, then a C¹ shoulder bounded by headroom. Ours."
            }
            TonemapCurve::HdrKnee => {
                "Identity to white then a C¹ shoulder. Ours, not a standard. Brighter \
                 mid-tones than SDR."
            }
            TonemapCurve::HableFilmic => {
                "Uncharted 2 filmic, toe + shoulder. SDR only (ceiling 1.0). Hable has \
                 since disowned the constants."
            }
            TonemapCurve::Bt2390 => {
                "ITU-R BT.2390 EETF in PQ space. Hermite shoulder, knee COMPUTED from the \
                 display: KS = 1.5·maxLum − 0.5."
            }
            TonemapCurve::Gt7 => {
                "Gran Turismo 7 (SIGGRAPH 2025). Colour-volume: per-channel blended with \
                 chroma-preserving ICtCp. Needs no content peak."
            }
        }
    }

    /// Whether this curve can emit values above 1.0 at all.
    ///
    /// [`TonemapCurve::Reinhard`] cannot, by construction — so selecting it on an HDR
    /// surface is a valid thing to do (it is the baseline) but produces no HDR, and the
    /// overlay should be able to say so rather than leaving the user to wonder why the
    /// speckles vanished.
    pub fn can_exceed_white(self) -> bool {
        matches!(
            self,
            TonemapCurve::ReinhardHeadroom
                | TonemapCurve::HdrKnee
                | TonemapCurve::Bt2390
                | TonemapCurve::Gt7
        )
    }

    /// The value written into `Lighting.output_params.y`. Must match the `TONEMAP_*`
    /// constants in `dda.wgsl`.
    pub fn shader_index(self) -> f32 {
        match self {
            TonemapCurve::Reinhard => 0.0,
            TonemapCurve::ReinhardHeadroom => 1.0,
            TonemapCurve::HdrKnee => 2.0,
            TonemapCurve::HableFilmic => 3.0,
            TonemapCurve::Bt2390 => 4.0,
            TonemapCurve::Gt7 => 5.0,
        }
    }

    /// Whether this curve reads `content_peak` at all — only BT.2390 does, so the overlay
    /// can hide a control that would otherwise look live and do nothing.
    pub fn uses_content_peak(self) -> bool {
        matches!(self, TonemapCurve::Bt2390)
    }
}

/// What BT.2390 assumes the scene's brightest pixel is, as a multiple of SDR reference
/// white. **10x = 1000 cd/m²**, HDR10's mastering baseline.
///
/// The EETF maps *content peak* to *display peak*, so it needs both, and only the display
/// half can be measured. Our content peak is genuinely unbounded — an emitter is authorable
/// to 64x white — so there is no scene maximum to read.
///
/// 1000 cd/m² is chosen because it is a real standard rather than a number picked to look
/// reasonable: it is what the overwhelming majority of HDR content is graded against. It is
/// still an ASSUMPTION, and a wrong one costs real quality in both directions — set it
/// above the true peak and the curve compresses range that was never used, set it below and
/// highlights clip before the shoulder reaches them.
///
/// The correct long-term answer is to measure it: a per-frame luminance reduction over the
/// rendered image, smoothed so the curve does not pump between frames. That is the same
/// machinery auto-exposure needs, which is why the two should land together rather than
/// separately.
pub const DEFAULT_CONTENT_PEAK: f32 = 10.0;

/// Options for the content-peak selector. Spans the range from "brighter than most SDR
/// content ever gets" to the maximum our own material table can author.
pub const CONTENT_PEAK_PRESETS: [f32; 5] = [2.0, 4.0, 10.0, 16.0, 64.0];

#[cfg(test)]
mod tests {
    use super::*;

    /// **The curve indices in the WGSL must agree with [`TonemapCurve::shader_index`].**
    ///
    /// This is the test the move into this crate was for. `shader_index` decides that GT7
    /// is 5; the shader independently declares `const TONEMAP_GT7: u32 = 5u` and dispatches
    /// on it. While those two lived in different crates nothing could compare them, and
    /// inserting or reordering a variant would have silently selected the wrong curve — a
    /// wiring bug that presents as a rendering bug, on a control whose entire purpose is
    /// comparing curves by eye. Now they are one `cargo test` apart.
    #[test]
    fn wgsl_curve_indices_match_the_rust_enum() {
        for curve in TonemapCurve::ALL {
            let declaration = format!("const {}: u32 = ", wgsl_const_name(curve));
            let index_text = WGSL
                .split(&declaration)
                .nth(1)
                .unwrap_or_else(|| panic!("{declaration} is missing from tonemap.wgsl"))
                .split('u')
                .next()
                .expect("a value follows the `=`");
            let wgsl_index: f32 = index_text
                .trim()
                .parse()
                .unwrap_or_else(|_| panic!("{declaration} does not declare a plain integer"));
            assert_eq!(
                wgsl_index,
                curve.shader_index(),
                "{:?}: the shader says {wgsl_index}, `shader_index` says {}",
                curve,
                curve.shader_index()
            );
        }
    }

    /// Every curve is reachable, on both sides. A variant with no WGSL function, or with
    /// one the dispatch never calls, is a menu entry that silently renders something else.
    ///
    /// The CPU side needs no equivalent: [`reference::apply`]'s `match` is exhaustive, so
    /// the compiler already refuses a curve without an implementation. Only the WGSL half,
    /// being text, has to be checked by hand.
    #[test]
    fn every_curve_has_a_wgsl_implementation_and_a_dispatch_arm() {
        let dispatch = WGSL
            .split("fn apply_tonemap(")
            .nth(1)
            .expect("tonemap.wgsl declares apply_tonemap");
        for curve in TonemapCurve::ALL {
            let function = wgsl_function_name(curve);
            assert!(
                WGSL.contains(&format!("fn {function}(")),
                "{curve:?} has no `fn {function}` in tonemap.wgsl"
            );
            assert!(
                dispatch.contains(&format!("{function}(")),
                "{curve:?}'s `{function}` is defined but `apply_tonemap` never calls it"
            );
        }
        // Reinhard is the fallthrough `return`, so the dispatch names one fewer constant
        // than there are curves. Pinned so a curve added without an arm cannot hide.
        let arms = dispatch.matches("if (curve == TONEMAP_").count();
        assert_eq!(
            arms,
            TonemapCurve::ALL.len() - 1,
            "apply_tonemap has {arms} branches for {} curves",
            TonemapCurve::ALL.len()
        );
    }

    /// The renderer applies exposure and the sRGB encode itself, so this file must not —
    /// and it must stay free of bindings, or it could not be concatenated into a module
    /// that already declares its own.
    #[test]
    fn the_wgsl_is_self_contained() {
        // CODE only. The prose above every function names `lighting.output_params` and
        // `srgb_encode` on purpose — describing what the caller must do is exactly this
        // file's job, and a check that cannot tell prose from code would forbid saying so.
        // WGSL has no string literals, so stripping from `//` to end-of-line is exact.
        let code: String = WGSL
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in [
            "@group(",
            "@binding(",
            "var<uniform>",
            "lighting.",
            "srgb_encode",
            "srgb_decode",
        ] {
            assert!(
                !code.contains(forbidden),
                "tonemap.wgsl's CODE contains `{forbidden}` — it must depend on nothing but \
                 its arguments, or it cannot be spliced into an arbitrary module"
            );
        }
    }

    /// Both halves must implement the same SET of curves. The WGSL function names are
    /// checked above and the Rust ones by the compiler; this pins the two lists together,
    /// so a curve cannot be added to one side and forgotten on the other.
    #[test]
    fn the_two_halves_implement_the_same_curves() {
        let wgsl_functions = WGSL.matches("fn tonemap_").count();
        assert_eq!(
            wgsl_functions,
            TonemapCurve::ALL.len(),
            "tonemap.wgsl defines {wgsl_functions} `tonemap_*` functions for {} curves",
            TonemapCurve::ALL.len()
        );
    }

    fn wgsl_const_name(curve: TonemapCurve) -> &'static str {
        match curve {
            TonemapCurve::Reinhard => "TONEMAP_REINHARD",
            TonemapCurve::ReinhardHeadroom => "TONEMAP_REINHARD_HEADROOM",
            TonemapCurve::HdrKnee => "TONEMAP_KNEE",
            TonemapCurve::HableFilmic => "TONEMAP_HABLE",
            TonemapCurve::Bt2390 => "TONEMAP_BT2390",
            TonemapCurve::Gt7 => "TONEMAP_GT7",
        }
    }

    fn wgsl_function_name(curve: TonemapCurve) -> &'static str {
        match curve {
            TonemapCurve::Reinhard => "tonemap_reinhard",
            TonemapCurve::ReinhardHeadroom => "tonemap_reinhard_headroom",
            TonemapCurve::HdrKnee => "tonemap_hdr_knee",
            TonemapCurve::HableFilmic => "tonemap_hable",
            TonemapCurve::Bt2390 => "tonemap_bt2390",
            TonemapCurve::Gt7 => "tonemap_gt7",
        }
    }

    /// GT7 takes no content peak — that is its practical advantage over BT.2390, so it is
    /// worth asserting rather than leaving to the doc comment.
    #[test]
    fn only_bt2390_needs_a_content_peak() {
        assert!(TonemapCurve::Bt2390.uses_content_peak());
        for curve in TonemapCurve::ALL {
            if curve != TonemapCurve::Bt2390 {
                assert!(
                    !curve.uses_content_peak(),
                    "{} should not need a content peak",
                    curve.label()
                );
            }
        }
    }

    /// Every curve needs a distinct label, description and shader index — the index
    /// especially, since a collision would silently select the wrong curve.
    #[test]
    fn every_curve_is_distinguishable_including_to_the_shader() {
        for (index, curve) in TonemapCurve::ALL.iter().enumerate() {
            assert_eq!(
                curve.shader_index(),
                index as f32,
                "{} must keep its wgsl index",
                curve.label()
            );
            for other in &TonemapCurve::ALL[index + 1..] {
                assert_ne!(curve.label(), other.label());
                assert_ne!(curve.description(), other.description());
                assert_ne!(curve.shader_index(), other.shader_index());
            }
        }
    }

    /// SDR must default to the curve it has always used, and HDR to the one that does not
    /// change the mid-tones — so flipping output depth is not silently a look change.
    #[test]
    fn the_defaults_keep_the_depth_toggle_honest() {
        assert_eq!(TonemapCurve::default_for(false), TonemapCurve::Reinhard);
        assert_eq!(
            TonemapCurve::default_for(true),
            TonemapCurve::ReinhardHeadroom
        );
        assert_eq!(TonemapCurve::default(), TonemapCurve::Reinhard);
    }
}
