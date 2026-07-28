//! A live geometry census: how many entities, vertices, triangles and bytes
//! each kind of mesh contributes to the scene right now.
//!
//! The perf overlay already reports frame time and a raw entity count, which
//! tells you *that* a frame is slow but never *where* the geometry went. This
//! module answers that, and it is the measuring stick for the optimization
//! candidates in `docs/voxel-optimization-candidates.md` — vertex packing,
//! grass instancing and distance LOD are all judged on the numbers below.
//!
//! The census is **per entity, not per system**: every spawn site attaches a
//! [`GeometryCensus`] recording what that entity's mesh costs, and one
//! aggregating system sums them a few times a second. That keeps the totals
//! automatically correct as the streamer spawns and despawns chunks — no
//! separate bookkeeping to drift out of sync — and it works identically for
//! the fixed island and the infinite streamed world.
//!
//! Set `VOXEL_STATS=1` to also dump the table to stdout once a second, which
//! is how the numbers get captured without reading them off the screen.

use bevy::prelude::*;

/// Which bucket an entity's geometry belongs to. Mirrors the mesher's mesh
/// groups plus the separately-spawned detail meshes, so the readout lines up
/// with the code that produces each one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GeometryKind {
    /// Greedy-merged terrain above the water plane.
    TerrainAbove,
    /// Greedy-merged terrain at or below the water plane.
    TerrainBelow,
    /// Meadow carpet: grass tufts, flowers, lily pads baked into the chunk.
    MeadowCover,
    /// Leaf confetti shell (near detail, not a shadow caster).
    Canopy,
    /// Solid inner canopy: the trees' only shadow caster.
    CanopySolid,
    /// Water surface.
    Water,
    /// Instanced grass clumps, one entity per clump today.
    GrassClump,
}

impl GeometryKind {
    /// Every kind, in overlay display order.
    pub const ALL: [GeometryKind; 7] = [
        GeometryKind::TerrainAbove,
        GeometryKind::TerrainBelow,
        GeometryKind::MeadowCover,
        GeometryKind::Canopy,
        GeometryKind::CanopySolid,
        GeometryKind::Water,
        GeometryKind::GrassClump,
    ];

    /// Short label for the overlay table.
    pub fn label(self) -> &'static str {
        match self {
            GeometryKind::TerrainAbove => "terrain",
            GeometryKind::TerrainBelow => "underwater",
            GeometryKind::MeadowCover => "cover",
            GeometryKind::Canopy => "canopy",
            GeometryKind::CanopySolid => "canopy solid",
            GeometryKind::Water => "water",
            GeometryKind::GrassClump => "grass",
        }
    }

    /// Row position in [`GeometryTotals::rows`].
    pub fn index(self) -> usize {
        GeometryKind::ALL
            .iter()
            .position(|&kind| kind == self)
            .expect("every kind is in ALL")
    }
}

/// What one entity's mesh costs. Attached at spawn, where the [`Mesh`] is
/// still in hand — meshes are uploaded with [`RenderAssetUsages::RENDER_WORLD`]
/// and dropped from the main world afterwards, so this is the only point where
/// the geometry can be measured at all.
///
/// [`RenderAssetUsages::RENDER_WORLD`]: bevy::asset::RenderAssetUsages
#[derive(Component, Clone, Copy)]
pub struct GeometryCensus {
    pub kind: GeometryKind,
    pub vertices: u32,
    pub triangles: u32,
    /// Vertex-buffer + index-buffer bytes as they will sit in VRAM.
    pub bytes: u32,
}

impl GeometryCensus {
    /// Measure a mesh before it is handed to [`Assets<Mesh>`].
    pub fn of(mesh: &Mesh, kind: GeometryKind) -> Self {
        let vertices = mesh.count_vertices();
        let vertex_bytes = vertices as u64 * mesh.get_vertex_size();
        let (index_count, index_bytes) = match mesh.indices() {
            Some(bevy::mesh::Indices::U16(indices)) => (indices.len(), indices.len() * 2),
            Some(bevy::mesh::Indices::U32(indices)) => (indices.len(), indices.len() * 4),
            None => (vertices, 0),
        };
        Self {
            kind,
            vertices: vertices as u32,
            triangles: (index_count / 3) as u32,
            bytes: (vertex_bytes + index_bytes as u64) as u32,
        }
    }
}

/// One row of the aggregated table.
#[derive(Clone, Copy, Default)]
pub struct GeometryTotal {
    pub entities: u32,
    pub vertices: u64,
    pub triangles: u64,
    pub bytes: u64,
}

/// The aggregated census, refreshed a few times a second by
/// [`aggregate_geometry_census`] and read by the perf overlay.
#[derive(Resource, Default)]
pub struct GeometryTotals {
    pub rows: [GeometryTotal; GeometryKind::ALL.len()],
}

impl GeometryTotals {
    pub fn row(&self, kind: GeometryKind) -> GeometryTotal {
        self.rows[kind.index()]
    }

    /// Column sums across every kind.
    pub fn overall(&self) -> GeometryTotal {
        self.rows
            .iter()
            .fold(GeometryTotal::default(), |mut sum, row| {
                sum.entities += row.entities;
                sum.vertices += row.vertices;
                sum.triangles += row.triangles;
                sum.bytes += row.bytes;
                sum
            })
    }
}

/// Refresh interval. The census is a debugging readout, not a per-frame cost,
/// and holding the numbers still makes them readable.
const REFRESH_SECONDS: f32 = 0.5;

/// Sum every live [`GeometryCensus`] into [`GeometryTotals`], and dump the
/// table to stdout once a second when `VOXEL_STATS=1`.
pub fn aggregate_geometry_census(
    census: Query<&GeometryCensus>,
    mut totals: ResMut<GeometryTotals>,
    time: Res<Time>,
    mut next_refresh: Local<f32>,
    mut next_dump: Local<f32>,
) {
    let now = time.elapsed_secs();
    if now < *next_refresh {
        return;
    }
    *next_refresh = now + REFRESH_SECONDS;

    let mut rows = [GeometryTotal::default(); GeometryKind::ALL.len()];
    for entry in census.iter() {
        let row = &mut rows[entry.kind.index()];
        row.entities += 1;
        row.vertices += entry.vertices as u64;
        row.triangles += entry.triangles as u64;
        row.bytes += entry.bytes as u64;
    }
    totals.rows = rows;

    if now >= *next_dump && std::env::var("VOXEL_STATS").is_ok() {
        *next_dump = now + 1.0;
        let overall = totals.overall();
        let mut report = String::from("geometry census\n");
        for kind in GeometryKind::ALL {
            let row = totals.row(kind);
            if row.entities == 0 {
                continue;
            }
            report.push_str(&format!(
                "  {:<13} {:>6} ent  {:>9} verts  {:>9} tris  {:>7.1} MB\n",
                kind.label(),
                row.entities,
                row.vertices,
                row.triangles,
                row.bytes as f64 / 1.0e6,
            ));
        }
        report.push_str(&format!(
            "  {:<13} {:>6} ent  {:>9} verts  {:>9} tris  {:>7.1} MB",
            "TOTAL",
            overall.entities,
            overall.vertices,
            overall.triangles,
            overall.bytes as f64 / 1.0e6,
        ));
        info!("{report}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};

    /// One quad in the mesher's exact vertex layout: position + normal +
    /// color, indexed. This is the 40-bytes-per-vertex figure the vertex
    /// packing candidate is measured against, so pin it down.
    fn quad() -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32; 3]; 4])
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0f32; 3]; 4])
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.0f32; 4]; 4])
        .with_inserted_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]))
    }

    #[test]
    fn census_counts_a_quad() {
        let entry = GeometryCensus::of(&quad(), GeometryKind::TerrainAbove);
        assert_eq!(entry.vertices, 4);
        assert_eq!(entry.triangles, 2);
        // 4 verts × (12 + 12 + 16) bytes + 6 indices × 4 bytes.
        assert_eq!(entry.bytes, 4 * 40 + 6 * 4);
    }

    #[test]
    fn totals_sum_per_kind_and_overall() {
        let mut totals = GeometryTotals::default();
        let entry = GeometryCensus::of(&quad(), GeometryKind::Canopy);
        let row = &mut totals.rows[GeometryKind::Canopy.index()];
        row.entities = 3;
        row.vertices = entry.vertices as u64 * 3;
        row.triangles = entry.triangles as u64 * 3;
        row.bytes = entry.bytes as u64 * 3;

        assert_eq!(totals.row(GeometryKind::Canopy).triangles, 6);
        assert_eq!(totals.row(GeometryKind::Water).entities, 0);
        let overall = totals.overall();
        assert_eq!(overall.entities, 3);
        assert_eq!(overall.vertices, 12);
    }
}
