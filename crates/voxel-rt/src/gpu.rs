//! wgpu instance/adapter/device/queue/surface setup and resize handling.
//! Platform-agnostic: winit hands in an `Arc<Window>`, everything else is wgpu.

use std::sync::Arc;

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
/// Vulkan drivers report 24+ storage buffers per stage, so 10 is comfortably
/// inside the Quest target too.
pub const REQUIRED_STORAGE_BUFFERS_PER_STAGE: u32 = 10;

/// The device descriptor every consumer must use: the raised storage-buffer limit
/// plus whatever timestamp support the adapter offers (GPU pass timing degrades to
/// "unavailable" rather than failing on adapters without it).
pub fn device_descriptor(adapter: &wgpu::Adapter) -> wgpu::DeviceDescriptor<'static> {
    wgpu::DeviceDescriptor {
        label: Some("voxel-rt device"),
        required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
        required_limits: wgpu::Limits {
            max_storage_buffers_per_shader_stage: REQUIRED_STORAGE_BUFFERS_PER_STAGE,
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
        let surface_format = surface_capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .unwrap_or(surface_capabilities.formats[0]);

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
        surface.configure(&device, &surface_config);
        println!(
            "surface present modes: {:?} (starting with {:?})",
            surface_capabilities.present_modes, surface_config.present_mode
        );

        Self {
            surface,
            device,
            queue,
            surface_config,
            supported_present_modes: surface_capabilities.present_modes,
        }
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
        self.surface.configure(&self.device, &self.surface_config);
    }

    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }
        self.surface_config.width = new_size.width;
        self.surface_config.height = new_size.height;
        self.surface.configure(&self.device, &self.surface_config);
    }

    /// Reconfigure with the current settings (after a lost/outdated surface).
    pub fn reconfigure(&self) {
        self.surface.configure(&self.device, &self.surface_config);
    }

    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.surface_config.format
    }
}
