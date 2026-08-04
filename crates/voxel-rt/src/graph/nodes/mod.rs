//! Atrium's node catalogue: one file per node, one line per node here.
//!
//! This flat list is the dispatch point. A node that exists as a file but is missing
//! from it is unreachable — nothing can instantiate it — which is why the list is
//! explicit rather than assembled from the family arrays: a missing line is visible
//! here, and `catalogue_matches_the_family_arrays` fails if a family disagrees.

use voxel_graph::NodeDeclaration;

pub mod material;
pub mod world;

pub static BUILTIN_NODES: &[NodeDeclaration] = &[
    material::constant_scalar::DECLARATION,
    material::output::DECLARATION,
    material::surface::DECLARATION,
    material::constant_color::DECLARATION,
    material::add_scalar::DECLARATION,
    material::mix_color::DECLARATION,
    material::clamp_scalar::DECLARATION,
    material::position::DECLARATION,
    material::normal::DECLARATION,
    material::base_color::DECLARATION,
    material::roughness::DECLARATION,
    material::emission::DECLARATION,
    material::emission_strength::DECLARATION,
    material::face_color::DECLARATION,
    material::face_roughness::DECLARATION,
    material::pattern_flat::DECLARATION,
    material::pattern_noise::DECLARATION,
    material::pattern_speckle::DECLARATION,
    material::pattern_perlin::DECLARATION,
    material::pattern_simplex::DECLARATION,
    material::pattern_ridged::DECLARATION,
    material::pattern_turbulence::DECLARATION,
    material::pattern_worley::DECLARATION,
    material::pattern_worley_edge::DECLARATION,
    material::pattern_worley_smooth::DECLARATION,
    material::pattern_wave::DECLARATION,
    material::pattern_checker::DECLARATION,
    material::pattern_tile_tone::DECLARATION,
    material::pattern_tile_edge::DECLARATION,
    material::tessellation::DECLARATION,
    material::pattern_layer::DECLARATION,
    material::multiply_scalar::DECLARATION,
    material::direction::DECLARATION,
    material::time::DECLARATION,
    material::oscillator::DECLARATION,
    material::event_sensor::DECLARATION,
    material::remap_scalar::DECLARATION,
    material::noise::DECLARATION,
    material::fbm::DECLARATION,
    material::color_ramp::DECLARATION,
    material::vector_add::DECLARATION,
    material::vector_scale::DECLARATION,
    material::normalize_vector::DECLARATION,
    material::dot_vector::DECLARATION,
    material::position_component::DECLARATION,
    material::normal_component::DECLARATION,
    material::passthrough_scalar::DECLARATION,
    material::reroute_scalar::DECLARATION,
    material::reroute_color::DECLARATION,
    material::reroute_vector::DECLARATION,
    world::generated_terrain::DECLARATION,
    world::compose::DECLARATION,
    world::output::DECLARATION,
    world::studio_preview::DECLARATION,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The flat list must be exactly the family arrays, concatenated. A node declared in its
    /// family but missing here would be unreachable — nothing could instantiate it — and
    /// nothing else would complain.
    #[test]
    fn catalogue_matches_the_family_arrays() {
        let families: Vec<&NodeDeclaration> =
            material::NODES.iter().chain(world::NODES.iter()).collect();
        let flat: Vec<&NodeDeclaration> = BUILTIN_NODES.iter().collect();
        assert_eq!(
            flat.len(),
            families.len(),
            "BUILTIN_NODES has {} entries, the family arrays have {}",
            flat.len(),
            families.len()
        );
        for declaration in &families {
            assert!(
                flat.contains(declaration),
                "{} is in its family array but missing from BUILTIN_NODES",
                declaration.id
            );
        }
    }

    /// One file per node is the layout rule, so a file nobody lists is the failure mode it
    /// creates: the node exists, reads as implemented, and cannot be used. Reading the
    /// directory is the only way to catch that — Rust cannot enumerate its own modules.
    #[test]
    fn every_node_file_is_declared_in_its_family() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/graph/nodes");
        for (family, declared) in [("material", material::NODES), ("world", world::NODES)] {
            let mut files: Vec<String> = std::fs::read_dir(root.join(family))
                .expect("family directory")
                .map(|entry| {
                    entry
                        .expect("entry")
                        .file_name()
                        .to_string_lossy()
                        .into_owned()
                })
                .filter(|name| name.ends_with(".rs") && name != "mod.rs")
                .map(|name| name.trim_end_matches(".rs").to_string())
                .collect();
            files.sort();
            let mut listed: Vec<String> = declared
                .iter()
                .map(|node| {
                    node.id
                        .split_once('.')
                        .expect("qualified id")
                        .1
                        .replace('.', "_")
                })
                .collect();
            listed.sort();
            assert_eq!(
                files, listed,
                "{family}/ has files not listed in its NODES array (or vice versa)"
            );
        }
    }
}
