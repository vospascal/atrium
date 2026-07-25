//! Runs the [`voxel_core::water_sim::WaterSim`] inside the app (F2 of the
//! volumetric-water arc). Seeds the sim from the generated world's wet region
//! (bounded — the 1000² world can't be simulated whole), steps it on a fixed
//! timestep, and drives a **dynamic water surface mesh** from the live depths
//! so the water visibly flows, pools, and settles. This mesh replaces the
//! static per-chunk water mesh over the wet region.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;
use bevy::render::storage::ShaderStorageBuffer;

use voxel_core::water_sim::WaterSim;
use voxel_core::world::{
    Voxel, VoxelWorld, PLATEAU_FLOOR, VOXEL_SIZE, WATER_LEVEL, WORLD_SIZE_X, WORLD_SIZE_Z,
};

/// Fixed simulation step (seconds) — waves need a stable dt independent of the
/// render frame rate.
const SIM_DT: f32 = 1.0 / 30.0;
/// Don't spend the whole frame catching up if the app stalls.
const MAX_STEPS_PER_FRAME: u32 = 3;

/// A column shallower than this (render metres) is treated as dry — no surface
/// quad is emitted, so the waterline retreats/advances with the sim instead of
/// leaving a paper-thin skin over damp shore cells.
const WET_EPS: f32 = 0.02;

/// How fast recharge tops a connected river back up toward its original level
/// (render metres/second). Capped at the target surface, so it never floods the
/// banks — it only replaces what spills over the rim, keeping the river full
/// while a steady current runs to the edge. Higher = livelier refill.
const RECHARGE_RATE: f32 = 0.8;

/// The live water simulation + where its sub-grid sits in the world (voxel
/// column coordinates of the sub-grid's `(0,0)`).
#[derive(Resource)]
pub struct FluidWater {
    pub sim: WaterSim,
    /// Voxel column of the sub-grid's `(0,0)`, so the surface mesh lands in the
    /// world and spill/recharge can map back to world columns.
    pub origin_x: usize,
    pub origin_z: usize,
    accumulator: f32,
    /// Set whenever the sim advanced at least one step since the height buffer
    /// was last refreshed — the render side only re-uploads on a real change.
    dirty: bool,
    /// Interior columns of every rim-connected water body, topped up toward
    /// `target_surface` each step so the river stays full as it spills.
    recharge: Vec<(usize, usize)>,
    /// Water surface the recharge aims for (the original water plane, render m).
    target_surface: f32,
}

impl FluidWater {
    /// Build a sim bounded to the axis-aligned bounding box of the world's
    /// water columns, seeded with the current water depth. Returns `None` if
    /// the world has no water.
    pub fn from_world(world: &VoxelWorld) -> Option<Self> {
        // Topmost solid ("bed") height in voxels, and whether the column holds
        // water, per column — plus the wet bounding box.
        let mut min_x = WORLD_SIZE_X;
        let mut max_x = 0usize;
        let mut min_z = WORLD_SIZE_Z;
        let mut max_z = 0usize;
        let mut any = false;
        for z in 0..WORLD_SIZE_Z as i32 {
            for x in 0..WORLD_SIZE_X as i32 {
                if column_has_water(world, x, z) {
                    any = true;
                    min_x = min_x.min(x as usize);
                    max_x = max_x.max(x as usize);
                    min_z = min_z.min(z as usize);
                    max_z = max_z.max(z as usize);
                }
            }
        }
        if !any {
            return None;
        }
        // Pad the box by one so shorelines have a dry ring to spread onto.
        let origin_x = min_x.saturating_sub(1);
        let origin_z = min_z.saturating_sub(1);
        let end_x = (max_x + 2).min(WORLD_SIZE_X);
        let end_z = (max_z + 2).min(WORLD_SIZE_Z);
        let size_x = end_x - origin_x;
        let size_z = end_z - origin_z;

        let surface_render = (WATER_LEVEL + 1) as f32 * VOXEL_SIZE;
        let mut terrain = vec![0.0f32; size_x * size_z];
        let mut sim = {
            // Seed floor heights first, then set initial depths.
            for lz in 0..size_z {
                for lx in 0..size_x {
                    let wx = (origin_x + lx) as i32;
                    let wz = (origin_z + lz) as i32;
                    terrain[lz * size_x + lx] = bed_height_render(world, wx, wz);
                }
            }
            WaterSim::new(size_x, size_z, terrain.clone(), VOXEL_SIZE)
        };
        // Seed depths + a wet mask for spill detection / connectivity.
        let mut wet = vec![false; size_x * size_z];
        for lz in 0..size_z {
            for lx in 0..size_x {
                let wx = (origin_x + lx) as i32;
                let wz = (origin_z + lz) as i32;
                if column_has_water(world, wx, wz) {
                    wet[lz * size_x + lx] = true;
                    let floor = terrain[lz * size_x + lx];
                    sim.set_depth(lx, lz, (surface_render - floor).max(0.0));
                }
            }
        }

        // Rim spill lips: wet columns touching the void beyond the plateau.
        // Marking them open drains makes water pour off the edge, which pulls a
        // steady current toward the rim (the emergent waterfall). Collect them
        // as flood-fill seeds for the recharge connectivity.
        let mut spill: Vec<(usize, usize)> = Vec::new();
        let mut is_spill = vec![false; size_x * size_z];
        for lz in 0..size_z {
            for lx in 0..size_x {
                if !wet[lz * size_x + lx] {
                    continue;
                }
                let wx = (origin_x + lx) as i32;
                let wz = (origin_z + lz) as i32;
                let touches_void = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                    .iter()
                    .any(|&(dx, dz)| is_rim_void(world, wx + dx, wz + dz));
                if touches_void {
                    sim.set_open(lx, lz, true);
                    spill.push((lx, lz));
                    is_spill[lz * size_x + lx] = true;
                }
            }
        }

        // Recharge only water bodies that actually spill (connected to a lip),
        // so isolated ponds stay closed/at-rest instead of overflowing. BFS out
        // from the spill lips over wet columns; the interior of that set is the
        // recharge target (the lips themselves drain, so skip them).
        let mut connected = vec![false; size_x * size_z];
        let mut queue: std::collections::VecDeque<(usize, usize)> = spill.iter().copied().collect();
        for &(lx, lz) in &spill {
            connected[lz * size_x + lx] = true;
        }
        while let Some((lx, lz)) = queue.pop_front() {
            for (nx, nz) in neighbors(lx, lz, size_x, size_z) {
                let ni = nz * size_x + nx;
                if wet[ni] && !connected[ni] {
                    connected[ni] = true;
                    queue.push_back((nx, nz));
                }
            }
        }
        let recharge: Vec<(usize, usize)> = (0..size_z)
            .flat_map(|lz| (0..size_x).map(move |lx| (lx, lz)))
            .filter(|&(lx, lz)| {
                let i = lz * size_x + lx;
                connected[i] && !is_spill[i]
            })
            .collect();

        info!(
            "fluid: {} rim spill columns, {} recharge columns (connected river bodies)",
            spill.len(),
            recharge.len(),
        );

        Some(Self {
            sim,
            origin_x,
            origin_z,
            accumulator: 0.0,
            dirty: true,
            recharge,
            target_surface: surface_render,
        })
    }

    /// Corner-lattice dimensions of the surface grid (`size + 1` in each axis).
    fn corner_dims(&self) -> (usize, usize) {
        (self.sim.size_x + 1, self.sim.size_z + 1)
    }

    /// Linear id of corner `(cx, cz)` — matches the id baked into the mesh's
    /// UV.x and the layout of the heights buffer.
    fn corner_id(&self, cx: usize, cz: usize) -> u32 {
        (cz * self.corner_dims().0 + cx) as u32
    }

    /// Map a render-space X/Z to the sub-grid cell it falls in, if in bounds.
    fn render_to_cell(&self, render_x: f32, render_z: f32) -> Option<(usize, usize)> {
        let world_x = render_x / VOXEL_SIZE + WORLD_SIZE_X as f32 / 2.0;
        let world_z = render_z / VOXEL_SIZE + WORLD_SIZE_Z as f32 / 2.0;
        let lx = (world_x.round() as i64) - self.origin_x as i64;
        let lz = (world_z.round() as i64) - self.origin_z as i64;
        if lx < 0 || lz < 0 || lx >= self.sim.size_x as i64 || lz >= self.sim.size_z as i64 {
            return None;
        }
        Some((lx as usize, lz as usize))
    }

    /// Interaction (F5): add (or, with a negative amount, remove) water over a
    /// small disc of cells around a render-space point — a pour or a scoop the
    /// sim then flows, pools, and spills. Marks the surface dirty so the height
    /// buffer refreshes. `radius` is in cells.
    pub fn splash_render(&mut self, render_x: f32, render_z: f32, amount: f32, radius: i32) {
        let Some((cx, cz)) = self.render_to_cell(render_x, render_z) else {
            return;
        };
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dz * dz > radius * radius {
                    continue;
                }
                let x = cx as i64 + dx as i64;
                let z = cz as i64 + dz as i64;
                if x < 0 || z < 0 || x >= self.sim.size_x as i64 || z >= self.sim.size_z as i64 {
                    continue;
                }
                self.sim.add_water(x as usize, z as usize, amount);
            }
        }
        self.dirty = true;
    }

    /// Render-space position of the corner shared by columns meeting at
    /// sub-grid corner `(cx, cz)` (`cx ∈ 0..=size_x`, `cz ∈ 0..=size_z`). The
    /// mapping matches the voxel mesher: world-centred, scaled by `VOXEL_SIZE`.
    fn corner_render_xz(&self, cx: usize, cz: usize) -> (f32, f32) {
        let world_x = (self.origin_x + cx) as f32;
        let world_z = (self.origin_z + cz) as f32;
        let render_x = (world_x - WORLD_SIZE_X as f32 / 2.0) * VOXEL_SIZE;
        let render_z = (world_z - WORLD_SIZE_Z as f32 / 2.0) * VOXEL_SIZE;
        (render_x, render_z)
    }
}

/// Topmost solid ground height for a column, in render-space metres (the top
/// face of the bed). Matches the walker's `bed`.
fn bed_height_render(world: &VoxelWorld, x: i32, z: i32) -> f32 {
    let mut top = None;
    for (voxel, y_start, length) in world.column_runs(x, z) {
        if matches!(
            voxel,
            Voxel::Grass | Voxel::Dirt | Voxel::Sand | Voxel::Sediment | Voxel::Stone
        ) {
            top = Some(y_start + length - 1);
        }
    }
    match top {
        Some(y) => (y + 1) as f32 * VOXEL_SIZE,
        None => 0.0,
    }
}

fn column_has_water(world: &VoxelWorld, x: i32, z: i32) -> bool {
    world
        .column_runs(x, z)
        .any(|(voxel, _, _)| voxel == Voxel::Water)
}

/// A world column that is open sky beyond the plateau rim (the void a rim
/// waterfall spills into). Mirrors the condition in `river_rim_exits`.
fn is_rim_void(world: &VoxelWorld, x: i32, z: i32) -> bool {
    if x < 0 || z < 0 || x >= WORLD_SIZE_X as i32 || z >= WORLD_SIZE_Z as i32 {
        return true; // off the world grid entirely = void
    }
    world.get(x, WATER_LEVEL, z) == Voxel::Air && world.get(x, PLATEAU_FLOOR + 4, z) == Voxel::Air
}

/// In-bounds 4-neighbours of a sub-grid cell.
fn neighbors(lx: usize, lz: usize, size_x: usize, size_z: usize) -> impl Iterator<Item = (usize, usize)> {
    [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .filter_map(move |(dx, dz)| {
            let nx = lx as i32 + dx;
            let nz = lz as i32 + dz;
            (nx >= 0 && nz >= 0 && nx < size_x as i32 && nz < size_z as i32)
                .then_some((nx as usize, nz as usize))
        })
}

/// Advance the sim on a fixed timestep, catching up at most a few steps a
/// frame. Before each step, recharge the rim-connected bodies back toward their
/// original level (capped, so they refill but never flood) — this is what keeps
/// a steady current running to the spill lips instead of the river draining dry.
pub fn step_fluid_water(
    time: Res<Time>,
    fluid: Option<ResMut<FluidWater>>,
    mut tick: Local<u32>,
) {
    let Some(mut fluid) = fluid else {
        return;
    };
    fluid.accumulator += time.delta_secs();
    let mut steps = 0;
    while fluid.accumulator >= SIM_DT && steps < MAX_STEPS_PER_FRAME {
        recharge(&mut fluid, SIM_DT);
        fluid.sim.step(SIM_DT);
        fluid.accumulator -= SIM_DT;
        steps += 1;
    }
    // If we couldn't keep up, drop the backlog rather than spiral.
    if fluid.accumulator > SIM_DT {
        fluid.accumulator = 0.0;
    }
    if steps > 0 {
        fluid.dirty = true;
        *tick += steps;
        // Stability readout (env-gated): confirm the river neither drains nor
        // floods and that a steady spill develops.
        if std::env::var("VOXEL_FLUID_DEBUG").is_ok() && tick.is_multiple_of(30) {
            let volume = fluid.sim.total_volume();
            let mut min_s = f32::MAX;
            let mut max_s = f32::MIN;
            for &(lx, lz) in &fluid.recharge {
                let s = fluid.sim.surface_at(lx, lz);
                min_s = min_s.min(s);
                max_s = max_s.max(s);
            }
            info!(
                "fluid t={} steps: volume {volume:.1}, recharge surface {min_s:.3}..{max_s:.3} (target {:.3})",
                *tick, fluid.target_surface
            );
        }
    }
}

/// Top each recharge column up toward `target_surface`, adding at most
/// `RECHARGE_RATE * dt` and never overshooting the target.
fn recharge(fluid: &mut FluidWater, dt: f32) {
    let target = fluid.target_surface;
    let max_add = RECHARGE_RATE * dt;
    for &(lx, lz) in &fluid.recharge {
        let deficit = target - fluid.sim.surface_at(lx, lz);
        if deficit > 0.0 {
            fluid.sim.add_water(lx, lz, deficit.min(max_add));
        }
    }
}

/// Marker for the single entity that renders the live water surface.
#[derive(Component)]
pub struct DynamicWaterSurface;

/// Handle to the per-corner heights storage buffer bound to the water material.
/// The fluid tick rewrites this each change; the vertex shader displaces the
/// static grid mesh from it (F4 GPU displacement — no mesh rebuild/re-upload).
#[derive(Resource)]
pub struct WaterHeightBuffer(pub Handle<ShaderStorageBuffer>);

/// Which cells get a surface quad in the static grid: any column that holds
/// water at seed time (recharge keeps the rim-connected bodies full, so the wet
/// footprint stays essentially constant — a static topology is safe).
fn seeded_wet(fluid: &FluidWater) -> Vec<bool> {
    let sim = &fluid.sim;
    let (size_x, size_z) = (sim.size_x, sim.size_z);
    let mut wet = vec![false; size_x * size_z];
    for lz in 0..size_z {
        for lx in 0..size_x {
            wet[lz * size_x + lx] = sim.depth_at(lx, lz) > WET_EPS;
        }
    }
    wet
}

/// Build the STATIC surface grid mesh, once. Each wet cell contributes a quad
/// (4 corner verts, non-indexed); every vertex sits at its final world X/Z with
/// `y = 0` and carries its corner id in `uv.x`. The vertex shader
/// (`water_surface.wgsl`) lifts `y` from the live heights buffer each frame, so
/// this mesh never changes — the sim only re-uploads the small buffer.
pub fn build_static_surface_mesh(fluid: &FluidWater) -> Mesh {
    let (size_x, size_z) = (fluid.sim.size_x, fluid.sim.size_z);
    let wet = seeded_wet(fluid);

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for lz in 0..size_z {
        for lx in 0..size_x {
            if !wet[lz * size_x + lx] {
                continue;
            }
            let base = positions.len() as u32;
            // Corner order: (0,0),(1,0),(1,1),(0,1) — CCW seen from above.
            for (cx, cz) in [(lx, lz), (lx + 1, lz), (lx + 1, lz + 1), (lx, lz + 1)] {
                let (render_x, render_z) = fluid.corner_render_xz(cx, cz);
                positions.push([render_x, 0.0, render_z]);
                normals.push([0.0, 1.0, 0.0]);
                colors.push([1.0, 1.0, 1.0, 1.0]);
                uvs.push([fluid.corner_id(cx, cz) as f32, 0.0]);
            }
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(bevy::mesh::Indices::U32(indices))
}

/// Live per-corner surface heights (render metres), laid out to match the
/// corner ids baked into the mesh. Each corner is the mean surface of its
/// adjacent wet cells (dry-only corners fall back to the target plane, but they
/// belong to no rendered quad).
pub fn corner_heights(fluid: &FluidWater) -> Vec<f32> {
    let sim = &fluid.sim;
    let (size_x, size_z) = (sim.size_x, sim.size_z);
    let (corners_x, corners_z) = fluid.corner_dims();
    let mut sum = vec![0.0f32; corners_x * corners_z];
    let mut hits = vec![0u16; corners_x * corners_z];
    for lz in 0..size_z {
        for lx in 0..size_x {
            if sim.depth_at(lx, lz) <= WET_EPS {
                continue;
            }
            let surface = sim.surface_at(lx, lz);
            for (cx, cz) in [(lx, lz), (lx + 1, lz), (lx, lz + 1), (lx + 1, lz + 1)] {
                let ci = cz * corners_x + cx;
                sum[ci] += surface;
                hits[ci] += 1;
            }
        }
    }
    let target = fluid.target_surface;
    (0..corners_x * corners_z)
        .map(|ci| {
            if hits[ci] > 0 {
                sum[ci] / hits[ci] as f32
            } else {
                target
            }
        })
        .collect()
}

/// Refresh the heights buffer from the sim after a step (F4: a small
/// `size²` float upload replaces the old full-mesh rebuild + re-upload). Runs
/// after [`step_fluid_water`]; only re-uploads when the sim actually advanced.
pub fn update_water_heights(
    fluid: Option<ResMut<FluidWater>>,
    height_buffer: Option<Res<WaterHeightBuffer>>,
    mut buffers: ResMut<Assets<ShaderStorageBuffer>>,
) {
    let (Some(mut fluid), Some(height_buffer)) = (fluid, height_buffer) else {
        return;
    };
    if !fluid.dirty {
        return;
    }
    fluid.dirty = false;
    let heights = corner_heights(&fluid);
    if let Some(buffer) = buffers.get_mut(&height_buffer.0) {
        buffer.set_data(heights.as_slice());
    }
}

/// Depth (render metres) added per second at a splash centre while held.
const POUR_RATE: f32 = 6.0;

/// Interaction (F5): hold **G** to pour water where you're looking, **H** to
/// scoop it away. The look ray is intersected with the water plane (or thrown a
/// few metres ahead if it points at the sky), and the sim takes it from there —
/// poured water flows downhill, pools, and spills over the rim.
pub fn water_interaction(
    keyboard: Res<ButtonInput<KeyCode>>,
    camera: Query<&GlobalTransform, With<bevy::core_pipeline::prepass::DepthPrepass>>,
    fluid: Option<ResMut<FluidWater>>,
    time: Res<Time>,
) {
    let Some(mut fluid) = fluid else {
        return;
    };
    let pour = keyboard.pressed(KeyCode::KeyG);
    let scoop = keyboard.pressed(KeyCode::KeyH);
    if !pour && !scoop {
        return;
    }
    let Ok(cam) = camera.single() else {
        return;
    };
    let origin = cam.translation();
    let dir = cam.forward().as_vec3();
    let plane_y = fluid.target_surface;
    // Where the look ray meets the water plane; if it aims skyward, drop the
    // splash a few metres ahead instead.
    let t = if dir.y.abs() > 1e-3 {
        (plane_y - origin.y) / dir.y
    } else {
        -1.0
    };
    let target = if t > 0.0 && t < 200.0 {
        origin + dir * t
    } else {
        origin + dir * 5.0
    };
    let amount = if pour { POUR_RATE } else { -POUR_RATE } * time.delta_secs();
    fluid.splash_render(target.x, target.z, amount, 3);
}
