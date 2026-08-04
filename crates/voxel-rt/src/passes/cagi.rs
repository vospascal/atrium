//! E4 — the CAGI compute pass: the cellular automaton that floods the integer
//! light volume with sun + sky light, plus the volume's GPU resources.
//!
//! Two objects, because their lifetimes differ:
//!
//! - [`LightVolume`] — the ping-pong buffer pair, the static per-cell attributes
//!   and the volume uniform. Recreated when the resolution lever moves or CAGI is
//!   switched off (a placeholder grid then keeps the shading pass's bindings
//!   valid for ~16 bytes instead of ~13 MB). It also owns the FRONT index (which
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
//! | 13 | storage (read)       per-cell attributes + E5b emission (5 u32 words) |
//! | 14 | uniform              grid dimensions, transport coefficients and the S3b event-response table |
//! | 15 | storage (read)       shared AADF directional bounds |

use wgpu::util::DeviceExt;

use crate::brickmap::Brickmap;
use crate::cagi::{
    build_cell_attributes_with_emission, CagiGrid, CagiSettings, GpuEventResponse, LightCellUpdate,
    MaterialAttributes, CELL_DATA_WORDS, EVENT_RESPONSE_SLOTS,
};
use crate::variants::RenderQuality;

use super::world_bindings::WorldBindings;
use super::ComputePipelineCache;
use voxel_environment::{EnvironmentGpu, HillaireEnvironment, ENVIRONMENT_BIND_GROUP};

/// Cells per workgroup edge — cubic, so a workgroup's six-neighbour stencil
/// mostly reads cells its own threads already loaded.
const WORKGROUP_SIZE: u32 = 4;

/// This crate's own half of the CA shader: the shared traversal core, the shared
/// light-volume half, then the automaton itself.
///
/// **Not the complete module** — [`CAGI_SHADER_SOURCE`] is. Same arrangement as the
/// shading pass's `OWN_SHADER_SOURCE`, and for the same reason: `concat!` takes literals,
/// so the piece that comes from another crate cannot join here.
const OWN_SHADER_SOURCE: &str = concat!(
    include_str!("../../shaders/world.wgsl"),
    include_str!("../../shaders/cagi_volume.wgsl"),
    include_str!("../../shaders/cagi.wgsl"),
);

/// The CA shader source: this crate's half, then the environment sampler.
///
/// A `LazyLock<String>` because the environment WGSL is a const in another crate. It used
/// to be a `const` built with an `include_str!` reaching across the crate boundary by
/// relative path — `../../../voxel-environment/shaders/environment.wgsl` — which bypassed
/// that crate's facade entirely and broke the moment its shader was split into fragments.
/// The environment's own `WGSL` const is the supported way in; the source of truth is a
/// crate, not a path.
pub static CAGI_SHADER_SOURCE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let mut source =
        String::with_capacity(OWN_SHADER_SOURCE.len() + HillaireEnvironment::WGSL.len());
    source.push_str(OWN_SHADER_SOURCE);
    source.push_str(HillaireEnvironment::WGSL);
    source
});

/// [`CAGI_SHADER_SOURCE`] with every lever this pass reads patched in: traversal,
/// shadow and CAGI levers from both shader halves. Environment source visibility
/// is deliberately not a compile-time ray marcher; it is sampled from the LUT.
pub fn build_shader_source(quality: &RenderQuality) -> String {
    let traversal_patched = quality.traversal.patch_shader_source(&CAGI_SHADER_SOURCE);
    let shadows_patched = quality.shadows.patch_shader_source(&traversal_patched);
    // E6: `LIQUIDS_CAST_NO_SHADOW` lives in the shared `world.wgsl`, so the CA
    // pass's source geometry must follow the water mode too — otherwise the
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
    /// Two u32 words per cell: the packed bounce attribute and E5b's HDR
    /// emission. Keeping them together stays within macOS's 11-storage-buffer
    /// compute-stage limit and avoids a mostly-zero vec4 allocation.
    cell_data_buffer: wgpu::Buffer,
    uniform_buffer: wgpu::Buffer,
    front: usize,
    needs_reflood: bool,
}

fn pack_emission(emission: [f32; 4]) -> u32 {
    crate::cagi::pack_light(crate::cagi::quantize_radiance([
        emission[0],
        emission[1],
        emission[2],
    ]))
}

/// Append ONE cell's binding-13 payload. The single place that decides how many
/// words a cell occupies, because both writers stride by
/// [`CELL_DATA_WORDS`] and a writer that pushed a different count would silently
/// shift every following cell — which is exactly what the incremental path did
/// while the full upload was correct.
fn push_cell_data(data: &mut Vec<u32>, attribute: u32, emission: [f32; 4]) {
    let before = data.len();
    data.push(attribute);
    data.push(pack_emission(emission));
    debug_assert_eq!(
        data.len() - before,
        CELL_DATA_WORDS,
        "a cell payload must be exactly CELL_DATA_WORDS, or the stride lies"
    );
}

fn pack_cell_data(attributes: &[u32], emissions: &[[f32; 4]]) -> Vec<u32> {
    assert_eq!(attributes.len(), emissions.len());
    let mut data = Vec::with_capacity(attributes.len() * CELL_DATA_WORDS);
    for (&attribute, emission) in attributes.iter().zip(emissions) {
        push_cell_data(&mut data, attribute, *emission);
    }
    data
}

impl LightVolume {
    /// Allocate the volume for `settings` and upload the static cell attributes
    /// built from `brickmap`. With CAGI disabled this is the 1-cell placeholder
    /// (the shading pass still declares the bindings; nothing reads them).
    pub fn new(
        device: &wgpu::Device,
        brickmap: &Brickmap,
        settings: &CagiSettings,
        attributes: &MaterialAttributes,
    ) -> Self {
        Self::new_with_attributes(
            device,
            brickmap,
            settings,
            AttributeSource::BuildNow,
            attributes,
        )
    }

    /// The same, with a choice about the ~0.5 s CPU attribute build (release bench
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
        material_attributes: &MaterialAttributes,
    ) -> Self {
        let grid = settings.grid(brickmap);
        let (attributes, emissions) = match (settings.enabled, attribute_source) {
            (true, AttributeSource::BuildNow) => {
                build_cell_attributes_with_emission(brickmap, &grid, material_attributes)
            }
            _ => (
                vec![0_u32; grid.cell_count()],
                vec![[0.0_f32; 4]; grid.cell_count()],
            ),
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
        let cell_data = pack_cell_data(&attributes, &emissions);
        Self {
            grid,
            volume_buffers: [
                volume_buffer("cagi light volume A"),
                volume_buffer("cagi light volume B"),
            ],
            cell_data_buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("cagi cell attributes and emissions"),
                contents: bytemuck::cast_slice(&cell_data),
                // COPY_DST since E2: an edit patches the touched cells, and a
                // resolution switch uploads a set that was built off-frame.
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            }),
            uniform_buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("cagi volume uniform"),
                contents: bytemuck::bytes_of(&grid.uniform(material_attributes)),
                // COPY_DST since S3b: the geometry half is fixed for the volume's
                // lifetime, but the event-response table follows the material
                // graphs and is re-uploaded whenever they recompile.
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

    /// Throw the volume away and flood it from scratch after a source change. E4's world is
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
            resource: self.cell_data_buffer.as_entire_binding(),
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
    /// **Consecutive cell indices become ONE `write_buffer`.** A one-metre edit
    /// overlaps several CAGI cells, and grouping those sorted rows avoids paying
    /// driver overhead once per four-byte attribute.
    ///
    /// `grid` is the grid the cell INDICES were computed for: a mismatch means the
    /// volume was reallocated at another resolution while the edit was in flight, so
    /// the indices mean nothing here and a full rebuild is already on its way.
    ///
    /// S3b leaves one narrow window open here, knowingly. The response slots
    /// packed into these words come from the edit's own `MaterialAttributes`; if
    /// a graph recompiled since the last full repack, that table may allocate
    /// slots differently from the rows the volume currently holds, so an edited
    /// cell can point at a stale response for the frame or two before the repack
    /// (which a recompile always requests) lands. Closing it would mean sending
    /// the 384-byte table alongside every 4-byte cell patch. The symptom is one
    /// voxel briefly at the wrong brightness; the settle burst erases it.
    pub fn write_cell_attributes(
        &self,
        queue: &wgpu::Queue,
        grid: Option<CagiGrid>,
        cells: &[LightCellUpdate],
    ) {
        if grid != Some(self.grid) {
            return;
        }
        let mut run: Vec<u32> = Vec::new();
        let mut run_first_cell = 0_usize;
        let flush = |run: &mut Vec<u32>, first_cell: usize| {
            if !run.is_empty() {
                queue.write_buffer(
                    &self.cell_data_buffer,
                    (first_cell * CELL_DATA_WORDS * 4) as u64,
                    bytemuck::cast_slice(run),
                );
                run.clear();
            }
        };
        for update in cells {
            if update.index >= self.grid.cell_count() {
                continue; // above the volume's clamped height
            }
            if run.is_empty() {
                run_first_cell = update.index;
            } else if update.index != run_first_cell + run.len() / CELL_DATA_WORDS {
                flush(&mut run, run_first_cell);
                run_first_cell = update.index;
            }
            // The SAME per-cell packer the initial upload and the full re-pack
            // use: the run offsets above are computed in cell strides, so a cell
            // written at any other width scribbles its emission over the next
            // cell's attribute word.
            push_cell_data(&mut run, update.attribute, update.emission);
        }
        flush(&mut run, run_first_cell);
    }

    /// Upload a whole attribute set built elsewhere (the world thread, after a
    /// resolution switch). Ignored when it was built for a different grid than the
    /// one currently allocated — the lever may have moved again while it built.
    pub fn write_all_attributes(
        &mut self,
        queue: &wgpu::Queue,
        grid: &CagiGrid,
        attributes: &[u32],
        emissions: &[[f32; 4]],
        responses: &[GpuEventResponse; EVENT_RESPONSE_SLOTS],
    ) -> bool {
        if *grid != self.grid
            || attributes.len() != self.grid.cell_count()
            || emissions.len() != self.grid.cell_count()
        {
            return false;
        }
        let cell_data = pack_cell_data(attributes, emissions);
        queue.write_buffer(&self.cell_data_buffer, 0, bytemuck::cast_slice(&cell_data));
        // The response rows and the slot indices packed into the attribute words
        // are ONE table: installing the words without the rows would point a
        // cell at a response the volume does not hold yet.
        self.write_event_responses(queue, responses);
        // The volume was flooded against zeroed attributes; start over.
        self.mark_dirty();
        true
    }

    /// Re-upload the S3b event-response rows alone.
    ///
    /// A partial write at the table's own offset rather than a whole new
    /// uniform: the geometry half is derived from the grid this volume was
    /// allocated for, and rebuilding it here would be a second place that could
    /// disagree with `CagiGrid::uniform`.
    fn write_event_responses(
        &self,
        queue: &wgpu::Queue,
        responses: &[GpuEventResponse; EVENT_RESPONSE_SLOTS],
    ) {
        queue.write_buffer(
            &self.uniform_buffer,
            std::mem::offset_of!(crate::cagi::CagiVolumeUniform, event_responses) as u64,
            bytemuck::cast_slice(responses.as_slice()),
        );
    }

    /// GPU bytes: both ping-pong buffers, the attributes and the uniform.
    pub fn gpu_bytes(&self) -> u64 {
        self.volume_buffers[0].size()
            + self.volume_buffers[1].size()
            + self.cell_data_buffer.size()
            + self.uniform_buffer.size()
    }
}

/// Where a fresh [`LightVolume`]'s static attributes come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributeSource {
    /// Build them from the brickmap now, on the calling thread (~0.5 s — a frame
    /// hitch if that thread is the frame thread).
    BuildNow,
    /// Allocate zeroed and wait for [`LightVolume::write_all_attributes`].
    Deferred,
}

pub struct CagiPass {
    pipeline_cache: ComputePipelineCache,
    bind_group_layout: wgpu::BindGroupLayout,
    environment_bind_group_layout: wgpu::BindGroupLayout,
    environment_bind_group: wgpu::BindGroup,
    /// `bind_groups[i]` reads volume `i` and writes volume `1 - i`.
    bind_groups: [wgpu::BindGroup; 2],
}

impl CagiPass {
    pub fn new(
        device: &wgpu::Device,
        world_bindings: &WorldBindings,
        light_volume: &LightVolume,
    ) -> Self {
        let environment = HillaireEnvironment::new(device);
        Self::new_with_environment(device, world_bindings, light_volume, &environment)
    }

    pub fn new_with_environment(
        device: &wgpu::Device,
        world_bindings: &WorldBindings,
        light_volume: &LightVolume,
        environment: &dyn EnvironmentGpu,
    ) -> Self {
        Self::new_with_environment_and_shader_source(
            device,
            world_bindings,
            light_volume,
            environment,
            &CAGI_SHADER_SOURCE,
        )
    }

    /// Build the pass from an explicit shader source — the benchmark's entry
    /// point for A/B variants (patched copies of [`CAGI_SHADER_SOURCE`]).
    pub fn new_with_shader_source(
        device: &wgpu::Device,
        world_bindings: &WorldBindings,
        light_volume: &LightVolume,
        shader_source: &str,
    ) -> Self {
        let environment = HillaireEnvironment::new(device);
        Self::new_with_environment_and_shader_source(
            device,
            world_bindings,
            light_volume,
            &environment,
            shader_source,
        )
    }

    pub fn new_with_environment_and_shader_source(
        device: &wgpu::Device,
        world_bindings: &WorldBindings,
        light_volume: &LightVolume,
        environment: &dyn EnvironmentGpu,
        shader_source: &str,
    ) -> Self {
        let bind_group_layout = create_bind_group_layout(device);
        let environment_bind_group_layout = environment.sample_bind_group_layout().clone();
        let pipeline_cache = ComputePipelineCache::new_with_layouts(
            device,
            "cagi pass",
            "cagi_main",
            shader_source,
            &[
                Some(&bind_group_layout),
                Some(&environment_bind_group_layout),
            ],
        );
        let bind_groups =
            create_bind_groups(device, &bind_group_layout, world_bindings, light_volume);
        Self {
            pipeline_cache,
            bind_group_layout,
            environment_bind_group_layout,
            environment_bind_group: environment.sample_bind_group().clone(),
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
        self.pipeline_cache.set_shader_source_with_layouts(
            device,
            shader_source,
            &[
                Some(&self.bind_group_layout),
                Some(&self.environment_bind_group_layout),
            ],
        );
    }

    /// Precompile the permutations the quality presets need.
    pub fn prewarm_pipelines(&mut self, device: &wgpu::Device, shader_sources: &[String]) -> usize {
        self.pipeline_cache.prewarm_with_layouts(
            device,
            shader_sources,
            &[
                Some(&self.bind_group_layout),
                Some(&self.environment_bind_group_layout),
            ],
        )
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
        compute_pass.set_bind_group(ENVIRONMENT_BIND_GROUP, &self.environment_bind_group, &[]);
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

    /// Every writer of binding 13 must lay a cell down in exactly
    /// [`CELL_DATA_WORDS`], because the byte offsets are computed in cell strides.
    ///
    /// This is a regression test, not a formality: the full upload packed 2 words
    /// per cell while the incremental edit path pushed 5 (the attribute plus four
    /// raw f32 channels). Nothing caught it — the GPU write path has no test — so
    /// every edit wrote at 2.5x the stride and scribbled f32 bit patterns over the
    /// following cells' attribute words.
    #[test]
    fn every_writer_lays_a_cell_down_in_one_stride() {
        let attributes = [0xdead_beef_u32, 0x0000_0001, 0x00ff_ffff];
        let emissions = [[3.0, 0.0, 0.0, 0.0], [0.0; 4], [0.25, 0.5, 1.0, 0.0]];

        let packed = pack_cell_data(&attributes, &emissions);
        assert_eq!(packed.len(), attributes.len() * CELL_DATA_WORDS);

        // The incremental path's per-cell push must produce byte-identical output
        // to the full upload's, or a patched cell disagrees with a re-packed one.
        let mut incremental = Vec::new();
        for (&attribute, &emission) in attributes.iter().zip(&emissions) {
            push_cell_data(&mut incremental, attribute, emission);
        }
        assert_eq!(incremental, packed);

        // And each cell's attribute is readable at its own stride, which is what
        // `cagi_cell_attribute` in the WGSL assumes.
        for (cell, &attribute) in attributes.iter().enumerate() {
            assert_eq!(packed[cell * CELL_DATA_WORDS], attribute);
        }
    }

    /// THE gate S3b exists for: an event-gated emitter must change the light in
    /// the cells AROUND it, not only on its own face.
    ///
    /// Runs the real CA on a real device against a real brickmap and reads the
    /// volume back, because every other S3b test stops at the CPU. Between "the
    /// response reaches the material table" and "the room gets brighter" sit the
    /// attribute pack, the uniform upload, the bind group and three shader read
    /// sites, and none of those had a test that would notice them failing.
    #[test]
    fn an_event_gated_emitter_lights_its_neighbours_only_when_triggered() {
        use crate::animation_clock::AnimationClockSample;
        use crate::cagi::material_attribute_table;
        use crate::lighting::SunSettings;
        use crate::material::{material_id, MATERIALS, MATERIAL_COUNT};
        use crate::material_graph::{EmissionEventResponse, EventSensorConfig, SensorFalloff};
        use crate::world_event::{GpuWorldEvent, MAX_WORLD_EVENTS};
        use voxel_core::world::Voxel;

        let Some((device, queue)) = crate::passes::dda::tests::headless_device() else {
            return;
        };

        // One 1 m glow block (8^3 traversal voxels) in an otherwise empty world,
        // so nothing but the emitter can put light into the cells beside it.
        let glow = material_id(Voxel::GlowBlock);
        let origin = [64_i32, 64, 64];
        let mut brickmap = Brickmap::empty();
        for offset_z in 0..8 {
            for offset_y in 0..8 {
                for offset_x in 0..8 {
                    brickmap.set_voxel(
                        origin[0] + offset_x,
                        origin[1] + offset_y,
                        origin[2] + offset_z,
                        Voxel::GlowBlock,
                        crate::brickmap::ClearanceUpdate::FullRebuild,
                    );
                }
            }
        }

        // Dark at rest, bright when something is within 8 m — the authored glow
        // block's shape, stated here rather than loaded so the gate does not
        // start failing because someone retuned an asset.
        let mut rows = MATERIALS.to_vec();
        rows[glow as usize].emission = None;
        let mut responses = vec![None; MATERIAL_COUNT];
        responses[glow as usize] = Some(EmissionEventResponse {
            sensor: EventSensorConfig {
                channel: 0,
                radius_meters: 8.0,
                falloff: SensorFalloff::Smoothstep,
                attack_seconds: 0.0,
                hold_seconds: 0.0,
                release_seconds: 0.0,
                invert: false,
            },
            resting: [0.0; 3],
            triggered: [1.0, 0.9, 0.8],
        });
        let attributes = material_attribute_table(&rows, &responses);
        assert_ne!(
            attributes.word(glow) & crate::cagi::CELL_EVENT_RESPONSE_MASK,
            0,
            "the emitter did not get a response slot, so nothing below can pass"
        );

        let settings = CagiSettings::default();
        let world_bindings = WorldBindings::new(&device, &brickmap);
        let mut light_volume = LightVolume::new(&device, &brickmap, &settings, &attributes);
        let cagi_pass = CagiPass::new(&device, &world_bindings, &light_volume);
        let grid = light_volume.grid();

        // The cell just above the block: air, touching an emissive face, and the
        // one `cagi_emitter_bounce` is supposed to carry the emission into.
        let neighbour = [
            (origin[0] as u32 + 4) / settings.cell_voxels,
            (origin[1] as u32 + 8) / settings.cell_voxels,
            (origin[2] as u32 + 4) / settings.cell_voxels,
        ];
        let neighbour_index = grid.cell_index(neighbour);

        let quality = RenderQuality::default();
        let mut run = |events: &[GpuWorldEvent; MAX_WORLD_EVENTS], count: usize| -> u32 {
            let (animation_params, event_params) = quality.animation_params(
                AnimationClockSample::FROZEN,
                AnimationClockSample::FROZEN,
                count,
            );
            // Sun and sky OFF: the emitter must be the only thing in the volume,
            // or its contribution is a rounding error against daylight — which is
            // exactly why this is measured here and not judged by eye.
            let mut lighting = SunSettings::default().lighting_uniform(
                quality.shading_params(),
                quality.gi_params(),
                quality.water_params(),
                // Zero height: this helper renders no pixels, so the octave
                // cutoff has no footprint to compare against and keeps them all.
                quality.material_params(0),
                animation_params,
                event_params,
            );
            lighting.sun_color_intensity[3] = 0.0;
            lighting.sky_ambient[3] = 0.0;
            world_bindings.write_lighting(&queue, &lighting);
            world_bindings.write_world_events(&queue, events);
            light_volume.mark_dirty();
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("s3b gate"),
            });
            cagi_pass.encode(&mut encoder, &mut light_volume, 24, None);
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("s3b gate readback"),
                size: light_volume.front_buffer().size(),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_buffer_to_buffer(
                light_volume.front_buffer(),
                0,
                &readback,
                0,
                readback.size(),
            );
            queue.submit([encoder.finish()]);
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, |result| result.expect("map failed"));
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .expect("poll failed");
            let words =
                bytemuck::cast_slice::<u8, u32>(&readback.slice(..).get_mapped_range()).to_vec();
            readback.unmap();
            crate::cagi::unpack_light(words[neighbour_index])[0]
        };

        let dark = run(&[GpuWorldEvent::INACTIVE; MAX_WORLD_EVENTS], 0);

        let mut events = [GpuWorldEvent::INACTIVE; MAX_WORLD_EVENTS];
        events[0] = GpuWorldEvent {
            // Standing right at the block, in METRES.
            position_meters: [
                (origin[0] as f32 + 4.0) * voxel_core::world::VOXEL_SIZE,
                (origin[1] as f32 + 12.0) * voxel_core::world::VOXEL_SIZE,
                (origin[2] as f32 + 4.0) * voxel_core::world::VOXEL_SIZE,
            ],
            radius_meters: 12.0,
            started_epoch: 0.0,
            started_remainder_seconds: 0.0,
            ended_epoch: 0.0,
            ended_remainder_seconds: 0.0,
            channel: 0,
            strength: 1.0,
            open: 1.0,
            _pad_row2: 0.0,
        };
        let lit = run(&events, 1);

        println!("neighbour cell above the emitter: rest {dark}/1023, lit {lit}/1023");
        assert_eq!(
            dark, 0,
            "the emitter is authored dark at rest, so the cell above it must hold \
             no light with nothing near — got {dark}/1023"
        );
        assert!(
            lit > 0,
            "THE S3b GATE FAILED: with an event on top of the emitter the cell \
             above it is still {lit}/1023. The surface lights up (that is the DDA \
             graph) but the light volume never saw it, so nothing around the block \
             changes"
        );
    }

    #[test]
    fn default_settings_build_the_shipped_shader() {
        assert_eq!(
            build_shader_source(&RenderQuality::default()),
            CAGI_SHADER_SOURCE.as_str()
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
        assert!(
            !cagi_body.contains("trace_shadow_visibility"),
            "CAGI source injection must stay ray-free"
        );

        // The output colour path is the same arrangement one crate out: `voxel-color`
        // owns both the Rust curves and the WGSL, the shading pass SPLICES it, and a copy
        // living here would be the drift the move was made to prevent. The CA pass must
        // NOT have it — it bakes cell radiance and never tonemaps anything.
        assert!(!dda_body.contains("fn apply_tonemap"));
        assert!(dda_body.contains("apply_tonemap("));
        assert!(crate::passes::dda::SHADER_SOURCE.contains("fn apply_tonemap"));
        assert!(!CAGI_SHADER_SOURCE.contains("fn apply_tonemap"));
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
                        // E5c sweeps here too, and specifically for the OFF value: the
                        // bounce is called from inside its own `if`, so with the lever
                        // off naga must delete `cagi_emitter_bounce` and its cell_data
                        // reads entirely. Pairing it with `rule` is the useful axis —
                        // the whole point of the lever is that it makes the rules agree.
                        let emitter_bounce = rule != CagiRule::MaxDecrement;
                        // S3b pairs with the sample mode rather than getting its
                        // own nesting level: `cagi_cell_emission_live` is called
                        // from three sites and with the lever OFF naga must
                        // delete the `world_event_sense` call and the response
                        // table read entirely, which is the shape that can break.
                        let event_light = sample_mode == CagiSampleMode::Trilinear;
                        let quality = RenderQuality {
                            global_illumination: CagiSettings {
                                rule,
                                sky_test,
                                sun_cache,
                                sample_mode,
                                emitter_bounce,
                                event_light,
                                ..CagiSettings::default()
                            },
                            ..RenderQuality::default()
                        };
                        let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
                        let environment = HillaireEnvironment::new(&device);
                        let _pipeline = crate::passes::create_compute_pipeline_with_layouts(
                            &device,
                            "cagi test pipeline",
                            "cagi_main",
                            &build_shader_source(&quality),
                            &[
                                Some(&bind_group_layout),
                                Some(environment.sample_bind_group_layout()),
                            ],
                        );
                        let validation_error = pollster::block_on(error_scope.pop());
                        assert!(
                            validation_error.is_none(),
                            "CAGI {rule:?} / {sky_test:?} / cache {sun_cache} / \
                             {sample_mode:?} / bounce {emitter_bounce} / event light \
                             {event_light} failed wgpu validation: {validation_error:?}"
                        );
                    }
                }
            }
        }
    }
}
