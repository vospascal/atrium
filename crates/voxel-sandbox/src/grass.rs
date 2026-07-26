//! Instanced grass.
//!
//! Grass used to be baked per-voxel into the chunk mesh (~1.2M verts, static).
//! Here it's **auto-instanced** instead: a small palette of pre-colored
//! grass-clump meshes, each spawned many times as its own entity sharing one
//! mesh handle + one material handle. Bevy batches entities that share a
//! mesh+material into a single instanced draw call, so all grass of one tone
//! is one draw. It uses its own [`GrassMaterial`] (StandardMaterial PBR + a
//! wind extension) — separate from the terrain material so the wind vertex
//! shader (and its matching depth-prepass shader) stays off the terrain.
//!
//! Per-instance variation: the **variant mesh** gives the biome tone
//! (green → straw), the entity `Transform` gives position + random yaw +
//! height.
//!
//! Each blade is a thin vertical **voxel box** (gvox_engine's grass model),
//! not a flat quad, and sways in the wind via the material's vertex shader —
//! see `build_clump_mesh`.

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use voxel_core::noise::{hash_3d, hash_to_unit, smoothstep};
use voxel_core::world::{Voxel, VoxelWorld, VOXEL_SIZE, WORLD_SIZE_X, WORLD_SIZE_Z};

use crate::voxel_material::{GrassExtension, GrassMaterial, FULL_SWAY_WIND_SPEED};
use crate::weather::WeatherState;
use crate::WorldMesh;

/// Marker for spawned grass clumps (despawned on world regenerate).
#[derive(Component)]
pub struct GrassClump;

/// Feed the frame time **and the live weather wind** into every grass material so
/// the sway both animates and answers the wind. Both ride on the material (not
/// `globals`) because `globals` isn't bound in the depth prepass — and the main
/// and prepass vertex shaders must read the *same* values or their depths
/// diverge and the grass z-fights.
///
/// The wind speed is normalised against [`FULL_SWAY_WIND_SPEED`]; the shader
/// turns that into flutter rate, wobble depth, and a steady downwind lean.
pub fn update_grass_wind(
    time: Res<Time>,
    weather: Res<WeatherState>,
    mut materials: ResMut<Assets<GrassMaterial>>,
) {
    let now = time.elapsed_secs();
    let previous = now - time.delta_secs();
    let direction = weather.wind_direction();
    // The GUSTING speed, not the mean: grass answering the wind in waves is the
    // whole point (see `WeatherState::gusting_wind_speed`).
    let strength = (weather.gusting_wind_speed() / FULL_SWAY_WIND_SPEED).clamp(0.0, 1.0);
    for (_, material) in materials.iter_mut() {
        material.extension.time = Vec4::new(now, previous, 0.0, 0.0);
        material.extension.wind = Vec4::new(direction.x, direction.y, strength, 0.0);
    }
}

/// The five biome tones, lush green → dry straw. Chosen per clump so the
/// meadow keeps its gradient without needing per-instance color data.
const GRASS_TONES: [[f32; 3]; 5] = [
    [0.27, 0.44, 0.22],
    [0.33, 0.46, 0.23],
    [0.44, 0.49, 0.27],
    [0.57, 0.54, 0.31],
    [0.66, 0.60, 0.35],
];

/// One clump per this many grass columns. Fewer, denser clumps hold the entity
/// count down (per-entity CPU cost dominates), so N matters more than the
/// per-clump geometry. Stride 3 ≈ a few thousand clumps.
const CLUMP_STRIDE: usize = 3;
/// Total blade height before the per-instance height scale.
const CLUMP_HEIGHT: f32 = 0.5;
/// Thin voxel-cube columns per clump (fanned around the center).
const CLUMP_BLADES: usize = 4;
/// Half-width of a blade column — thin, so it reads as a voxel blade.
const BLADE_HALF_WIDTH: f32 = 0.035;
/// How far the blade bases fan out from the clump center.
const CLUMP_SPREAD: f32 = 0.13;
/// Root/tip brightness of a blade (gvox brightens up the blade): the base is
/// shaded down toward the ground, the tip lifted toward a lit yellow-green.
const ROOT_DARKEN: f32 = 0.55;
const TIP_BRIGHTEN: f32 = 1.4;

/// The clump-mesh palette, one pre-coloured mesh per biome tone. Shared by the
/// island and the streamed world, so both draw the same instanced grass.
pub fn build_clump_meshes(meshes: &mut Assets<Mesh>) -> Vec<Handle<Mesh>> {
    GRASS_TONES
        .iter()
        .map(|tone| meshes.add(build_clump_mesh(*tone)))
        .collect()
}

/// The shared wind material. Solid voxel-box blades → default back-face
/// culling; the wind extension swaps in the sway vertex shaders and the tone
/// comes from the mesh's vertex colours.
pub fn build_grass_material(materials: &mut Assets<GrassMaterial>) -> Handle<GrassMaterial> {
    materials.add(GrassMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.95,
            ..default()
        },
        extension: GrassExtension::default(),
    })
}

/// Which tone in [`GRASS_TONES`] a column gets: drier and farther from water →
/// straw. Indexes the palette from [`build_clump_meshes`].
pub fn clump_variant(dryness: f32, water_distance: f32) -> usize {
    let lushness = smoothstep(9.0, 1.5, water_distance);
    let strawness = (dryness * 0.7 + (1.0 - lushness) * 0.3).clamp(0.0, 1.0);
    ((strawness * GRASS_TONES.len() as f32) as usize).min(GRASS_TONES.len() - 1)
}

/// Is this column a clump site? One clump per [`CLUMP_STRIDE`]-th column on each
/// axis, keyed on **world** coordinates so streamed chunks tile at the same
/// density as their neighbours (a chunk-local stride would break at borders,
/// since the chunk span isn't a multiple of the stride).
pub fn is_clump_column(world_x: i32, world_z: i32) -> bool {
    world_x.rem_euclid(CLUMP_STRIDE as i32) == 0 && world_z.rem_euclid(CLUMP_STRIDE as i32) == 0
}

/// Per-instance placement for a clump sitting on top of `top_y`: centred in its
/// column, with a hashed yaw and height so no two clumps look alike. `offset_x`
/// / `offset_z` centre the world (the island straddles the origin, the streamed
/// world does not).
pub fn clump_transform(
    world_x: i32,
    top_y: i32,
    world_z: i32,
    offset_x: f32,
    offset_z: f32,
    seed: u32,
) -> Transform {
    let yaw =
        hash_to_unit(hash_3d(world_x, 0, world_z, seed.wrapping_add(31))) * std::f32::consts::TAU;
    let height = 0.7 + 0.6 * hash_to_unit(hash_3d(world_x, 1, world_z, seed.wrapping_add(32)));
    Transform {
        translation: Vec3::new(
            (world_x as f32 + 0.5 - offset_x) * VOXEL_SIZE,
            top_y as f32 * VOXEL_SIZE,
            (world_z as f32 + 0.5 - offset_z) * VOXEL_SIZE,
        ),
        rotation: Quat::from_rotation_y(yaw),
        scale: Vec3::new(1.0, height, 1.0),
    }
}

/// Build the palette + material and spawn a clump at every `CLUMP_STRIDE`-th
/// `TallGrass` column, picking a tone variant from the biome.
pub fn spawn_instanced_grass(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<GrassMaterial>,
    world: &VoxelWorld,
    seed: u32,
) {
    let clumps = build_clump_meshes(meshes);
    let material = build_grass_material(materials);

    let half_x = WORLD_SIZE_X as f32 / 2.0;
    let half_z = WORLD_SIZE_Z as f32 / 2.0;
    let mut count = 0_usize;

    for z in (0..WORLD_SIZE_Z as i32).step_by(CLUMP_STRIDE) {
        for x in (0..WORLD_SIZE_X as i32).step_by(CLUMP_STRIDE) {
            // Topmost tall-grass cell in this column, if any.
            let mut grass_top: Option<i32> = None;
            for (voxel, y_start, length) in world.column_runs(x, z) {
                if voxel == Voxel::TallGrass {
                    grass_top = Some(y_start + length - 1);
                }
            }
            let Some(top_y) = grass_top else {
                continue;
            };

            let variant = clump_variant(world.dryness_at(x, z), world.water_distance_at(x, z));
            commands.spawn((
                Mesh3d(clumps[variant].clone()),
                MeshMaterial3d(material.clone()),
                clump_transform(x, top_y, z, half_x, half_z, seed),
                NotShadowCaster,
                GrassClump,
                WorldMesh,
            ));
            count += 1;
        }
    }
    info!("spawned {count} instanced grass clumps");
}

/// A clump = a few thin voxel-box columns fanned around the center (solid,
/// back-face culled) — the gvox-style voxel blade. Vertex color = the biome
/// tone (opaque); StandardMaterial multiplies it into the base color.
fn build_clump_mesh(tone: [f32; 3]) -> Mesh {
    let linear = Color::srgb(tone[0], tone[1], tone[2]).to_linear();
    let root_color = [
        linear.red * ROOT_DARKEN,
        linear.green * ROOT_DARKEN,
        linear.blue * ROOT_DARKEN,
        1.0,
    ];
    let tip_color = [
        (linear.red * TIP_BRIGHTEN).min(1.0),
        (linear.green * TIP_BRIGHTEN).min(1.0),
        (linear.blue * TIP_BRIGHTEN).min(1.0),
        1.0,
    ];

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for blade in 0..CLUMP_BLADES {
        let angle = blade as f32 * (std::f32::consts::TAU / CLUMP_BLADES as f32) + 0.4;
        let base_x = angle.cos() * CLUMP_SPREAD;
        let base_z = angle.sin() * CLUMP_SPREAD;
        // One box per blade. Stacking separate cubes produced coincident
        // internal faces (top of one == bottom of the next) that z-fought into
        // jagged noise; the wind shader bends the single box smoothly by
        // vertex height instead, so no segments are needed. The root/tip colors
        // give the dark-base → bright-tip gradient.
        push_box(
            &mut positions,
            &mut normals,
            &mut colors,
            &mut indices,
            base_x,
            base_z,
            0.0,
            CLUMP_HEIGHT,
            BLADE_HALF_WIDTH,
            root_color,
            tip_color,
        );
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_indices(Indices::U32(indices))
}

/// Append one axis-aligned cube (6 faces, per-face normals, CCW-front winding
/// so back-face culling keeps the outside) centered at `(center_x, center_z)`
/// in x/z, spanning `y0..y1` vertically, with half-width `half`. Vertices at
/// `y0` get `color_bottom`, at `y1` get `color_top` — the blade's root→tip
/// gradient.
#[allow(clippy::too_many_arguments)]
fn push_box(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
    center_x: f32,
    center_z: f32,
    y0: f32,
    y1: f32,
    half: f32,
    color_bottom: [f32; 4],
    color_top: [f32; 4],
) {
    let x0 = center_x - half;
    let x1 = center_x + half;
    let z0 = center_z - half;
    let z1 = center_z + half;

    // Each face: outward normal + 4 corners wound CCW as seen from outside.
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        // +Y (top)
        (
            [0.0, 1.0, 0.0],
            [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]],
        ),
        // -Y (bottom)
        (
            [0.0, -1.0, 0.0],
            [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
        ),
        // +X
        (
            [1.0, 0.0, 0.0],
            [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
        ),
        // -X
        (
            [-1.0, 0.0, 0.0],
            [[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]],
        ),
        // +Z
        (
            [0.0, 0.0, 1.0],
            [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
        ),
        // -Z
        (
            [0.0, 0.0, -1.0],
            [[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]],
        ),
    ];

    for (normal, verts) in faces {
        let base = positions.len() as u32;
        for vertex in verts {
            positions.push(vertex);
            normals.push(normal);
            // Top vertices (at y1) get the bright tip color, base vertices the
            // darkened root color.
            let color = if vertex[1] >= y1 - 1e-4 {
                color_top
            } else {
                color_bottom
            };
            colors.push(color);
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}
