//! Batched grass.
//!
//! Every clump in a chunk is baked into ONE mesh (see
//! [`build_chunk_grass_mesh`]), chunk-local like the chunk meshes, placed by
//! the chunk entity's `Transform`. Grass was previously one entity per clump
//! sharing a palette of pre-coloured meshes; that made the meadow
//! entity-bound, and thousands of tiny entities cost far more frame time than
//! their handful of vertices ever did.
//!
//! Per-clump variation — biome tone, yaw, height, wind phase — is therefore
//! baked into vertex data rather than read from a transform.
//!
//! It uses its own [`GrassMaterial`] (StandardMaterial PBR + a wind extension)
//! — separate from the terrain material so the wind vertex shader (and its
//! matching depth-prepass shader) stays off the terrain.
//!
//! Each blade is a thin vertical **voxel box** (gvox_engine's grass model),
//! not a flat quad, and sways in the wind via the material's vertex shader.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use voxel_core::noise::{hash_3d, hash_to_unit, smoothstep};
use voxel_core::world::VOXEL_SIZE;

use crate::mesh::CHUNK_SIZE;
use crate::voxel_material::{GrassExtension, GrassMaterial, FULL_SWAY_WIND_SPEED};
use crate::weather::WeatherState;

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
/// straw. Baked straight into the batched mesh's vertex colours.
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

/// How far a blade face's shading normal is bent toward world up: `0` keeps
/// the true cube normal, `1` points every face straight up.
///
/// A grass blade is a thin box, so nearly all of what you see is its *sides* —
/// and a vertical face catches almost no light from a high sun, which is why
/// voxel grass reads as dark clutter sitting on a bright meadow. Real grass
/// doesn't behave like a wall: the blades are thin and translucent and the
/// field as a whole bounces light upward, so a meadow shades much closer to a
/// lit horizontal surface than to a mass of vertical ones.
///
/// Bending the *shading* normal up recovers that. Geometry and winding are
/// untouched, so back-face culling still works and the silhouette is identical
/// — only the lighting changes. Kept below `1.0` so blades retain some
/// side-to-side form instead of flattening into cardboard.
const BLADE_NORMAL_LIFT: f32 = 0.75;

/// Bend a face normal toward world up by [`BLADE_NORMAL_LIFT`].
///
/// The downward face is left alone: it is hidden against the ground, and
/// lifting it would flip it to face the sky and make the blade glow from
/// underneath.
fn upward_shading_normal(normal: [f32; 3]) -> [f32; 3] {
    if normal[1] < 0.0 {
        return normal;
    }
    let lifted = Vec3::from(normal).lerp(Vec3::Y, BLADE_NORMAL_LIFT);
    lifted.normalize_or(Vec3::Y).to_array()
}

/// Where one clump sits and how it is turned — previously an entity
/// `Transform`, now baked into the batch mesh's vertices.
struct ClumpPlacement {
    /// Clump origin in world (render) space.
    base: Vec3,
    yaw: f32,
    /// Per-clump height multiplier, so no two clumps are the same size.
    height_scale: f32,
    /// The wind phase for this clump. Baked per-vertex because the shader can
    /// no longer read it from a per-clump transform — see
    /// [`build_chunk_grass_mesh`].
    phase: f32,
}

/// Hashed placement for a clump sitting on top of `top_y`: centred in its
/// column, with a yaw and height so no two clumps look alike. `offset_x` /
/// `offset_z` centre the world.
fn clump_placement(
    world_x: i32,
    top_y: i32,
    world_z: i32,
    origin_x: i32,
    origin_z: i32,
    seed: u32,
) -> ClumpPlacement {
    let yaw =
        hash_to_unit(hash_3d(world_x, 0, world_z, seed.wrapping_add(31))) * std::f32::consts::TAU;
    let height_scale =
        0.7 + 0.6 * hash_to_unit(hash_3d(world_x, 1, world_z, seed.wrapping_add(32)));
    let world_base = Vec3::new(
        (world_x as f32 + 0.5) * VOXEL_SIZE,
        top_y as f32 * VOXEL_SIZE,
        (world_z as f32 + 0.5) * VOXEL_SIZE,
    );
    // Chunk-local, to match the chunk meshes; the entity transform places it.
    let base = Vec3::new(
        world_base.x - origin_x as f32 * VOXEL_SIZE,
        world_base.y,
        world_base.z - origin_z as f32 * VOXEL_SIZE,
    );
    ClumpPlacement {
        base,
        yaw,
        height_scale,
        // Deliberately from the WORLD position, not the chunk-local one:
        // derive it locally and every chunk repeats the same sway pattern,
        // tiling the meadow visibly. Also identical to what the shader used to
        // compute from the clump's transform translation.
        phase: world_base.x * 0.6 + world_base.z * 0.8,
    }
}

/// Bake **every clump in a chunk into one mesh**.
///
/// Grass used to be one entity per clump sharing a palette of pre-coloured
/// meshes, leaning on bevy to auto-instance them. That made the meadow
/// entity-bound: thousands of tiny entities cost far more frame time than
/// their handful of vertices ever did, and the streamer's detail tier had to
/// spawn and despawn them in bulk as the camera moved.
///
/// Batching moves that cost to build time. The catch is that a clump's
/// identity used to live in its `Transform`, and the wind shader read it from
/// there — so two things have to be baked into vertex data instead:
///
/// * **`UV.x` = wind phase.** The shader took this from the transform's
///   translation. Merged, every clump in a chunk would share the chunk's
///   origin and the whole patch would sway in lockstep.
/// * **`UV.y` = unscaled blade height.** The shader took this from
///   `position.y` in object space. Merged, positions are world space, so
///   `position.y` is terrain height plus blade height — meaningless as a bend
///   factor. It stays *unscaled* by `height_scale` deliberately: the old
///   transform scaled the geometry but not the value handed to the wind
///   function, and matching that keeps the sway looking the same.
///
/// Both `grass.wgsl` and `grass_prepass.wgsl` read these, and must agree
/// bit-for-bit or the depth prepass z-fights.
pub fn build_chunk_grass_mesh(
    clumps: &[(i32, i32, i32, usize)],
    chunk_x: i32,
    chunk_z: i32,
    seed: u32,
) -> Option<Mesh> {
    if clumps.is_empty() {
        return None;
    }

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let (origin_x, origin_z) = (chunk_x * CHUNK_SIZE, chunk_z * CHUNK_SIZE);
    for &(world_x, top_y, world_z, variant) in clumps {
        let placement = clump_placement(world_x, top_y, world_z, origin_x, origin_z, seed);
        let tone = GRASS_TONES[variant.min(GRASS_TONES.len() - 1)];
        push_clump(
            &mut positions,
            &mut normals,
            &mut colors,
            &mut uvs,
            &mut indices,
            &placement,
            tone,
        );
    }

    // 32-bit indices, uniformly — see the note in `mesh::MeshBuffers::into_mesh`
    // on why a mixed index format costs more in batching than it saves in bytes.
    Some(
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(indices)),
    )
}

/// Append one clump — a few thin voxel-box blades fanned around its centre,
/// placed by `placement`. The gvox-style voxel blade: solid boxes, back-face
/// culled, root-dark to tip-bright.
#[allow(clippy::too_many_arguments)]
fn push_clump(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 4]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    placement: &ClumpPlacement,
    tone: [f32; 3],
) {
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

    let rotation = Quat::from_rotation_y(placement.yaw);
    // Object space → world: scale the blade's height, turn it by the clump's
    // yaw, drop it at the clump's base. What the entity `Transform` used to do.
    let place = |local: Vec3| {
        placement.base + rotation * Vec3::new(local.x, local.y * placement.height_scale, local.z)
    };

    for blade in 0..CLUMP_BLADES {
        let angle = blade as f32 * (std::f32::consts::TAU / CLUMP_BLADES as f32) + 0.4;
        let center_x = angle.cos() * CLUMP_SPREAD;
        let center_z = angle.sin() * CLUMP_SPREAD;
        let (x0, x1) = (center_x - BLADE_HALF_WIDTH, center_x + BLADE_HALF_WIDTH);
        let (z0, z1) = (center_z - BLADE_HALF_WIDTH, center_z + BLADE_HALF_WIDTH);
        let (y0, y1) = (0.0, CLUMP_HEIGHT);

        // Each face: TRUE outward normal + 4 corners wound CCW as seen from
        // outside. The winding (and so back-face culling) uses the true normal;
        // only the shading normal is bent upward — see `upward_shading_normal`.
        let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
            (
                [0.0, 1.0, 0.0],
                [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]],
            ),
            (
                [0.0, -1.0, 0.0],
                [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
            ),
            (
                [1.0, 0.0, 0.0],
                [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
            ),
            (
                [-1.0, 0.0, 0.0],
                [[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]],
            ),
            (
                [0.0, 0.0, 1.0],
                [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
            ),
            (
                [0.0, 0.0, -1.0],
                [[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]],
            ),
        ];

        for (normal, verts) in faces {
            let base_index = positions.len() as u32;
            // Lift then rotate: the lift is toward world up and a yaw rotation
            // leaves Y alone, so the order doesn't matter — but doing it in
            // object space keeps it the same maths the palette meshes used.
            let shaded = rotation * Vec3::from(upward_shading_normal(normal));
            for vertex in verts {
                positions.push(place(Vec3::from(vertex)).to_array());
                normals.push(shaded.to_array());
                colors.push(if vertex[1] >= y1 - 1e-4 {
                    tip_color
                } else {
                    root_color
                });
                // Wind phase + the UNSCALED object-space height the bend uses.
                uvs.push([placement.phase, vertex[1]]);
            }
            indices.extend([
                base_index,
                base_index + 1,
                base_index + 2,
                base_index,
                base_index + 2,
                base_index + 3,
            ]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the lift: a blade's side faces must shade closer to
    /// a lit horizontal surface than to a wall, or a meadow reads as dark
    /// clutter under a high sun.
    #[test]
    fn side_normals_are_bent_toward_up() {
        for side in [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ] {
            let lifted = upward_shading_normal(side);
            assert!(
                lifted[1] > 0.5,
                "side normal {side:?} should tilt strongly upward, got {lifted:?}"
            );
            let along_face = lifted[0] * side[0] + lifted[2] * side[2];
            assert!(
                along_face > 0.0,
                "side normal {side:?} lost its facing direction: {lifted:?}"
            );
        }
    }

    /// The underside is hidden against the ground. Lifting it would flip it to
    /// face the sky and light the blade from below.
    #[test]
    fn downward_normal_is_left_alone() {
        assert_eq!(upward_shading_normal([0.0, -1.0, 0.0]), [0.0, -1.0, 0.0]);
    }

    /// Normals must stay unit length or PBR shading skews.
    #[test]
    fn shading_normals_stay_unit_length() {
        for normal in [
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0],
            [0.0, -1.0, 0.0],
        ] {
            let lifted = upward_shading_normal(normal);
            let length = Vec3::from(lifted).length();
            assert!(
                (length - 1.0).abs() < 1e-5,
                "normal {normal:?} → {lifted:?} has length {length}"
            );
        }
    }

    /// The mesh's UV_0 as `[f32; 2]` pairs. `Mesh` only exposes a typed
    /// accessor for float3, so the two-component case is matched by hand.
    fn uvs_of(mesh: &Mesh) -> &[[f32; 2]] {
        match mesh.attribute(Mesh::ATTRIBUTE_UV_0).expect("uvs") {
            bevy::mesh::VertexAttributeValues::Float32x2(values) => values,
            other => panic!("UV_0 should be Float32x2, got {other:?}"),
        }
    }

    fn clumps(count: i32) -> Vec<(i32, i32, i32, usize)> {
        (0..count)
            .map(|index| (index * CLUMP_STRIDE as i32, 40, 0, 0))
            .collect()
    }

    /// Batching must produce exactly the geometry the per-clump entities did:
    /// 4 blades × 6 faces × 4 corners each, all in one mesh.
    #[test]
    fn batched_mesh_holds_every_clump() {
        let per_clump = CLUMP_BLADES * 6 * 4;
        for count in [1, 5, 40] {
            let mesh = build_chunk_grass_mesh(&clumps(count), 0, 0, 7).expect("clumps present");
            assert_eq!(mesh.count_vertices(), per_clump * count as usize);
        }
        assert!(
            build_chunk_grass_mesh(&[], 0, 0, 7).is_none(),
            "a chunk with no clumps should produce no mesh at all"
        );
    }

    /// The regression batching could most easily cause: if every clump shared
    /// one wind phase, a whole chunk of grass would sway in lockstep. Phase is
    /// baked per-vertex in UV.x precisely to prevent that.
    #[test]
    fn clumps_keep_distinct_wind_phases() {
        let mesh = build_chunk_grass_mesh(&clumps(8), 0, 0, 7).expect("clumps present");
        let uvs = uvs_of(&mesh);
        let per_clump = CLUMP_BLADES * 6 * 4;

        // Within one clump the phase is constant; across clumps it differs.
        let mut phases = Vec::new();
        for clump in 0..8usize {
            let first = uvs[clump * per_clump][0];
            for vertex in 0..per_clump {
                assert_eq!(
                    uvs[clump * per_clump + vertex][0],
                    first,
                    "one clump must share a single wind phase"
                );
            }
            phases.push(first);
        }
        for (index, phase) in phases.iter().enumerate() {
            for (other_index, other) in phases.iter().enumerate() {
                if index != other_index {
                    assert_ne!(
                        phase, other,
                        "clumps {index} and {other_index} share a wind phase — the patch \
                         would sway in lockstep"
                    );
                }
            }
        }
    }

    /// Wind phase comes from the clump's WORLD position, not its chunk-local
    /// one. Derive it locally and every chunk would repeat the same sway
    /// pattern — the meadow would visibly tile.
    #[test]
    fn wind_phase_does_not_repeat_per_chunk() {
        // The same position *within* a chunk, in two different chunks.
        let (far_x, far_z) = (5, 3);
        let here = clump_placement(0, 40, 0, 0, 0, 7);
        let there = clump_placement(
            far_x * CHUNK_SIZE,
            40,
            far_z * CHUNK_SIZE,
            far_x * CHUNK_SIZE,
            far_z * CHUNK_SIZE,
            7,
        );

        assert_ne!(
            here.phase, there.phase,
            "two chunks gave the same local position the same wind phase — the meadow would tile"
        );
        // ...while the chunk-local base is identical, which is what makes the
        // geometry independent of which chunk it lands in.
        assert_eq!(
            here.base, there.base,
            "chunk-local base should not depend on which chunk it is in"
        );
    }

    /// UV.y carries the blade's UNSCALED object-space height, which is what the
    /// old shader read from `position.y`. It must span root (0) to tip
    /// regardless of the clump's height scale, or the sway strength changes.
    #[test]
    fn blade_height_is_baked_unscaled() {
        let mesh = build_chunk_grass_mesh(&clumps(4), 0, 0, 7).expect("clumps present");
        let uvs = uvs_of(&mesh);
        let heights: Vec<f32> = uvs.iter().map(|uv| uv[1]).collect();
        let lowest = heights.iter().copied().fold(f32::MAX, f32::min);
        let highest = heights.iter().copied().fold(f32::MIN, f32::max);
        assert_eq!(lowest, 0.0, "blade roots should bake height 0");
        assert_eq!(
            highest, CLUMP_HEIGHT,
            "blade tips should bake the unscaled clump height"
        );
    }

    /// Clumps must land where their placement says, so batching doesn't move
    /// the meadow relative to the terrain it grows on.
    #[test]
    fn batched_clumps_sit_at_their_placement() {
        let clump = (30, 40, 12, 0);
        let placement = clump_placement(clump.0, clump.1, clump.2, 0, 0, 7);
        let mesh = build_chunk_grass_mesh(&[clump], 0, 0, 7).expect("clump present");
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("positions")
            .as_float3()
            .expect("float3 positions");

        // Every vertex sits within a clump's reach of its base.
        let reach = CLUMP_SPREAD + BLADE_HALF_WIDTH + 1e-4;
        for position in positions {
            let offset = Vec3::from(*position) - placement.base;
            assert!(
                offset.x.abs() <= reach && offset.z.abs() <= reach,
                "vertex {position:?} is not within the clump at {:?}",
                placement.base
            );
            assert!(
                offset.y >= -1e-4 && offset.y <= CLUMP_HEIGHT * placement.height_scale + 1e-4,
                "vertex {position:?} sits outside the blade's height range"
            );
        }
    }
}
