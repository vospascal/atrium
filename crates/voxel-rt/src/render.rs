//! Frame composition: owns the shared world bindings, the CAGI light volume, the
//! intermediate storage texture and the pass list, and records one frame into a
//! command encoder. Passes stay self-contained (see `passes/`); this module only
//! wires them together. Camera and lighting math stay outside — the caller hands
//! in a finished [`CameraUniform`] and [`LightingUniform`] each frame.
//!
//! Render scale: the storage texture is created at the active viewport size
//! times `render_scale` (the overlay's perf lever, default 1.0). The DDA pass
//! then traces proportionally fewer rays and the blit upscales with linear
//! filtering. [`Renderer::resolution`] always reports the STORAGE size —
//! that is what the camera's ray basis and the dispatch must match.
//!
//! E4 pass order: the CAGI cellular automaton runs BEFORE the shading pass, and
//! in its own command encoder ([`Renderer::encode_light_volume`]) — Metal
//! resolves pass-boundary timestamps to zero once a command buffer holds more
//! than one compute pass, so the two compute passes must be submitted as two
//! command buffers for the overlay's per-pass readout to survive.

use crate::brickmap::Brickmap;
use crate::cagi::{
    CagiGrid, CagiSettings, GpuEventResponse, MaterialAttributes, EVENT_RESPONSE_SLOTS,
};
use crate::camera::CameraUniform;
use crate::frame_timing::{GpuFrameTimers, SPAN_CAGI, SPAN_DDA, SPAN_POST};
use crate::lighting::LightingUniform;
use crate::material::GpuMaterial;
use crate::passes::blit::BlitPass;
use crate::passes::cagi::{AttributeSource, CagiPass, LightVolume};
use crate::passes::dda::DdaPass;
use crate::passes::world_bindings::WorldBindings;
use crate::variants::{MAX_RENDER_SCALE, MIN_RENDER_SCALE};
use crate::world_edit::WorldDelta;
use crate::world_event::{GpuWorldEvent, MAX_WORLD_EVENTS};

/// Format of the compute-written intermediate texture. Srgb formats cannot be
/// storage textures, so the DDA pass writes display-ready (sRGB-encoded)
/// values into this linear-tagged format and the blit undoes the swapchain's
/// re-encode (see `shaders/blit.wgsl`).
const STORAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub struct Renderer {
    storage_view: wgpu::TextureView,
    storage_width: u32,
    storage_height: u32,
    surface_width: u32,
    surface_height: u32,
    render_scale: f32,
    world_bindings: WorldBindings,
    light_volume: LightVolume,
    cagi_pass: CagiPass,
    dda_pass: DdaPass,
    blit_pass: BlitPass,
    /// Height of the active rendered viewport. The Studio node editor can
    /// reserve a bottom region without stretching the 3D view underneath it.
    viewport_height: u32,
}

impl Renderer {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        brickmap: &Brickmap,
        global_illumination: &CagiSettings,
        material_attributes: &MaterialAttributes,
    ) -> Self {
        let render_scale = MAX_RENDER_SCALE;
        let (storage_view, storage_width, storage_height) =
            create_storage_texture(device, width, height, render_scale);
        let world_bindings = WorldBindings::new(device, brickmap);
        let light_volume =
            LightVolume::new(device, brickmap, global_illumination, material_attributes);
        let cagi_pass = CagiPass::new(device, &world_bindings, &light_volume);
        let dda_pass = DdaPass::new(device, &world_bindings, &light_volume, &storage_view);
        let blit_pass = BlitPass::new(device, surface_format, &storage_view);

        Self {
            storage_view,
            storage_width,
            storage_height,
            surface_width: width,
            surface_height: height,
            render_scale,
            viewport_height: height,
            world_bindings,
            light_volume,
            cagi_pass,
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

    /// The CAGI light volume's grid — the GI memory footprint, reported at
    /// startup and after a resolution change.
    pub fn light_volume_grid(&self) -> CagiGrid {
        self.light_volume.grid()
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.surface_width = width;
        self.surface_height = height;
        self.viewport_height = self.viewport_height.min(height).max(1);
        self.recreate_storage(device);
    }

    /// Set the physical height occupied by the rendered viewport. The remainder
    /// of the surface is reserved for editor panels and stays black until egui
    /// draws those panels. Recreating the storage texture keeps the camera's
    /// aspect ratio and DDA dispatch aligned with the visible region.
    pub fn set_viewport_height(&mut self, device: &wgpu::Device, height: u32) {
        let height = height.clamp(1, self.surface_height.max(1));
        if height == self.viewport_height {
            return;
        }
        self.viewport_height = height;
        self.recreate_storage(device);
    }

    /// Switch the DDA pass to a patched shader source (the overlay path: a
    /// compile-time lever or a preset changed). Buffers and bind groups are
    /// untouched, and a prewarmed permutation costs a hash lookup.
    pub fn set_dda_shader_source(&mut self, device: &wgpu::Device, shader_source: &str) {
        self.dda_pass.set_shader_source(device, shader_source);
    }

    /// The same for the CAGI pass (a propagation lever or the master switch
    /// changed).
    pub fn set_cagi_shader_source(&mut self, device: &wgpu::Device, shader_source: &str) {
        self.cagi_pass.set_shader_source(device, shader_source);
    }

    /// Reallocate the light volume for `global_illumination` (its resolution or
    /// the master lever moved) and rebind both consumers. The new volume starts
    /// dirty, so the next frame floods it from scratch.
    ///
    /// `attribute_source` is E2's world-thread seam: `Deferred` allocates the
    /// static attributes zeroed and expects
    /// [`Renderer::write_light_volume_attributes`] once the world thread has built
    /// them, which is how a GI resolution switch stops being a ~0.5 s frame hitch.
    pub fn rebuild_light_volume(
        &mut self,
        device: &wgpu::Device,
        brickmap: &Brickmap,
        global_illumination: &CagiSettings,
        attribute_source: AttributeSource,
        material_attributes: &MaterialAttributes,
    ) {
        self.light_volume = LightVolume::new_with_attributes(
            device,
            brickmap,
            global_illumination,
            attribute_source,
            material_attributes,
        );
        self.cagi_pass
            .rebind(device, &self.world_bindings, &self.light_volume);
        self.dda_pass.rebind(
            device,
            &self.world_bindings,
            &self.light_volume,
            &self.storage_view,
        );
    }

    /// Install a CAGI attribute set built off-frame. Returns false when it was
    /// built for a grid the renderer no longer holds (the lever moved again).
    pub fn write_light_volume_attributes(
        &mut self,
        queue: &wgpu::Queue,
        grid: &CagiGrid,
        attributes: &[u32],
        emissions: &[[f32; 4]],
        responses: &[GpuEventResponse; EVENT_RESPONSE_SLOTS],
    ) -> bool {
        self.light_volume
            .write_all_attributes(queue, grid, attributes, emissions, responses)
    }

    /// Apply one edit's GPU delta (E2): the touched brickmap words, the touched
    /// CAGI cell attributes and, if it moved, the metadata uniform. Nothing here
    /// reads the brickmap — the payloads are owned, which is what lets the
    /// authority live on another thread.
    ///
    /// Returns whether the delta was applied; `false` means the level-1 arrays
    /// outgrew their headroom and the caller must call
    /// [`Renderer::reupload_world`] with the brickmap instead.
    pub fn apply_world_delta(&mut self, queue: &wgpu::Queue, delta: &WorldDelta) -> bool {
        // The light volume is not part of the world buffers, so its cells are
        // patched either way — a re-upload would not cover them.
        self.light_volume
            .write_cell_attributes(queue, delta.light_grid, &delta.light_cells);
        if delta.arrays_grew {
            return false;
        }
        for write in &delta.writes {
            self.world_bindings.apply_array_write(queue, write);
        }
        if let Some(metadata) = &delta.metadata {
            self.world_bindings.write_metadata(queue, metadata);
        }
        true
    }

    /// Recreate the world's GPU buffers from `brickmap` and rebind both passes —
    /// the rare path where an edit outgrew the brick headroom. Costs a full ~41 MB
    /// upload, which is why the headroom exists.
    pub fn reupload_world(&mut self, device: &wgpu::Device, brickmap: &Brickmap) {
        self.world_bindings = WorldBindings::new(device, brickmap);
        self.cagi_pass
            .rebind(device, &self.world_bindings, &self.light_volume);
        self.dda_pass.rebind(
            device,
            &self.world_bindings,
            &self.light_volume,
            &self.storage_view,
        );
    }

    /// Throw the light volume's contents away and flood again from scratch — what
    /// a sun move (or any injection/transport change) requires.
    pub fn mark_light_volume_dirty(&mut self) {
        self.light_volume.mark_dirty();
    }

    /// Re-upload the material table — S0's live-editing seam.
    ///
    /// Passes straight through to the world bindings because the table belongs to
    /// the world, not to a pass: both the shading pass and the CAGI pass bind it.
    /// Only the direct-shading tier is covered here; the GI bounce reads CAGI's own
    /// baked cell attributes and needs a re-pack to follow.
    pub fn write_material_table(&self, queue: &wgpu::Queue, rows: &[GpuMaterial]) {
        self.world_bindings.write_material_table(queue, rows);
    }

    /// Precompile the pipeline permutations the quality presets need, so
    /// switching preset in-app never compiles a shader mid-frame. Returns how many
    /// distinct pipelines each cache holds (shading pass, CAGI pass).
    pub fn prewarm_pipelines(
        &mut self,
        device: &wgpu::Device,
        dda_shader_sources: &[String],
        cagi_shader_sources: &[String],
    ) -> (usize, usize) {
        (
            self.dda_pass.prewarm_pipelines(device, dda_shader_sources),
            self.cagi_pass
                .prewarm_pipelines(device, cagi_shader_sources),
        )
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
            self.viewport_height,
            self.render_scale,
        );
        self.storage_view = storage_view;
        self.storage_width = storage_width;
        self.storage_height = storage_height;
        self.dda_pass.rebind(
            device,
            &self.world_bindings,
            &self.light_volume,
            &self.storage_view,
        );
        self.blit_pass.rebind(
            device,
            &self.storage_view,
            self.render_scale < MAX_RENDER_SCALE,
        );
    }

    /// Record this frame's CAGI iterations and upload the two buffers BOTH
    /// passes read: the lighting uniform (so the CA cannot inject a different
    /// sun than the shading pass shades with) and the world event field (so a
    /// surface cannot light up from an event the volume has not seen yet).
    ///
    /// Both are written HERE rather than in [`Renderer::encode_frame`] because
    /// this pass runs first and consumes them; writing them later would give the
    /// CA the previous frame's events. Must go into a separate command buffer
    /// from `encode_frame` — see the module docs on Metal's timestamps.
    pub fn encode_light_volume(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        lighting_uniform: &LightingUniform,
        world_events: &[GpuWorldEvent; MAX_WORLD_EVENTS],
        iterations: u32,
        frame_timers: Option<&GpuFrameTimers>,
    ) {
        self.world_bindings.write_lighting(queue, lighting_uniform);
        self.world_bindings.write_world_events(queue, world_events);
        self.cagi_pass.encode(
            encoder,
            &mut self.light_volume,
            iterations,
            frame_timers.map(|timers| timers.compute_span_writes(SPAN_CAGI)),
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
        target_view: &wgpu::TextureView,
        frame_timers: Option<&GpuFrameTimers>,
    ) {
        self.dda_pass.encode(
            queue,
            encoder,
            camera_uniform,
            self.light_volume.front(),
            self.storage_width,
            self.storage_height,
            frame_timers.map(|timers| timers.compute_span_writes(SPAN_DDA)),
        );
        self.blit_pass.encode(
            encoder,
            target_view,
            self.surface_width,
            self.surface_height,
            self.surface_width,
            self.viewport_height,
            frame_timers.map(|timers| timers.render_span_begin_writes(SPAN_POST)),
        );
    }
}

fn create_storage_texture(
    device: &wgpu::Device,
    surface_width: u32,
    viewport_height: u32,
    render_scale: f32,
) -> (wgpu::TextureView, u32, u32) {
    let width = ((surface_width as f32 * render_scale) as u32).max(1);
    let height = ((viewport_height as f32 * render_scale) as u32).max(1);
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
