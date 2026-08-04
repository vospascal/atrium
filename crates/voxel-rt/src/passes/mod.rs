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

pub mod blit;
pub mod cagi;
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
        shader_source: &str,
        bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        Self::new_with_layouts(
            device,
            label,
            entry_point,
            shader_source,
            &[Some(bind_group_layout)],
        )
    }

    pub fn new_with_layouts(
        device: &wgpu::Device,
        label: &'static str,
        entry_point: &'static str,
        shader_source: &str,
        bind_group_layouts: &[Option<&wgpu::BindGroupLayout>],
    ) -> Self {
        let mut cache = Self {
            label,
            entry_point,
            pipelines: HashMap::new(),
            active_key: 0,
        };
        cache.active_key = cache.compile(device, shader_source, bind_group_layouts);
        cache
    }

    /// Hash of a shader source — pipelines are keyed by it rather than by the
    /// ~70 KB string itself.
    pub fn source_key(shader_source: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        shader_source.hash(&mut hasher);
        hasher.finish()
    }

    /// Dispatch `shader_source` from now on, compiling it only on a cache miss.
    pub fn set_shader_source(
        &mut self,
        device: &wgpu::Device,
        shader_source: &str,
        bind_group_layout: &wgpu::BindGroupLayout,
    ) {
        self.active_key = self.compile(device, shader_source, &[Some(bind_group_layout)]);
    }

    pub fn set_shader_source_with_layouts(
        &mut self,
        device: &wgpu::Device,
        shader_source: &str,
        bind_group_layouts: &[Option<&wgpu::BindGroupLayout>],
    ) {
        self.active_key = self.compile(device, shader_source, bind_group_layouts);
    }

    /// Precompile `shader_sources` (duplicates cost nothing). Returns how many
    /// distinct pipelines the cache holds afterwards.
    pub fn prewarm(
        &mut self,
        device: &wgpu::Device,
        shader_sources: &[String],
        bind_group_layout: &wgpu::BindGroupLayout,
    ) -> usize {
        for shader_source in shader_sources {
            self.compile(device, shader_source, &[Some(bind_group_layout)]);
        }
        self.pipelines.len()
    }

    pub fn prewarm_with_layouts(
        &mut self,
        device: &wgpu::Device,
        shader_sources: &[String],
        bind_group_layouts: &[Option<&wgpu::BindGroupLayout>],
    ) -> usize {
        for shader_source in shader_sources {
            self.compile(device, shader_source, bind_group_layouts);
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
        shader_source: &str,
        bind_group_layouts: &[Option<&wgpu::BindGroupLayout>],
    ) -> u64 {
        let key = Self::source_key(shader_source);
        let label = self.label;
        let entry_point = self.entry_point;
        self.pipelines.entry(key).or_insert_with(|| {
            create_compute_pipeline_with_layouts(
                device,
                label,
                entry_point,
                shader_source,
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
    shader_source: &str,
    bind_group_layout: &wgpu::BindGroupLayout,
) -> wgpu::ComputePipeline {
    create_compute_pipeline_with_layouts(
        device,
        label,
        entry_point,
        shader_source,
        &[Some(bind_group_layout)],
    )
}

pub fn create_compute_pipeline_with_layouts(
    device: &wgpu::Device,
    label: &str,
    entry_point: &str,
    shader_source: &str,
    bind_group_layouts: &[Option<&wgpu::BindGroupLayout>],
) -> wgpu::ComputePipeline {
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
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
