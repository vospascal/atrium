//! Shallow-water fluid simulation ("virtual pipes", after Mei et al.).
//!
//! Water is a **heightfield**: one column per `(x, z)` cell with a `terrain`
//! floor and a `depth` of water above it. Adjacent columns exchange water
//! through virtual pipes whose flow accelerates with the difference in *water
//! surface* height (`terrain + depth`), so water runs downhill, pools level in
//! basins, and spills over low edges. This is the CPU core (F1) of the
//! volumetric-water arc — engine-agnostic and unit-tested; the Bevy layer
//! drives + renders it (F2+). See `docs/fluid-water-plan.md`.
//!
//! It is generic over a rectangular sub-grid so it can be tested on tiny grids
//! and later bounded to just the island's wet region (the full 1000×1000 world
//! is far too big to simulate whole).

/// Directions to the 4 edge-neighbours: −x, +x, −z, +z. `OPPOSITE[d]` is the
/// matching inflow direction on the neighbour.
const OFFSETS: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
const OPPOSITE: [usize; 4] = [1, 0, 3, 2];

/// A shallow-water heightfield sim over a `size_x × size_z` grid.
pub struct WaterSim {
    pub size_x: usize,
    pub size_z: usize,
    /// Floor height per column (same units as `depth`, e.g. metres).
    terrain: Vec<f32>,
    /// Water depth above the floor per column (≥ 0).
    depth: Vec<f32>,
    /// Outflow flux per column to each of the 4 neighbours (volume/time, ≥ 0).
    flux: Vec<[f32; 4]>,
    /// Columns that drain to nothing each step (rim spill / open edges).
    open: Vec<bool>,
    cell_size: f32,
    gravity: f32,
}

impl WaterSim {
    /// New sim with the given floor heights (`terrain.len() == size_x*size_z`,
    /// row-major `z*size_x + x`). Starts dry.
    pub fn new(size_x: usize, size_z: usize, terrain: Vec<f32>, cell_size: f32) -> Self {
        assert_eq!(terrain.len(), size_x * size_z, "terrain size mismatch");
        let count = size_x * size_z;
        Self {
            size_x,
            size_z,
            terrain,
            depth: vec![0.0; count],
            flux: vec![[0.0; 4]; count],
            open: vec![false; count],
            cell_size,
            gravity: 9.8,
        }
    }

    #[inline]
    fn index(&self, x: usize, z: usize) -> usize {
        z * self.size_x + x
    }

    pub fn depth_at(&self, x: usize, z: usize) -> f32 {
        self.depth[self.index(x, z)]
    }

    /// Water surface height (`terrain + depth`) at a column.
    pub fn surface_at(&self, x: usize, z: usize) -> f32 {
        let i = self.index(x, z);
        self.terrain[i] + self.depth[i]
    }

    /// Total outgoing flux (volume/time) leaving a column this step — how hard
    /// it is spilling. Zero at rest; large where water pours over an open lip.
    pub fn outflow_at(&self, x: usize, z: usize) -> f32 {
        self.flux[self.index(x, z)].iter().sum()
    }

    pub fn set_depth(&mut self, x: usize, z: usize, depth: f32) {
        let i = self.index(x, z);
        self.depth[i] = depth.max(0.0);
    }

    pub fn add_water(&mut self, x: usize, z: usize, amount: f32) {
        let i = self.index(x, z);
        self.depth[i] = (self.depth[i] + amount).max(0.0);
    }

    /// Mark a column as an open drain (water there spills away — the rim / a
    /// waterfall lip). Drained columns never hold water.
    pub fn set_open(&mut self, x: usize, z: usize, open: bool) {
        let i = self.index(x, z);
        self.open[i] = open;
    }

    /// Total water volume — constant in a closed sim (the conservation anchor).
    pub fn total_volume(&self) -> f32 {
        let cell_area = self.cell_size * self.cell_size;
        self.depth.iter().map(|d| d * cell_area).sum()
    }

    /// Advance the simulation by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        let (sx, sz) = (self.size_x as i32, self.size_z as i32);
        let cell_area = self.cell_size * self.cell_size;

        // 1. Accelerate each outgoing pipe by the surface-height drop toward its
        //    neighbour (out-of-grid neighbours are walls → no flow). Light
        //    damping keeps it from ringing.
        for z in 0..sz {
            for x in 0..sx {
                let c = (z * sx + x) as usize;
                let surface_c = self.terrain[c] + self.depth[c];
                for (dir, &(ox, oz)) in OFFSETS.iter().enumerate() {
                    let (nx, nz) = (x + ox, z + oz);
                    let drop = if nx < 0 || nz < 0 || nx >= sx || nz >= sz {
                        0.0
                    } else {
                        let n = (nz * sx + nx) as usize;
                        surface_c - (self.terrain[n] + self.depth[n])
                    };
                    let updated = self.flux[c][dir] + dt * self.gravity * self.cell_size * drop;
                    self.flux[c][dir] = (updated * 0.98).max(0.0);
                }
            }
        }

        // 2. Scale a column's outflows so it never sends out more water than it
        //    holds this step — this is what conserves volume.
        for c in 0..self.depth.len() {
            let outflow: f32 = self.flux[c].iter().sum();
            let available = self.depth[c] * cell_area / dt.max(1e-6);
            if outflow > available && outflow > 1e-12 {
                let scale = available / outflow;
                for flux in &mut self.flux[c] {
                    *flux *= scale;
                }
            }
        }

        // 3. Apply net flux (inflow from neighbours − own outflow) to depth.
        for z in 0..sz {
            for x in 0..sx {
                let c = (z * sx + x) as usize;
                let mut inflow = 0.0;
                for (dir, &(ox, oz)) in OFFSETS.iter().enumerate() {
                    let (nx, nz) = (x + ox, z + oz);
                    if nx >= 0 && nz >= 0 && nx < sx && nz < sz {
                        let n = (nz * sx + nx) as usize;
                        inflow += self.flux[n][OPPOSITE[dir]];
                    }
                }
                let outflow: f32 = self.flux[c].iter().sum();
                self.depth[c] += dt * (inflow - outflow) / cell_area;
                if self.depth[c] < 0.0 {
                    self.depth[c] = 0.0;
                }
            }
        }

        // 4. Open columns spill (drain away).
        for c in 0..self.depth.len() {
            if self.open[c] {
                self.depth[c] = 0.0;
                self.flux[c] = [0.0; 4];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(size: usize, floor: f32) -> WaterSim {
        WaterSim::new(size, size, vec![floor; size * size], 1.0)
    }

    fn run(sim: &mut WaterSim, steps: usize, dt: f32) {
        for _ in 0..steps {
            sim.step(dt);
        }
    }

    #[test]
    fn closed_basin_conserves_volume() {
        // Flat floor, walls all around: a lump of water must neither grow nor
        // vanish as it spreads.
        let mut sim = flat(9, 0.0);
        sim.set_depth(4, 4, 10.0);
        let before = sim.total_volume();
        run(&mut sim, 400, 0.05);
        let after = sim.total_volume();
        assert!(
            (after - before).abs() < 1e-3,
            "volume drifted: {before} -> {after}"
        );
    }

    #[test]
    fn water_settles_level() {
        // The lump should spread out to a near-flat surface (min ≈ max depth).
        let mut sim = flat(9, 0.0);
        sim.set_depth(4, 4, 5.0);
        run(&mut sim, 2000, 0.05);
        let mut min = f32::MAX;
        let mut max = f32::MIN;
        for z in 0..9 {
            for x in 0..9 {
                let d = sim.depth_at(x, z);
                min = min.min(d);
                max = max.max(d);
            }
        }
        assert!(max - min < 0.05, "surface not level: min {min} max {max}");
    }

    #[test]
    fn water_flows_downhill() {
        // A ramp sloping down toward +x: water dropped at the high end must end
        // up deeper at the low end than the high end.
        let size = 12;
        let mut terrain = vec![0.0; size * size];
        for z in 0..size {
            for x in 0..size {
                terrain[z * size + x] = (size - 1 - x) as f32; // high at x=0
            }
        }
        let mut sim = WaterSim::new(size, size, terrain, 1.0);
        for z in 0..size {
            sim.set_depth(0, z, 3.0); // pour at the high end
        }
        run(&mut sim, 1500, 0.03);
        let high: f32 = (0..size).map(|z| sim.depth_at(0, z)).sum();
        let low: f32 = (0..size).map(|z| sim.depth_at(size - 1, z)).sum();
        assert!(
            low > high,
            "water didn't flow downhill: high {high} low {low}"
        );
    }

    #[test]
    fn basin_fills_level() {
        // A bowl (low center, high rim) filled with water settles to one flat
        // surface across the whole basin.
        let size = 11;
        let mut terrain = vec![0.0; size * size];
        let center = (size / 2) as i32;
        for z in 0..size {
            for x in 0..size {
                let dx = x as i32 - center;
                let dz = z as i32 - center;
                terrain[z * size + x] = ((dx * dx + dz * dz) as f32).sqrt(); // bowl
            }
        }
        let mut sim = WaterSim::new(size, size, terrain, 1.0);
        sim.set_depth(center as usize, center as usize, 30.0);
        run(&mut sim, 3000, 0.03);
        // Sample two submerged columns; their surfaces should match.
        let s1 = sim.surface_at(center as usize, center as usize);
        let s2 = sim.surface_at(center as usize + 1, center as usize);
        assert!(
            (s1 - s2).abs() < 0.1,
            "basin surface not level: {s1} vs {s2}"
        );
    }

    #[test]
    fn recharged_spill_reaches_steady_state() {
        // A basin with one open rim column (a spill lip) and a capped recharge
        // on the interior reaches a steady state: it neither drains dry (the
        // recharge tops it up) nor floods past the target (the cap stops), and
        // water keeps spilling over the lip — an emergent waterfall.
        let size = 9;
        let mut sim = flat(size, 0.0);
        // Start at the target level everywhere.
        let target = 2.0;
        for z in 0..size {
            for x in 0..size {
                sim.set_depth(x, z, target);
            }
        }
        let lip = (0usize, 4usize); // one open drain on the −x edge
        sim.set_open(lip.0, lip.1, true);
        let dt = 0.03;
        // Capped recharge on every non-lip column, each step.
        let recharge_step = |sim: &mut WaterSim| {
            for z in 0..size {
                for x in 0..size {
                    if (x, z) == lip {
                        continue;
                    }
                    let deficit = target - sim.surface_at(x, z);
                    if deficit > 0.0 {
                        sim.add_water(x, z, deficit.min(0.4 * dt));
                    }
                }
            }
        };
        for _ in 0..4000 {
            recharge_step(&mut sim);
            sim.step(dt);
        }
        // Interior stays near the target (didn't drain dry, didn't overflow).
        let interior = sim.surface_at(size / 2, size / 2);
        assert!(
            interior > target * 0.6 && interior <= target + 0.05,
            "interior level ran away: {interior} (target {target})"
        );
        // The lip is still actively spilling (steady flow, not settled).
        let spill: f32 = (0..size).map(|z| sim.outflow_at(1, z)).sum();
        assert!(spill > 1e-4, "no steady spill over the lip: {spill}");
    }

    #[test]
    fn open_edge_drains() {
        // With an open drain column, a closed lump of water leaves over time.
        let mut sim = flat(9, 0.0);
        sim.set_depth(4, 4, 8.0);
        sim.set_open(0, 4, true);
        let before = sim.total_volume();
        run(&mut sim, 3000, 0.05);
        let after = sim.total_volume();
        assert!(
            after < before * 0.5,
            "drain didn't remove water: {before} -> {after}"
        );
    }
}
