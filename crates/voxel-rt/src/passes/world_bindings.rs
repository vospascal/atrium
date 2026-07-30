//! The world's GPU buffers and their bind-group entries — the resources EVERY
//! pass that traverses the voxel world needs (`shaders/world.wgsl` declares
//! exactly these bindings).
//!
//! Introduced in E4: the CAGI pass traces one real sun shadow ray per candidate
//! cell through the same brickmap the shading pass traverses, so the brickmap
//! buffers stopped being the DDA pass's private property. Uploading a second copy
//! would have cost ~30 MB for nothing, and letting the CAGI pass reach into the
//! DDA pass would have coupled two passes that must stay independently
//! excludable (plan isolation rule). One owner, two consumers instead.
//!
//! Layout (group 0), matching `shaders/world.wgsl`:
//!
//! | binding | resource |
//! |---------|----------|
//! | 1  | uniform  brickmap metadata |
//! | 2  | storage  brick pointer grid |
//! | 3  | storage  per-brick occupancy words |
//! | 4  | storage  per-brick material bytes |
//! | 5  | storage  material table |
//! | 7  | uniform  lighting (sun, ambient, the runtime quality + GI knobs) |
//! | 8  | storage  per-XZ-column max occupied brick Y |
//! | 9  | storage  1-bit-per-brick occupancy grid |
//! | 10 | storage  chebyshev skip-distance bytes |
//!
//! Bindings 0 and 6 are deliberately absent: they belong to the shading pass
//! (camera, output texture) and stay free for a consumer that has no camera.
//!
//! E2 made these buffers WRITABLE from the CPU side (`COPY_DST`): a voxel edit
//! patches the words it touched ([`WorldBindings::apply_array_write`]) instead of
//! re-uploading ~41 MB. The buffers are created from the brickmap's arrays, which
//! carry `brickmap::EDIT_BRICK_HEADROOM` spare brick slots, so materializing a
//! brick is a patch too — only outgrowing that headroom needs new buffers, and
//! that path goes through a plain [`WorldBindings::new`] plus a rebind.

use wgpu::util::DeviceExt;

use crate::brickmap::{Brickmap, BrickmapArray, BrickmapMetadata};
use crate::lighting::LightingUniform;
use crate::world_edit::ArrayWrite;

pub struct WorldBindings {
    metadata_uniform_buffer: wgpu::Buffer,
    brick_indices_buffer: wgpu::Buffer,
    occupancy_words_buffer: wgpu::Buffer,
    material_words_buffer: wgpu::Buffer,
    material_table_buffer: wgpu::Buffer,
    lighting_uniform_buffer: wgpu::Buffer,
    column_max_buffer: wgpu::Buffer,
    brick_occupancy_bits_buffer: wgpu::Buffer,
    skip_distance_buffer: wgpu::Buffer,
}

impl WorldBindings {
    pub fn new(device: &wgpu::Device, brickmap: &Brickmap) -> Self {
        let metadata_uniform_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("world brickmap metadata uniform"),
                contents: bytemuck::bytes_of(&brickmap.metadata()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let lighting_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("world lighting uniform"),
            size: std::mem::size_of::<LightingUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let storage_buffer = |label: &str, contents: &[u32]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(contents),
                // COPY_DST since E2: an edit patches word ranges in place.
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            })
        };

        Self {
            metadata_uniform_buffer,
            brick_indices_buffer: storage_buffer("world brick indices", &brickmap.brick_indices),
            occupancy_words_buffer: storage_buffer(
                "world occupancy words",
                &brickmap.occupancy_words,
            ),
            material_words_buffer: storage_buffer("world material words", &brickmap.material_words),
            // 1152 bytes for all 24 rows — never patched, so no COPY_DST.
            material_table_buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("world material table"),
                contents: bytemuck::cast_slice(&crate::material::gpu_materials()),
                usage: wgpu::BufferUsages::STORAGE,
            }),
            lighting_uniform_buffer,
            column_max_buffer: storage_buffer(
                "world column max brick y",
                &brickmap.column_max_brick_y,
            ),
            brick_occupancy_bits_buffer: storage_buffer(
                "world brick occupancy bits",
                &brickmap.brick_occupancy_bit_words,
            ),
            skip_distance_buffer: storage_buffer(
                "world brick skip distances",
                &brickmap.brick_skip_distance_words,
            ),
        }
    }

    /// The shared bind-group-layout entries, to be concatenated with a pass's own.
    pub fn layout_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
        let uniform_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        vec![
            uniform_entry(1),  // brickmap metadata
            storage_entry(2),  // brick indices
            storage_entry(3),  // occupancy words
            storage_entry(4),  // material words
            storage_entry(5),  // material table
            uniform_entry(7),  // lighting
            storage_entry(8),  // per-XZ-column max occupied brick Y
            storage_entry(9),  // 1-bit-per-brick occupancy grid
            storage_entry(10), // chebyshev skip-distance bytes
        ]
    }

    /// The shared bind-group entries, to be concatenated with a pass's own.
    pub fn bind_group_entries(&self) -> Vec<wgpu::BindGroupEntry<'_>> {
        fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            }
        }
        vec![
            entry(1, &self.metadata_uniform_buffer),
            entry(2, &self.brick_indices_buffer),
            entry(3, &self.occupancy_words_buffer),
            entry(4, &self.material_words_buffer),
            entry(5, &self.material_table_buffer),
            entry(7, &self.lighting_uniform_buffer),
            entry(8, &self.column_max_buffer),
            entry(9, &self.brick_occupancy_bits_buffer),
            entry(10, &self.skip_distance_buffer),
        ]
    }

    /// Patch one word range of one brickmap buffer — E2's delta upload. The
    /// payload is owned by the caller (it crossed a thread boundary to get here),
    /// so this never touches the brickmap.
    pub fn apply_array_write(&self, queue: &wgpu::Queue, write: &ArrayWrite) {
        queue.write_buffer(
            self.buffer_of(write.array),
            (write.first_word * 4) as u64,
            bytemuck::cast_slice(&write.words),
        );
    }

    /// Re-upload the brickmap metadata uniform (an edit moved the brick count or
    /// the global max brick Y — the traversal's sky-out height).
    pub fn write_metadata(&self, queue: &wgpu::Queue, metadata: &BrickmapMetadata) {
        queue.write_buffer(
            &self.metadata_uniform_buffer,
            0,
            bytemuck::bytes_of(metadata),
        );
    }

    fn buffer_of(&self, array: BrickmapArray) -> &wgpu::Buffer {
        match array {
            BrickmapArray::BrickIndices => &self.brick_indices_buffer,
            BrickmapArray::OccupancyWords => &self.occupancy_words_buffer,
            BrickmapArray::MaterialWords => &self.material_words_buffer,
            BrickmapArray::ColumnMaxBrickY => &self.column_max_buffer,
            BrickmapArray::BrickOccupancyBits => &self.brick_occupancy_bits_buffer,
            BrickmapArray::BrickSkipDistances => &self.skip_distance_buffer,
        }
    }

    /// Total GPU bytes of the world's buffers — the "GPU memory" column of the E2
    /// verdict.
    pub fn gpu_bytes(&self) -> u64 {
        self.metadata_uniform_buffer.size()
            + self.brick_indices_buffer.size()
            + self.occupancy_words_buffer.size()
            + self.material_words_buffer.size()
            + self.material_table_buffer.size()
            + self.lighting_uniform_buffer.size()
            + self.column_max_buffer.size()
            + self.brick_occupancy_bits_buffer.size()
            + self.skip_distance_buffer.size()
    }

    /// Upload this frame's lighting uniform. Written ONCE per frame by the frame
    /// composer, not per pass: the CAGI pass injects the same sun the shading pass
    /// shades with, and two writes of the same buffer inside one frame would be
    /// two chances to disagree.
    pub fn write_lighting(&self, queue: &wgpu::Queue, lighting_uniform: &LightingUniform) {
        queue.write_buffer(
            &self.lighting_uniform_buffer,
            0,
            bytemuck::bytes_of(lighting_uniform),
        );
    }
}
