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
use crate::lighting::LightingUniform;
use crate::passes::blit::BlitPass;
use crate::passes::cagi::{AttributeSource, CagiPass, LightVolume};
use crate::passes::composer::ShaderProgram;
use crate::passes::dda::DdaPass;
use crate::passes::world_bindings::WorldBindings;
use crate::profiling::{FrameTimers, GPU_BLIT, GPU_CAGI, GPU_DDA};
use crate::variants::{MAX_RENDER_SCALE, MIN_RENDER_SCALE};
use crate::world_edit::WorldDelta;
use voxel_color::OutputFormat;
use voxel_environment::{
    EnvironmentFrame, EnvironmentGpu, EnvironmentRequest, FroxelCamera, HillaireEnvironment,
};
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
    /// This frame's cloud deck, stated by the app via [`Renderer::set_clouds`].
    clouds: voxel_environment::CloudRequest,
    /// The single CPU environment evaluation shared by lighting, clouds, CAGI input and the
    /// atmosphere adapter.
    environment_frame: EnvironmentFrame,
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
            clouds: voxel_environment::CloudRequest::default(),
            environment_frame: voxel_environment::SunSettings::default().environment_frame(),
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

    /// GPU bytes held by the world buffers (brickmap and its derived structures).
    ///
    /// Read from `wgpu::Buffer::size` rather than recomputed from the brickmap, so
    /// it reports what was actually ALLOCATED — including the brick headroom that
    /// exists precisely so an edit does not have to reallocate 41 MB.
    pub fn world_gpu_bytes(&self) -> u64 {
        self.world_bindings.gpu_bytes()
    }

    /// GPU bytes held by the CAGI light volume: both ping-pong buffers plus the
    /// cell attribute data.
    pub fn light_volume_gpu_bytes(&self) -> u64 {
        self.light_volume.gpu_bytes()
    }

    /// GPU bytes held by the ray-traced storage texture.
    ///
    /// Scales with BOTH the render scale and the output depth — a half-scale
    /// preset at 8-bit and a full-scale one at 16-bit float differ by 8x, which is
    /// exactly the tradeoff the quality levers make and therefore worth showing
    /// next to them.
    pub fn storage_texture_bytes(&self) -> u64 {
        let bytes_per_pixel = self
            .output_format
            .storage()
            .block_copy_size(None)
            .unwrap_or(0);
        u64::from(self.storage_width) * u64::from(self.storage_height) * u64::from(bytes_per_pixel)
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

    /// Switch the DDA pass to a rebuilt program (the overlay path: a compile-time
    /// lever or a preset changed). Buffers and bind groups are untouched, and a
    /// prewarmed permutation costs a hash lookup.
    pub fn set_dda_shader(
        &mut self,
        device: &wgpu::Device,
        source: &str,
        compose: impl FnOnce() -> naga::Module,
    ) {
        self.dda_pass.set_shader(device, source, compose);
    }

    /// The same for the CAGI pass (a propagation lever or the master switch
    /// changed).
    pub fn set_cagi_shader(
        &mut self,
        device: &wgpu::Device,
        source: &str,
        compose: impl FnOnce() -> naga::Module,
    ) {
        self.cagi_pass.set_shader(device, source, compose);
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
        dda_programs: &[ShaderProgram],
        cagi_programs: &[ShaderProgram],
    ) -> (usize, usize) {
        (
            self.dda_pass.prewarm_pipelines(device, dda_programs),
            self.cagi_pass.prewarm_pipelines(device, cagi_programs),
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
    /// State this frame's cloud deck.
    ///
    /// Set by the app rather than owned here, for the same reason [`SunSettings`] is: the deck
    /// is authored state driven by weather, and the renderer's job is to submit it, not to
    /// decide it. The ground-bounce coefficients are the exception — those are derived from the
    /// lighting the renderer already has, so [`environment_request`](Self::environment_request)
    /// fills them in and a caller cannot get them wrong.
    pub fn set_clouds(&mut self, clouds: voxel_environment::CloudRequest) {
        self.clouds = clouds;
    }

    /// State this frame's physical and active celestial lights. The app evaluates its
    /// [`SunSettings`] once and sends the resulting frame here so the renderer never reconstructs
    /// a second, slightly different version from the shading uniform.
    pub fn set_environment_frame(&mut self, frame: EnvironmentFrame) {
        self.environment_frame = frame;
    }

    /// The cloud deck currently submitted.
    pub fn clouds(&self) -> voxel_environment::CloudRequest {
        self.clouds
    }

    /// Public so the bench can bake the SAME request the app submits — a second
    /// hand-rolled request builder is how the froxel camera or the ambient scale
    /// would silently drift between the two.
    pub fn environment_request(
        environment_frame: EnvironmentFrame,
        lighting_uniform: &LightingUniform,
        camera_uniform: &CameraUniform,
        clouds: voxel_environment::CloudRequest,
    ) -> EnvironmentRequest {
        EnvironmentRequest {
            // The SUN, not the active light.
            //
            // `lighting_uniform.sun_direction` is `active_direction`, which FLIPS TO THE MOON
            // after sunset — correct for the shading it was built for, since the moon is what
            // casts shadows at night. But this field drives the physical atmosphere: the
            // transmittance and sky-view LUTs, the cloud sun-march, and the "sunward horizon" tap
            // that gives cloud its warm ambient. Handing it the moon integrates the whole
            // atmosphere with the sun 180 degrees from where it is, so the sky and every cloud lit
            // by it are coloured for the wrong hemisphere.
            //
            // `celestial_sun.xyz` is the real sun and is already in this uniform, so this is a
            // re-route rather than new data. The active light keeps its own path to the shading.
            sun_direction: environment_frame.sun_direction.to_array(),
            sun_illuminance: environment_frame.sun_illuminance,
            moon_direction: environment_frame.moon_direction.to_array(),
            moon_illuminance: environment_frame.moon_illuminance,
            active_light_direction: environment_frame.active_direction.to_array(),
            active_light_illuminance: environment_frame.active_illuminance,
            active_light_is_sun: environment_frame.daylight,
            celestial_sun: lighting_uniform.celestial_sun,
            celestial_moon: lighting_uniform.celestial_moon,
            sky_zenith: lighting_uniform.sky_zenith,
            sky_horizon: lighting_uniform.sky_horizon,
            camera_position: camera_uniform.position,
            // The identical number the environment's shader used to reach into
            // `lighting.sky_ambient.w` for. Passing it rather than recomputing it is what makes
            // this a pure re-route: the value the diffuse term multiplies by cannot drift.
            ambient_scale: environment_frame.ambient_scale,
            camera: FroxelCamera {
                forward: camera_uniform.forward,
                right_scaled: camera_uniform.right_scaled,
                up_scaled: camera_uniform.up_scaled,
                near_world: 0.1,
                far_world: FROXEL_FAR_KILOMETERS * voxel_environment::FROM_KILOMETERS_SCALE,
            },
            clouds,
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
        frame_timers: Option<&FrameTimers>,
    ) {
        self.world_bindings.write_lighting(queue, lighting_uniform);
        self.world_bindings.write_world_events(queue, world_events);
        // State the environment's inputs; the adapter decides what they invalidated and
        // encodes only that. Whether this frame costs four compute dispatches, two, or
        // none is not a decision this file makes any more.
        // The ground bounce arrives ALREADY STATED on the cloud request, from whoever owns the
        // real sun. It used to be derived here from `HillaireEnvironment::frame()`, which read a
        // second `SunSettings` that nothing ever wrote — a frozen default noon sun, and up to 96%
        // of a cloud's ambient at sunset. The comment here claimed it came from "the lighting
        // state this function already holds"; it did not, and that mismatch is exactly why it
        // survived. This function no longer has a sun of its own to get wrong.
        let environment_request = Self::environment_request(
            self.environment_frame,
            lighting_uniform,
            camera_uniform,
            self.clouds,
        );
        self.environment
            .submit(queue, encoder, &environment_request);
        self.cagi_pass.encode(
            encoder,
            &mut self.light_volume,
            iterations,
            frame_timers.map(|timers| timers.compute_span_writes(GPU_CAGI)),
        );
    }

    /// Record the DDA + blit passes. When `frame_timers` is present, the DDA
    /// compute pass and the blit pass each carry their own self-contained timing
    /// span. The overlay pass, encoded by the caller afterwards, carries its own
    /// — deliberately NOT one span across both, which would swallow the GPU's
    /// wait between them (see [`crate::profiling::GPU_BLIT`]).
    pub fn encode_frame(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera_uniform: &CameraUniform,
        target_view: &wgpu::TextureView,
        frame_timers: Option<&FrameTimers>,
    ) {
        self.dda_pass.encode(
            queue,
            encoder,
            camera_uniform,
            self.light_volume.front(),
            self.storage_width,
            self.storage_height,
            frame_timers.map(|timers| timers.compute_span_writes(GPU_DDA)),
        );
        self.blit_pass.encode(
            encoder,
            target_view,
            self.surface_width,
            self.surface_height,
            self.surface_width,
            self.viewport_height,
            frame_timers.map(|timers| timers.render_span_writes(GPU_BLIT)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lighting::{
        lighting_uniform, AnimationParams, EventParams, GiParams, MaterialParams, ShadingParams,
        WaterParams,
    };
    use voxel_environment::SunSettings;

    fn uniform_at(day_phase: f32) -> LightingUniform {
        let sun = SunSettings {
            day_night_enabled: true,
            day_phase,
            ..SunSettings::default()
        };
        // Every param vector is irrelevant here — only the two sun directions are asserted — but
        // they are stated rather than defaulted because these structs deliberately have no
        // `Default`: their field order is a GPU vector's component order.
        lighting_uniform(
            &sun,
            ShadingParams {
                ambient_occlusion_strength: 0.8,
                sun_shadow: 1.0,
                ambient_occlusion_fade_start_voxels: 240.0,
                ambient_occlusion_fade_end_voxels: 480.0,
            },
            GiParams {
                strength: 1.0,
                ambient_floor: 0.0,
                sun_bounce: 0.35,
                emissive_scale: 0.0,
            },
            WaterParams {
                absorption_scale: 1.0,
                scattering_scale: 1.0,
                ray_cutoff: 0.02,
                turbidity_scattering_fraction: crate::water::WaterSettings::default()
                    .turbidity_scattering_fraction,
                refraction_strength: 1.0,
                // E7: the shipped visibility horizon, so an offscreen render of a pool
                // matches what the window shows. Unlike wind, turbidity is not a property
                // of the running world — there is no history to attach.
                turbidity_per_meter: crate::water::turbidity_per_meter(
                    crate::water::WaterSettings::default().visibility_depth_blocks,
                ),
                // Offscreen caller with no wind history — flat water. `with_wind`
                // is how the app attaches one.
                waves: crate::water::WaveField::FLAT,
            },
            MaterialParams {
                pattern_fade_start_meters: 64.0,
                pattern_fade_end_meters: 192.0,
                pixel_footprint_at_one_meter: 0.001,
            },
            AnimationParams {
                remainder_seconds: 0.0,
                epoch: 0.0,
                reserved_flow: 0.0,
                reserved: 0.0,
            },
            EventParams {
                remainder_seconds: 0.0,
                epoch: 0.0,
                event_count: 0.0,
            },
        )
    }

    /// The atmosphere must be driven by the SUN, never by the active light.
    ///
    /// `LightingUniform::sun_direction` carries `active_direction`, which flips to the MOON after
    /// sunset. That is right for shading — the moon is what casts shadows at night — but this field
    /// also feeds the transmittance and sky-view LUTs, the cloud sun-march, and the sunward-horizon
    /// tap that gives cloud its warm ambient. Handing it the moon integrates the entire atmosphere
    /// with the sun 180 degrees from where it is.
    ///
    /// Pinned here rather than in `lighting.rs` because the defect was the WIRING, not either value:
    /// both directions were correct, and the wrong one was selected.
    #[test]
    fn the_atmosphere_is_driven_by_the_sun_not_the_active_light() {
        let camera: CameraUniform = bytemuck::Zeroable::zeroed();
        let clouds = voxel_environment::CloudRequest::default();

        // Deep night, where the active light is unambiguously the moon.
        let night_settings = SunSettings {
            day_night_enabled: true,
            day_phase: 0.0,
            ..SunSettings::default()
        };
        let night = uniform_at(0.0);
        let night_frame = night_settings.environment_frame();
        let moon = night.sun_direction;
        let sun = [
            night.celestial_sun[0],
            night.celestial_sun[1],
            night.celestial_sun[2],
        ];
        // The precondition: at night these genuinely differ, or the test proves nothing.
        assert!(
            (moon[0] - sun[0]).abs() + (moon[1] - sun[1]).abs() + (moon[2] - sun[2]).abs() > 0.5,
            "expected the active light ({moon:?}) to differ from the sun ({sun:?}) at night"
        );

        let request = Renderer::environment_request(night_frame, &night, &camera, clouds);
        assert_eq!(
            request.sun_direction, sun,
            "the atmosphere received the active light instead of the sun"
        );
        assert_eq!(request.sun_illuminance, night_frame.sun_illuminance);
        assert_eq!(
            request.moon_direction,
            night_frame.moon_direction.to_array()
        );
        assert_eq!(request.moon_illuminance, night_frame.moon_illuminance);
        assert_eq!(
            request.active_light_illuminance,
            night_frame.active_illuminance
        );
        assert_eq!(request.active_light_is_sun, 0.0);

        // And by day the two coincide, so the fix cannot have broken the shipped daylight look.
        let noon = uniform_at(0.5);
        let noon_frame = SunSettings {
            day_night_enabled: true,
            day_phase: 0.5,
            ..SunSettings::default()
        }
        .environment_frame();
        let noon_request = Renderer::environment_request(noon_frame, &noon, &camera, clouds);
        for axis in 0..3 {
            assert!(
                (noon_request.sun_direction[axis] - noon.sun_direction[axis]).abs() < 1.0e-6,
                "by day the sun and the active light must agree"
            );
        }
    }
}
