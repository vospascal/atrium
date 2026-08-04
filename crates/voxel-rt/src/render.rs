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
use crate::passes::blit::BlitPass;
use crate::passes::cagi::{AttributeSource, CagiPass, LightVolume};
use crate::passes::dda::DdaPass;
use crate::passes::world_bindings::WorldBindings;
use crate::variants::{MAX_RENDER_SCALE, MIN_RENDER_SCALE};
use crate::world_edit::WorldDelta;
use voxel_color::OutputFormat;
use voxel_environment::{EnvironmentGpu, EnvironmentRequest, FroxelCamera, HillaireEnvironment};
use voxel_material::material::GpuMaterial;
use voxel_material::world_event::{GpuWorldEvent, MAX_WORLD_EVENTS};

/// Far bound of the aerial-perspective froxel grid, in kilometres. The grid's Z axis is
/// logarithmic, so this is a horizon distance rather than a resolution cost.
const FROXEL_FAR_KILOMETERS: f32 = 32.0;

// The compute-written intermediate texture's format is NOT a const here any more.
// Srgb formats cannot be storage textures, so the DDA pass writes display-ready
// (sRGB-encoded) values into a linear-tagged format and the blit undoes the
// swapchain's re-encode (see `shaders/blit.wgsl`). Which format that is now depends
// on the output depth, and it is resolved together with the surface format and the
// blit's transfer function in the `voxel-color` crate — a storage texture wider or
// narrower than the swapchain is a silent bug rather than a compile error.

pub struct Renderer {
    storage_view: wgpu::TextureView,
    /// The resolved output formats this renderer was built for. See
    /// [the `voxel-color` crate] — the single source of truth, never re-derived.
    output_format: OutputFormat,
    storage_width: u32,
    storage_height: u32,
    surface_width: u32,
    surface_height: u32,
    render_scale: f32,
    world_bindings: WorldBindings,
    /// The environment backend, behind `voxel-environment`'s contract. This renderer knows
    /// it has a bind group, some WGSL and a per-frame `submit`; how many lookup tables sit
    /// behind that, and which of them a head turn invalidates, is the adapter's business.
    environment: HillaireEnvironment,
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
        output_format: OutputFormat,
        width: u32,
        height: u32,
        brickmap: &Brickmap,
        global_illumination: &CagiSettings,
        material_attributes: &MaterialAttributes,
    ) -> Self {
        let render_scale = MAX_RENDER_SCALE;
        let (storage_view, storage_width, storage_height) =
            create_storage_texture(device, output_format, width, height, render_scale);
        let world_bindings = WorldBindings::new(device, brickmap);
        let environment = HillaireEnvironment::new(device);
        let light_volume =
            LightVolume::new(device, brickmap, global_illumination, material_attributes);
        let cagi_pass =
            CagiPass::new_with_environment(device, &world_bindings, &light_volume, &environment);
        let dda_pass = DdaPass::new_with_environment(
            device,
            &world_bindings,
            &light_volume,
            &storage_view,
            &environment,
            output_format,
        );
        let blit_pass = BlitPass::new(device, output_format, &storage_view);

        Self {
            storage_view,
            output_format,
            storage_width,
            storage_height,
            surface_width: width,
            surface_height: height,
            render_scale,
            viewport_height: height,
            world_bindings,
            environment,
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

    /// Adopt a new resolved output format: reallocate the frame storage texture at
    /// the new format and rebuild the two pipelines that depend on it.
    ///
    /// Mirrors [`Self::set_render_scale`] exactly, because the work is the same —
    /// the storage texture is recreated and both consumers rebind. The blit pipeline
    /// additionally has to be rebuilt, since its render target format and its
    /// transfer-function const both moved.
    pub fn set_output_format(&mut self, device: &wgpu::Device, output_format: OutputFormat) {
        if output_format == self.output_format {
            return;
        }
        self.output_format = output_format;
        // ORDER MATTERS, and getting it wrong reproduces exactly the error this is
        // fixing. `recreate_storage` REBINDS the existing passes, and the existing
        // DDA pass's bind group layout still declares the OLD storage format — so
        // handing it the new view is the "expects Rgba8Unorm, given Rgba16Unorm"
        // validation failure. The texture is therefore recreated inline here,
        // without rebinding anything, and both passes are then built from scratch
        // against the new view.
        //
        // Rebuilt rather than rebound because each holds format state a rebind
        // cannot reach: the blit's render target format, and the DDA's bind group
        // layout. Recreating the DDA pass drops its pipeline cache, which is
        // correct — every entry was compiled for the old format.
        let (storage_view, storage_width, storage_height) = create_storage_texture(
            device,
            output_format,
            self.surface_width,
            self.viewport_height,
            self.render_scale,
        );
        self.storage_view = storage_view;
        self.storage_width = storage_width;
        self.storage_height = storage_height;
        self.blit_pass = BlitPass::new(device, output_format, &self.storage_view);
        self.dda_pass = DdaPass::new_with_environment(
            device,
            &self.world_bindings,
            &self.light_volume,
            &self.storage_view,
            &self.environment,
            output_format,
        );
        // A fresh BlitPass starts on its 1:1 sampler; a non-native render scale
        // needs the linear one, which `recreate_storage` would have set.
        self.blit_pass.rebind(
            device,
            &self.storage_view,
            self.render_scale < MAX_RENDER_SCALE,
        );
    }

    /// The resolved format in force, so the caller can patch the shading source to
    /// match without keeping its own copy.
    pub fn output_format(&self) -> OutputFormat {
        self.output_format
    }

    fn recreate_storage(&mut self, device: &wgpu::Device) {
        let (storage_view, storage_width, storage_height) = create_storage_texture(
            device,
            self.output_format,
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

    /// This frame's environment inputs, translated from the renderer's own uniforms.
    ///
    /// The physical half (`sun_direction`, `sun_illuminance`) is what CAGI and the
    /// transmittance table integrate. The `celestial_*`/`sky_*` half is the camera-only
    /// appearance layer: [`LightingUniform`] still carries it for surface shading, so it is
    /// mirrored across here rather than being resolved twice from [`SunSettings`].
    fn environment_request(
        lighting_uniform: &LightingUniform,
        camera_uniform: &CameraUniform,
    ) -> EnvironmentRequest {
        let intensity = lighting_uniform.sun_color_intensity[3];
        EnvironmentRequest {
            sun_direction: lighting_uniform.sun_direction,
            sun_illuminance: [
                lighting_uniform.sun_color_intensity[0] * intensity,
                lighting_uniform.sun_color_intensity[1] * intensity,
                lighting_uniform.sun_color_intensity[2] * intensity,
            ],
            celestial_sun: lighting_uniform.celestial_sun,
            celestial_moon: lighting_uniform.celestial_moon,
            sky_zenith: lighting_uniform.sky_zenith,
            sky_horizon: lighting_uniform.sky_horizon,
            camera_position: camera_uniform.position,
            // The identical number the environment's shader used to reach into
            // `lighting.sky_ambient.w` for. Passing it rather than recomputing it is what makes
            // this a pure re-route: the value the diffuse term multiplies by cannot drift.
            ambient_scale: lighting_uniform.sky_ambient[3],
            camera: FroxelCamera {
                forward: camera_uniform.forward,
                right_scaled: camera_uniform.right_scaled,
                up_scaled: camera_uniform.up_scaled,
                near_world: 0.1,
                far_world: FROXEL_FAR_KILOMETERS * voxel_environment::FROM_KILOMETERS_SCALE,
            },
        }
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
        camera_uniform: &CameraUniform,
        world_events: &[GpuWorldEvent; MAX_WORLD_EVENTS],
        iterations: u32,
        frame_timers: Option<&GpuFrameTimers>,
    ) {
        self.world_bindings.write_lighting(queue, lighting_uniform);
        self.world_bindings.write_world_events(queue, world_events);
        // State the environment's inputs; the adapter decides what they invalidated and
        // encodes only that. Whether this frame costs four compute dispatches, two, or
        // none is not a decision this file makes any more.
        let environment_request = Self::environment_request(lighting_uniform, camera_uniform);
        self.environment
            .submit(queue, encoder, &environment_request);
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
    output_format: OutputFormat,
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
        format: output_format.storage(),
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (view, width, height)
}
