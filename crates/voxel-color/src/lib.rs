//! The output colour path: how finished radiance becomes pixels on a display.
//!
//! A SEPARATE CRATE because the domain is separate. Nothing here knows about voxels,
//! materials, brickmaps or render passes — only bit depth, transfer functions, texture
//! formats and luminance. The dependency runs one way: the renderer asks this crate
//! what formats and curves to use, never the reverse. If this crate ever needs to know
//! about a voxel, the boundary is in the wrong place.
//!
//! Output bit depth is not a single switch. It reaches four separate places that
//! must agree exactly, and the point of this module is that none of them branches
//! on the depth:
//!
//!   1. the SURFACE format the swapchain is configured with (`gpu.rs`)
//!   2. the STORAGE format of the texture the compute pass writes (`render.rs`)
//!   3. the `texture_storage_2d<FORMAT, write>` type in `dda.wgsl` — the format is
//!      part of the WGSL TYPE, so the shader source has to be patched, not just a
//!      const flipped
//!   4. the DDA pass's BIND GROUP LAYOUT entry for binding 6 (`passes/dda.rs`)
//!   5. whether the blit DECODES sRGB or passes it through (`blit.wgsl`)
//!   6. the COLOUR SPACE the swapchain surface is tagged with, so the compositor
//!      knows whether it is holding encoded or linear light ([`color_space`])
//!
//! **wgpu wants a storage texture's format in three of those places** — the texture,
//! the layout entry and the WGSL type — and validates all three against each other.
//! Missing the layout entry is what produced *"Storage texture binding 6 expects
//! format = Rgba8Unorm, but given a view with format = Rgba16Unorm"* on the first
//! attempt, after the texture itself was already correct. The layout is baked at
//! pass construction, so a depth change rebuilds the DDA pass rather than rebinding
//! it, and the pipeline cache goes with it — correctly, since every entry was
//! compiled for the old format.
//!
//! Get any one of those out of step with the others and the picture is silently
//! wrong: too dark, too bright, or double-encoded. So [`OutputFormat::resolve`]
//! holds the only `match` on depth in the codebase, and every consumer asks it a
//! question instead of deciding for itself.
//!
//! Feature gating is the other half, and it bit twice: `TEXTURE_FORMAT_16BIT_NORM`
//! must be requested at DEVICE CREATION (see [`REQUIRED_DEVICE_FEATURES`]) because
//! features cannot be added later, and [`OutputSupport::probe`] must read
//! `device.features()` rather than `adapter.features()` or it will report support
//! the device does not have.
//!
//! **Why the sRGB asymmetry exists**, since it is the least obvious of the four.
//! The 8-bit path uses an `*Srgb` surface, so the hardware applies the transfer
//! function when the blit stores its fragment — which is why the blit currently
//! DECODES: it hands back linear light for the hardware to re-encode, making the
//! round trip exact. A 10-bit surface is `Rgb10a2Unorm`, plain unorm, with no
//! hardware transfer at all — so the blit must pass the already-encoded value
//! straight through. Decoding into a non-sRGB surface would present a washed-out
//! image; passing through into an sRGB surface would present a dark one.
//!
//! **Ten bits is useless without a wider storage texture.** A 10-bit swapchain fed
//! from an `Rgba8Unorm` storage texture gains nothing — the value was already
//! quantised to 8 bits before the blit ever saw it. That is why depth resolves
//! BOTH formats together and never one alone.
//!
//! This is deliberately NOT part of voxel-rt's `RenderQuality`. Output depth
//! is a property of the DISPLAY, not a quality tier: the cheapest tier on an OLED
//! still wants ten bits, and the most expensive tier on an old panel cannot have
//! them. It lives beside the vsync toggle for the same reason vsync does.
//!
//! # Before changing anything here, read `README.md`
//!
//! These docs describe what the colour path IS. The crate README describes what it should
//! eventually BE: the two-stage tone-map / display-map pipeline this sits in, which stage
//! we occupy and why that decides the curve, what blocks each unfinished piece — and a list
//! of **decisions that look wrong and are right**, each arrived at expensively. The
//! `extendedSRGB`-not-`extendedLinearSRGB` tag and the 1.0 headroom fallback are both on
//! that list.

pub mod adapters;
pub mod api;
pub mod color_space;
pub mod gpu;
pub mod headroom;
pub mod state;
pub mod tonemap;

pub use adapters::DefaultColorAdapter;
pub use api::{ColorAdapter, ColorCapabilities, ColorRequest, ResolvedColorPath};
pub use color_space::{ColorSpaceOutcome, SurfaceColorSpace};
pub use gpu::GpuColorAdapter;
pub use headroom::{DisplayHeadroom, HeadroomChoice, HeadroomProvider, HeadroomSource};
pub use state::ColorState;
pub use tonemap::TonemapCurve;

/// The user-facing toggle: how many bits per channel reach the display.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputDepth {
    /// `Rgba8Unorm` storage into an sRGB swapchain. The shipped path.
    #[default]
    EightBit,
    /// `Rgba16Unorm` storage into an `Rgb10a2Unorm` swapchain.
    ///
    /// Sixteen bits of storage for ten bits of output is not waste — it is the
    /// narrowest format that is BOTH wider than 8 bits AND filterable, which the
    /// blit's linear upscale needs at render scales below 1.0. The alternative,
    /// packing 10:10:10 into `R32Uint`, keeps 4 bytes per pixel but is not
    /// filterable, so it would force a hand-rolled bilinear into the blit.
    ///
    /// Note it is `Unorm`, NOT float: integer storage throughout, no float
    /// accumulation introduced anywhere by choosing it.
    TenBit,
    /// `Rgba16Float` storage into an `Rgba16Float` surface, carrying sRGB-encoded
    /// values in EXTENDED RANGE — above 1.0 for brighter than SDR white.
    ///
    /// The mode `GPUCanvasToneMappingMode::"extended"` describes: values above 1.0 are
    /// preserved for a display with headroom, and clamped by the compositor where there
    /// is none. Plain Reinhard cannot serve it — `L/(1+L)` has a fixed ceiling of 1.0 —
    /// so the app defaults this mode to [`TonemapCurve::ReinhardHeadroom`].
    ///
    /// **It swaps the tonemap and NOTHING ELSE.** The sRGB encode stays on, which is a
    /// correction to an earlier design that dropped it and wrote scene-linear instead.
    /// Range survives the extended sRGB encode (it is monotonic and finite above 1.0);
    /// it does not survive Reinhard. Those
    /// are different problems and only the second one needed solving.
    ///
    /// Dropping the encode also broke the overlay, which is the part that made the
    /// mistake obvious: this surface has TWO writers. egui picks its own transfer
    /// function from `format.is_srgb()` and writes gamma-encoded into any non-sRGB
    /// target, so a scene-linear surface left egui pale and washed. Keeping the encode
    /// puts both writers on one convention — see [`color_space`].
    ///
    /// **Only correct once the surface is TAGGED**, with `extendedSRGB`. An untagged
    /// layer is displayed pass-through, which is a different wrong picture in each mode.
    ///
    /// `CAMetalLayer.wantsExtendedDynamicRangeContent`, contrary to an earlier note
    /// here, needs nothing from us: `wgpu-hal` sets it itself for `Rgba16Float`
    /// (`wgpu-hal-29.0.4/src/metal/surface.rs:93`). The flag was never the gap.
    HdrFloat,
}

impl OutputDepth {
    pub const ALL: [OutputDepth; 3] = [
        OutputDepth::EightBit,
        OutputDepth::TenBit,
        OutputDepth::HdrFloat,
    ];

    pub fn label(self) -> &'static str {
        match self {
            OutputDepth::EightBit => "8-bit",
            OutputDepth::TenBit => "10-bit",
            OutputDepth::HdrFloat => "HDR float",
        }
    }
}

/// What the device and its surface can actually do, probed once at startup.
///
/// Requested depth is a WISH; this is the veto. Modelled on the vsync toggle,
/// which checks `supported_present_modes` and falls back rather than failing — a
/// display that cannot do ten bits should quietly render eight, not panic.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputSupport {
    /// The surface advertises [`TEN_BIT_SURFACE_FORMAT`].
    pub ten_bit_surface: bool,
    /// The surface advertises `Rgba16Float`, which [`OutputDepth::HdrFloat`] needs as
    /// BOTH its surface and its storage format. Storage support is guaranteed for that
    /// format with no feature, but the platform presentation contract is a separate veto.
    pub float_surface: bool,
    /// This platform can tell its compositor that the float surface contains encoded
    /// extended sRGB. A float format without this semantic contract is not HDR support.
    pub extended_srgb_presentation: bool,
    /// The adapter offers `TEXTURE_FORMAT_16BIT_NORM`, without which
    /// [`OutputDepth::TenBit`]'s storage format cannot be created. Native only —
    /// it is not a web feature, so a browser build is 8-bit by construction.
    pub sixteen_bit_norm_storage: bool,
}

impl OutputSupport {
    /// Probe from the two things that can each independently veto ten bits.
    ///
    /// `features` MUST come from the **device**, not the adapter. An adapter that
    /// merely *offers* `TEXTURE_FORMAT_16BIT_NORM` proves nothing: features have to
    /// be requested at device creation and cannot be added later, so a probe against
    /// `adapter.features()` will happily report ten-bit support on a device that
    /// will then fail `create_texture`. That exact bug shipped for one run — the
    /// toggle enabled itself, reconfigured the surface, and panicked on the storage
    /// texture. [`REQUIRED_DEVICE_FEATURES`] is what keeps the two in step.
    pub fn probe(
        surface_formats: &[wgpu::TextureFormat],
        features: wgpu::Features,
    ) -> OutputSupport {
        OutputSupport {
            ten_bit_surface: surface_formats.contains(&TEN_BIT_SURFACE_FORMAT),
            float_surface: surface_formats.contains(&FLOAT_SURFACE_FORMAT),
            extended_srgb_presentation: color_space::PLATFORM_SUPPORTS_EXTENDED_SRGB,
            sixteen_bit_norm_storage: features.contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM),
        }
    }

    /// Whether [`OutputDepth::TenBit`] is selectable at all.
    ///
    /// BOTH halves are required, which is the whole reason this is one predicate
    /// rather than two booleans consumers check separately.
    pub fn supports(self, depth: OutputDepth) -> bool {
        match depth {
            OutputDepth::EightBit => true,
            OutputDepth::TenBit => self.ten_bit_surface && self.sixteen_bit_norm_storage,
            OutputDepth::HdrFloat => self.float_surface && self.extended_srgb_presentation,
        }
    }

    /// One line for the startup log and the overlay's tooltip, naming WHICH half
    /// is missing — "unsupported" alone sends the reader to the wrong place.
    pub fn ten_bit_diagnosis(self) -> &'static str {
        match (self.ten_bit_surface, self.sixteen_bit_norm_storage) {
            (true, true) => "available",
            (false, true) => "no Rgb10a2Unorm surface format advertised",
            (true, false) => "adapter lacks TEXTURE_FORMAT_16BIT_NORM",
            (false, false) => "no 10-bit surface format and no TEXTURE_FORMAT_16BIT_NORM",
        }
    }

    /// Why HDR float is or is not selectable. Both the resource format and the meaning
    /// of its values must be supported; either one missing is a veto.
    pub fn hdr_diagnosis(self) -> &'static str {
        match (self.float_surface, self.extended_srgb_presentation) {
            (true, true) => "available",
            (false, true) => "no Rgba16Float surface format advertised",
            (true, false) => "platform cannot present encoded extended sRGB",
            (false, false) => {
                "no Rgba16Float surface format and no encoded extended-sRGB presentation hook"
            }
        }
    }
}

/// Every feature any [`OutputDepth`] can need, to be intersected with
/// `adapter.features()` in the device descriptor.
///
/// Requested UP FRONT and unconditionally, because the depth is a runtime toggle and
/// device features cannot be added after creation — so the device must be born able
/// to do whatever the toggle might later ask for. Costs nothing while unused.
pub const REQUIRED_DEVICE_FEATURES: wgpu::Features = wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;

/// SDR reference white, in cd/m². The anchor of the whole luminance convention:
/// **linear 1.0 means exactly this brightness**, in every mode.
///
/// Chosen as 100 rather than measured off any particular panel because it is what the
/// standards fix — Rec.709's reference white, HDR10's `opticalOutputScale`, and the
/// value `color(rec2100-pq …)` is defined against. It is a UNIT, not a setting: a
/// display brighter than 100 cd/m² does not change what 1.0 means, it changes how much
/// headroom exists above it.
///
/// Everything downstream follows from this one number. Both extended colour spaces put
/// reference white at signal 1.0, which is why either needs no rescaling — unlike PQ,
/// where 1.0 is [`PQ_CEILING_NITS`] and white sits near 0.5. See [`color_space`].
pub const SDR_REFERENCE_WHITE_NITS: f32 = 100.0;

/// The ceiling PQ can signal, in cd/m² — `color(rec2100-pq 1 0 0)` is this bright.
///
/// A SIGNALLING limit, not a display one, and the distinction matters: no panel made
/// reaches it, so it is an upper bound on what an encoding can *say*, never a target
/// to author toward. Present so ranges can be checked against something real instead
/// of a magic number.
pub const PQ_CEILING_NITS: f32 = 10_000.0;

/// Linear radiance → luminance in cd/m². The whole of the convention, in one multiply.
pub fn nits(linear: f32) -> f32 {
    linear * SDR_REFERENCE_WHITE_NITS
}

/// Luminance in cd/m² → the linear value that carries it.
///
/// The direction authoring goes: someone knows they want a 400 cd/m² highlight and
/// needs the number to type. Above [`PQ_CEILING_NITS`] the answer is still returned
/// rather than clamped — clamping here would hide the mistake at the point where the
/// caller could still see it, and the tone-map is what bounds the value anyway.
pub fn linear_from_nits(nits: f32) -> f32 {
    nits / SDR_REFERENCE_WHITE_NITS
}

/// The 10-bit swapchain format. Plain unorm, so the hardware applies NO transfer
/// function — see the module docs on the sRGB asymmetry.
pub const TEN_BIT_SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgb10a2Unorm;

/// The extended-range surface and storage format. Storage-capable with no feature
/// (`msaa_resolve | s_ro_wo`, `all_flags`), and filterable, so the blit's linear
/// upscale keeps working.
pub const FLOAT_SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The WGSL storage-texture type in the shipped `dda.wgsl`. Patched, so the
/// unpatched file is the 8-bit configuration — the same discipline every lever
/// group in voxel-rt's lever registry follows.
const SHIPPED_WGSL_STORAGE_FORMAT: &str = "rgba8unorm";

/// One resolved answer, consistent across all four consumers by construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputFormat {
    depth: OutputDepth,
    surface: wgpu::TextureFormat,
    storage: wgpu::TextureFormat,
}

impl OutputFormat {
    /// THE ONLY `match` ON DEPTH IN THE CODEBASE.
    ///
    /// `requested` is what the user asked for; `support` vetoes it. An unsupported
    /// request silently resolves to eight bits — [`OutputFormat::depth`] then
    /// reports what was actually chosen, so the overlay can show the truth rather
    /// than the wish.
    ///
    /// `srgb_surface_fallback` is the format `gpu.rs` already picks for the 8-bit
    /// path, threaded in rather than hardcoded because which sRGB format a surface
    /// offers is platform-dependent (`Bgra8UnormSrgb` on macOS, others elsewhere).
    pub fn resolve(
        requested: OutputDepth,
        support: OutputSupport,
        srgb_surface_fallback: wgpu::TextureFormat,
    ) -> OutputFormat {
        let depth = if support.supports(requested) {
            requested
        } else {
            OutputDepth::EightBit
        };
        match depth {
            OutputDepth::EightBit => OutputFormat {
                depth,
                surface: srgb_surface_fallback,
                storage: wgpu::TextureFormat::Rgba8Unorm,
            },
            OutputDepth::TenBit => OutputFormat {
                depth,
                surface: TEN_BIT_SURFACE_FORMAT,
                storage: wgpu::TextureFormat::Rgba16Unorm,
            },
            OutputDepth::HdrFloat => OutputFormat {
                depth,
                surface: FLOAT_SURFACE_FORMAT,
                storage: FLOAT_SURFACE_FORMAT,
            },
        }
    }

    /// The depth actually in effect, which is not always the one requested.
    pub fn depth(&self) -> OutputDepth {
        self.depth
    }

    /// Consumer 1 — the swapchain (`gpu.rs`).
    pub fn surface(&self) -> wgpu::TextureFormat {
        self.surface
    }

    /// Consumer 2 — the texture the compute pass writes (`render.rs`).
    pub fn storage(&self) -> wgpu::TextureFormat {
        self.storage
    }

    /// Consumer 4 — whether the blit undoes an sRGB encode the hardware will
    /// re-apply. True only when the surface carries the transfer function.
    pub fn blit_decodes_srgb(&self) -> bool {
        self.surface.is_srgb()
    }

    /// The device features this format needs, to be OR-ed into the device
    /// descriptor. Requesting a feature the adapter lacks fails device creation, so
    /// the caller must intersect with `adapter.features()` — which
    /// [`OutputSupport::probe`] has already checked.
    pub fn required_features(&self) -> wgpu::Features {
        match self.depth {
            // `Rgba16Float` storage is guaranteed, so HdrFloat needs no feature.
            OutputDepth::EightBit | OutputDepth::HdrFloat => wgpu::Features::empty(),
            OutputDepth::TenBit => wgpu::Features::TEXTURE_FORMAT_16BIT_NORM,
        }
    }

    /// Consumer 6 — what the compositor must be told these values mean.
    ///
    /// Derived from the RANGE rather than from the depth or the transfer function,
    /// because every mode now shares one transfer function and the range is the only
    /// thing left that differs. [`color_space::apply`] is what acts on it.
    pub fn color_space(&self) -> SurfaceColorSpace {
        if self.writes_extended_range() {
            SurfaceColorSpace::ExtendedSrgb
        } else {
            SurfaceColorSpace::Srgb
        }
    }

    /// Whether the shading pass may emit values ABOVE 1.0. The app uses this to choose an
    /// HDR-capable default curve; the selected curve itself remains a runtime uniform.
    ///
    /// Note what this no longer means. It used to also skip the sRGB encode, on the
    /// reasoning that an encode would flatten the range the way Reinhard does. That was
    /// wrong: the extended sRGB transfer is monotonic and finite above 1.0, so it
    /// PRESERVES range
    /// while Reinhard destroys it. Skipping the encode only desynchronised the shading
    /// pass from egui, which writes into the same surface on its own convention.
    pub fn writes_extended_range(&self) -> bool {
        matches!(self.depth, OutputDepth::HdrFloat)
    }

    /// The `texture_storage_2d<...>` declaration a source must carry to match this
    /// format. Exposed so consumers can ASSERT agreement instead of discovering it
    /// as a wgpu pipeline-validation error several layers down.
    pub fn wgsl_storage_declaration(&self) -> String {
        let wgsl = match self.storage {
            wgpu::TextureFormat::Rgba8Unorm => "rgba8unorm",
            wgpu::TextureFormat::Rgba16Unorm => "rgba16unorm",
            wgpu::TextureFormat::Rgba16Float => "rgba16float",
            other => panic!("no WGSL storage-format name for {other:?}"),
        };
        format!("texture_storage_2d<{wgsl}, write>")
    }

    /// Consumer 4 — patch the blit's transfer-function switch.
    ///
    /// Separate from [`Self::patch_shader_source`] because it targets a different
    /// file: the blit is its own pipeline with its own source, and coupling the two
    /// patches would mean rebuilding the shading pipeline whenever the surface
    /// format moved.
    pub fn patch_blit_source(&self, source: &str) -> String {
        patch_const(
            source,
            "BLIT_DECODES_SRGB",
            if self.blit_decodes_srgb() {
                "true"
            } else {
                "false"
            },
        )
    }

    /// Consumer 3 — patch the storage-texture TYPE into the shading source.
    ///
    /// The format is part of the WGSL type (`texture_storage_2d<rgba8unorm,
    /// write>`), not a const, so this is a source substitution rather than a value
    /// patch. Returns the source unchanged for the 8-bit path, so the shipped file
    /// really is the shipped configuration and existing pipeline cache keys are
    /// untouched.
    pub fn patch_shader_source(&self, source: &str) -> String {
        let wanted = self.wgsl_storage_declaration();
        let shipped = format!("texture_storage_2d<{SHIPPED_WGSL_STORAGE_FORMAT}, write>");
        if wanted == shipped {
            return source.to_string();
        }
        assert!(
            source.contains(&shipped),
            "`{shipped}` not found in the shading source — dda.wgsl's output \
             binding changed shape and this patcher no longer matches it"
        );
        source.replace(&shipped, &wanted)
    }
}

impl Default for OutputFormat {
    /// The shipped path, with no probe needed.
    fn default() -> OutputFormat {
        OutputFormat {
            depth: OutputDepth::EightBit,
            surface: wgpu::TextureFormat::Bgra8UnormSrgb,
            storage: wgpu::TextureFormat::Rgba8Unorm,
        }
    }
}

/// Replace a WGSL `const NAME: type = value;` declaration's value.
///
/// Private and deliberately duplicated rather than borrowed from the renderer: a crate
/// that reaches into its consumer for a string utility is the wrong dependency
/// direction, and re-exporting one would be a shim. Ten lines is the cheaper honesty.
fn patch_const(source: &str, name: &str, value: &str) -> String {
    let declaration = format!("const {name}: ");
    let start = source
        .find(&declaration)
        .unwrap_or_else(|| panic!("`{declaration}` not found in the shader source"));
    let equals = start
        + source[start..]
            .find('=')
            .expect("a const declaration must have an `=`");
    let semicolon = equals
        + source[equals..]
            .find(';')
            .expect("a const declaration must end in `;`");
    format!("{}= {value}{}", &source[..equals], &source[semicolon..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal stand-in for the shading source. The patchers only need the two
    /// declarations they rewrite, and unit-testing them against a fixture keeps this
    /// crate independent of the renderer's 3000-line shader. `voxel-rt` carries the
    /// matching integration test that the REAL shader still contains these.
    const SHADER_FIXTURE: &str = "\
@group(0) @binding(6) var output: texture_storage_2d<rgba8unorm, write>;
const OUTPUT_EXTENDED_RANGE: bool = false;
const BLIT_DECODES_SRGB: bool = true;
";

    /// The 8-bit path must leave the source BYTE-IDENTICAL, so the shipped file is the
    /// shipped configuration and no pipeline cache key moves.
    #[test]
    fn the_eight_bit_path_does_not_touch_the_shader_source() {
        let eight = OutputFormat::default();
        assert_eq!(eight.patch_shader_source(SHADER_FIXTURE), SHADER_FIXTURE);
        assert_eq!(eight.required_features(), wgpu::Features::empty());
        assert!(!eight.writes_extended_range());
    }

    /// And 10-bit really does change the binding's TYPE — the format is part of the WGSL
    /// type, so a const patch could never have done this.
    #[test]
    fn the_ten_bit_path_rewrites_the_storage_texture_type() {
        let ten = OutputFormat::resolve(
            OutputDepth::TenBit,
            full_support(),
            wgpu::TextureFormat::Bgra8UnormSrgb,
        );
        let patched = ten.patch_shader_source(SHADER_FIXTURE);
        assert!(patched.contains("texture_storage_2d<rgba16unorm, write>"));
        assert!(!patched.contains("texture_storage_2d<rgba8unorm, write>"));
        assert_eq!(
            ten.required_features(),
            wgpu::Features::TEXTURE_FORMAT_16BIT_NORM
        );
    }

    #[test]
    fn the_blit_patcher_flips_the_transfer_switch() {
        let ten = OutputFormat::resolve(
            OutputDepth::TenBit,
            full_support(),
            wgpu::TextureFormat::Bgra8UnormSrgb,
        );
        assert!(OutputFormat::default()
            .patch_blit_source(SHADER_FIXTURE)
            .contains("const BLIT_DECODES_SRGB: bool = true;"));
        assert!(ten
            .patch_blit_source(SHADER_FIXTURE)
            .contains("const BLIT_DECODES_SRGB: bool = false;"));
    }

    fn full_support() -> OutputSupport {
        OutputSupport {
            ten_bit_surface: true,
            float_surface: true,
            extended_srgb_presentation: true,
            sixteen_bit_norm_storage: true,
        }
    }

    /// The four consumers must never disagree. Ten bits of surface with eight bits
    /// of storage is the silent-failure case this module exists to prevent: it
    /// would compile, run, and gain exactly nothing.
    #[test]
    fn ten_bit_widens_the_storage_texture_too() {
        let format = OutputFormat::resolve(
            OutputDepth::TenBit,
            full_support(),
            wgpu::TextureFormat::Bgra8UnormSrgb,
        );
        assert_eq!(format.depth(), OutputDepth::TenBit);
        assert_eq!(format.surface(), wgpu::TextureFormat::Rgb10a2Unorm);
        assert_eq!(format.storage(), wgpu::TextureFormat::Rgba16Unorm);
    }

    /// The sRGB asymmetry, pinned. Getting this backwards presents a washed-out or
    /// a dark image, and neither failure names itself.
    #[test]
    fn the_blit_decodes_only_when_the_surface_carries_the_transfer_function() {
        let eight = OutputFormat::resolve(
            OutputDepth::EightBit,
            full_support(),
            wgpu::TextureFormat::Bgra8UnormSrgb,
        );
        let ten = OutputFormat::resolve(
            OutputDepth::TenBit,
            full_support(),
            wgpu::TextureFormat::Bgra8UnormSrgb,
        );
        assert!(
            eight.blit_decodes_srgb(),
            "an sRGB surface re-encodes, so the blit must hand back linear"
        );
        assert!(
            !ten.blit_decodes_srgb(),
            "Rgb10a2Unorm applies no transfer, so the blit must pass through"
        );
    }

    /// An unsupported request degrades rather than failing, and `depth()` reports
    /// what HAPPENED — so the overlay shows the truth instead of the wish.
    #[test]
    fn an_unsupported_request_falls_back_and_says_so() {
        for support in [
            OutputSupport {
                ten_bit_surface: false,
                float_surface: true,
                extended_srgb_presentation: true,
                sixteen_bit_norm_storage: true,
            },
            OutputSupport {
                ten_bit_surface: true,
                float_surface: true,
                extended_srgb_presentation: true,
                sixteen_bit_norm_storage: false,
            },
            OutputSupport::default(),
        ] {
            let format = OutputFormat::resolve(
                OutputDepth::TenBit,
                support,
                wgpu::TextureFormat::Bgra8UnormSrgb,
            );
            assert_eq!(
                format.depth(),
                OutputDepth::EightBit,
                "{support:?} cannot do ten bits and must fall back"
            );
            assert_eq!(format, OutputFormat::default());
            assert_ne!(
                support.ten_bit_diagnosis(),
                "available",
                "{support:?} must name which half is missing"
            );
        }
    }

    /// BOTH halves are required. Either one alone is a veto, which is why support
    #[test]
    fn every_depths_required_features_are_requested_at_device_creation() {
        for depth in OutputDepth::ALL {
            let format =
                OutputFormat::resolve(depth, full_support(), wgpu::TextureFormat::Bgra8UnormSrgb);
            let needed = format.required_features();
            assert!(
                REQUIRED_DEVICE_FEATURES.contains(needed),
                "{depth:?} needs {needed:?}, which the device descriptor does not request"
            );
        }
    }

    /// The HDR path swaps the tonemap and changes NOTHING else. Reinhard's ceiling is
    /// 1.0, so it has to go; the sRGB encode has no ceiling, so it stays. An earlier
    /// version of this test asserted the opposite and passed — the assertion was
    /// wrong, not the code — so the point is pinned here explicitly.
    #[test]
    fn the_hdr_float_path_swaps_only_the_tonemap() {
        let hdr = OutputFormat::resolve(
            OutputDepth::HdrFloat,
            full_support(),
            wgpu::TextureFormat::Bgra8UnormSrgb,
        );
        assert_eq!(hdr.depth(), OutputDepth::HdrFloat);
        assert_eq!(hdr.surface(), wgpu::TextureFormat::Rgba16Float);
        assert_eq!(hdr.storage(), wgpu::TextureFormat::Rgba16Float);
        assert!(hdr.writes_extended_range());
        // A float surface is not sRGB, so the blit must pass through rather than decode.
        assert!(!hdr.blit_decodes_srgb());
        assert_eq!(hdr.required_features(), wgpu::Features::empty());

        let patched = hdr.patch_shader_source(SHADER_FIXTURE);
        assert!(patched.contains("texture_storage_2d<rgba16float, write>"));

        // And the two SDR modes must leave that const alone.
        for depth in [OutputDepth::EightBit, OutputDepth::TenBit] {
            let sdr =
                OutputFormat::resolve(depth, full_support(), wgpu::TextureFormat::Bgra8UnormSrgb);
            assert!(!sdr.writes_extended_range(), "{depth:?}");
        }
    }

    /// Each mode has its OWN veto, and conflating them would disable a mode the device
    /// can do: 8-bit always works, 10-bit needs a surface format AND a device feature,
    /// HDR float needs a float surface and an encoded extended-sRGB presentation hook.
    #[test]
    fn each_depth_has_its_own_veto() {
        let float_only = OutputSupport {
            ten_bit_surface: false,
            float_surface: true,
            extended_srgb_presentation: true,
            sixteen_bit_norm_storage: false,
        };
        assert!(float_only.supports(OutputDepth::EightBit));
        assert!(!float_only.supports(OutputDepth::TenBit));
        assert!(float_only.supports(OutputDepth::HdrFloat));
    }

    #[test]
    fn hdr_needs_a_presentation_contract_not_only_a_float_format() {
        let untaggable_float = OutputSupport {
            ten_bit_surface: false,
            float_surface: true,
            extended_srgb_presentation: false,
            sixteen_bit_norm_storage: false,
        };
        assert!(!untaggable_float.supports(OutputDepth::HdrFloat));
        assert_eq!(
            untaggable_float.hdr_diagnosis(),
            "platform cannot present encoded extended sRGB"
        );
        assert_eq!(
            OutputFormat::resolve(
                OutputDepth::HdrFloat,
                untaggable_float,
                wgpu::TextureFormat::Bgra8UnormSrgb,
            ),
            OutputFormat::default()
        );
    }

    #[test]
    fn every_depth_has_a_label_and_resolves() {
        for depth in OutputDepth::ALL {
            assert!(!depth.label().is_empty());
            let format =
                OutputFormat::resolve(depth, full_support(), wgpu::TextureFormat::Bgra8UnormSrgb);
            assert_eq!(format.depth(), depth);
        }
    }

    /// The anchor, both ways. If these two ever disagree the nits shown in a tooltip
    /// stop matching the brightness on the panel, which is the kind of error that gets
    /// argued about instead of measured.
    #[test]
    fn the_nits_convention_round_trips() {
        assert_eq!(nits(1.0), SDR_REFERENCE_WHITE_NITS);
        assert_eq!(linear_from_nits(SDR_REFERENCE_WHITE_NITS), 1.0);
        assert_eq!(nits(4.0), 400.0);
        assert_eq!(linear_from_nits(400.0), 4.0);
        for linear in [0.0, 0.18, 1.0, 16.0, 100.0] {
            assert!((linear_from_nits(nits(linear)) - linear).abs() < 1e-6);
        }
    }

    /// Reference white must sit far below the signalling ceiling, and the ratio between
    /// them is the headroom PQ can express — two decades. Pinned because the pair of
    /// constants only means anything together: 100 alone could be either of them.
    #[test]
    fn the_pq_ceiling_is_a_hundred_times_reference_white() {
        assert_eq!(linear_from_nits(PQ_CEILING_NITS), 100.0);
    }
}
