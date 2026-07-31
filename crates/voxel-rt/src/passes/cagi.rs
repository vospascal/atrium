//! E4 — the CAGI compute pass: the cellular automaton that floods the integer
//! light volume with sun + sky light, plus the volume's GPU resources.
//!
//! Two objects, because their lifetimes differ:
//!
//! - [`LightVolume`] — the ping-pong buffer pair, the static per-cell attributes
//!   and the volume uniform. Recreated when the resolution lever moves or CAGI is
//!   switched off (a placeholder grid then keeps the shading pass's bindings
//!   valid for ~12 bytes instead of ~13 MB). It also owns the FRONT index (which
//!   buffer holds the newest values) and the re-flood flag.
//! - [`CagiPass`] — the pipeline (cached per compile-time lever permutation) and
//!   the two bind groups, one per ping-pong direction.
//!
//! The shading pass reads the same volume (`shaders/cagi_volume.wgsl` is
//! concatenated into both shaders), so [`LightVolume::layout_entries`] /
//! [`LightVolume::bind_group_entries`] serve both consumers — with
//! `include_back_buffer` deciding whether the writable binding 12 is part of the
//! layout, which is the only difference between them.
//!
//! Own bindings (group 0), on top of [`super::world_bindings::WorldBindings`]:
//!
//! | binding | resource |
//! |---------|----------|
//! | 11 | storage (read)       front light volume |
//! | 12 | storage (read_write) back light volume — CA pass only |
//! | 13 | storage (read)       static per-cell attributes (albedo + solid bit) |
//! | 14 | uniform              grid dimensions + transport coefficients |

use wgpu::util::DeviceExt;

use crate::brickmap::Brickmap;
use crate::cagi::{build_cell_attributes, material_attribute_table, CagiGrid, CagiSettings};
use crate::material::Material;
use crate::variants::RenderQuality;

use super::world_bindings::WorldBindings;
use super::ComputePipelineCache;

/// Cells per workgroup edge — cubic, so a workgroup's six-neighbour stencil
/// mostly reads cells its own threads already loaded.
const WORKGROUP_SIZE: u32 = 4;

/// The CA shader source: the shared traversal core (the sun ray goes through
/// `trace_shadow_visibility`), then the shared light-volume half, then the
/// automaton itself.
pub const CAGI_SHADER_SOURCE: &str = concat!(
    include_str!("../../shaders/world.wgsl"),
    include_str!("../../shaders/cagi_volume.wgsl"),
    include_str!("../../shaders/cagi.wgsl"),
);

/// [`CAGI_SHADER_SOURCE`] with every lever this pass reads patched in: the
/// traversal and shadow levers (its sun ray is a real brickmap trace) and the
/// CAGI levers from both shader halves. Returns the source verbatim for the
/// shipped quality.
pub fn build_shader_source(quality: &RenderQuality) -> String {
    let traversal_patched = quality.traversal.patch_shader_source(CAGI_SHADER_SOURCE);
    let shadows_patched = quality.shadows.patch_shader_source(&traversal_patched);
    // E6: `LIQUIDS_CAST_NO_SHADOW` lives in the shared `world.wgsl`, so the CA
    // pass's per-cell sun ray must follow the water mode too — otherwise the
    // volume would still shadow the bed under water that the shading pass now
    // lights.
    let water_patched = quality.water.patch_shader_source(&shadows_patched);
    let volume_patched = quality
        .global_illumination
        .patch_volume_consts(&water_patched);
    quality
        .global_illumination
        .patch_propagation_consts(&volume_patched)
}

/// The light volume's GPU resources.
pub struct LightVolume {
    grid: CagiGrid,
    /// Ping-pong pair. `volume_buffers[front]` holds the newest values.
    volume_buffers: [wgpu::Buffer; 2],
    attributes_buffer: wgpu::Buffer,
    uniform_buffer: wgpu::Buffer,
    front: usize,
    needs_reflood: bool,
}

impl LightVolume {
    /// Allocate the volume for `settings` and upload the static cell attributes
    /// built from `brickmap`. With CAGI disabled this is the 1-cell placeholder
    /// (the shading pass still declares the bindings; nothing reads them).
    pub fn new(
        device: &wgpu::Device,
        brickmap: &Brickmap,
        settings: &CagiSettings,
        rows: &[Material],
    ) -> Self {
        Self::new_with_attributes(device, brickmap, settings, AttributeSource::BuildNow, rows)
    }

    /// The same, with a choice about the ~50 ms CPU attribute build (E4 measured
    /// it; E2 moved it off the frame): build it here, or allocate the buffer zeroed
    /// and wait for [`Self::write_all_attributes`] from the world thread.
    ///
    /// A zeroed attribute buffer is a *valid* volume — every cell reads as empty
    /// and non-absorbing — so the frame after a resolution switch renders with no
    /// GI rather than with a hitch, and the flood starts the moment the real
    /// attributes land.
    pub fn new_with_attributes(
        device: &wgpu::Device,
        brickmap: &Brickmap,
        settings: &CagiSettings,
        attribute_source: AttributeSource,
        rows: &[Material],
    ) -> Self {
        let grid = settings.grid(brickmap);
        let attributes = match (settings.enabled, attribute_source) {
            (true, AttributeSource::BuildNow) => {
                build_cell_attributes(brickmap, &grid, &material_attribute_table(rows))
            }
            _ => vec![0_u32; grid.cell_count()],
        };
        let volume_buffer = |label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: grid.volume_bytes() as u64,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        Self {
            grid,
            volume_buffers: [
                volume_buffer("cagi light volume A"),
                volume_buffer("cagi light volume B"),
            ],
            attributes_buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("cagi cell attributes"),
                contents: bytemuck::cast_slice(&attributes),
                // COPY_DST since E2: an edit patches the touched cells, and a
                // resolution switch uploads a set that was built off-frame.
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            }),
            uniform_buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("cagi volume uniform"),
                contents: bytemuck::bytes_of(&grid.uniform(rows)),
                // COPY_DST since S2: the emitter palette is built from the material
                // table, and that table is live-editable — so authoring an emissive
                // material has to be able to reach this buffer.
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
            front: 0,
            // A fresh volume is uninitialized GPU memory: the first encode must
            // clear it before any iteration reads it.
            needs_reflood: true,
        }
    }

    pub fn grid(&self) -> CagiGrid {
        self.grid
    }

    /// Which ping-pong buffer holds the newest values — what the shading pass
    /// must bind this frame.
    pub fn front(&self) -> usize {
        self.front
    }

    /// Throw the volume away and flood it from scratch: the sun moved, so every
    /// injected value AND every pinned sun-source flag is stale. E4's world is
    /// static, so this global re-flood is the only invalidation there is (E5's
    /// dirty regions need E2's edit API).
    pub fn mark_dirty(&mut self) {
        self.needs_reflood = true;
    }

    pub fn awaiting_reflood(&self) -> bool {
        self.needs_reflood
    }

    /// Bind-group-layout entries for the volume. `include_back_buffer` adds the
    /// writable binding 12 (the CA pass); the shading pass leaves it out.
    pub fn layout_entries(include_back_buffer: bool) -> Vec<wgpu::BindGroupLayoutEntry> {
        let storage_entry = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let mut entries = vec![storage_entry(11, true)];
        if include_back_buffer {
            entries.push(storage_entry(12, false));
        }
        entries.push(storage_entry(13, true));
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 14,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        entries
    }

    /// Bind-group entries reading `read_index` (and, for the CA pass, writing the
    /// other buffer). The shading pass passes `include_back_buffer = false` and
    /// [`Self::front`] as the read index.
    pub fn bind_group_entries(
        &self,
        read_index: usize,
        include_back_buffer: bool,
    ) -> Vec<wgpu::BindGroupEntry<'_>> {
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 11,
            resource: self.volume_buffers[read_index].as_entire_binding(),
        }];
        if include_back_buffer {
            entries.push(wgpu::BindGroupEntry {
                binding: 12,
                resource: self.volume_buffers[1 - read_index].as_entire_binding(),
            });
        }
        entries.push(wgpu::BindGroupEntry {
            binding: 13,
            resource: self.attributes_buffer.as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 14,
            resource: self.uniform_buffer.as_entire_binding(),
        });
        entries
    }

    /// The front buffer, for a readback (the bench's convergence + cross-check
    /// measurements).
    pub fn front_buffer(&self) -> &wgpu::Buffer {
        &self.volume_buffers[self.front]
    }

    /// Patch the static attributes of the cells an edit touched (E2). One `u32`
    /// per cell, so a placed voxel costs 4 bytes here.
    ///
    /// **Consecutive cell indices become ONE `write_buffer`.** A click touches a
    /// single cell and never notices, but a bulk edit
    /// ([`crate::world_edit::apply_bulk`]) touches a box of them, which is
    /// contiguous along X: without this, the test-pool carve spent **93 ms of one
    /// frame** issuing 28 672 four-byte writes at ~3 µs of driver overhead each —
    /// the upload, not the edit, was the hitch. Grouping the rows takes that to
    /// under a millisecond. Requires `cells` to be sorted by index, which both
    /// producers are.
    ///
    /// `grid` is the grid the cell INDICES were computed for: a mismatch means the
    /// volume was reallocated at another resolution while the edit was in flight, so
    /// the indices mean nothing here and a full rebuild is already on its way.
    pub fn write_cell_attributes(
        &self,
        queue: &wgpu::Queue,
        grid: Option<CagiGrid>,
        cells: &[(usize, u32)],
    ) {
        if grid != Some(self.grid) {
            return;
        }
        let mut run: Vec<u32> = Vec::new();
        let mut run_first_cell = 0_usize;
        let flush = |run: &mut Vec<u32>, first_cell: usize| {
            if !run.is_empty() {
                queue.write_buffer(
                    &self.attributes_buffer,
                    (first_cell * 4) as u64,
                    bytemuck::cast_slice(run),
                );
                run.clear();
            }
        };
        for (cell_index, attribute) in cells {
            if *cell_index >= self.grid.cell_count() {
                continue; // above the volume's clamped height
            }
            if run.is_empty() {
                run_first_cell = *cell_index;
            } else if *cell_index != run_first_cell + run.len() {
                flush(&mut run, run_first_cell);
                run_first_cell = *cell_index;
            }
            run.push(*attribute);
        }
        flush(&mut run, run_first_cell);
    }

    /// S2 — re-upload the volume uniform, which carries the **emitter palette**.
    ///
    /// The palette is derived from the material table, and since S2 that table is
    /// live-editable: authoring emission on a row (or an emission pattern layer on one)
    /// changes what the CA should inject. Cheap enough to be unconditional on a dirty
    /// table — the uniform is a few hundred bytes and the rest of it is grid geometry
    /// that simply rewrites to the same values.
    ///
    /// Note what this does NOT do: it does not tell any *cell* that it is an emitter.
    /// That lives in the attribute volume, so a row that becomes emissive for the first
    /// time also needs [`Self::write_all_attributes`] — which is the ~50 ms re-pack the
    /// panel offers explicitly rather than running on a slider tick.
    pub fn write_uniform(&self, queue: &wgpu::Queue, rows: &[Material]) {
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&self.grid.uniform(rows)),
        );
    }

    /// Upload a whole attribute set built elsewhere (the world thread, after a
    /// resolution switch). Ignored when it was built for a different grid than the
    /// one currently allocated — the lever may have moved again while it built.
    pub fn write_all_attributes(
        &mut self,
        queue: &wgpu::Queue,
        grid: &CagiGrid,
        attributes: &[u32],
    ) -> bool {
        if *grid != self.grid || attributes.len() != self.grid.cell_count() {
            return false;
        }
        queue.write_buffer(&self.attributes_buffer, 0, bytemuck::cast_slice(attributes));
        // The volume was flooded against zeroed attributes; start over.
        self.mark_dirty();
        true
    }

    /// GPU bytes: both ping-pong buffers, the attributes and the uniform.
    pub fn gpu_bytes(&self) -> u64 {
        self.volume_buffers[0].size()
            + self.volume_buffers[1].size()
            + self.attributes_buffer.size()
            + self.uniform_buffer.size()
    }
}

/// Where a fresh [`LightVolume`]'s static attributes come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributeSource {
    /// Build them from the brickmap now, on the calling thread (~50 ms — a frame
    /// hitch if that thread is the frame thread).
    BuildNow,
    /// Allocate zeroed and wait for [`LightVolume::write_all_attributes`].
    Deferred,
}

pub struct CagiPass {
    pipeline_cache: ComputePipelineCache,
    bind_group_layout: wgpu::BindGroupLayout,
    /// `bind_groups[i]` reads volume `i` and writes volume `1 - i`.
    bind_groups: [wgpu::BindGroup; 2],
}

impl CagiPass {
    pub fn new(
        device: &wgpu::Device,
        world_bindings: &WorldBindings,
        light_volume: &LightVolume,
    ) -> Self {
        Self::new_with_shader_source(device, world_bindings, light_volume, CAGI_SHADER_SOURCE)
    }

    /// Build the pass from an explicit shader source — the benchmark's entry
    /// point for A/B variants (patched copies of [`CAGI_SHADER_SOURCE`]).
    pub fn new_with_shader_source(
        device: &wgpu::Device,
        world_bindings: &WorldBindings,
        light_volume: &LightVolume,
        shader_source: &str,
    ) -> Self {
        let bind_group_layout = create_bind_group_layout(device);
        let pipeline_cache = ComputePipelineCache::new(
            device,
            "cagi pass",
            "cagi_main",
            shader_source,
            &bind_group_layout,
        );
        let bind_groups =
            create_bind_groups(device, &bind_group_layout, world_bindings, light_volume);
        Self {
            pipeline_cache,
            bind_group_layout,
            bind_groups,
        }
    }

    /// Refresh the bind groups after the volume was recreated (resolution lever
    /// or the CAGI master lever moved).
    pub fn rebind(
        &mut self,
        device: &wgpu::Device,
        world_bindings: &WorldBindings,
        light_volume: &LightVolume,
    ) {
        self.bind_groups = create_bind_groups(
            device,
            &self.bind_group_layout,
            world_bindings,
            light_volume,
        );
    }

    /// Switch to a patched shader source (a compile-time CAGI/traversal lever
    /// changed), compiling only on a cache miss.
    pub fn set_shader_source(&mut self, device: &wgpu::Device, shader_source: &str) {
        self.pipeline_cache
            .set_shader_source(device, shader_source, &self.bind_group_layout);
    }

    /// Precompile the permutations the quality presets need.
    pub fn prewarm_pipelines(&mut self, device: &wgpu::Device, shader_sources: &[String]) -> usize {
        self.pipeline_cache
            .prewarm(device, shader_sources, &self.bind_group_layout)
    }

    pub fn cached_pipeline_count(&self) -> usize {
        self.pipeline_cache.len()
    }

    /// Record `iterations` CA steps, flipping the ping-pong buffers between them.
    ///
    /// ALL iterations share ONE compute pass, deliberately: WebGPU orders
    /// dispatches within a pass and makes each one's storage writes visible to the
    /// next, and keeping the command buffer down to a single compute pass is what
    /// lets the frame's GPU timestamps resolve on Metal (the bench doc records
    /// that pass-boundary counters read zero once a command buffer holds more than
    /// one compute pass — which is also why the app submits this pass in its own
    /// command buffer).
    ///
    /// A pending re-flood clears both buffers first (outside the pass — a clear is
    /// a transfer, not a dispatch). The pass is opened even for zero iterations so
    /// the timing span always resolves.
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        light_volume: &mut LightVolume,
        iterations: u32,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        if light_volume.needs_reflood {
            for buffer in &light_volume.volume_buffers {
                encoder.clear_buffer(buffer, 0, None);
            }
            light_volume.front = 0;
            light_volume.needs_reflood = false;
        }
        let grid = light_volume.grid;
        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("cagi pass"),
            timestamp_writes,
        });
        compute_pass.set_pipeline(self.pipeline_cache.active());
        for _ in 0..iterations {
            compute_pass.set_bind_group(0, &self.bind_groups[light_volume.front], &[]);
            compute_pass.dispatch_workgroups(
                grid.size[0].div_ceil(WORKGROUP_SIZE),
                grid.size[1].div_ceil(WORKGROUP_SIZE),
                grid.size[2].div_ceil(WORKGROUP_SIZE),
            );
            light_volume.front = 1 - light_volume.front;
        }
    }
}

fn create_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let mut entries = WorldBindings::layout_entries();
    entries.extend(LightVolume::layout_entries(true));
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("cagi bind group layout"),
        entries: &entries,
    })
}

fn create_bind_groups(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    world_bindings: &WorldBindings,
    light_volume: &LightVolume,
) -> [wgpu::BindGroup; 2] {
    let bind_group = |read_index: usize| {
        let mut entries = world_bindings.bind_group_entries();
        entries.extend(light_volume.bind_group_entries(read_index, true));
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cagi bind group"),
            layout,
            entries: &entries,
        })
    };
    [bind_group(0), bind_group(1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cagi::{CagiRule, CagiSampleMode, CagiSkyTest};

    #[test]
    fn default_settings_build_the_shipped_shader() {
        assert_eq!(
            build_shader_source(&RenderQuality::default()),
            CAGI_SHADER_SOURCE
        );
    }

    /// The CA shader must see the SAME traversal core the shading pass uses — the
    /// point of concatenating `world.wgsl` into both instead of copying the DDA.
    #[test]
    fn both_pass_shaders_share_the_traversal_core() {
        let world = include_str!("../../shaders/world.wgsl");
        assert!(CAGI_SHADER_SOURCE.starts_with(world));
        assert!(crate::passes::dda::SHADER_SOURCE.starts_with(world));
        assert!(world.contains("fn trace_shadow_visibility"));
        assert!(world.contains("fn dda_step"));
        // ...and neither pass shader may carry its own copy.
        let cagi_body = include_str!("../../shaders/cagi.wgsl");
        let dda_body = include_str!("../../shaders/dda.wgsl");
        assert!(!cagi_body.contains("fn dda_step"));
        assert!(!dda_body.contains("fn dda_step"));
    }

    /// Every CAGI lever combination must validate under naga: a WGSL error on a
    /// non-default propagation rule must not wait for someone to click the radio
    /// button.
    #[test]
    fn every_cagi_lever_combination_compiles_headless() {
        let Some((device, _queue)) = crate::passes::dda::tests::headless_device() else {
            return;
        };
        let bind_group_layout = create_bind_group_layout(&device);
        for rule in [
            CagiRule::MaxDecrement,
            CagiRule::Diffusion6,
            CagiRule::Diffusion26,
        ] {
            for sky_test in [CagiSkyTest::ColumnMax, CagiSkyTest::UpwardTrace] {
                for sun_cache in [true, false] {
                    for sample_mode in [CagiSampleMode::Nearest, CagiSampleMode::Trilinear] {
                        let quality = RenderQuality {
                            global_illumination: CagiSettings {
                                rule,
                                sky_test,
                                sun_cache,
                                sample_mode,
                                ..CagiSettings::default()
                            },
                            ..RenderQuality::default()
                        };
                        let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
                        let _pipeline = crate::passes::create_compute_pipeline(
                            &device,
                            "cagi test pipeline",
                            "cagi_main",
                            &build_shader_source(&quality),
                            &bind_group_layout,
                        );
                        let validation_error = pollster::block_on(error_scope.pop());
                        assert!(
                            validation_error.is_none(),
                            "CAGI {rule:?} / {sky_test:?} / cache {sun_cache} / \
                             {sample_mode:?} failed wgpu validation: {validation_error:?}"
                        );
                    }
                }
            }
        }
    }
}
