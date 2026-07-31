//! Measurement probe: classify every cell of the generated world as
//! EMPTY / UNIFORM / duplicated / unique, so the tagged-cell design is decided
//! by numbers rather than by intuition.
//!
//! The cell edge is a parameter because it is the thing under test: NAADF
//! (Ulschmid et al., CGF 2026) dedups at a 4^3 block where we brick at 8^3, and
//! a smaller cell repeats far more often. Run `cargo run --release --example
//! brick_census -p voxel-core` to sweep 2/4/8/16.

use std::collections::HashMap;
use voxel_core::world::{VoxelWorld, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z};

fn main() {
    let world = VoxelWorld::generate(1337, 0.35);
    println!("world {WORLD_SIZE_X}x{WORLD_SIZE_Y}x{WORLD_SIZE_Z}");
    for edge in [2usize, 4, 8, 16] {
        census(&world, edge);
    }
}

/// One pass over the world at a given cell edge. Cost is a fresh full traversal
/// per edge, which is seconds — cheap enough not to bother sharing work.
fn census(world: &VoxelWorld, edge: usize) {
    let cells_x = WORLD_SIZE_X.div_ceil(edge);
    let cells_y = WORLD_SIZE_Y.div_ceil(edge);
    let cells_z = WORLD_SIZE_Z.div_ceil(edge);

    let mut empty = 0usize;
    let mut uniform = 0usize;
    let mut occupied = 0usize;
    // Keyed by a 64-bit FNV of the cell contents: storing the bytes themselves
    // would cost hundreds of megabytes, and a collision here would misreport a
    // fraction of a percent. NAADF resolves collisions with a full memcmp
    // before sharing; for a census the hash alone is accurate enough.
    let mut shapes: HashMap<u64, usize> = HashMap::new();
    // Same census restricted to cells that actually have internal structure —
    // the population a TEMPLATE palette would serve. Uniform cells are already
    // handled by the tag and would flatter the dedup numbers.
    let mut sculpted_shapes: HashMap<u64, usize> = HashMap::new();

    let mut cell = vec![0u8; edge * edge * edge];
    for cell_z in 0..cells_z {
        for cell_y in 0..cells_y {
            for cell_x in 0..cells_x {
                let mut solid_count = 0usize;
                let mut first = None;
                let mut all_same = true;
                for local_z in 0..edge {
                    for local_y in 0..edge {
                        for local_x in 0..edge {
                            let voxel = world.get(
                                (cell_x * edge + local_x) as i32,
                                (cell_y * edge + local_y) as i32,
                                (cell_z * edge + local_z) as i32,
                            ) as u8;
                            cell[(local_z * edge + local_y) * edge + local_x] = voxel;
                            if voxel != 0 {
                                solid_count += 1;
                            }
                            match first {
                                None => first = Some(voxel),
                                Some(f) if f != voxel => all_same = false,
                                _ => {}
                            }
                        }
                    }
                }

                if solid_count == 0 {
                    empty += 1;
                    continue;
                }
                occupied += 1;
                if all_same {
                    uniform += 1;
                }
                let mut hash = 0xcbf2_9ce4_8422_2325u64;
                for &byte in &cell {
                    hash = (hash ^ byte as u64).wrapping_mul(0x100_0000_01b3);
                }
                *shapes.entry(hash).or_insert(0) += 1;
                if !all_same {
                    *sculpted_shapes.entry(hash).or_insert(0) += 1;
                }
            }
        }
    }

    let total = empty + occupied;
    let sculpted = occupied - uniform;
    let sculpted_distinct = sculpted_shapes.len();
    let percent = |part: usize, whole: usize| 100.0 * part as f64 / whole.max(1) as f64;

    // Bytes for the leaf payload only: one byte of material per voxel. The
    // deduped figure adds the 4-byte pointer every occupied cell still needs.
    let payload = edge * edge * edge;
    let bytes_now = sculpted * payload;
    let bytes_shared = sculpted_distinct * payload + occupied * 4;

    println!(
        "\ncell {edge}^3 ({cells_x}x{cells_y}x{cells_z} = {total} cells, {payload} B payload)"
    );
    println!("  empty     {empty:>9}  {:>5.1}%", percent(empty, total));
    println!(
        "  occupied  {occupied:>9}  {:>5.1}%   of which uniform {:.1}%",
        percent(occupied, total),
        percent(uniform, occupied)
    );
    println!(
        "  sculpted  {sculpted:>9}  distinct {sculpted_distinct} = {:.1}%  (dedup factor {:.1}x)",
        percent(sculpted_distinct, sculpted),
        sculpted as f64 / sculpted_distinct.max(1) as f64
    );
    println!(
        "  sculpted payload  {:.1} MB -> {:.1} MB deduped",
        bytes_now as f64 / 1.0e6,
        bytes_shared as f64 / 1.0e6
    );
}
