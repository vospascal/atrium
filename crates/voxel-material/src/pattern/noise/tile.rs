//! Brick/tile tessellation: bond, gap and aspect, plus the per-face UV projection.

use super::hash::hash_cell;

pub(crate) const TILE_SALT: u32 = 61;

/// The tessellation: tile-local `u`/`v`, a per-tile hash and the distance to the
/// nearest edge, from one walk. Mirrors `pattern_tessellate`.
///
/// Returns `[u, v, tone, edge]`. See [`PatternFrame::Tile`] for why the courses are
/// bonded and why the gap is taken out of the tile's interior rather than added
/// around it.
pub(crate) fn tessellate(local: [f32; 2], aspect: f32, bond: f32, gap: f32) -> [f32; 4] {
    let scaled = [local[0] / aspect.max(1e-4), local[1]];
    let row = scaled[1].floor();
    let shifted_x = scaled[0] + row * bond;
    let column = shifted_x.floor();
    let cell = [shifted_x - column, scaled[1] - row];

    let tone = hash_cell([column as i32, row as i32, 0], TILE_SALT);

    let to_edge = cell[0].min(1.0 - cell[0]).min(cell[1].min(1.0 - cell[1]));
    let interior = (0.5 - gap).max(1e-4);
    let edge = ((to_edge - gap) / interior).clamp(0.0, 1.0);

    let span = (1.0 - 2.0 * gap).max(1e-4);
    [
        ((cell[0] - gap) / span).clamp(0.0, 1.0),
        ((cell[1] - gap) / span).clamp(0.0, 1.0),
        tone,
        edge,
    ]
}

/// The edge distance shaped from a bevel into a joint. Mirrors
/// `pattern_tile_edge_shaped`.
pub(crate) fn tile_edge_shaped(edge: f32, sharpness: f32) -> f32 {
    let amount = sharpness.clamp(0.0, 1.0);
    if amount <= 0.0 {
        return edge;
    }
    edge.powf(1.0 / (1.0 + 15.0 * amount))
}

/// The two world axes lying in a face. Mirrors `pattern_face_uv`.
pub(crate) fn face_uv(meters: [f32; 3], axis: u32) -> [f32; 2] {
    match axis {
        0 => [meters[2], meters[1]],
        1 => [meters[0], meters[2]],
        _ => [meters[0], meters[1]],
    }
}
