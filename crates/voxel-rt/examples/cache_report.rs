//! Report which pattern layers in the checked-in project could be cached.
//!
//! ```sh
//! cargo run --release -p voxel-rt --example cache_report
//! ```
//!
//! **What the verdicts mean.** Pattern layers are evaluated per pixel per frame
//! with no cache — bench section 11 prices one `worley` layer at ~1.0 ms at
//! 2560x1440. A layer is *cacheable* when nothing time-varying reaches the field
//! itself, so the expensive evaluation could be done once instead of every frame.
//!
//! Animation does not automatically disqualify a layer, because the two
//! animation sockets factor out of the field:
//!
//! - `cacheable (gain)` — an oscillator drives `animation_gain`, which multiplies
//!   the layer AFTER the field is sampled.
//! - `cacheable (drift)` — `drift_velocity` is connected, which moves *where* the
//!   field is read. Exact, because the offset is quantised to whole texels.
//! - `LIVE` — time reaches something that shapes the pattern, so there is nothing
//!   to cache. Not reachable with the current node set; it would take promoting a
//!   generator parameter from a property to a socket.
//!
//! The `source(s)` column counts nodes that read the clock or the event field.
//! Note it can be zero on a layer that still animates: `drift_velocity` carries a
//! velocity the shader multiplies by the clock itself, so a constant vector makes
//! a pattern flow with no oscillator anywhere in the graph. Lava is authored that
//! way.
//!
//! Graphs with no pattern layers are skipped — they have no per-pixel field work
//! either way.

use std::path::PathBuf;

use voxel_graph::GraphAsset;
use voxel_rt::material_cacheability::analyse;

fn main() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../studio-project/graphs");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&directory)
        .expect("the checked-in graphs directory")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.to_string_lossy().ends_with(".vgraph.json"))
        .collect();
    paths.sort();

    println!("cacheability of {} graphs", paths.len());
    println!();

    let (mut with_layers, mut layer_count, mut live_count) = (0_usize, 0_usize, 0_usize);
    for path in &paths {
        let text = std::fs::read_to_string(path).expect("reading a graph");
        let graph: GraphAsset = serde_json::from_str(&text).expect("parsing a graph");
        let report = analyse(&graph, &voxel_rt::graph::CATALOGUE);
        if report.layers.is_empty() {
            continue;
        }
        with_layers += 1;
        layer_count += report.layers.len();
        live_count += report.live_layers().count();

        let verdicts: Vec<String> = report
            .layers
            .iter()
            .map(|layer| layer.cache.summary())
            .collect();
        println!(
            "{:<28} {} layer(s), {} clock/event source(s)",
            path.file_name().unwrap_or_default().to_string_lossy(),
            report.layers.len(),
            report.sources.len(),
        );
        for verdict in verdicts {
            println!("    {verdict}");
        }
        for diagnostic in report.diagnostics() {
            println!("    !! {}", diagnostic.message);
        }
    }

    println!();
    println!(
        "{with_layers} graph(s) author pattern layers, {layer_count} layer(s) total, \
         {live_count} not cacheable"
    );
}
