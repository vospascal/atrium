//! Atrium's node catalogue: one file per node, one line per node here.
//!
//! This flat list is the dispatch point. A node that exists as a file but is missing
//! from it is unreachable — nothing can instantiate it — which is why the list is
//! explicit rather than assembled from the family arrays: a missing line is visible
//! here, and `catalogue_matches_the_family_arrays` fails if a family disagrees.

use voxel_graph::NodeDeclaration;

pub(crate) mod biome;
pub mod world;

/// The families `voxel-rt` itself declares. The material family is
/// `voxel_material_graph::NODES`, and `graph::CATALOGUE` composes both — this crate does not
/// restate another crate's nodes.
pub static BUILTIN_NODES: &[NodeDeclaration] = &[
    world::generated_terrain::DECLARATION,
    world::cave_carver::DECLARATION,
    world::cave_chambers::DECLARATION,
    world::cave_detail::DECLARATION,
    world::cave_fluids::DECLARATION,
    world::scatter_rule::DECLARATION,
    world::scatter::DECLARATION,
    world::compose::DECLARATION,
    world::output::DECLARATION,
    world::studio_preview::DECLARATION,
    biome::definition::DECLARATION,
    biome::noise::DECLARATION,
    biome::blend::DECLARATION,
    biome::cellular_seed::DECLARATION,
    biome::cellular_smooth::DECLARATION,
    biome::cellular_cleanup::DECLARATION,
    biome::cellular_connect::DECLARATION,
    biome::output::DECLARATION,
    biome::sample::DECLARATION,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The flat list must be exactly this crate's family arrays, concatenated. The material
    /// family is `voxel_material_graph::NODES` and is composed in by `graph::CATALOGUE`; its own
    /// crate tests it. A node declared in its
    /// family but missing here would be unreachable — nothing could instantiate it — and
    /// nothing else would complain.
    #[test]
    fn catalogue_matches_the_family_arrays() {
        let families: Vec<&NodeDeclaration> =
            world::NODES.iter().chain(biome::NODES.iter()).collect();
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
        for (family, declared) in [("world", world::NODES), ("biome", biome::NODES)] {
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
