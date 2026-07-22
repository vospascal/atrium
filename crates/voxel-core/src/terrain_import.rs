//! Import of Blender-authored terrain.
//!
//! Counterpart of `tools/blender/export_terrain.py`: a `.terrain.json`
//! metadata file next to a raw little-endian `f32` height grid. Heights are
//! meters relative to the water plane (Blender Z=0); `NaN` means no surface
//! (open sky beyond the plateau rim). Optional tree positions come from a
//! Blender scatter, in normalized `[0, 1]` UV space across the grid.

use std::path::Path;

use serde::Deserialize;

#[derive(Deserialize)]
struct TerrainMeta {
    width: usize,
    depth: usize,
    heights_file: String,
    #[serde(default)]
    trees_uv: Option<Vec<[f32; 2]>>,
}

pub struct ImportedTerrain {
    width: usize,
    depth: usize,
    /// Row-major `width × depth`, meters relative to the water plane.
    heights: Vec<f32>,
    pub tree_points_uv: Option<Vec<[f32; 2]>>,
}

impl ImportedTerrain {
    /// Terrain from an in-memory grid (heights in meters relative to the
    /// water plane, row-major, `NaN` = no surface). Programmatic twin of
    /// the Blender export path; currently exercised by the biome tests.
    #[cfg(test)]
    pub fn from_grid(
        width: usize,
        depth: usize,
        heights: Vec<f32>,
        tree_points_uv: Option<Vec<[f32; 2]>>,
    ) -> Self {
        assert_eq!(heights.len(), width * depth, "grid dimensions mismatch");
        Self {
            width,
            depth,
            heights,
            tree_points_uv,
        }
    }

    /// Bilinear height sample at normalized coordinates. `NaN` if any
    /// contributing cell has no surface.
    pub fn sample_height(&self, u: f32, v: f32) -> f32 {
        let x = u.clamp(0.0, 1.0) * (self.width - 1) as f32;
        let z = v.clamp(0.0, 1.0) * (self.depth - 1) as f32;
        let x0 = x.floor() as usize;
        let z0 = z.floor() as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let z1 = (z0 + 1).min(self.depth - 1);
        let tx = x - x0 as f32;
        let tz = z - z0 as f32;

        let height_00 = self.heights[z0 * self.width + x0];
        let height_10 = self.heights[z0 * self.width + x1];
        let height_01 = self.heights[z1 * self.width + x0];
        let height_11 = self.heights[z1 * self.width + x1];

        let bottom = height_00 + (height_10 - height_00) * tx;
        let top = height_01 + (height_11 - height_01) * tx;
        bottom + (top - bottom) * tz
    }
}

pub fn load_terrain(json_path: &Path) -> Result<ImportedTerrain, String> {
    let json_text = std::fs::read_to_string(json_path)
        .map_err(|error| format!("cannot read {}: {error}", json_path.display()))?;
    let meta: TerrainMeta = serde_json::from_str(&json_text)
        .map_err(|error| format!("invalid terrain metadata {}: {error}", json_path.display()))?;

    let heights_path = json_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(&meta.heights_file);
    let raw_bytes = std::fs::read(&heights_path)
        .map_err(|error| format!("cannot read {}: {error}", heights_path.display()))?;

    let expected_bytes = meta.width * meta.depth * 4;
    if raw_bytes.len() != expected_bytes {
        return Err(format!(
            "{}: expected {} bytes ({}×{} f32), found {}",
            heights_path.display(),
            expected_bytes,
            meta.width,
            meta.depth,
            raw_bytes.len()
        ));
    }

    let heights = raw_bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();

    Ok(ImportedTerrain {
        width: meta.width,
        depth: meta.depth,
        heights,
        tree_points_uv: meta.trees_uv,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_test_terrain(directory: &Path, width: usize, depth: usize, heights: &[f32]) {
        let raw: Vec<u8> = heights.iter().flat_map(|h| h.to_le_bytes()).collect();
        std::fs::write(directory.join("test.heights.raw"), raw).unwrap();
        std::fs::write(
            directory.join("test.terrain.json"),
            format!(
                r#"{{"width": {width}, "depth": {depth}, "heights_file": "test.heights.raw", "trees_uv": [[0.5, 0.5]]}}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn round_trips_heights_and_interpolates() {
        let directory = std::env::temp_dir().join("voxel_sandbox_terrain_import_test");
        std::fs::create_dir_all(&directory).unwrap();
        // 2×2 grid: flat slope from 0 m to 4 m along x.
        write_test_terrain(&directory, 2, 2, &[0.0, 4.0, 0.0, 4.0]);

        let terrain = load_terrain(&directory.join("test.terrain.json")).unwrap();
        assert_eq!(terrain.sample_height(0.0, 0.0), 0.0);
        assert_eq!(terrain.sample_height(1.0, 1.0), 4.0);
        assert!((terrain.sample_height(0.5, 0.5) - 2.0).abs() < 1e-5);
        assert_eq!(terrain.tree_points_uv.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn nan_marks_open_sky() {
        let directory = std::env::temp_dir().join("voxel_sandbox_terrain_import_nan_test");
        std::fs::create_dir_all(&directory).unwrap();
        write_test_terrain(&directory, 2, 2, &[f32::NAN, 1.0, 1.0, 1.0]);

        let terrain = load_terrain(&directory.join("test.terrain.json")).unwrap();
        assert!(terrain.sample_height(0.0, 0.0).is_nan());
        assert!(!terrain.sample_height(1.0, 1.0).is_nan());
    }
}
