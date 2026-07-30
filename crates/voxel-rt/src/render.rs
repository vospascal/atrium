//! Frame composition: owns the shared intermediate storage texture and the
//! pass list, records one frame into a command encoder. Passes stay
//! self-contained (see `passes/`); this module only wires them together.
//! Camera and lighting math stay outside — the caller hands in a finished
//! [`CameraUniform`] and [`LightingUniform`] each frame.
//!
//! Render scale: the storage texture is created at `surface size *
//! render_scale` (the overlay's perf lever, default 1.0). The DDA pass then
//! traces proportionally fewer rays and the blit upscales with linear
//! filtering. [`Renderer::resolution`] always reports the STORAGE size —
//! that is what the camera's ray basis and the dispatch must match.

use crate::brickmap::Brickmap;
use crate::camera::CameraUniform;
use crate::frame_timing::{GpuFrameTimers, SPAN_DDA, SPAN_POST};
use crate::lighting::LightingUniform;
use crate::passes::blit::BlitPass;
use crate::passes::dda::DdaPass;

/// Format of the compute-written intermediate texture. Srgb formats cannot be
/// storage textures, so the DDA pass writes display-ready (sRGB-encoded)
/// values into this linear-tagged format and the blit undoes the swapchain's
/// re-encode (see `shaders/blit.wgsl`).
const STORAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Render-scale bounds exposed by the overlay slider.
pub const MIN_RENDER_SCALE: f32 = 0.5;
pub const MAX_RENDER_SCALE: f32 = 1.0;

pub struct Renderer {
    storage_view: wgpu::TextureView,
    storage_width: u32,
    storage_height: u32,
    surface_width: u32,
    surface_height: u32,
    render_scale: f32,
    dda_pass: DdaPass,
    blit_pass: BlitPass,
}

impl Renderer {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        brickmap: &Brickmap,
    ) -> Self {
        let render_scale = MAX_RENDER_SCALE;
        let (storage_view, storage_width, storage_height) =
            create_storage_texture(device, width, height, render_scale);
        let dda_pass = DdaPass::new(device, brickmap, &storage_view);
        let blit_pass = BlitPass::new(device, surface_format, &storage_view);

        Self {
            storage_view,
            storage_width,
            storage_height,
            surface_width: width,
            surface_height: height,
            render_scale,
            dda_pass,
            blit_pass,
        }
    }

    /// Output size in pixels (the scaled storage texture) — what the camera
    /// uniform's resolution must be.
    pub fn resolution(&self) -> (u32, u32) {
        (self.storage_width, self.storage_height)
    }

    /// Current render scale (storage size / surface size, 1.0 = native).
    pub fn render_scale(&self) -> f32 {
        self.render_scale
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.surface_width = width;
        self.surface_height = height;
        self.recreate_storage(device);
    }

    /// Apply the overlay's render-scale lever (clamped to the slider range).
    /// No-op when the scale is unchanged.
    pub fn set_render_scale(&mut self, device: &wgpu::Device, render_scale: f32) {
        let render_scale = render_scale.clamp(MIN_RENDER_SCALE, MAX_RENDER_SCALE);
        if render_scale == self.render_scale {
            return;
        }
        self.render_scale = render_scale;
        self.recreate_storage(device);
    }

    fn recreate_storage(&mut self, device: &wgpu::Device) {
        let (storage_view, storage_width, storage_height) = create_storage_texture(
            device,
            self.surface_width,
            self.surface_height,
            self.render_scale,
        );
        self.storage_view = storage_view;
        self.storage_width = storage_width;
        self.storage_height = storage_height;
        self.dda_pass.rebind(device, &self.storage_view);
        self.blit_pass.rebind(
            device,
            &self.storage_view,
            self.render_scale < MAX_RENDER_SCALE,
        );
    }

    /// Record the DDA + blit passes. When `frame_timers` is present, the DDA
    /// compute pass carries its full timing span and the blit pass OPENS the
    /// post span — the overlay pass (encoded by the caller afterwards) closes
    /// it via [`GpuFrameTimers::render_span_end_writes`].
    pub fn encode_frame(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera_uniform: &CameraUniform,
        lighting_uniform: &LightingUniform,
        target_view: &wgpu::TextureView,
        frame_timers: Option<&GpuFrameTimers>,
    ) {
        self.dda_pass.encode(
            queue,
            encoder,
            camera_uniform,
            lighting_uniform,
            self.storage_width,
            self.storage_height,
            frame_timers.map(|timers| timers.compute_span_writes(SPAN_DDA)),
        );
        self.blit_pass.encode(
            encoder,
            target_view,
            frame_timers.map(|timers| timers.render_span_begin_writes(SPAN_POST)),
        );
    }
}

fn create_storage_texture(
    device: &wgpu::Device,
    surface_width: u32,
    surface_height: u32,
    render_scale: f32,
) -> (wgpu::TextureView, u32, u32) {
    let width = ((surface_width as f32 * render_scale) as u32).max(1);
    let height = ((surface_height as f32 * render_scale) as u32).max(1);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frame storage texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: STORAGE_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (view, width, height)
}
