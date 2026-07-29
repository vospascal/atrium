//! Frame composition: owns the shared intermediate storage texture and the
//! pass list, records one frame into a command encoder. Passes stay
//! self-contained (see `passes/`); this module only wires them together.
//! Camera math stays outside — the caller hands in a finished
//! [`CameraUniform`] each frame.

use crate::brickmap::Brickmap;
use crate::camera::CameraUniform;
use crate::passes::blit::BlitPass;
use crate::passes::dda::DdaPass;

/// Format of the compute-written intermediate texture. Srgb formats cannot be
/// storage textures, so the DDA pass writes display-ready (sRGB-encoded)
/// values into this linear-tagged format and the blit undoes the swapchain's
/// re-encode (see `shaders/blit.wgsl`).
const STORAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub struct Renderer {
    storage_view: wgpu::TextureView,
    storage_width: u32,
    storage_height: u32,
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
        let (storage_view, storage_width, storage_height) =
            create_storage_texture(device, width, height);
        let dda_pass = DdaPass::new(device, brickmap, &storage_view);
        let blit_pass = BlitPass::new(device, surface_format, &storage_view);

        Self {
            storage_view,
            storage_width,
            storage_height,
            dda_pass,
            blit_pass,
        }
    }

    /// Output size in pixels — what the camera uniform's resolution must be.
    pub fn resolution(&self) -> (u32, u32) {
        (self.storage_width, self.storage_height)
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let (storage_view, storage_width, storage_height) =
            create_storage_texture(device, width, height);
        self.storage_view = storage_view;
        self.storage_width = storage_width;
        self.storage_height = storage_height;
        self.dda_pass.rebind(device, &self.storage_view);
        self.blit_pass.rebind(device, &self.storage_view);
    }

    pub fn encode_frame(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera_uniform: &CameraUniform,
        target_view: &wgpu::TextureView,
    ) {
        self.dda_pass.encode(
            queue,
            encoder,
            camera_uniform,
            self.storage_width,
            self.storage_height,
        );
        self.blit_pass.encode(encoder, target_view);
    }
}

fn create_storage_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::TextureView, u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
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
