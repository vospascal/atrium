//! Render passes. Each pass is a self-contained unit with a consistent shape:
//! `new(device, ...)` creates all GPU resources, `rebind(device, ...)` refreshes
//! size-dependent bindings after a resize, and `encode(...)` records its work
//! into a caller-owned command encoder. The frame loop composes passes; later
//! stages add reflection / post passes with the same shape.
//!
//! Shared here: [`world_bindings::WorldBindings`] (the brickmap + lighting
//! resources every world-traversing pass binds) and [`ComputePipelineCache`] (the
//! compile-on-miss pipeline cache both compute passes use, so switching a
//! compile-time lever is a hash lookup instead of a mid-frame shader compile).

pub mod binding;
pub mod blit;
pub mod cagi;
pub mod composer;

use composer::ShaderProgram;
pub mod dda;
pub mod world_bindings;

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

/// Compiled compute pipelines keyed by the hash of their shader source.
///
/// Every compile-time lever (E1c's registry) is a WGSL const patched into a
/// pass's shader source, so a preset switch means a different source string. The
/// app prewarms every preset's permutation at startup ([`Self::prewarm`]) and the
/// switch is then a lookup — measured at 67 µs for all four presets against ~2 ms
/// per shader compile (bench doc, E1c section).
pub struct ComputePipelineCache {
    label: &'static str,
    entry_point: &'static str,
    pipelines: HashMap<u64, wgpu::ComputePipeline>,
    active_key: u64,
}

impl ComputePipelineCache {
    pub fn new(
        device: &wgpu::Device,
        label: &'static str,
        entry_point: &'static str,
        program: &ShaderProgram,
        bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        Self::new_with_layouts(
            device,
            label,
            entry_point,
            program,
            &[Some(bind_group_layout)],
        )
    }

    pub(crate) fn new_with_layouts(
        device: &wgpu::Device,
        label: &'static str,
        entry_point: &'static str,
        program: &ShaderProgram,
        bind_group_layouts: &[Option<&wgpu::BindGroupLayout>],
    ) -> Self {
        let mut cache = Self {
            label,
            entry_point,
            pipelines: HashMap::new(),
            active_key: 0,
        };
        cache.active_key = cache.compile(device, program, bind_group_layouts);
        cache
    }

    /// Hash of a shader source — pipelines are keyed by it rather than by the
    /// ~70 KB string itself.
    ///
    /// Keyed on the joined *source* rather than on the composed module or the lever def set, and
    /// deliberately: it is the same key this cache used before composition existed, so every
    /// preset switch hits and misses exactly as it did. Re-keying would have been a second
    /// behaviour change riding along with the compile path.
    pub(crate) fn source_key(shader_source: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        shader_source.hash(&mut hasher);
        hasher.finish()
    }

    /// Dispatch the pipeline for `source` from now on, composing and compiling only on a miss.
    ///
    /// `compose` is a closure rather than a built [`ShaderProgram`] because composing is
    /// ~30 ms — naga_oil parses every fragment and round-trips each composable module through
    /// its WGSL backend to validate it — while a hit is a hash lookup. The hot path is
    /// overwhelmingly a hit: `rebuild_dda_shader` runs on *every* graph editor command, and
    /// prewarming already put all four presets in the cache. Building the program eagerly here
    /// put that 30 ms back onto every node drag, which is exactly what the cache exists to
    /// avoid.
    pub fn set_source_with_layouts(
        &mut self,
        device: &wgpu::Device,
        source: &str,
        compose: impl FnOnce() -> naga::Module,
        bind_group_layouts: &[Option<&wgpu::BindGroupLayout>],
    ) {
        let key = Self::source_key(source);
        if !self.pipelines.contains_key(&key) {
            let pipeline = create_compute_pipeline_from_module(
                device,
                self.label,
                self.entry_point,
                &compose(),
                bind_group_layouts,
            );
            self.pipelines.insert(key, pipeline);
        }
        self.active_key = key;
    }

    /// Precompile `programs` (duplicates cost nothing). Returns how many
    /// distinct pipelines the cache holds afterwards.
    pub fn prewarm(
        &mut self,
        device: &wgpu::Device,
        programs: &[ShaderProgram],
        bind_group_layout: &wgpu::BindGroupLayout,
    ) -> usize {
        for program in programs {
            self.compile(device, program, &[Some(bind_group_layout)]);
        }
        self.pipelines.len()
    }

    pub(crate) fn prewarm_with_layouts(
        &mut self,
        device: &wgpu::Device,
        programs: &[ShaderProgram],
        bind_group_layouts: &[Option<&wgpu::BindGroupLayout>],
    ) -> usize {
        for program in programs {
            self.compile(device, program, bind_group_layouts);
        }
        self.pipelines.len()
    }

    pub fn len(&self) -> usize {
        self.pipelines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pipelines.is_empty()
    }

    pub fn active(&self) -> &wgpu::ComputePipeline {
        self.pipelines
            .get(&self.active_key)
            .expect("the active pipeline is always cached")
    }

    fn compile(
        &mut self,
        device: &wgpu::Device,
        program: &ShaderProgram,
        bind_group_layouts: &[Option<&wgpu::BindGroupLayout>],
    ) -> u64 {
        let key = Self::source_key(&program.source);
        let label = self.label;
        let entry_point = self.entry_point;
        self.pipelines.entry(key).or_insert_with(|| {
            create_compute_pipeline_with_layouts(
                device,
                label,
                entry_point,
                program,
                bind_group_layouts,
            )
        });
        key
    }
}

/// Shader module + compute pipeline against an existing bind group layout,
/// separated from resource upload so a headless test can validate a shader and
/// its layout without building a world.
pub fn create_compute_pipeline(
    device: &wgpu::Device,
    label: &str,
    entry_point: &str,
    program: &ShaderProgram,
    bind_group_layout: &wgpu::BindGroupLayout,
) -> wgpu::ComputePipeline {
    create_compute_pipeline_with_layouts(
        device,
        label,
        entry_point,
        program,
        &[Some(bind_group_layout)],
    )
}

pub(crate) fn create_compute_pipeline_with_layouts(
    device: &wgpu::Device,
    label: &str,
    entry_point: &str,
    program: &ShaderProgram,
    bind_group_layouts: &[Option<&wgpu::BindGroupLayout>],
) -> wgpu::ComputePipeline {
    create_compute_pipeline_from_module(
        device,
        label,
        entry_point,
        &program.module,
        bind_group_layouts,
    )
}

/// The same, from an already-composed module.
pub(crate) fn create_compute_pipeline_from_module(
    device: &wgpu::Device,
    label: &str,
    entry_point: &str,
    module: &naga::Module,
    bind_group_layouts: &[Option<&wgpu::BindGroupLayout>],
) -> wgpu::ComputePipeline {
    // The composed naga IR goes straight to the device. Handing over WGSL text would mean naga's
    // backend regenerating source that wgpu then re-parses — two extra conversions, each able to
    // change the module in ways no test here would see.
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module.clone())),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts,
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        module: &shader_module,
        entry_point: Some(entry_point),
        compilation_options: Default::default(),
        cache: None,
    })
}
