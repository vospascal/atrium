//! MagicaVoxel `.vox` loading, renderer-independent.
//!
//! Lives here rather than in a renderer because a `.vox` file is **world data**:
//! a grid of cells and a palette. It was previously parsed inside
//! `voxel-sandbox`'s Bevy prop importer, which meant the axis swap, the palette
//! decode and the index-map handling were only reachable by a renderer that
//! depends on Bevy — so the second consumer (voxel-rt's material import) would
//! have had to spell all three out again. One parser, two consumers.
//!
//! ## Why `.vox` at all
//!
//! It is the de-facto interchange format for voxel art, so reading it makes every
//! external editor — MagicaVoxel and its procedural generators, Goxel, Avoyd,
//! VoxEdit, Blender exporters — an authoring tool for this engine at no cost. We
//! do not build a voxel editor.
//!
//! That framing sets the requirements, and they are about TOLERANCE rather than
//! completeness. Most writers are not MagicaVoxel and emit only `RGBA` + `XYZI`:
//!
//! * **A file with no material chunks at all is the NORMAL case**, not a degraded
//!   one. Verified on this repo's own `assets/vox/campfire.vox`, which carries
//!   zero `MATL` chunks, no scene graph and no layers. Every palette entry
//!   therefore reports [`VoxMaterialKind::Diffuse`] with every optional property
//!   `None`, and a consumer fills in its own defaults.
//! * **Missing properties within a chunk are independent.** A `MATL` with only
//!   `_rough` is not a failed import; it is a roughness.
//!
//! ## Three traps this module exists to get right once
//!
//! 1. **Axes.** MagicaVoxel is Z-up; the engine is Y-up. Swapped on load, so no
//!    consumer has to remember.
//! 2. **The palette index is off by one between voxels and materials.**
//!    `dot_vox` hands back `Voxel::i` already decremented to an in-memory 0..254,
//!    while a `MATL` chunk's id is the *file* index, which runs 1..255. Pairing
//!    them naively shifts every material by one entry — the kind of bug that
//!    silently gives grass stone's roughness. [`VoxFile::load`] resolves it
//!    through the index map (see below) so a consumer never sees the two spaces.
//! 3. **`_ior` is the refractive index MINUS ONE.** MagicaVoxel stores 0.0 for
//!    "does not refract", so water's 1.333 is `_ior = 0.333`. `dot_vox`'s own
//!    `Material::ri()` documents this ("appears to just be 1 + _ior") and then
//!    returns `_ior` unchanged, so the crate cannot be relied on for it. We add
//!    the 1. The pleasant consequence is that an absent or zero `_ior` maps to
//!    exactly 1.0, which is already this engine's "does not refract" value.
//!
//! ## The index map (`IMAP`)
//!
//! MagicaVoxel rewrites palette indices when entries are dragged around, and
//! records the mapping in an `IMAP` chunk. `dot_vox` parses it into
//! `DotVoxData::index_map` but does **not** apply it, and the reference
//! implementation this design was compared against (VoxelChain's `vox.ts`) reads
//! the chunk and then throws outright. Applying it matters because a consumer
//! keyed on palette index — voxel-rt's material tuning layer — would otherwise
//! silently move every tuned row when the file is re-saved after a reorder.
//!
//! The map is indexed by in-memory index and yields the 1-based file index; the
//! crate's identity default is `index_map[i] = i + 1`, which is what fixes trap 2
//! as a side effect. [`VoxPaletteEntry::file_index`] carries it through so a
//! consumer can key on a value that survives a reorder.
//!
//! **Honest limit:** the identity path is verified against real files; the
//! reordered path is implemented from the format's semantics and has no
//! reordered fixture behind it yet.
//!
//! ## Deliberately not read
//!
//! The scene graph (`nTRN`/`nGRP`/`nSHP`) and layers, which carry per-model
//! *placement*. Materials and single-model props do not need them — the same call
//! VoxelChain's parser makes. A future prop/scene importer is what would want
//! them, so this is a deferral rather than an oversight.

use std::path::Path;

/// Palette entries in a `.vox` file. Fixed by the format.
pub const PALETTE_LENGTH: usize = 256;

/// Which of MagicaVoxel's material classes a palette entry is, from `_type`.
///
/// A closed set with an explicit unknown, rather than a string, so a consumer
/// maps a case it understands and cannot typo one. Deliberately mirrors the
/// format's own vocabulary instead of this engine's material kinds: translating
/// is the consumer's job, and doing it here would bake one renderer's model into
/// a loader shared by two.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VoxMaterialKind {
    /// `_diffuse`, and the default for an entry with no material chunk — which is
    /// the common case, not a fallback.
    #[default]
    Diffuse,
    /// `_metal`.
    Metal,
    /// `_emit`: a light source.
    Emit,
    /// `_glass`: transparent, refracting.
    Glass,
    /// `_media`/`_cloud`: a participating volume rather than a surface.
    Media,
    /// A `_type` this crate does not know. Carried rather than dropped so a
    /// consumer can log it instead of silently treating it as diffuse.
    Unknown,
}

impl VoxMaterialKind {
    fn from_type(value: &str) -> VoxMaterialKind {
        match value {
            "_diffuse" => VoxMaterialKind::Diffuse,
            "_metal" => VoxMaterialKind::Metal,
            "_emit" => VoxMaterialKind::Emit,
            "_glass" => VoxMaterialKind::Glass,
            "_media" | "_cloud" => VoxMaterialKind::Media,
            _ => VoxMaterialKind::Unknown,
        }
    }
}

/// One palette entry: its colour, and whatever the file said about its material.
///
/// Every optional field is `Option` rather than a defaulted number, because
/// "absent" and "authored as zero" are different facts and only the consumer knows
/// what to do with the first. A file with no `MATL` chunks yields 256 of these
/// with `kind: Diffuse` and every option `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VoxPaletteEntry {
    /// sRGB-encoded colour plus alpha, exactly as stored.
    pub rgba: [u8; 4],
    /// The **1-based file index** of this entry, from the index map.
    ///
    /// Not the same as this entry's position in [`VoxFile::palette`] once the
    /// palette has been reordered in an editor. A consumer that persists anything
    /// per-material — voxel-rt's tuning layer — should key on this, because it is
    /// the value that survives a reorder-and-resave.
    pub file_index: u8,
    pub kind: VoxMaterialKind,
    /// `_rough`.
    pub roughness: Option<f32>,
    /// `_sp`, specular reflectance.
    pub specular: Option<f32>,
    /// `_metal`, metalness.
    pub metalness: Option<f32>,
    /// `_emit`, emission strength.
    pub emission: Option<f32>,
    /// `_flux`, radiant flux — a coarse multiplier on top of `_emit`.
    pub radiant_flux: Option<f32>,
    /// Refractive index, **already converted from `_ior` by adding 1** (see the
    /// module docs). `None` when the file said nothing; 1.0 means "does not
    /// refract".
    pub index_of_refraction: Option<f32>,
    /// `_trans`, physical transparency.
    ///
    /// Note this is NOT `_alpha`, which is a compositing value with no physical
    /// meaning and is deliberately not read.
    pub transparency: Option<f32>,
    /// `_att`, attenuation — the medium's optical density. A single scalar where a
    /// per-channel triple is what a physically-derived medium colour needs, so a
    /// consumer can only ever seed from it.
    pub attenuation: Option<f32>,
    /// `_d`, volumetric density.
    pub density: Option<f32>,
}

/// One model from a `.vox` file, in **engine axes** (Y up) with the origin at the
/// model's own corner.
#[derive(Clone, Debug, PartialEq)]
pub struct VoxModel {
    pub size_x: i32,
    pub size_y: i32,
    pub size_z: i32,
    /// Dense grid of palette indices, `None` where the cell is empty. Indexed by
    /// [`VoxModel::index`]. Holds the **in-memory** index, i.e. a direct subscript
    /// into [`VoxFile::palette`].
    pub cells: Vec<Option<u8>>,
}

impl VoxModel {
    /// Cell index for a coordinate, matching `cells`' layout
    /// (`(y * size_z + z) * size_x + x`; x varies fastest).
    pub fn index(&self, x: i32, y: i32, z: i32) -> Option<usize> {
        if x < 0 || y < 0 || z < 0 || x >= self.size_x || y >= self.size_y || z >= self.size_z {
            return None;
        }
        Some(((y * self.size_z + z) * self.size_x + x) as usize)
    }

    /// The palette index at a coordinate, or `None` for an empty or out-of-bounds
    /// cell.
    pub fn palette_index_at(&self, x: i32, y: i32, z: i32) -> Option<u8> {
        self.cells[self.index(x, y, z)?]
    }

    /// How many cells are occupied.
    pub fn occupied_count(&self) -> usize {
        self.cells.iter().filter(|cell| cell.is_some()).count()
    }
}

/// A whole `.vox` file: its models and its resolved palette.
#[derive(Clone, Debug, PartialEq)]
pub struct VoxFile {
    /// Every model in the file. Packs commonly hold several, and none is more
    /// "the" model than another, so all are kept.
    pub models: Vec<VoxModel>,
    /// The 256-entry palette, indexed by the values in [`VoxModel::cells`], with
    /// each entry's material properties already paired in.
    pub palette: Vec<VoxPaletteEntry>,
}

impl VoxFile {
    /// Load and resolve a `.vox` file.
    ///
    /// Does the three things the module docs promise — swaps axes, resolves the
    /// palette-index spaces through the index map, and converts `_ior` — so no
    /// consumer repeats them.
    pub fn load(path: &Path) -> Result<VoxFile, String> {
        let data = dot_vox::load(path.to_str().unwrap_or_default())
            .map_err(|error| format!("cannot load {}: {error}", path.display()))?;
        if data.models.is_empty() {
            return Err(format!("{} contains no models", path.display()));
        }
        Ok(VoxFile {
            palette: resolve_palette(&data),
            models: data.models.iter().map(convert_model).collect(),
        })
    }

    /// How many palette entries the file actually described a material for — the
    /// number a caller wants when reporting "this file brought no materials".
    pub fn described_material_count(&self) -> usize {
        self.palette
            .iter()
            .filter(|entry| entry.describes_a_material())
            .count()
    }
}

impl VoxPaletteEntry {
    /// Whether the file said anything about this entry's material beyond its
    /// colour.
    ///
    /// False for every entry of a file with no `MATL` chunks, which is the common
    /// case: a consumer uses this to decide between "the artist authored this" and
    /// "fill in my own defaults", rather than guessing from a zeroed field.
    pub fn describes_a_material(&self) -> bool {
        self.kind != VoxMaterialKind::Diffuse
            || self.roughness.is_some()
            || self.specular.is_some()
            || self.metalness.is_some()
            || self.emission.is_some()
            || self.radiant_flux.is_some()
            || self.index_of_refraction.is_some()
            || self.transparency.is_some()
            || self.attenuation.is_some()
            || self.density.is_some()
    }

    /// Colour as linear-space floats, for a consumer that shades in linear.
    ///
    /// Applies the real sRGB transfer function rather than a 2.2 power
    /// approximation, because the two differ most in the darks — which is exactly
    /// where a voxel palette's shadow tones live.
    pub fn linear_rgb(&self) -> [f32; 3] {
        [0, 1, 2].map(|channel| srgb_to_linear(self.rgba[channel] as f32 / 255.0))
    }

    /// Colour as sRGB-encoded floats, i.e. the bytes scaled to 0..1 with no
    /// transfer applied — what a consumer storing "sRGB as authored" wants.
    pub fn srgb_rgb(&self) -> [f32; 3] {
        [0, 1, 2].map(|channel| self.rgba[channel] as f32 / 255.0)
    }
}

/// The sRGB electro-optical transfer function.
fn srgb_to_linear(channel: f32) -> f32 {
    if channel <= 0.040_45 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

/// MagicaVoxel (x/y ground plane, z up) -> engine (x/z ground plane, y up).
fn convert_model(model: &dot_vox::Model) -> VoxModel {
    let size_x = model.size.x as i32;
    let size_y = model.size.z as i32;
    let size_z = model.size.y as i32;
    let mut converted = VoxModel {
        size_x,
        size_y,
        size_z,
        cells: vec![None; (size_x * size_y * size_z) as usize],
    };
    for voxel in &model.voxels {
        // The swap: the file's z is the engine's y.
        if let Some(index) = converted.index(voxel.x as i32, voxel.z as i32, voxel.y as i32) {
            converted.cells[index] = Some(voxel.i);
        }
    }
    converted
}

/// Pair every palette colour with its material chunk, resolving the two index
/// spaces (trap 2 in the module docs).
fn resolve_palette(data: &dot_vox::DotVoxData) -> Vec<VoxPaletteEntry> {
    (0..PALETTE_LENGTH)
        .map(|memory_index| {
            let colour = data.palette.get(memory_index);
            // The index map is indexed by in-memory index and yields the 1-based
            // file index; its identity default is `memory_index + 1`, which is the
            // same relation a MATL id has to a voxel's index. Falling back to that
            // relation keeps a file with a short or absent map working.
            let file_index = data
                .index_map
                .get(memory_index)
                .copied()
                .unwrap_or_else(|| (memory_index as u8).wrapping_add(1));
            let mut entry = VoxPaletteEntry {
                rgba: colour.map_or([0, 0, 0, 0], |colour| {
                    [colour.r, colour.g, colour.b, colour.a]
                }),
                file_index,
                ..VoxPaletteEntry::default()
            };
            if let Some(material) = data
                .materials
                .iter()
                .find(|material| material.id == file_index as u32)
            {
                apply_material(&mut entry, material);
            }
            entry
        })
        .collect()
}

/// Copy one `MATL` chunk's properties onto a palette entry. Every property is
/// read independently: a chunk carrying only `_rough` is a roughness, not a
/// failure.
fn apply_material(entry: &mut VoxPaletteEntry, material: &dot_vox::Material) {
    if let Some(kind) = material.material_type() {
        entry.kind = VoxMaterialKind::from_type(kind);
    }
    entry.roughness = material.roughness();
    entry.specular = material.specular();
    entry.metalness = material.metalness();
    entry.emission = material.emission();
    entry.radiant_flux = material.radiant_flux();
    // Trap 3: `_ior` is the refractive index minus one, so 0 means "does not
    // refract" and maps to exactly 1.0. `dot_vox::Material::ri()` documents this
    // conversion and then does not perform it, so it is done here.
    entry.index_of_refraction = material.refractive_index().map(|ior| 1.0 + ior);
    // `_trans` (physical) rather than `_alpha` (compositing) — deliberately.
    entry.transparency = material.transparency();
    entry.attenuation = material.attenuation();
    entry.density = material.density();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(pairs: &[(&str, &str)]) -> dot_vox::Dict {
        let mut properties = dot_vox::Dict::new();
        for (key, value) in pairs {
            properties.insert(key.to_string(), value.to_string());
        }
        properties
    }

    /// Build a `.vox` in memory and read it back, so the fixtures below exercise
    /// the real parser rather than a hand-built `DotVoxData`.
    fn round_trip(
        models: Vec<dot_vox::Model>,
        palette_head: &[[u8; 4]],
        materials: Vec<dot_vox::Material>,
    ) -> VoxFile {
        let mut palette: Vec<dot_vox::Color> = (0..PALETTE_LENGTH)
            .map(|_| dot_vox::Color {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            })
            .collect();
        for (slot, rgba) in palette_head.iter().enumerate() {
            palette[slot] = dot_vox::Color {
                r: rgba[0],
                g: rgba[1],
                b: rgba[2],
                a: rgba[3],
            };
        }
        let data = dot_vox::DotVoxData {
            version: 150,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models,
            palette,
            materials,
            scenes: vec![],
            layers: vec![],
        };
        let mut bytes = Vec::new();
        data.write_vox(&mut bytes).expect("write");
        let path = std::env::temp_dir().join(format!(
            "voxel_core_vox_test_{}.vox",
            // Distinct per call so parallel tests cannot collide on one path.
            std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .replace(|character: char| !character.is_ascii_alphanumeric(), "_")
        ));
        std::fs::write(&path, &bytes).expect("write file");
        let file = VoxFile::load(&path).expect("load");
        let _ = std::fs::remove_file(&path);
        file
    }

    fn voxel(x: u8, y: u8, z: u8, i: u8) -> dot_vox::Voxel {
        dot_vox::Voxel { x, y, z, i }
    }

    /// **The common case**: a file with no `MATL` chunks at all must load
    /// cleanly, report that it described nothing, and still carry every colour.
    #[test]
    fn a_file_with_no_material_chunks_is_a_normal_file() {
        let file = round_trip(
            vec![dot_vox::Model {
                size: dot_vox::Size { x: 2, y: 1, z: 1 },
                voxels: vec![voxel(0, 0, 0, 0), voxel(1, 0, 0, 1)],
            }],
            &[[200, 60, 60, 255], [60, 200, 90, 255]],
            vec![],
        );
        assert_eq!(file.described_material_count(), 0);
        assert_eq!(file.palette[0].rgba, [200, 60, 60, 255]);
        assert_eq!(file.palette[0].kind, VoxMaterialKind::Diffuse);
        assert!(file.palette[0].roughness.is_none());
        assert!(!file.palette[0].describes_a_material());
        assert_eq!(file.models[0].occupied_count(), 2);
    }

    /// Trap 2: a `MATL` id is a 1-based FILE index while a voxel's index is
    /// 0-based in memory. Getting this wrong shifts every material by one entry,
    /// so the test pins the pairing by giving adjacent entries different types.
    #[test]
    fn material_chunks_pair_with_the_right_palette_entry() {
        let file = round_trip(
            vec![dot_vox::Model {
                size: dot_vox::Size { x: 3, y: 1, z: 1 },
                voxels: vec![voxel(0, 0, 0, 0), voxel(1, 0, 0, 1), voxel(2, 0, 0, 2)],
            }],
            &[[10, 10, 10, 255], [20, 20, 20, 255], [30, 30, 30, 255]],
            vec![
                // id 1 describes the entry a voxel calls index 0.
                dot_vox::Material {
                    id: 1,
                    properties: dict(&[("_type", "_diffuse"), ("_rough", "0.8")]),
                },
                dot_vox::Material {
                    id: 2,
                    properties: dict(&[("_type", "_emit"), ("_emit", "0.7")]),
                },
                dot_vox::Material {
                    id: 3,
                    properties: dict(&[("_type", "_glass"), ("_ior", "0.5")]),
                },
            ],
        );
        assert_eq!(file.palette[0].kind, VoxMaterialKind::Diffuse);
        assert_eq!(file.palette[0].roughness, Some(0.8));
        assert_eq!(file.palette[1].kind, VoxMaterialKind::Emit);
        assert_eq!(file.palette[1].emission, Some(0.7));
        assert_eq!(file.palette[2].kind, VoxMaterialKind::Glass);
        assert_eq!(file.described_material_count(), 3);
        // The file index each entry keys on.
        assert_eq!(file.palette[0].file_index, 1);
        assert_eq!(file.palette[2].file_index, 3);
    }

    /// Trap 3: `_ior` is the refractive index minus one. `dot_vox` documents the
    /// conversion and does not do it, so an unconverted import would give glass a
    /// refractive index of 0.5 — below air, i.e. physically impossible.
    #[test]
    fn the_refractive_index_has_the_format_offset_removed() {
        let file = round_trip(
            vec![dot_vox::Model {
                size: dot_vox::Size { x: 2, y: 1, z: 1 },
                voxels: vec![voxel(0, 0, 0, 0), voxel(1, 0, 0, 1)],
            }],
            &[[0, 0, 0, 255], [0, 0, 0, 255]],
            vec![
                dot_vox::Material {
                    id: 1,
                    properties: dict(&[("_type", "_glass"), ("_ior", "0.333")]),
                },
                // Zero must mean "does not refract", i.e. exactly air.
                dot_vox::Material {
                    id: 2,
                    properties: dict(&[("_type", "_glass"), ("_ior", "0")]),
                },
            ],
        );
        let water = file.palette[0].index_of_refraction.expect("authored");
        assert!(
            (water - 1.333).abs() < 1e-5,
            "expected water's 1.333, got {water}"
        );
        assert_eq!(file.palette[1].index_of_refraction, Some(1.0));
        // Absent stays absent — "not authored" is not the same as "1.0".
        assert!(file.palette[2].index_of_refraction.is_none());
    }

    /// Trap 1: MagicaVoxel is Z-up, the engine is Y-up. A model that is tall in
    /// the file must be tall in the engine, and the cell must land at the swapped
    /// coordinate rather than merely somewhere in a same-sized box.
    #[test]
    fn axes_are_swapped_from_z_up_to_y_up() {
        let file = round_trip(
            vec![dot_vox::Model {
                // 1 wide, 2 deep, 3 TALL in file terms.
                size: dot_vox::Size { x: 1, y: 2, z: 3 },
                voxels: vec![voxel(0, 1, 2, 0)],
            }],
            &[[255, 255, 255, 255]],
            vec![],
        );
        let model = &file.models[0];
        assert_eq!((model.size_x, model.size_y, model.size_z), (1, 3, 2));
        // File (x=0, y=1, z=2) -> engine (x=0, y=2, z=1).
        assert_eq!(model.palette_index_at(0, 2, 1), Some(0));
        assert_eq!(model.palette_index_at(0, 1, 2), None);
        assert_eq!(model.occupied_count(), 1);
    }

    /// Packs hold several models and none is privileged, so all must survive.
    #[test]
    fn every_model_in_a_pack_is_loaded() {
        let file = round_trip(
            vec![
                dot_vox::Model {
                    size: dot_vox::Size { x: 1, y: 1, z: 1 },
                    voxels: vec![voxel(0, 0, 0, 0)],
                },
                dot_vox::Model {
                    size: dot_vox::Size { x: 2, y: 2, z: 2 },
                    voxels: vec![voxel(0, 0, 0, 1), voxel(1, 1, 1, 1)],
                },
            ],
            &[[1, 2, 3, 255], [4, 5, 6, 255]],
            vec![],
        );
        assert_eq!(file.models.len(), 2);
        assert_eq!(file.models[0].occupied_count(), 1);
        assert_eq!(file.models[1].occupied_count(), 2);
    }

    /// Out-of-bounds reads must be empty rather than panicking: a consumer's
    /// neighbour lookups walk off the edge of a model by design.
    #[test]
    fn out_of_bounds_reads_are_empty() {
        let file = round_trip(
            vec![dot_vox::Model {
                size: dot_vox::Size { x: 1, y: 1, z: 1 },
                voxels: vec![voxel(0, 0, 0, 0)],
            }],
            &[[9, 9, 9, 255]],
            vec![],
        );
        let model = &file.models[0];
        assert_eq!(model.palette_index_at(0, 0, 0), Some(0));
        for (x, y, z) in [(-1, 0, 0), (0, -1, 0), (0, 0, -1), (1, 0, 0), (0, 1, 0)] {
            assert_eq!(model.palette_index_at(x, y, z), None, "({x},{y},{z})");
        }
    }

    /// The colour conversions must not be confused with each other: sRGB-encoded
    /// floats are the bytes scaled, linear ones have the transfer applied, and the
    /// two differ most in the darks.
    #[test]
    fn colour_conversions_are_distinct_and_correct() {
        let entry = VoxPaletteEntry {
            rgba: [255, 128, 0, 255],
            ..VoxPaletteEntry::default()
        };
        assert_eq!(entry.srgb_rgb(), [1.0, 128.0 / 255.0, 0.0]);
        let linear = entry.linear_rgb();
        assert!((linear[0] - 1.0).abs() < 1e-6, "white stays white");
        assert_eq!(linear[2], 0.0, "black stays black");
        // Mid grey is much darker in linear space — the whole reason to convert.
        assert!(
            linear[1] < 0.25,
            "sRGB 128 should be ~0.216 linear, got {}",
            linear[1]
        );
    }

    /// The repo's own real file, as the check that this works on actual tool
    /// output and not only on what we synthesise. It carries NO material chunks,
    /// which is the point: that is the normal case.
    #[test]
    fn the_repo_campfire_loads_and_brings_no_materials() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/vox/campfire.vox");
        if !path.exists() {
            // The asset is optional to the build; do not fail a checkout without it.
            return;
        }
        let file = VoxFile::load(&path).expect("campfire must load");
        assert_eq!(file.models.len(), 1);
        assert!(file.models[0].occupied_count() > 0);
        assert_eq!(
            file.described_material_count(),
            0,
            "campfire.vox has no MATL chunks — if this changes the fixture changed"
        );
        // Z-up 20x20x14 becomes Y-up 20x14x20.
        let model = &file.models[0];
        assert_eq!((model.size_x, model.size_y, model.size_z), (20, 14, 20));
        // Every occupied cell must name a palette entry that exists.
        for cell in model.cells.iter().flatten() {
            assert!((*cell as usize) < file.palette.len());
        }
    }
}
