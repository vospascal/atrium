//! Group-0 binding allocation: one table, indices derived, WGSL consts generated.
//!
//! # What this replaces
//!
//! Every index used to be written by hand on both sides — `@group(0) @binding(11)` in the
//! shader, `binding: 11` in the layout — and which file owned which number lived in prose.
//! The old comment on the event field said it plainly: *"16 is the first free index:
//! `world.wgsl` owns 1-5, 7-10 and this, `cagi_volume.wgsl` owns 11, 13 and 14, `cagi.wgsl`
//! owns 12, and the shading pass owns 0 (camera) and 6 (the output texture)."* Adding one
//! buffer meant reading that sentence, trusting it, and editing two places per shader that
//! declares the slot. A crate that wanted its own resource had to know what every other
//! crate had taken.
//!
//! Now [`WorldBinding`] is the only place a number exists. The index is the variant's
//! position, so a collision is not something you can write, and [`WorldBinding::wgsl_prelude`]
//! emits the consts the shaders use by name.
//!
//! # Why the WGSL side can use a name at all
//!
//! `@binding` and `@group` parameters "must be a const-expression that resolves to an i32 or
//! u32" — <https://gpuweb.github.io/gpuweb/wgsl/#binding-attr> and
//! <https://gpuweb.github.io/gpuweb/wgsl/#group-attr>. So a named `const` is not a naga
//! tolerance, it is specified. Confirmed against naga 29 by probe: parses and validates,
//! arithmetic included.
//!
//! # Two constraints this does NOT decide
//!
//! **Which layout includes which binding is still per-pass, deliberately.** The device allows
//! 11 storage buffers per stage (the WebGPU default is 8, so the engine already asks for more
//! than the spec guarantees), and the CA pass's layout is the shared set plus its own three.
//! Adding a twelfth storage buffer to the *shared* layout makes `create_bind_group_layout`
//! fail outright for the CA pass. Allocating an index is free; adding it to a layout is not.
//!
//! **The order of these variants is the GPU wire format.** Reordering for tidiness would
//! silently renumber every binding. `generated_indices_match_the_shipped_layout` pins all 18
//! so that is a test failure rather than a black frame. Add new bindings at the END.

/// The bind group the world resources occupy in every compute pass.
///
/// `voxel-environment` owns group 1 (`voxel_environment::ENVIRONMENT_BIND_GROUP`). A new
/// group belongs to whoever adds it, and its constant belongs next to its resources.
pub const WORLD_BIND_GROUP: u32 = 0;

/// Every group-0 slot, in wire order.
///
/// `blit.wgsl` is not here: it is a render pass with its own group 0 and its own two-entry
/// layout, so its numbers are private to it and sharing this table would couple them for no
/// reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum WorldBinding {
    /// Shading pass only — it is the only pass with a camera.
    Camera,
    BrickmapMeta,
    BrickIndices,
    OccupancyWords,
    MaterialWords,
    Materials,
    /// Shading pass only — the storage texture it writes.
    Output,
    Lighting,
    ColumnMaxBrickY,
    BrickOccupancyBits,
    BrickSkipDistances,
    /// The ping-pong volume the CA reads and the shading pass samples.
    LightVolumeFront,
    /// Writable, CA pass only. Its absence is what makes the shading pass's layout read-only.
    LightVolumeBack,
    CagiCellData,
    CagiVolumeMeta,
    BrickBounds,
    WorldEvents,
    /// Shading pass only, and not in the shared layout — see the storage-buffer limit above.
    PatternCache,
}

impl WorldBinding {
    /// Every slot, in wire order. The array length is the allocation high-water mark.
    pub const ALL: [Self; 18] = [
        Self::Camera,
        Self::BrickmapMeta,
        Self::BrickIndices,
        Self::OccupancyWords,
        Self::MaterialWords,
        Self::Materials,
        Self::Output,
        Self::Lighting,
        Self::ColumnMaxBrickY,
        Self::BrickOccupancyBits,
        Self::BrickSkipDistances,
        Self::LightVolumeFront,
        Self::LightVolumeBack,
        Self::CagiCellData,
        Self::CagiVolumeMeta,
        Self::BrickBounds,
        Self::WorldEvents,
        Self::PatternCache,
    ];

    /// The binding number, allocated by position. Nothing else assigns one.
    pub const fn index(self) -> u32 {
        self as u32
    }

    /// The WGSL constant the shaders bind through.
    pub const fn wgsl_const(self) -> &'static str {
        match self {
            Self::Camera => "B_CAMERA",
            Self::BrickmapMeta => "B_BRICKMAP_META",
            Self::BrickIndices => "B_BRICK_INDICES",
            Self::OccupancyWords => "B_OCCUPANCY_WORDS",
            Self::MaterialWords => "B_MATERIAL_WORDS",
            Self::Materials => "B_MATERIALS",
            Self::Output => "B_OUTPUT",
            Self::Lighting => "B_LIGHTING",
            Self::ColumnMaxBrickY => "B_COLUMN_MAX_BRICK_Y",
            Self::BrickOccupancyBits => "B_BRICK_OCCUPANCY_BITS",
            Self::BrickSkipDistances => "B_BRICK_SKIP_DISTANCES",
            Self::LightVolumeFront => "B_LIGHT_VOLUME_FRONT",
            Self::LightVolumeBack => "B_LIGHT_VOLUME_BACK",
            Self::CagiCellData => "B_CAGI_CELL_DATA",
            Self::CagiVolumeMeta => "B_CAGI_VOLUME_META",
            Self::BrickBounds => "B_BRICK_BOUNDS",
            Self::WorldEvents => "B_WORLD_EVENTS",
            Self::PatternCache => "B_PATTERN_CACHE",
        }
    }

    /// The shader file that declares this slot's variable. Documentation the compiler cannot
    /// check, but `wgsl_declares_every_binding_through_its_const` checks it against the files.
    pub const fn declared_in(self) -> &'static str {
        match self {
            Self::Camera | Self::Output => "dda.wgsl",
            Self::LightVolumeFront | Self::CagiCellData | Self::CagiVolumeMeta => {
                "cagi_volume.wgsl"
            }
            Self::LightVolumeBack => "cagi.wgsl",
            Self::PatternCache => "pattern.wgsl",
            _ => "world.wgsl",
        }
    }

    /// The WGSL block declaring the group index and every binding index.
    ///
    /// Prepended to each compute shader module, so a shader never spells a number. Generated
    /// rather than checked-in for the usual reason: a second copy is a second thing to drift.
    pub fn wgsl_prelude() -> String {
        let mut source = String::with_capacity(1024);
        source.push_str(
            "// ---- GENERATED by passes::binding — do not edit, do not check in a copy ----\n",
        );
        source.push_str(&format!("const G_WORLD: u32 = {WORLD_BIND_GROUP}u;\n"));
        for binding in Self::ALL {
            source.push_str(&format!(
                "const {}: u32 = {}u;\n",
                binding.wgsl_const(),
                binding.index()
            ));
        }
        source.push('\n');
        source
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The variant order IS the GPU layout, so this pins all 18. A reorder that looked like
    /// tidying would otherwise renumber every resource at once — every pass would still
    /// compile and the frame would be garbage.
    #[test]
    fn generated_indices_match_the_shipped_layout() {
        let expected = [
            (WorldBinding::Camera, 0),
            (WorldBinding::BrickmapMeta, 1),
            (WorldBinding::BrickIndices, 2),
            (WorldBinding::OccupancyWords, 3),
            (WorldBinding::MaterialWords, 4),
            (WorldBinding::Materials, 5),
            (WorldBinding::Output, 6),
            (WorldBinding::Lighting, 7),
            (WorldBinding::ColumnMaxBrickY, 8),
            (WorldBinding::BrickOccupancyBits, 9),
            (WorldBinding::BrickSkipDistances, 10),
            (WorldBinding::LightVolumeFront, 11),
            (WorldBinding::LightVolumeBack, 12),
            (WorldBinding::CagiCellData, 13),
            (WorldBinding::CagiVolumeMeta, 14),
            (WorldBinding::BrickBounds, 15),
            (WorldBinding::WorldEvents, 16),
            (WorldBinding::PatternCache, 17),
        ];
        for (binding, index) in expected {
            assert_eq!(binding.index(), index, "{binding:?} moved");
        }
        assert_eq!(WorldBinding::ALL.len(), expected.len());
    }

    #[test]
    fn indices_are_unique_and_dense() {
        let indices: Vec<u32> = WorldBinding::ALL.iter().map(|b| b.index()).collect();
        let unique: std::collections::BTreeSet<u32> = indices.iter().copied().collect();
        assert_eq!(unique.len(), indices.len(), "two bindings share an index");
        assert_eq!(
            *unique.last().expect("non-empty"),
            indices.len() as u32 - 1,
            "indices must be dense — a gap means a slot nothing can claim"
        );
    }

    #[test]
    fn constant_names_are_unique() {
        let names: std::collections::BTreeSet<&str> =
            WorldBinding::ALL.iter().map(|b| b.wgsl_const()).collect();
        assert_eq!(names.len(), WorldBinding::ALL.len());
    }

    /// The point of the whole module: no shader may spell a group-0 binding number. A literal
    /// is how the two sides drifted before, and it is invisible until the frame is wrong.
    #[test]
    fn wgsl_declares_every_binding_through_its_const() {
        let shader_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders");
        for binding in WorldBinding::ALL {
            let source = std::fs::read_to_string(shader_dir.join(binding.declared_in()))
                .expect("shader file");
            let expected = format!("@binding({})", binding.wgsl_const());
            assert!(
                source.contains(&expected),
                "{} should declare {binding:?} as {expected}",
                binding.declared_in()
            );
        }

        // …and nowhere in the compute shaders may a group-0 binding be a literal. `blit.wgsl`
        // is excluded: separate pass, separate layout, its own two numbers.
        for name in [
            "world.wgsl",
            "dda.wgsl",
            "cagi.wgsl",
            "cagi_volume.wgsl",
            "pattern.wgsl",
            "water.wgsl",
        ] {
            let source = std::fs::read_to_string(shader_dir.join(name)).expect("shader file");
            for line in source.lines() {
                assert!(
                    !line.contains("@group(0)"),
                    "{name} spells group 0 as a literal: {line}"
                );
                if line.contains("@binding(") {
                    let argument = line
                        .split("@binding(")
                        .nth(1)
                        .and_then(|rest| rest.split(')').next())
                        .unwrap_or_default();
                    assert!(
                        !argument.chars().next().is_some_and(|c| c.is_ascii_digit()),
                        "{name} spells a binding index as a literal: {line}"
                    );
                }
            }
        }
    }

    /// The prelude must define exactly what the shaders reference — no more, no less.
    #[test]
    fn prelude_defines_every_constant_once() {
        let prelude = WorldBinding::wgsl_prelude();
        assert!(prelude.contains("const G_WORLD: u32 = 0u;"));
        for binding in WorldBinding::ALL {
            let declaration = format!(
                "const {}: u32 = {}u;\n",
                binding.wgsl_const(),
                binding.index()
            );
            assert_eq!(
                prelude.matches(&declaration).count(),
                1,
                "{binding:?} must be declared exactly once"
            );
        }
    }
}
