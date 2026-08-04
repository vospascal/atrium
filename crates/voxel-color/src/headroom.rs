//! How much brighter than white this display can actually go, right now.
//!
//! The HDR tone curves need one number: the ratio between the display's peak luminance and
//! SDR reference white. Get it wrong in either direction and the picture suffers:
//!
//! - **Too high** — we roll highlights toward a brightness the panel cannot produce, the
//!   compositor clamps them, and the image reads bright and blown. This was the shipped
//!   behaviour: a hard-coded `4.0` that nothing ever checked.
//! - **Too low** — we compress into less range than exists and waste the display.
//!
//! **It is not a constant, and that is the whole point of this module.** EDR headroom
//! moves at runtime with the brightness slider, with thermal state, and with whether any
//! HDR content is on screen at all. A value baked into the shader is a claim about
//! hardware that was never true for more than a moment.
//!
//! The idea is borrowed from **gain maps** (ISO 21496-1, Apple "Adaptive HDR", Android
//! Ultra HDR), where an SDR base image plus a per-pixel gain map lets the *display*
//! reconstruct HDR up to whatever headroom it has. We deliberately do not use gain maps
//! themselves — they are a storage and delivery format, and we render fresh every frame,
//! so encoding a base plus a reconstruction map would be strictly more work than
//! tone-mapping to the real headroom directly. What transfers is the principle:
//! **adapt to the display you actually have, measured, not assumed.**
//!
//! ## The abstraction, and why it is a trait rather than a pile of `cfg`
//!
//! [`HeadroomProvider`] is one method. The reason it exists at all is that the platforms
//! do not merely differ in *syntax* — they differ in whether the question is answerable,
//! and a bare `#[cfg]` chain hides that. As a trait each platform becomes a **named,
//! documented type** carrying the exact API it needs, so an unimplemented backend is a
//! visible gap with instructions rather than an invisible `else` branch. It also makes
//! the manual override ([`ManualHeadroom`]) just another provider instead of a special
//! case, and lets a test inject a fake without a display.
//!
//! | platform | provider | API | verified |
//! |---|---|---|---|
//! | macOS | [`MetalScreenHeadroom`] | `NSScreen.maximumExtendedDynamicRangeColorComponentValue` | **runs on hardware** |
//! | Android / Quest | `AndroidDisplayHeadroom` | `Display.getHdrSdrRatio()` (API 34) | compiles for `aarch64-linux-android`, **untested on a device** |
//! | Windows | `DxgiOutputHeadroom` | `IDXGISwapChain::GetContainingOutput` → `IDXGIOutput6::GetDesc1` | compiles for `x86_64-pc-windows-msvc`, **untested on hardware** |
//! | Linux, web | [`UnsupportedHeadroom`] | none stable | n/a — see below |
//!
//! **Be precise about "verified".** Only macOS has been seen working. The other two
//! type-check against their real platform SDKs, which is worth more than nothing — it
//! caught `GetDesc1` returning by value rather than filling an out-param, which reading the
//! docs had not — but compiling is not running. Treat their first execution as untested.
//!
//! **Windows cannot currently be compiled from this workspace at all**, and the reason is
//! not in this crate. `gpu-allocator 0.28` accepts `windows = ">=0.53, <=0.62"`, and Cargo
//! unifies it onto `0.61` because `sysinfo` (via `bevy_egui` → `atrium-bevy`) requires
//! `^0.61`, while `wgpu-hal 29` needs `0.62` — so `wgpu-hal`'s DX12 backend fails to build
//! with ten type errors before reaching our code. Checked in a workspace containing only
//! this crate, it builds clean. The fix is to stop sharing a lockfile with the bevy crate
//! (a separate workspace for the renderer) or to wait for `sysinfo` to move to `0.62`;
//! either way it is a workspace problem, not a provider problem.
//!
//! **Linux is deliberately skipped, not overlooked.** X11 has no API. Wayland's
//! `wp_color_management_v1` is only recently stabilising, and until it is something to
//! depend on, [`ManualHeadroom`] is the honest answer there.
//!
//! Web is a special case worth naming: `matchMedia("(dynamic-range: high)")` answers
//! *whether* the display is HDR but not *how much* headroom it has, so it cannot drive a
//! tonemap. There is no ratio to read.

use crate::OutputFormat;

/// The conservative fallback when nothing can be measured: **no headroom at all**.
///
/// Chosen as 1.0 rather than an optimistic guess because the two failure modes are not
/// symmetric. Claiming headroom that does not exist produces the blown-highlight picture
/// this module was written to fix; claiming none produces a hard clip at white, which is
/// just SDR — correct, if unexciting. A guess in the middle would be folklore, and this
/// codebase has paid for folklore already.
///
/// Android's `getHdrSdrRatio()` returns this same 1.0 when its ratio is unavailable,
/// which is a small confirmation that the conservative choice is the conventional one.
///
/// The consequence is worth stating plainly: on a platform that can present HDR but whose
/// headroom probe is temporarily unreachable, `HdrFloat` falls back to the SDR curve until
/// a measurement lands or [`ManualHeadroom`] is used. Platforms without a compatible
/// presentation hook veto `HdrFloat` before this value is consulted.
pub const UNMEASURED_HEADROOM: f32 = 1.0;

/// What the user's SDR white actually sits at, in cd/m², when a platform reports absolute
/// luminance instead of a ratio.
///
/// **NOT a shader parameter, and that is the design decision worth recording.** Reference
/// implementations pass `paper_white_nits` into the tonemap alongside `peak_nits`, because
/// they carry ABSOLUTE luminance through the shader. Ours is RELATIVE — 1.0 is SDR white by
/// definition — so paper white cannot change the shader maths at all. What it changes is
/// the conversion from a platform's absolute nits into our ratio, which happens here.
///
/// Where it matters, per platform:
///
/// - **macOS and Android already account for it.**
///   `maximumExtendedDynamicRangeColorComponentValue` and `getHdrSdrRatio()` both report a
///   RATIO against the system's current SDR white point, so the user's brightness setting
///   is already folded in. Nothing to correct.
/// - **Windows does not.** `DXGI_OUTPUT_DESC1` gives the panel's peak in absolute nits and
///   says nothing about SDR white, which Windows lets the user set independently. Dividing
///   by the 100 cd/m² standard over-reports headroom by roughly the ratio between them —
///   the dangerous direction.
///
/// 200 rather than 100 because that is where Windows' "SDR content brightness" slider
/// lands by default in HDR mode, not because it is a standard. It is an assumption, and the
/// honest fix is a paper-white control the user sets once for their display — which is what
/// shipping games expose, for exactly this reason. Reading it properly needs
/// `QueryDisplayConfig` plus `DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL` and a
/// path-to-output match.
pub const ASSUMED_WINDOWS_SDR_WHITE_NITS: f32 = 200.0;

/// The largest headroom we will believe from a display.
///
/// A sanity bound, not a policy. A bad value — or a platform arm returning something
/// uninitialised — must not reach the tonemap and produce `inf`. Sixteen times reference
/// white is 1600 cd/m², the peak of the brightest panel we target, so anything beyond it
/// is a bug rather than a display.
pub const MAX_BELIEVABLE_HEADROOM: f32 = 16.0;

/// Where a headroom value came from. Reported so the overlay can distinguish "your
/// display says 1.6x" from "we have no idea and assumed 1.0" — a distinction invisible in
/// the number alone, and precisely what went wrong before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadroomSource {
    /// Read from the display, this frame.
    Measured,
    /// Supplied by the caller, because this platform cannot be asked.
    Manual,
    /// The platform HAS an API and we have not written the binding yet. Distinct from
    /// [`HeadroomSource::PlatformUnsupported`] because it is a task, not a limit.
    Unimplemented,
    /// No API exists to ask. Linux/X11, and Wayland until colour management stabilises.
    PlatformUnsupported,
    /// The platform has an API but the display could not be reached — not Metal-backed,
    /// a software adapter, or a window not yet attached to a screen.
    DisplayUnreachable,
}

impl HeadroomSource {
    /// One line for the overlay, in the shape of
    /// [`crate::ColorSpaceOutcome::diagnosis`].
    pub fn diagnosis(self) -> &'static str {
        match self {
            HeadroomSource::Measured => "measured",
            HeadroomSource::Manual => "set by hand",
            HeadroomSource::Unimplemented => "assumed — no backend for this platform yet",
            HeadroomSource::PlatformUnsupported => "assumed — this platform cannot be asked",
            HeadroomSource::DisplayUnreachable => "assumed — display not reachable",
        }
    }

    /// Whether the number beside this source is a fact about the hardware.
    pub fn is_trustworthy(self) -> bool {
        matches!(self, HeadroomSource::Measured | HeadroomSource::Manual)
    }
}

/// A display's headroom as a multiple of SDR reference white, plus where it came from.
///
/// Always at least 1.0 and never above [`MAX_BELIEVABLE_HEADROOM`], enforced in the
/// constructors so the tonemap can never receive a value that produces `inf` or a
/// negative `room`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayHeadroom {
    ratio: f32,
    source: HeadroomSource,
}

impl Default for DisplayHeadroom {
    fn default() -> DisplayHeadroom {
        DisplayHeadroom {
            ratio: UNMEASURED_HEADROOM,
            source: HeadroomSource::PlatformUnsupported,
        }
    }
}

impl DisplayHeadroom {
    /// Clamp anything into the believable band. NaN resolves to no headroom rather than
    /// propagating: `f32::clamp` returns NaN for a NaN value, and a NaN reaching a float
    /// surface is a black or garbage pixel rather than a clamped one.
    fn sanitise(ratio: f32) -> f32 {
        if ratio.is_nan() {
            UNMEASURED_HEADROOM
        } else {
            ratio.clamp(UNMEASURED_HEADROOM, MAX_BELIEVABLE_HEADROOM)
        }
    }

    fn new(ratio: f32, source: HeadroomSource) -> DisplayHeadroom {
        DisplayHeadroom {
            ratio: Self::sanitise(ratio),
            source,
        }
    }

    /// A value read from the display.
    pub fn measured(ratio: f32) -> DisplayHeadroom {
        DisplayHeadroom::new(ratio, HeadroomSource::Measured)
    }

    /// A value the user or a config supplied, for platforms that cannot be asked.
    pub fn manual(ratio: f32) -> DisplayHeadroom {
        DisplayHeadroom::new(ratio, HeadroomSource::Manual)
    }

    /// No headroom claimed, and why.
    pub fn unmeasured(source: HeadroomSource) -> DisplayHeadroom {
        DisplayHeadroom {
            ratio: UNMEASURED_HEADROOM,
            source,
        }
    }

    /// The multiple of SDR reference white this display can reach. Feeds
    /// `apply_tonemap`'s `headroom` argument directly.
    pub fn ratio(self) -> f32 {
        self.ratio
    }

    pub fn source(self) -> HeadroomSource {
        self.source
    }

    /// Peak luminance in cd/m², for display beside the ratio — `1.6x` means little,
    /// `160 cd/m²` means something.
    pub fn peak_nits(self) -> f32 {
        crate::nits(self.ratio)
    }

    /// Whether there is any headroom to tone-map into. At exactly 1.0 every bounded HDR
    /// curve is confined to SDR range, and the default Reinhard+HDR curve becomes plain
    /// Reinhard exactly.
    pub fn has_headroom(self) -> bool {
        self.ratio > 1.0
    }
}

/// One way of asking a display how much headroom it has.
///
/// Implementations are per-platform and are expected to be cheap enough to call per
/// frame — headroom changes while the user drags the brightness slider, so a value cached
/// at startup is the bug this abstraction exists to prevent. An implementation that
/// cannot be cheap should cache internally rather than making callers guess a cadence.
pub trait HeadroomProvider {
    /// For the overlay and the startup log, so an unimplemented backend names itself.
    fn name(&self) -> &'static str;

    /// Ask the display. Must never panic and must never return an out-of-band ratio —
    /// use [`DisplayHeadroom`]'s constructors, which clamp.
    fn probe(&self, surface: &wgpu::Surface<'_>) -> DisplayHeadroom;
}

/// The provider for the platform this binary was built for.
///
/// Returned boxed rather than as a concrete type so the caller stores one field
/// regardless of target, and so [`ManualHeadroom`] can be substituted without the
/// call site changing shape.
pub fn platform_provider() -> Box<dyn HeadroomProvider> {
    #[cfg(target_os = "macos")]
    return Box::new(MetalScreenHeadroom);
    #[cfg(target_os = "windows")]
    return Box::new(DxgiOutputHeadroom);
    #[cfg(target_os = "android")]
    return Box::new(AndroidDisplayHeadroom);
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
    return Box::new(UnsupportedHeadroom);
}

/// A fixed value, for platforms with no provider and for anyone who knows their display
/// better than the API does.
///
/// Reports [`HeadroomSource::Manual`], so the overlay shows it as deliberate rather than
/// as a measurement — the distinction matters when someone later wonders why the
/// highlights clip where they do.
pub struct ManualHeadroom(pub f32);

impl HeadroomProvider for ManualHeadroom {
    fn name(&self) -> &'static str {
        "manual"
    }

    fn probe(&self, _surface: &wgpu::Surface<'_>) -> DisplayHeadroom {
        DisplayHeadroom::manual(self.0)
    }
}

/// Nothing to ask. Linux/X11 has no API at all; Wayland's `wp_color_management_v1` is
/// only recently stabilising and is not yet something to depend on. The web exposes
/// `matchMedia("(dynamic-range: high)")`, which is a boolean and cannot answer *how
/// much*.
pub struct UnsupportedHeadroom;

impl HeadroomProvider for UnsupportedHeadroom {
    fn name(&self) -> &'static str {
        "unsupported"
    }

    fn probe(&self, _surface: &wgpu::Surface<'_>) -> DisplayHeadroom {
        DisplayHeadroom::unmeasured(HeadroomSource::PlatformUnsupported)
    }
}

/// **Android and therefore Quest — the next one to write, and the easiest to get right.**
///
/// `Display.getHdrSdrRatio()` (API 34) is documented as
/// `targetHdrPeakBrightnessInNits / targetSdrWhitePointInNits`, which is exactly
/// [`DisplayHeadroom::ratio`] with no conversion, and returns `1.0` when
/// `isHdrSdrRatioAvailable()` is false, which is exactly [`UNMEASURED_HEADROOM`]. The
/// semantics need no interpretation at all.
///
/// What it needs:
///
/// 1. a `Display` — via JNI from the `android-activity` `AndroidApp`, not from the wgpu
///    surface, since the surface knows nothing about the display,
/// 2. `isHdrSdrRatioAvailable()` first: on false, `getHdrSdrRatio()` returns 1.0 anyway,
///    but reporting [`HeadroomSource::PlatformUnsupported`] rather than a measured 1.0 is
///    the honest answer,
/// 3. ideally `registerHdrSdrRatioChangedListener` rather than polling, because a JNI
///    call per frame is a very different cost from three Objective-C sends.
///
/// Below API 34 the fallback is `getHdrCapabilities().getDesiredMaxLuminance()`, which is
/// in nits and so needs dividing by the SDR white point — a capability rather than a
/// current value, so it will over-report and should be treated as
/// [`HeadroomSource::Manual`] at best.
#[cfg(target_os = "android")]
pub struct AndroidDisplayHeadroom;

#[cfg(target_os = "android")]
impl HeadroomProvider for AndroidDisplayHeadroom {
    fn name(&self) -> &'static str {
        "android Display.getHdrSdrRatio"
    }

    fn probe(&self, _surface: &wgpu::Surface<'_>) -> DisplayHeadroom {
        // The surface is not the route: the ratio belongs to the Display, which is reached
        // from the Activity. `ndk_context` is how every Rust Android app gets there, and
        // winit already populates it.
        let context = ndk_context::android_context();
        if context.vm().is_null() || context.context().is_null() {
            return DisplayHeadroom::unmeasured(HeadroomSource::DisplayUnreachable);
        }

        // SAFETY: `ndk_context` hands out the process's JavaVM pointer, valid for as long
        // as the app is running. `from_raw` only validates and wraps it.
        let vm = match unsafe { jni::JavaVM::from_raw(context.vm().cast()) } {
            Ok(vm) => vm,
            Err(_) => return DisplayHeadroom::unmeasured(HeadroomSource::DisplayUnreachable),
        };
        let Ok(mut env) = vm.attach_current_thread() else {
            return DisplayHeadroom::unmeasured(HeadroomSource::DisplayUnreachable);
        };

        // SAFETY: same source, same lifetime — the Activity outlives any frame.
        let activity = unsafe { jni::objects::JObject::from_raw(context.context().cast()) };

        let read = |env: &mut jni::JNIEnv| -> jni::errors::Result<Option<f32>> {
            // Activity.getDisplay() — API 30+, and the correct call on Android 11 and
            // later. WindowManager.getDefaultDisplay() is deprecated and, more to the
            // point, returns the *default* display rather than the one this activity is
            // on, which is the same "wrong screen" mistake `mainScreen` would be on macOS.
            let display = env
                .call_method(&activity, "getDisplay", "()Landroid/view/Display;", &[])?
                .l()?;
            if display.is_null() {
                return Ok(None);
            }
            // isHdrSdrRatioAvailable() first. When false, getHdrSdrRatio() still returns
            // 1.0, so skipping this check would report a MEASURED 1.0 on a device that
            // simply cannot answer — a guess wearing a measurement's label, which is
            // exactly the confusion `HeadroomSource` exists to prevent.
            let available = env
                .call_method(&display, "isHdrSdrRatioAvailable", "()Z", &[])?
                .z()?;
            if !available {
                return Ok(None);
            }
            let ratio = env
                .call_method(&display, "getHdrSdrRatio", "()F", &[])?
                .f()?;
            Ok(Some(ratio))
        };

        match read(&mut env) {
            Ok(Some(ratio)) => DisplayHeadroom::measured(ratio),
            // Available-but-false is a real answer about the hardware, not a failure to
            // ask, so it reports as unsupported rather than unreachable.
            Ok(None) => DisplayHeadroom::unmeasured(HeadroomSource::PlatformUnsupported),
            Err(_) => {
                // A pending Java exception left unhandled poisons the next JNI call, so it
                // has to be cleared even though we are discarding it. `getHdrSdrRatio` is
                // API 34, so on an older device this is a NoSuchMethodError rather than
                // anything wrong.
                let _ = env.exception_clear();
                DisplayHeadroom::unmeasured(HeadroomSource::PlatformUnsupported)
            }
        }
    }
}

/// **Windows.** `IDXGIOutput6::GetDesc1` fills `DXGI_OUTPUT_DESC1`, whose `MaxLuminance`
/// and `MaxFullFrameLuminance` are in nits.
///
/// Two lookups, not one: Windows lets the user set the SDR white level independently, so
/// the ratio is `MaxLuminance / SdrWhiteLevel` and both have to be read — unlike macOS
/// and Android, which hand over the ratio directly.
///
/// Reaching the output means `Surface::as_hal::<hal::api::Dx12>`, then the adapter's
/// outputs, then finding the one the window occupies — the same "which display is the
/// window actually on" problem [`MetalScreenHeadroom`] solves by walking to the window's
/// screen, and for the same reason: a laptop with an external monitor has two displays
/// with different headroom, and picking the primary is wrong about half the time.
///
/// Prefer `MaxFullFrameLuminance` over `MaxLuminance` for a full-screen render: the
/// latter is a small-highlight peak that a whole bright frame cannot sustain, so using it
/// would over-commit in exactly the way this module exists to stop.
#[cfg(target_os = "windows")]
pub struct DxgiOutputHeadroom;

#[cfg(target_os = "windows")]
impl HeadroomProvider for DxgiOutputHeadroom {
    fn name(&self) -> &'static str {
        "windows IDXGIOutput6::GetDesc1"
    }

    fn probe(&self, surface: &wgpu::Surface<'_>) -> DisplayHeadroom {
        use windows::core::Interface;
        use windows::Win32::Graphics::Dxgi::IDXGIOutput6;

        // SAFETY: read-only. We borrow the swapchain wgpu already owns and only query it.
        let Some(dx12_surface) = (unsafe { surface.as_hal::<wgpu::hal::api::Dx12>() }) else {
            // Vulkan on Windows lands here. `VK_EXT_hdr_metadata` is a *setter*, not a
            // getter, so there is no equivalent query — a Vulkan build needs
            // `ManualHeadroom` or the DisplayConfig route below.
            return DisplayHeadroom::unmeasured(HeadroomSource::DisplayUnreachable);
        };
        let Some(swap_chain) = dx12_surface.swap_chain() else {
            return DisplayHeadroom::unmeasured(HeadroomSource::DisplayUnreachable);
        };

        // GetContainingOutput is the whole reason this is clean: it returns the output the
        // swapchain's window is actually on. Enumerating the adapter's outputs and taking
        // the first would be the same "wrong display" bug that `mainScreen` would be on
        // macOS, and it matters most on exactly the setup that exposes it — a laptop with
        // an external monitor of different capability.
        let output = match unsafe { swap_chain.GetContainingOutput() } {
            Ok(output) => output,
            // Documented to fail while the swapchain is in fullscreen-transition or the
            // window straddles no output yet. Transient, so keep the conservative answer
            // and let the next frame's probe succeed.
            Err(_) => return DisplayHeadroom::unmeasured(HeadroomSource::DisplayUnreachable),
        };
        let Ok(output6) = output.cast::<IDXGIOutput6>() else {
            // IDXGIOutput6 arrived in Windows 10 1703. Older than that has no luminance
            // query at all, which is a platform limit rather than a missing binding.
            return DisplayHeadroom::unmeasured(HeadroomSource::PlatformUnsupported);
        };

        // Returns the struct by value in `windows` 0.62 rather than filling an out-param —
        // the crate wraps the raw C signature.
        let Ok(description) = (unsafe { output6.GetDesc1() }) else {
            return DisplayHeadroom::unmeasured(HeadroomSource::DisplayUnreachable);
        };

        // MaxFullFrameLuminance, NOT MaxLuminance. The latter is a small-highlight peak
        // that a panel cannot sustain across a whole frame, and we render full frames — so
        // using it would over-commit the tonemap in precisely the way this module exists
        // to prevent. On panels that do not distinguish them the two are equal, so the
        // conservative choice costs nothing there.
        let peak_nits = if description.MaxFullFrameLuminance > 0.0 {
            description.MaxFullFrameLuminance
        } else {
            description.MaxLuminance
        };
        if peak_nits <= 0.0 {
            // Reported as zero on an SDR output, and on some drivers even in HDR mode.
            return DisplayHeadroom::unmeasured(HeadroomSource::PlatformUnsupported);
        }

        // Divided by the ASSUMED SDR white, not the 100 cd/m² standard — see
        // `ASSUMED_WINDOWS_SDR_WHITE_NITS` for why Windows is the only platform that needs
        // this and why the number is a guess. macOS and Android report a ratio and have
        // already folded their SDR white point in.
        DisplayHeadroom::measured(peak_nits / ASSUMED_WINDOWS_SDR_WHITE_NITS)
    }
}

/// **macOS.** Walk up from the `CAMetalLayer` to the `NSView` that owns it, then to its
/// window and that window's screen.
///
/// **The window's screen, not `NSScreen::mainScreen`.** A laptop with an external monitor
/// has two displays with different headroom, and which one the window is on is the entire
/// question — using the main screen would report the wrong display's capability roughly
/// half the time, which is worse than reporting none.
///
/// The traversal mirrors the one `wgpu-hal` performs for its occlusion workaround
/// (`wgpu-hal-29.0.4/src/metal/surface.rs:135`): the `CAMetalLayer` is normally a
/// sublayer, so the layer that has a delegate is the one to ask. Untyped `msg_send!` for
/// the same reason wgpu-hal uses it — the delegate arrives as a `CALayerDelegate` protocol
/// object, and downcasting it to a typed `NSView` would pull all of AppKit in to read one
/// float.
#[cfg(target_os = "macos")]
pub struct MetalScreenHeadroom;

#[cfg(target_os = "macos")]
impl HeadroomProvider for MetalScreenHeadroom {
    fn name(&self) -> &'static str {
        "macOS NSScreen EDR headroom"
    }

    fn probe(&self, surface: &wgpu::Surface<'_>) -> DisplayHeadroom {
        use objc2::rc::Retained;
        use objc2::runtime::NSObject;
        use objc2_quartz_core::CALayer;

        // SAFETY: read-only. We clone the layer handle out from under wgpu-hal's lock and
        // release the lock before walking, so we cannot deadlock against `configure` or
        // `acquire_texture`.
        let Some(metal_surface) = (unsafe { surface.as_hal::<wgpu::hal::api::Metal>() }) else {
            return DisplayHeadroom::unmeasured(HeadroomSource::DisplayUnreachable);
        };
        let root: Retained<CALayer> =
            Retained::into_super(metal_surface.render_layer().lock().clone());

        let mut current = Some(root);
        while let Some(layer) = current {
            if let Some(delegate) = layer.delegate() {
                // SAFETY: `window`, `screen` and the headroom property are documented
                // AppKit selectors on the objects this chain yields, and each returns nil
                // rather than raising when unavailable — a window not yet on screen has
                // no `screen` — so every step is checked.
                let window: Option<Retained<NSObject>> =
                    unsafe { objc2::msg_send![&*delegate, window] };
                let screen: Option<Retained<NSObject>> = match window {
                    Some(window) => unsafe { objc2::msg_send![&*window, screen] },
                    None => None,
                };
                if let Some(screen) = screen {
                    let ratio: f64 = unsafe {
                        objc2::msg_send![&*screen, maximumExtendedDynamicRangeColorComponentValue]
                    };
                    return DisplayHeadroom::measured(ratio as f32);
                }
                // The delegate IS the view. If it has no window or no screen there is
                // nothing further up worth asking, so stop rather than walking past it.
                break;
            }
            current = layer.superlayer();
        }
        DisplayHeadroom::unmeasured(HeadroomSource::DisplayUnreachable)
    }
}

/// What the user asked for: trust the display, or force a value to see what it does.
///
/// Exists because "the display says 1.6x" is unfalsifiable from the outside — if the
/// picture looks wrong you cannot tell whether the measurement or the tonemap is at
/// fault. Being able to pin the ratio and watch the image change separates the two, and
/// pinning it to `4.0` reproduces the old hard-coded behaviour exactly, which makes the
/// bug this replaced visible rather than merely described.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum HeadroomChoice {
    /// Ask the platform provider every frame. The only setting that tracks the brightness
    /// slider, and the only correct one for shipping.
    #[default]
    Auto,
    /// Force a ratio, reported as [`HeadroomSource::Manual`] so it is never mistaken for a
    /// measurement. Also the only way to get headroom at all on a platform without a
    /// provider.
    Fixed(f32),
}

impl HeadroomChoice {
    /// The selector's options.
    ///
    /// `1x` is included because it is genuinely informative rather than a degenerate
    /// case: it shows what every platform without a provider currently does, and what an
    /// HDR surface looks like with nothing to tone-map into. `4x` is the value that used
    /// to be hard-coded. `16x` is 1600 cd/m², the XDR peak, so the top of the list is a
    /// real panel rather than an arbitrary number.
    pub const PRESETS: [HeadroomChoice; 6] = [
        HeadroomChoice::Auto,
        HeadroomChoice::Fixed(1.0),
        HeadroomChoice::Fixed(2.0),
        HeadroomChoice::Fixed(4.0),
        HeadroomChoice::Fixed(8.0),
        HeadroomChoice::Fixed(16.0),
    ];

    /// Short label for the selector. Allocates, which is fine for a per-frame UI string
    /// and keeps the ratio formatting in one place rather than at the call site.
    pub fn label(self) -> String {
        match self {
            HeadroomChoice::Auto => "Auto".to_string(),
            HeadroomChoice::Fixed(ratio) => format!("{}x", FixedLabel(ratio)),
        }
    }

    /// Resolve to an actual headroom.
    ///
    /// **The single entry point**, so there is no second way to obtain a headroom that
    /// could bypass either the SDR short-circuit or the override. Short-circuits to
    /// [`DisplayHeadroom::default`] for any mode that cannot use headroom, so a caller
    /// need not check the depth first — an SDR surface has none to report by definition,
    /// and showing a display's capability beside a mode that cannot reach it would
    /// mislead. Note this means the override is ignored in SDR rather than silently
    /// changing a picture the setting does not apply to.
    pub fn resolve(
        self,
        provider: &dyn HeadroomProvider,
        surface: &wgpu::Surface<'_>,
        format: &OutputFormat,
    ) -> DisplayHeadroom {
        if !format.writes_extended_range() {
            return DisplayHeadroom::default();
        }
        match self {
            HeadroomChoice::Auto => provider.probe(surface),
            HeadroomChoice::Fixed(ratio) => DisplayHeadroom::manual(ratio),
        }
    }
}

/// Formats a ratio without a trailing `.0`, so the selector reads `4x` rather than `4.0x`.
struct FixedLabel(f32);

impl core::fmt::Display for FixedLabel {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0.fract() == 0.0 {
            write!(formatter, "{}", self.0 as i32)
        } else {
            write!(formatter, "{}", self.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tonemap divides by `room = headroom - 1`, so a value below 1.0 would make
    /// `room` negative and invert the highlight rolloff. Every constructor has to close
    /// that off, including against whatever a broken platform arm might return.
    #[test]
    fn no_constructor_can_produce_an_unusable_ratio() {
        for candidate in [
            f32::NAN,
            f32::NEG_INFINITY,
            f32::INFINITY,
            -1.0,
            0.0,
            0.999,
            1.0,
            4.0,
            1.0e9,
        ] {
            for headroom in [
                DisplayHeadroom::measured(candidate),
                DisplayHeadroom::manual(candidate),
            ] {
                let ratio = headroom.ratio();
                assert!(
                    ratio.is_finite(),
                    "{candidate} produced a non-finite ratio {ratio}"
                );
                assert!(
                    (UNMEASURED_HEADROOM..=MAX_BELIEVABLE_HEADROOM).contains(&ratio),
                    "{candidate} escaped the believable band as {ratio}"
                );
            }
        }
    }

    /// NaN is called out separately because it is the one input that survives a naive
    /// `clamp`: `f32::clamp` returns NaN for a NaN value, and NaN in a float surface is a
    /// garbage pixel rather than a clamped one.
    #[test]
    fn a_nan_reading_becomes_no_headroom_rather_than_propagating() {
        let headroom = DisplayHeadroom::measured(f32::NAN);
        assert_eq!(headroom.ratio(), UNMEASURED_HEADROOM);
        assert!(!headroom.has_headroom());
    }

    /// The default must be conservative AND must admit it is a guess. A default that
    /// claimed headroom would reproduce the original bug on every platform without a
    /// provider, silently.
    #[test]
    fn the_default_claims_no_headroom_and_says_it_is_unmeasured() {
        let headroom = DisplayHeadroom::default();
        assert_eq!(headroom.ratio(), UNMEASURED_HEADROOM);
        assert!(!headroom.has_headroom());
        assert!(!headroom.source().is_trustworthy());
    }

    /// A measurement is only useful if the overlay can tell it apart from a guess, and
    /// every reason for a guess has to read differently — "no backend yet" and "this
    /// platform cannot be asked" are a task and a limit respectively, and conflating them
    /// is how a gap becomes permanent.
    #[test]
    fn every_source_is_distinguishable() {
        assert!(DisplayHeadroom::measured(2.0).source().is_trustworthy());
        assert!(DisplayHeadroom::manual(2.0).source().is_trustworthy());
        assert!(!DisplayHeadroom::default().source().is_trustworthy());

        let sources = [
            HeadroomSource::Measured,
            HeadroomSource::Manual,
            HeadroomSource::Unimplemented,
            HeadroomSource::PlatformUnsupported,
            HeadroomSource::DisplayUnreachable,
        ];
        for (index, source) in sources.iter().enumerate() {
            for other in &sources[index + 1..] {
                assert_ne!(source.diagnosis(), other.diagnosis());
            }
        }
    }

    /// Ratios are reported in nits too, on the same convention as everything else in this
    /// crate — 1.0 is 100 cd/m², so the brightest panel we target reads as 16x.
    #[test]
    fn peak_nits_follows_the_crate_convention() {
        assert_eq!(DisplayHeadroom::measured(1.0).peak_nits(), 100.0);
        assert_eq!(DisplayHeadroom::measured(16.0).peak_nits(), 1600.0);
        assert_eq!(
            DisplayHeadroom::measured(MAX_BELIEVABLE_HEADROOM).peak_nits(),
            1600.0,
            "the believable ceiling should be the brightest panel we target, stated in \
             nits so the two constants cannot drift apart"
        );
    }

    /// Paper white is a Windows-only correction, and the reason is worth pinning: the two
    /// platforms that report a RATIO have already folded their SDR white point in, so
    /// applying it again would double-count. Only the platform reporting absolute nits
    /// needs to divide by anything.
    #[test]
    fn paper_white_is_a_windows_only_correction() {
        // A 1000 cd/m² panel against Windows' default SDR white is 5x, not 10x. Using the
        // 100 cd/m² standard instead would claim twice the headroom that exists — the
        // over-claim that produces blown highlights.
        assert_eq!(1000.0 / ASSUMED_WINDOWS_SDR_WHITE_NITS, 5.0);
        assert_eq!(1000.0 / crate::SDR_REFERENCE_WHITE_NITS, 10.0);
        // The two numbers above ARE the claim: 5x versus 10x for the same panel. A higher
        // assumed white yields a lower reported headroom, and under-claiming is the safe
        // direction, so the Windows assumption being the larger one is what makes it the
        // conservative one.
    }

    /// A provider must be substitutable without the call site changing — the reason this
    /// is a trait. `ManualHeadroom` is the one implementation that needs no display, so
    /// it is also the one a test can exercise.
    #[test]
    fn a_manual_provider_reports_its_value_as_deliberate() {
        let provider = ManualHeadroom(4.0);
        assert_eq!(provider.name(), "manual");

        // Exercised through the trait object, so this pins dyn-compatibility too — the
        // whole point is that `platform_provider` can return any of these boxed.
        let boxed: Box<dyn HeadroomProvider> = Box::new(ManualHeadroom(4.0));
        assert_eq!(boxed.name(), "manual");
    }

    /// Every provider must clamp, not just the constructors, and a provider that ignores
    /// the surface can be checked without one.
    #[test]
    fn a_manual_provider_still_clamps() {
        assert_eq!(
            DisplayHeadroom::manual(1.0e9).ratio(),
            MAX_BELIEVABLE_HEADROOM
        );
        assert_eq!(DisplayHeadroom::manual(-5.0).ratio(), UNMEASURED_HEADROOM);
    }
}
