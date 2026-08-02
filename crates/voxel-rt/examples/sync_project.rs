//! Bring the checked-in studio project up to date with the compiled material table.
//!
//! **Why this exists.** `MATERIALS` is the source of truth for what materials the
//! renderer has; `studio-project/` is a checked-in project that assigns a graph and
//! a `.vmat.json` to each of those slots. Adding a row to the table does not create
//! those files, so the project silently ends up with fewer slots than the table has
//! — and the symptom is not an error. The studio simply has nothing to open for the
//! new material, which reads as "the editor is broken" rather than as "the project
//! is stale".
//!
//! That is exactly what happened when the `slate tile` row was added: the table went
//! to 28 rows and the project still had 27 graphs, so the material existed on the
//! GPU and could not be edited.
//!
//! `save_live_state` already fills the gap — it walks `0..MATERIAL_COUNT` and
//! bootstraps a canonical graph for any slot the manifest does not assign. This
//! example is just a way to run it against the checked-in project from the command
//! line instead of having to open the studio and save.
//!
//! ```sh
//! cargo run -p voxel-rt --example sync_project
//! ```
//!
//! Existing asset IDs are retained, so re-running updates files in place rather than
//! minting new identities — the diff on an already-current project is empty.

use std::path::PathBuf;

use voxel_rt::material_table::MaterialTable;
use voxel_rt::studio_assets::{StudioProject, StudioProjectStore};
use voxel_rt::variants::RenderQuality;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../studio-project");
    let store = StudioProjectStore::new(&root);

    let manifest = store
        .load_manifest()
        .expect("the checked-in project manifest");
    let before = manifest.material_assignments.len();
    let mut project = StudioProject { manifest };

    // The COMPILED table, not one loaded from the project: the whole point is to
    // propagate rows the project does not know about yet.
    let table = MaterialTable::default();
    project
        .save_live_state(&store, "active", &table, &RenderQuality::default())
        .expect("saving the project");

    let after = project.manifest.material_assignments.len();
    println!("studio project at {}", root.display());
    println!("  material assignments: {before} -> {after}");
    if after > before {
        println!("  bootstrapped {} new slot(s)", after - before);
    } else {
        println!("  already current");
    }
}
