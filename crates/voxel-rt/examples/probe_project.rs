//! What the studio project actually delivers to the REAL world.
//!
//! Not a benchmark — a diagnostic that answers "are the authored assets reaching
//! the generated world, and reaching it intact". It exists because the same
//! question kept being answered by eye from screenshots, which cannot distinguish
//! "the asset did not load" from "the asset loaded and the lighting ignores it".
//!
//! It loads through the SAME calls the app's startup uses
//! (`StudioProject::load_live_state`,
//! `MaterialGraphAssetService::load_shader_set_for_editing`,
//! `compile_active_world_profile`, `apply_initial_generation_profile`), so a
//! divergence here is a real one and not an artefact of a second loading path.
//! The graph compile is part of that list deliberately: without it the probe
//! reported the authored material ROWS while the app was running graph-derived
//! ones, which is the exact class of "loaded but ignored" this exists to catch.
//!
//! Run: `cargo run --release -p voxel-rt --example probe_project [project_path]`

use std::collections::BTreeMap;
use std::path::PathBuf;

use voxel_core::world::{
    VoxelWorld, WorldVoxelCoord, WORLD_VOXELS_X, WORLD_VOXELS_Y, WORLD_VOXELS_Z,
};
use voxel_material::material::{material_voxel, Material, MATERIALS, MATERIAL_COUNT};
use voxel_rt::brickmap::Brickmap;
use voxel_rt::environment::{RuntimeEnvironmentState, Season};
use voxel_rt::material_graph_assets::MaterialGraphAssetService;
use voxel_rt::material_table::MaterialTable;
use voxel_rt::studio_assets::{StudioProject, StudioProjectStore};
use voxel_rt::variants::RenderQuality;
use voxel_rt::world_profile_runtime::apply_initial_generation_profile;

const WORLD_SEED: u32 = 1337;
const WORLD_SEASON: f32 = 0.45;

fn main() {
    let project_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("studio-project"));
    let season = match std::env::args().nth(2).as_deref() {
        Some("spring") => Season::Spring,
        Some("autumn") => Season::Autumn,
        Some("winter") => Season::Winter,
        _ => Season::Summer,
    };
    println!("project: {}", project_path.display());
    if !project_path.exists() {
        println!("  MISSING — the app would silently fall back to the compiled table here.");
        return;
    }

    // ---- 1. what the assets change in the material table ------------------------
    let store = StudioProjectStore::new(&project_path);
    let mut table = MaterialTable::default();
    let mut quality = RenderQuality::default();
    let (project, warnings) = match StudioProject::load_live_state(&store, &mut table, &mut quality)
    {
        Ok(loaded) => loaded,
        Err(error) => {
            println!("  LOAD FAILED: {error}");
            return;
        }
    };
    println!("  loaded, {} warning(s): {warnings:?}", warnings.len());

    // Compile the material graphs into the table, exactly as the app's startup
    // does (`engine_runtime.rs`). `load_live_state` alone applies only the
    // authored material ROWS — before this call the probe reported a table one
    // step behind the running app, which for S3b meant every event-gated emitter
    // looked like a plain constant one.
    let (_graphs, graph_diagnostics) =
        MaterialGraphAssetService::load_shader_set_for_editing(&project_path, &project, &mut table);
    if !graph_diagnostics.is_empty() {
        println!("  graph diagnostics: {graph_diagnostics:?}");
    }

    println!();
    println!("== rows the project CHANGED vs the compiled table ==");
    let mut changed = 0;
    for (slot, compiled) in MATERIALS.iter().enumerate() {
        let loaded = table.row(slot as u8).expect("slot in range");
        if loaded == compiled {
            continue;
        }
        changed += 1;
        println!(
            "  {slot:>2} {:<16} {}",
            compiled.name,
            describe_difference(compiled, loaded)
        );
    }
    if changed == 0 {
        println!("  none — every row is byte-identical to the compiled defaults.");
    }

    // ---- 2. which rows can light anything ---------------------------------------
    println!();
    println!("== rows CAGI will treat as emitters (after load) ==");
    // The value the VOLUME will hold, not the row's own mean. Since S3b they can
    // differ: a row whose graph gates emission on an event stores the channel-wise
    // PEAK of its resting and triggered ends and scales down, so reading the row
    // here would have quietly reported the resting value as "what CAGI sees".
    let attributes = table.cagi_attributes();
    let responses = table.emission_event_responses();
    let mut emitters = 0;
    for slot in 0..MATERIAL_COUNT {
        let row = table.row(slot as u8).expect("slot in range");
        if !row.is_emissive() && responses.get(slot).copied().flatten().is_none() {
            continue;
        }
        emitters += 1;
        let stored = attributes.emission(slot as u8);
        match responses.get(slot).copied().flatten() {
            Some(response) => println!(
                "  {slot:>2} {:<16} stored {stored:?}  (event-gated on channel {}, \
                 radius {} m: resting {:?} -> triggered {:?})",
                row.name,
                response.sensor.channel,
                response.sensor.radius_meters,
                response.resting,
                response.triggered,
            ),
            None => println!("  {slot:>2} {:<16} stored {stored:?}", row.name),
        }
    }
    if emitters == 0 {
        println!("  NONE — nothing in this project contributes light to the volume.");
    }
    let overflow = attributes.event_response_overflow();
    if overflow > 0 {
        println!(
            "  {overflow} row(s) wanted an event response and the volume holds only \
             {} — those keep their peak emission and stop reacting.",
            voxel_rt::cagi::EVENT_RESPONSE_SLOTS - 1
        );
    }

    // ---- 3. does the world profile reach the generated world? -------------------
    println!();
    println!("== world profile applied to the generated world ==");
    let compiled_profile = match project.compile_active_world_profile(&store) {
        Ok(profile) => profile,
        Err(error) => {
            println!("  COMPILE FAILED: {error}");
            return;
        }
    };
    let Some(profile) = compiled_profile else {
        println!("  no active world profile — the generated world is the raw terrain.");
        return;
    };

    // Season is an argument because a rule can be authored correctly and still do
    // nothing: this project's only `AddWorldVoxelLayer` is winter-gated, so a summer
    // run reports zero and looks broken. Sweeping the seasons is what tells
    // "inert" apart from "not applicable right now".
    let runtime = RuntimeEnvironmentState {
        season,
        ..RuntimeEnvironmentState::default()
    };
    println!("  runtime season: {season:?}");
    let world = VoxelWorld::generate(WORLD_SEED, WORLD_SEASON);
    let mut brickmap = Brickmap::build(&world);
    let before = material_histogram(&brickmap);
    match apply_initial_generation_profile(&mut brickmap, &profile, &runtime, u64::from(WORLD_SEED))
    {
        Ok(applied) => println!(
            "  sampled {} columns, changed {} voxels",
            applied.sampled_columns, applied.changed_voxels
        ),
        Err(error) => {
            println!("  APPLY FAILED: {error}");
            return;
        }
    }
    let after = material_histogram(&brickmap);

    println!();
    println!("== materials present in the REAL world (voxel counts) ==");
    println!(
        "  {:>2} {:<16} {:>12} {:>12}",
        "id", "name", "before", "after"
    );
    for slot in 0..MATERIAL_COUNT as u8 {
        let (start, end) = (
            before.get(&slot).copied().unwrap_or(0),
            after.get(&slot).copied().unwrap_or(0),
        );
        if start == 0 && end == 0 {
            continue;
        }
        let row = table.row(slot).expect("slot in range");
        let mark = if row.is_emissive() { " <- emits" } else { "" };
        println!("  {slot:>2} {:<16} {start:>12} {end:>12}{mark}", row.name);
    }
    println!();
    println!(
        "  A row that emits but has a zero count is authored light that no voxel in \
         the world carries; a nonzero count on a row the project did not change means \
         the world is using the COMPILED material, not the authored one."
    );
}

/// Which fields of a loaded row differ from the compiled default — the point is to
/// see at a glance whether the project is authoring anything the renderer reads.
fn describe_difference(compiled: &Material, loaded: &Material) -> String {
    let mut fields = Vec::new();
    if compiled.albedo != loaded.albedo {
        fields.push("albedo".to_string());
    }
    if compiled.roughness != loaded.roughness {
        fields.push("roughness".to_string());
    }
    if compiled.specular != loaded.specular {
        fields.push("specular".to_string());
    }
    if compiled.emission != loaded.emission {
        fields.push(format!(
            "emission {:?}->{:?}",
            compiled.emission, loaded.emission
        ));
    }
    if compiled.patterns != loaded.patterns {
        fields.push(format!(
            "patterns {}->{}",
            compiled.patterns.active_count(),
            loaded.patterns.active_count()
        ));
    }
    if compiled.face_roles != loaded.face_roles {
        fields.push("face_roles".to_string());
    }
    if fields.is_empty() {
        "differs in a field this probe does not name".to_string()
    } else {
        fields.join(", ")
    }
}

/// Voxel count per material id across the world, sampled once per WORLD voxel at
/// its detail origin.
///
/// One sample per world voxel rather than per detail cell: 125x32x125 is half a
/// million reads (instant), where the detail grid is 512x that for a histogram
/// whose shape would not change. Air is skipped — it is the miss sentinel, not a
/// placed material.
fn material_histogram(brickmap: &Brickmap) -> BTreeMap<u8, u64> {
    let mut counts: BTreeMap<u8, u64> = BTreeMap::new();
    for z in 0..WORLD_VOXELS_Z as i32 {
        for y in 0..WORLD_VOXELS_Y as i32 {
            for x in 0..WORLD_VOXELS_X as i32 {
                let detail = WorldVoxelCoord::new(x, y, z).detail_origin();
                let material = brickmap.get(detail[0], detail[1], detail[2]);
                if matches!(material_voxel(material), voxel_core::world::Voxel::Air) {
                    continue;
                }
                *counts.entry(material).or_insert(0) += 1;
            }
        }
    }
    counts
}
