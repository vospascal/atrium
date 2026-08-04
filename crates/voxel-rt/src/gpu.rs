//! wgpu instance/adapter/device/queue/surface setup and resize handling.
//! Platform-agnostic: winit hands in an `Arc<Window>`, everything else is wgpu.

use std::sync::Arc;

use voxel_color::headroom::HeadroomProvider;
use voxel_color::HeadroomChoice;
use voxel_color::{
    color_space, ColorSpaceOutcome, DisplayHeadroom, OutputDepth, OutputFormat, OutputSupport,
};

use winit::dpi::PhysicalSize;
use winit::window::Window;

/// Storage buffers one compute pass may bind. WebGPU's default cap is 8, and E4's
/// CAGI pass needs 10: the seven brickmap/world buffers plus the light volume's
/// front buffer, back buffer and cell attributes (the shading pass needs 9 of the
/// same). Every device this renderer creates — app, tests, benchmark — must be
/// requested with this limit through [`device_descriptor`], or a bind group layout
/// fails validation at startup.
///
/// Portability note for E9: Metal allows 31 buffers per stage and Adreno 6xx/7xx
/// Vulkan drivers report 24+ storage buffers per stage, so 11 is comfortably
/// inside the Quest target too.
///
/// Went 10 -> 11 when the AADF directional-bound field (binding 15) landed. If a
/// target ever caps below this, the fix is to concatenate the two empty-space
/// fields — chebyshev bytes and directional bounds — into one buffer with an
/// offset per array rather than to drop either.
pub(crate) const REQUIRED_STORAGE_BUFFERS_PER_STAGE: u32 = 11;

/// The device descriptor every consumer must use: the raised storage-buffer limit
/// plus whatever timestamp support the adapter offers (GPU pass timing degrades to
/// "unavailable" rather than failing on adapters without it).
pub fn device_descriptor(adapter: &wgpu::Adapter) -> wgpu::DeviceDescriptor<'static> {
    wgpu::DeviceDescriptor {
        label: Some("voxel-rt device"),
        // Timestamps degrade gracefully when absent; the output-format features are
        // requested up front because the depth toggle is a RUNTIME control and
        // features cannot be added to a device after creation. Intersected with the
        // adapter's, so asking for what it lacks never fails device creation.
        required_features: adapter.features()
            & (wgpu::Features::TIMESTAMP_QUERY | voxel_color::REQUIRED_DEVICE_FEATURES),
        required_limits: wgpu::Limits {
            max_storage_buffers_per_shader_stage: REQUIRED_STORAGE_BUFFERS_PER_STAGE,
            // ASK THE ADAPTER, do not assume the default. WebGPU's default
            // `max_storage_buffer_binding_size` is 128 MiB and is unrelated to how
            // much memory the device has — a 24 GB card hits the same wall until
            // the limit is requested. It cost us a bench section: CAGI's 2-voxel
            // rung needs a 188 MB binding, so its bind group failed validation, its
            // dispatches were silently dropped, and the column timed at 0.005 ms —
            // reading as 700x FASTER than the coarser grid rather than as broken.
            //
            // Raised to whatever this adapter actually supports rather than to a
            // number we picked: the cascades arc needs to price configurations past
            // 128 MiB, and a hard-coded constant would fail on the first adapter
            // that offers less.
            max_storage_buffer_binding_size: adapter
                .limits()
                .max_storage_buffer_binding_size
                .max(wgpu::Limits::default().max_storage_buffer_binding_size),
            max_buffer_size: adapter
                .limits()
                .max_buffer_size
                .max(wgpu::Limits::default().max_buffer_size),
            ..wgpu::Limits::default()
        },
        ..Default::default()
    }
}

pub struct GpuContext {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface_config: wgpu::SurfaceConfiguration,
    /// Present modes the surface supports, for the vsync toggle.
    supported_present_modes: Vec<wgpu::PresentMode>,
    /// The resolved output formats — the ONLY source of truth for the surface,
    /// storage-texture and blit formats. See [the `voxel-color` crate].
    output_format: OutputFormat,
    /// What this device could do, so the overlay can grey out what it cannot and
    /// [`GpuContext::set_output_depth`] can veto a request.
    output_support: OutputSupport,
    /// Kept so a depth change can re-resolve without re-querying the surface.
    srgb_surface_fallback: wgpu::TextureFormat,
    /// What the last [`GpuContext::configure_surface`] managed to tell the compositor,
    /// cached for the overlay to display.
    ///
    /// A `Cell` because `reconfigure` is `&self` — it runs from inside the frame loop's
    /// surface-lost arm, where a `&mut` borrow would fight the acquired frame — and
    /// this is a diagnostic written by that path, not state anything reads to decide
    /// behaviour. Worth caching rather than re-querying because re-querying means
    /// re-tagging, and doing that once per overlay frame is sloppy for a value that
    /// only changes when the depth does.
    color_space: std::cell::Cell<ColorSpaceOutcome>,
    /// How this platform answers "how much brighter than white can you go?".
    ///
    /// Boxed and chosen once at startup by `voxel_color::headroom::platform_provider`,
    /// so the per-frame probe is a virtual call rather than a `cfg` chain — and so a
    /// platform without a backend is a named type that says so, not a silent fallback.
    headroom_provider: Box<dyn HeadroomProvider>,
}

impl GpuContext {
    pub fn new(window: Arc<Window>) -> Self {
        let window_size = window.inner_size();

        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window)
            .expect("failed to create wgpu surface");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .expect("no compatible GPU adapter found");

        let (device, queue) =
            pollster::block_on(adapter.request_device(&device_descriptor(&adapter)))
                .expect("failed to create wgpu device");

        let surface_capabilities = surface.get_capabilities(&adapter);
        // OUTPUT DEPTH IS RESOLVED IN ONE PLACE (src/output_format.rs), never
        // branched on here. A 10-bit swapchain needs both a 10-bit surface format
        // advertised AND a wider storage format for the frame the compute pass
        // writes, and either one missing is a veto — so the module answers "what
        // formats" as a single consistent triple rather than letting each consumer
        // decide.
        let srgb_surface_fallback = surface_capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .unwrap_or(surface_capabilities.formats[0]);
        let output_support = OutputSupport::probe(&surface_capabilities.formats, device.features());
        let output_format = OutputFormat::resolve(
            OutputDepth::default(),
            output_support,
            srgb_surface_fallback,
        );
        println!("surface formats: {:?}", surface_capabilities.formats);
        println!(
            "output: {} | 10-bit: {}",
            output_format.depth().label(),
            output_support.ten_bit_diagnosis(),
        );
        let surface_format = output_format.surface();

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: window_size.width.max(1),
            height: window_size.height.max(1),
            // Vsync on by default; the overlay checkbox toggles via `set_vsync`.
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: surface_capabilities.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        println!(
            "surface present modes: {:?} (starting with {:?})",
            surface_capabilities.present_modes, surface_config.present_mode
        );

        let mut gpu = Self {
            surface,
            device,
            queue,
            surface_config,
            supported_present_modes: surface_capabilities.present_modes,
            output_format,
            output_support,
            srgb_surface_fallback,
            // Overwritten by the `configure_surface` below before anything can read
            // it. Seeded with the "we did nothing" outcome so that if a future edit
            // ever removes that call, the overlay reports untagged — which is the
            // truth — instead of a tag that was never applied.
            color_space: std::cell::Cell::new(ColorSpaceOutcome::NoPlatformHook),
            headroom_provider: voxel_color::headroom::platform_provider(),
        };
        let initial_color_space = gpu.configure_surface();
        // The compile-time platform arm says whether a hook exists; the first real tag
        // says whether THIS surface is actually backed by it. A non-Metal Apple surface
        // must not expose HDR merely because the binary was compiled for Apple.
        gpu.output_support.extended_srgb_presentation = matches!(
            initial_color_space,
            ColorSpaceOutcome::Tagged(voxel_color::SurfaceColorSpace::Srgb)
        );
        println!("output colour space: {}", initial_color_space.diagnosis());
        println!("HDR float: {}", gpu.output_support.hdr_diagnosis());
        println!("display headroom: {}", gpu.headroom_provider.name());
        gpu
    }

    /// What the compositor was last told these pixels mean, for the overlay.
    pub fn color_space(&self) -> ColorSpaceOutcome {
        self.color_space.get()
    }

    /// How much brighter than white this display can go, asked fresh.
    ///
    /// **Call per frame.** Headroom moves with the brightness slider and with thermal
    /// state, so a value cached at startup is precisely the bug this replaced — and the
    /// probe is three Objective-C sends plus a short layer walk, which is nothing beside
    /// the frame it feeds. Returns no headroom for the SDR depths, which have none to
    /// report.
    pub fn display_headroom(&self, choice: HeadroomChoice) -> DisplayHeadroom {
        choice.resolve(
            self.headroom_provider.as_ref(),
            &self.surface,
            &self.output_format,
        )
    }

    /// Which platform backend answers the probe, for the overlay to name.
    pub fn headroom_backend(&self) -> &'static str {
        self.headroom_provider.name()
    }

    /// Configure the surface AND tell the compositor what the pixels mean.
    ///
    /// ONE helper rather than the five call sites this replaced, because the two steps
    /// are not independent: an untagged surface is displayed pass-through, so a
    /// configure that forgets the tag renders a wrong picture that nothing validates
    /// and no error mentions. Keeping them in one function makes forgetting impossible
    /// rather than merely unlikely. See `voxel_color::color_space` for what goes wrong.
    fn configure_surface(&self) -> ColorSpaceOutcome {
        self.surface.configure(&self.device, &self.surface_config);
        let outcome = color_space::apply(&self.surface, &self.output_format);
        self.color_space.set(outcome);
        outcome
    }

    /// The resolved output formats. Consumers ASK — nobody branches on depth.
    pub fn output_format(&self) -> OutputFormat {
        self.output_format
    }

    /// What this device can actually do, for the overlay's greying-out.
    pub fn output_support(&self) -> OutputSupport {
        self.output_support
    }

    /// Request an output depth, reconfiguring the surface. Returns the resolved
    /// format when it CHANGED, so the caller knows to reallocate the storage
    /// texture and rebuild the two pipelines that depend on the formats — and
    /// `None` when nothing moved, so a no-op toggle costs nothing.
    ///
    /// Vetoes silently the way [`Self::set_vsync`] does: a device that cannot do
    /// ten bits renders eight rather than failing, and
    /// [`OutputFormat::depth`] then reports what actually happened.
    ///
    /// THE HEAVIEST TOGGLE IN THE ENGINE — a surface reconfigure plus a texture
    /// reallocation plus two pipeline rebuilds. Not a per-frame knob.
    pub fn set_output_depth(&mut self, requested: OutputDepth) -> Option<OutputFormat> {
        let resolved =
            OutputFormat::resolve(requested, self.output_support, self.srgb_surface_fallback);
        if resolved == self.output_format {
            return None;
        }
        self.output_format = resolved;
        self.surface_config.format = resolved.surface();
        // AFTER `output_format` is updated, never before: `configure_surface` reads it
        // to pick the tag, so tagging first would label the new surface with the old
        // depth's colour space.
        let tag = self.configure_surface();
        println!(
            "output depth -> {} (surface {:?}, storage {:?}, {})",
            resolved.depth().label(),
            resolved.surface(),
            resolved.storage(),
            tag.diagnosis(),
        );
        Some(resolved)
    }

    /// Toggle vsync: Fifo when on, Immediate when off (falling back to
    /// AutoNoVsync if the surface does not offer Immediate). Reconfigures the
    /// surface immediately.
    pub fn set_vsync(&mut self, vsync_enabled: bool) {
        self.surface_config.present_mode = if vsync_enabled {
            wgpu::PresentMode::Fifo
        } else if self
            .supported_present_modes
            .contains(&wgpu::PresentMode::Immediate)
        {
            wgpu::PresentMode::Immediate
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        println!(
            "vsync toggled: present mode {:?}",
            self.surface_config.present_mode
        );
        self.configure_surface();
    }

    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }
        self.surface_config.width = new_size.width;
        self.surface_config.height = new_size.height;
        self.configure_surface();
    }

    /// Reconfigure with the current settings (after a lost/outdated surface).
    pub fn reconfigure(&self) {
        self.configure_surface();
    }

    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.surface_config.format
    }
}
