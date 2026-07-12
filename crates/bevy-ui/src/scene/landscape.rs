//! Procedural landscape backdrop for the 2D sound emission map.
//!
//! Scatters decorative terrain, vegetation, and water below the schematic
//! (draw layers < 0) so the map reads as a living environment instead of a
//! blank grid. Everything is deterministic (seeded LCG, no `rand` crate) and
//! theme-driven: five biomes (wetland, jungle, desert, snow, beach) x
//! day/night, switchable at runtime.
//!
//! Keys: `B` cycles biome, `N` toggles day/night.

use bevy::prelude::*;

use super::SceneDescription;

// ── Draw layers (all below the floor sprite at -1) ───────────────────────────

const LAYER_TERRAIN: f32 = -9.0;
const LAYER_WATER: f32 = -8.0;
const LAYER_SPECKLE: f32 = -7.0;
const LAYER_VEGETATION: f32 = -6.0;

/// Radius around the listener spawn kept free of vegetation clusters.
const CLEAR_RADIUS: f32 = 3.0;

// ── Theme ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Biome {
    /// Ponds, reeds, and soft tree clusters — the default "night garden" look.
    Wetland,
    /// Dense saturated canopy with a river winding through.
    Jungle,
    /// Sparse dunes, rocks, and columnar cacti. No water.
    Desert,
    /// Drifts, pines, and a frozen pond.
    Snow,
    /// Sand, a shoreline along one edge, and palm clusters.
    Beach,
}

impl Biome {
    pub fn next(self) -> Self {
        match self {
            Biome::Wetland => Biome::Jungle,
            Biome::Jungle => Biome::Desert,
            Biome::Desert => Biome::Snow,
            Biome::Snow => Biome::Beach,
            Biome::Beach => Biome::Wetland,
        }
    }

    /// All biomes, in cycle order — for building picker buttons.
    pub const ALL: [Biome; 5] = [
        Biome::Wetland,
        Biome::Jungle,
        Biome::Desert,
        Biome::Snow,
        Biome::Beach,
    ];

    /// Short display label for the picker button.
    pub fn label(self) -> &'static str {
        match self {
            Biome::Wetland => "Wetland",
            Biome::Jungle => "Jungle",
            Biome::Desert => "Desert",
            Biome::Snow => "Snow",
            Biome::Beach => "Beach",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TimeOfDay {
    Day,
    Night,
}

impl TimeOfDay {
    pub fn toggled(self) -> Self {
        match self {
            TimeOfDay::Day => TimeOfDay::Night,
            TimeOfDay::Night => TimeOfDay::Day,
        }
    }

    pub const ALL: [TimeOfDay; 2] = [TimeOfDay::Day, TimeOfDay::Night];

    pub fn label(self) -> &'static str {
        match self {
            TimeOfDay::Day => "Day",
            TimeOfDay::Night => "Night",
        }
    }
}

/// Active landscape theme. `B` cycles biome, `N` toggles day/night.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub struct LandscapeTheme {
    pub biome: Biome,
    pub time_of_day: TimeOfDay,
}

impl Default for LandscapeTheme {
    fn default() -> Self {
        Self {
            biome: Biome::Wetland,
            time_of_day: TimeOfDay::Night,
        }
    }
}

/// Resolved colors for the current theme. Overlay colors (link lines, listener
/// rings, outlines) live here too so the schematic stays readable on both dark
/// night and bright day backgrounds.
pub struct Palette {
    pub background: Color,
    pub floor: Color,
    pub terrain: [Color; 3],
    pub vegetation: [Color; 3],
    pub water: Option<Color>,
    pub speckle: Color,
    /// Dashed source → listener connection lines (alpha applied per line).
    pub link_line: Color,
    /// Listener halo rings and facing cone.
    pub listener_ring: Color,
    /// Icon glyph strokes (drawn on dark badges, stays light in all themes).
    pub icon: Color,
    /// Room / environment outlines.
    pub outline: Color,
}

impl LandscapeTheme {
    pub fn palette(&self) -> Palette {
        use Biome::*;
        use TimeOfDay::*;
        match (self.biome, self.time_of_day) {
            (Wetland, Night) => Palette {
                background: Color::srgb(0.043, 0.075, 0.118),
                floor: Color::srgba(0.10, 0.15, 0.21, 0.30),
                terrain: [
                    Color::srgba(0.06, 0.13, 0.15, 0.55),
                    Color::srgba(0.05, 0.11, 0.16, 0.50),
                    Color::srgba(0.08, 0.15, 0.13, 0.45),
                ],
                vegetation: [
                    Color::srgba(0.09, 0.21, 0.14, 0.80),
                    Color::srgba(0.12, 0.25, 0.15, 0.70),
                    Color::srgba(0.07, 0.17, 0.12, 0.85),
                ],
                water: Some(Color::srgba(0.07, 0.19, 0.28, 0.65)),
                speckle: Color::srgba(0.55, 0.80, 0.60, 0.30),
                link_line: Color::srgb(0.80, 0.88, 1.00),
                listener_ring: Color::srgb(0.55, 0.75, 1.00),
                icon: Color::srgb(0.94, 0.97, 1.00),
                outline: Color::srgb(0.45, 0.55, 0.70),
            },
            (Wetland, Day) => Palette {
                background: Color::srgb(0.55, 0.65, 0.53),
                floor: Color::srgba(0.85, 0.90, 0.85, 0.25),
                terrain: [
                    Color::srgba(0.44, 0.58, 0.38, 0.60),
                    Color::srgba(0.40, 0.54, 0.36, 0.55),
                    Color::srgba(0.50, 0.62, 0.40, 0.50),
                ],
                vegetation: [
                    Color::srgba(0.22, 0.45, 0.24, 0.85),
                    Color::srgba(0.28, 0.52, 0.28, 0.75),
                    Color::srgba(0.18, 0.38, 0.20, 0.90),
                ],
                water: Some(Color::srgba(0.35, 0.60, 0.74, 0.75)),
                speckle: Color::srgba(0.95, 0.92, 0.60, 0.45),
                link_line: Color::srgb(0.10, 0.16, 0.26),
                listener_ring: Color::srgb(0.12, 0.28, 0.55),
                icon: Color::srgb(0.96, 0.98, 1.00),
                outline: Color::srgb(0.22, 0.30, 0.40),
            },
            (Jungle, Night) => Palette {
                background: Color::srgb(0.030, 0.075, 0.055),
                floor: Color::srgba(0.09, 0.16, 0.13, 0.30),
                terrain: [
                    Color::srgba(0.05, 0.13, 0.09, 0.60),
                    Color::srgba(0.04, 0.11, 0.10, 0.55),
                    Color::srgba(0.07, 0.15, 0.08, 0.50),
                ],
                vegetation: [
                    Color::srgba(0.08, 0.22, 0.11, 0.85),
                    Color::srgba(0.11, 0.27, 0.13, 0.75),
                    Color::srgba(0.05, 0.17, 0.10, 0.90),
                ],
                water: Some(Color::srgba(0.06, 0.17, 0.22, 0.65)),
                speckle: Color::srgba(0.70, 0.90, 0.50, 0.35),
                link_line: Color::srgb(0.80, 0.92, 0.85),
                listener_ring: Color::srgb(0.55, 0.90, 0.75),
                icon: Color::srgb(0.94, 0.98, 0.96),
                outline: Color::srgb(0.40, 0.58, 0.50),
            },
            (Jungle, Day) => Palette {
                background: Color::srgb(0.42, 0.58, 0.36),
                floor: Color::srgba(0.80, 0.88, 0.72, 0.25),
                terrain: [
                    Color::srgba(0.30, 0.50, 0.26, 0.65),
                    Color::srgba(0.26, 0.46, 0.24, 0.60),
                    Color::srgba(0.36, 0.55, 0.28, 0.55),
                ],
                vegetation: [
                    Color::srgba(0.13, 0.40, 0.16, 0.90),
                    Color::srgba(0.18, 0.48, 0.20, 0.80),
                    Color::srgba(0.10, 0.32, 0.14, 0.95),
                ],
                water: Some(Color::srgba(0.28, 0.55, 0.62, 0.80)),
                speckle: Color::srgba(0.95, 0.75, 0.35, 0.50),
                link_line: Color::srgb(0.08, 0.14, 0.10),
                listener_ring: Color::srgb(0.10, 0.30, 0.22),
                icon: Color::srgb(0.96, 0.99, 0.97),
                outline: Color::srgb(0.18, 0.28, 0.20),
            },
            (Desert, Night) => Palette {
                background: Color::srgb(0.085, 0.070, 0.110),
                floor: Color::srgba(0.18, 0.15, 0.18, 0.30),
                terrain: [
                    Color::srgba(0.20, 0.15, 0.14, 0.50),
                    Color::srgba(0.16, 0.13, 0.15, 0.45),
                    Color::srgba(0.23, 0.18, 0.14, 0.40),
                ],
                vegetation: [
                    Color::srgba(0.10, 0.19, 0.13, 0.85),
                    Color::srgba(0.13, 0.23, 0.15, 0.75),
                    Color::srgba(0.08, 0.15, 0.11, 0.90),
                ],
                water: None,
                speckle: Color::srgba(0.85, 0.80, 0.60, 0.25),
                link_line: Color::srgb(0.92, 0.86, 0.75),
                listener_ring: Color::srgb(0.90, 0.75, 0.50),
                icon: Color::srgb(0.98, 0.95, 0.90),
                outline: Color::srgb(0.60, 0.50, 0.42),
            },
            (Desert, Day) => Palette {
                background: Color::srgb(0.80, 0.70, 0.52),
                floor: Color::srgba(0.95, 0.88, 0.72, 0.30),
                terrain: [
                    Color::srgba(0.88, 0.78, 0.58, 0.70),
                    Color::srgba(0.82, 0.70, 0.50, 0.60),
                    Color::srgba(0.74, 0.62, 0.46, 0.55),
                ],
                vegetation: [
                    Color::srgba(0.28, 0.46, 0.26, 0.90),
                    Color::srgba(0.34, 0.52, 0.30, 0.80),
                    Color::srgba(0.24, 0.40, 0.24, 0.95),
                ],
                water: None,
                speckle: Color::srgba(0.55, 0.42, 0.30, 0.40),
                link_line: Color::srgb(0.24, 0.16, 0.08),
                listener_ring: Color::srgb(0.42, 0.24, 0.08),
                icon: Color::srgb(0.99, 0.97, 0.94),
                outline: Color::srgb(0.40, 0.30, 0.20),
            },
            (Snow, Night) => Palette {
                background: Color::srgb(0.060, 0.090, 0.145),
                floor: Color::srgba(0.30, 0.38, 0.50, 0.22),
                terrain: [
                    Color::srgba(0.45, 0.55, 0.72, 0.18),
                    Color::srgba(0.55, 0.65, 0.80, 0.14),
                    Color::srgba(0.38, 0.48, 0.66, 0.20),
                ],
                vegetation: [
                    Color::srgba(0.09, 0.18, 0.17, 0.90),
                    Color::srgba(0.12, 0.23, 0.20, 0.80),
                    Color::srgba(0.07, 0.14, 0.14, 0.95),
                ],
                water: Some(Color::srgba(0.40, 0.55, 0.72, 0.25)),
                speckle: Color::srgba(0.92, 0.96, 1.00, 0.40),
                link_line: Color::srgb(0.85, 0.92, 1.00),
                listener_ring: Color::srgb(0.65, 0.82, 1.00),
                icon: Color::srgb(0.95, 0.98, 1.00),
                outline: Color::srgb(0.50, 0.62, 0.80),
            },
            (Beach, Night) => Palette {
                background: Color::srgb(0.048, 0.080, 0.135),
                floor: Color::srgba(0.20, 0.20, 0.20, 0.28),
                terrain: [
                    Color::srgba(0.24, 0.21, 0.18, 0.40),
                    Color::srgba(0.20, 0.18, 0.17, 0.35),
                    Color::srgba(0.28, 0.25, 0.20, 0.30),
                ],
                vegetation: [
                    Color::srgba(0.08, 0.18, 0.13, 0.85),
                    Color::srgba(0.11, 0.22, 0.15, 0.75),
                    Color::srgba(0.06, 0.14, 0.11, 0.90),
                ],
                water: Some(Color::srgba(0.09, 0.24, 0.35, 0.70)),
                speckle: Color::srgba(0.80, 0.90, 1.00, 0.30),
                link_line: Color::srgb(0.82, 0.90, 1.00),
                listener_ring: Color::srgb(0.55, 0.80, 1.00),
                icon: Color::srgb(0.95, 0.97, 1.00),
                outline: Color::srgb(0.48, 0.58, 0.72),
            },
            (Beach, Day) => Palette {
                background: Color::srgb(0.86, 0.79, 0.62),
                floor: Color::srgba(0.96, 0.92, 0.80, 0.35),
                terrain: [
                    Color::srgba(0.92, 0.86, 0.70, 0.70),
                    Color::srgba(0.88, 0.80, 0.62, 0.60),
                    Color::srgba(0.82, 0.74, 0.56, 0.55),
                ],
                vegetation: [
                    Color::srgba(0.18, 0.46, 0.26, 0.90),
                    Color::srgba(0.24, 0.54, 0.30, 0.80),
                    Color::srgba(0.14, 0.38, 0.22, 0.95),
                ],
                water: Some(Color::srgba(0.22, 0.62, 0.70, 0.85)),
                speckle: Color::srgba(0.98, 0.98, 0.95, 0.55),
                link_line: Color::srgb(0.10, 0.17, 0.24),
                listener_ring: Color::srgb(0.05, 0.32, 0.52),
                icon: Color::srgb(0.97, 0.99, 1.00),
                outline: Color::srgb(0.32, 0.40, 0.48),
            },
            (Snow, Day) => Palette {
                background: Color::srgb(0.80, 0.85, 0.90),
                floor: Color::srgba(0.98, 0.99, 1.00, 0.45),
                terrain: [
                    Color::srgba(0.94, 0.96, 1.00, 0.70),
                    Color::srgba(0.88, 0.92, 0.98, 0.60),
                    Color::srgba(0.82, 0.88, 0.96, 0.55),
                ],
                vegetation: [
                    Color::srgba(0.14, 0.30, 0.26, 0.90),
                    Color::srgba(0.18, 0.36, 0.30, 0.80),
                    Color::srgba(0.11, 0.24, 0.22, 0.95),
                ],
                water: Some(Color::srgba(0.62, 0.78, 0.90, 0.75)),
                speckle: Color::srgba(1.00, 1.00, 1.00, 0.80),
                link_line: Color::srgb(0.14, 0.20, 0.32),
                listener_ring: Color::srgb(0.16, 0.30, 0.55),
                icon: Color::srgb(0.97, 0.99, 1.00),
                outline: Color::srgb(0.30, 0.40, 0.55),
            },
        }
    }
}

// ── Rendering-only markers ───────────────────────────────────────────────────

/// Marker for every decorative landscape entity (despawned on theme change).
#[derive(Component)]
pub(crate) struct LandscapeDecor;

/// Marker for the atrium floor sprite (retinted on theme change).
#[derive(Component)]
pub(crate) struct FloorSprite;

/// Gentle drift around a base position — tree sway, firefly wander.
#[derive(Component)]
pub(crate) struct Sway {
    pub base: Vec2,
    pub phase: f32,
    pub amplitude: f32,
}

// ── Deterministic PRNG (no rand dependency; layouts must be reproducible) ────

struct Lcg(u64);

impl Lcg {
    fn next_f32(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as f32 / (1u64 << 31) as f32
    }

    fn range(&mut self, low: f32, high: f32) -> f32 {
        low + self.next_f32() * (high - low)
    }
}

// ── Spawning ─────────────────────────────────────────────────────────────────

/// How a vegetation cluster is arranged.
enum ClusterShape {
    /// Overlapping puffs — broadleaf canopy.
    Canopy,
    /// Vertically stacked small circles — cactus.
    Column,
    /// Concentric circles — pine seen from above.
    Pine,
}

/// Spawn the full decorative landscape for the current theme.
pub(crate) fn spawn_landscape(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<ColorMaterial>,
    description: &SceneDescription,
    theme: LandscapeTheme,
) {
    let palette = theme.palette();
    let env = &description.environment;
    // Environment coordinates span 0..width × 0..depth (corner origin); the
    // listener starts at `spawn`. Scatter around the env center, keep the
    // spawn area clear.
    let center = Vec2::new(env.width * 0.5, env.depth * 0.5);
    let clear_center = Vec2::new(env.spawn[0], env.spawn[1]);
    let half_width = env.width * 0.5 + 2.0;
    let half_depth = env.depth * 0.5 + 2.0;
    let area = (half_width * 2.0) * (half_depth * 2.0);

    // Unit circle scaled per entity — one mesh for the whole landscape.
    let circle = meshes.add(Circle::new(1.0));
    let terrain_materials: Vec<_> = palette
        .terrain
        .iter()
        .map(|&color| materials.add(ColorMaterial::from_color(color)))
        .collect();
    let vegetation_materials: Vec<_> = palette
        .vegetation
        .iter()
        .map(|&color| materials.add(ColorMaterial::from_color(color)))
        .collect();
    let speckle_material = materials.add(ColorMaterial::from_color(palette.speckle));

    // Seed differs per biome/time so each theme gets its own layout.
    let seed = 0x5EED_0001
        + theme.biome as u64 * 7919
        + match theme.time_of_day {
            TimeOfDay::Day => 0,
            TimeOfDay::Night => 104729,
        };
    let mut rng = Lcg(seed);

    // ── Terrain blobs: large soft patches of ground variation ──
    let terrain_count = (area / 14.0).clamp(6.0, 24.0) as usize;
    for layer_index in 0..terrain_count {
        let position = center
            + Vec2::new(
                rng.range(-half_width, half_width),
                rng.range(-half_depth, half_depth),
            );
        let radius = rng.range(1.6, 4.2);
        let material = &terrain_materials[layer_index % terrain_materials.len()];
        spawn_blob(
            commands,
            &circle,
            material,
            position,
            radius,
            LAYER_TERRAIN,
            rng.range(0.7, 1.0),
        );
    }

    // ── Water ──
    if palette.water.is_some() {
        spawn_water(
            commands,
            &circle,
            materials,
            &palette,
            &mut rng,
            theme.biome,
            center,
            half_width,
            half_depth,
        );
    }

    // ── Vegetation clusters ──
    let (shape, cluster_count, sway_amplitude) = match theme.biome {
        Biome::Wetland => (ClusterShape::Canopy, (area / 11.0) as usize, 0.06),
        Biome::Jungle => (ClusterShape::Canopy, (area / 6.5) as usize, 0.09),
        Biome::Desert => (ClusterShape::Column, (area / 30.0) as usize, 0.015),
        Biome::Snow => (ClusterShape::Pine, (area / 16.0) as usize, 0.02),
        Biome::Beach => (ClusterShape::Canopy, (area / 22.0) as usize, 0.08),
    };
    for _ in 0..cluster_count.clamp(4, 48) {
        // Keep the spawn area clear so the listener + close sources stay readable.
        let position = loop {
            let candidate = center
                + Vec2::new(
                    rng.range(-half_width, half_width),
                    rng.range(-half_depth, half_depth),
                );
            if (candidate - clear_center).length() > CLEAR_RADIUS {
                break candidate;
            }
        };
        spawn_cluster(
            commands,
            &circle,
            &vegetation_materials,
            position,
            &shape,
            sway_amplitude,
            &mut rng,
        );
    }

    // ── Speckles: fireflies at night, pollen/snowflakes by day ──
    let speckle_count = (area / 4.0).clamp(16.0, 60.0) as usize;
    for _ in 0..speckle_count {
        let position = center
            + Vec2::new(
                rng.range(-half_width, half_width),
                rng.range(-half_depth, half_depth),
            );
        let radius = rng.range(0.03, 0.08);
        commands.spawn((
            LandscapeDecor,
            Sway {
                base: position,
                phase: rng.range(0.0, std::f32::consts::TAU),
                amplitude: rng.range(0.08, 0.25),
            },
            Mesh2d(circle.clone()),
            MeshMaterial2d(speckle_material.clone()),
            Transform::from_xyz(position.x, position.y, LAYER_SPECKLE)
                .with_scale(Vec3::splat(radius)),
        ));
    }
}

/// One scaled translucent circle.
fn spawn_blob(
    commands: &mut Commands,
    circle: &Handle<Mesh>,
    material: &Handle<ColorMaterial>,
    position: Vec2,
    radius: f32,
    layer: f32,
    squash: f32,
) {
    commands.spawn((
        LandscapeDecor,
        Mesh2d(circle.clone()),
        MeshMaterial2d(material.clone()),
        Transform::from_xyz(position.x, position.y, layer).with_scale(Vec3::new(
            radius,
            radius * squash,
            1.0,
        )),
    ));
}

/// A vegetation cluster: canopy puffs, cactus column, or top-down pine.
fn spawn_cluster(
    commands: &mut Commands,
    circle: &Handle<Mesh>,
    vegetation_materials: &[Handle<ColorMaterial>],
    center: Vec2,
    shape: &ClusterShape,
    sway_amplitude: f32,
    rng: &mut Lcg,
) {
    // Collect puffs first (offset, radius, material index), then spawn — keeps
    // the RNG borrow out of the spawn loop.
    let mut puffs: Vec<(Vec2, f32, usize)> = Vec::new();
    match shape {
        ClusterShape::Canopy => {
            let base_radius = rng.range(0.7, 1.5);
            let puff_count = 3 + (rng.next_f32() * 3.0) as usize;
            for puff_index in 0..puff_count {
                let offset = Vec2::new(rng.range(-0.7, 0.7), rng.range(-0.7, 0.7)) * base_radius;
                let radius = base_radius * rng.range(0.45, 0.85);
                puffs.push((offset, radius, puff_index));
            }
        }
        ClusterShape::Column => {
            let radius = rng.range(0.18, 0.30);
            let segments = 2 + (rng.next_f32() * 2.0) as usize;
            for segment_index in 0..segments {
                puffs.push((
                    Vec2::new(0.0, segment_index as f32 * radius * 1.4),
                    radius,
                    segment_index,
                ));
            }
        }
        ClusterShape::Pine => {
            let base_radius = rng.range(0.5, 1.0);
            puffs.push((Vec2::ZERO, base_radius, 2));
            puffs.push((Vec2::ZERO, base_radius * 0.60, 1));
            puffs.push((Vec2::ZERO, base_radius * 0.28, 0));
        }
    }

    for (offset, radius, material_index) in puffs {
        let position = center + offset;
        commands.spawn((
            LandscapeDecor,
            Sway {
                base: position,
                phase: rng.range(0.0, std::f32::consts::TAU),
                amplitude: sway_amplitude,
            },
            Mesh2d(circle.clone()),
            MeshMaterial2d(
                vegetation_materials[material_index % vegetation_materials.len()].clone(),
            ),
            Transform::from_xyz(
                position.x,
                position.y,
                LAYER_VEGETATION + material_index as f32 * 0.01,
            )
            .with_scale(Vec3::splat(radius)),
        ));
    }
}

/// Water bodies: a pond (wetland/snow) or a winding river (jungle).
#[allow(clippy::too_many_arguments)]
fn spawn_water(
    commands: &mut Commands,
    circle: &Handle<Mesh>,
    materials: &mut Assets<ColorMaterial>,
    palette: &Palette,
    rng: &mut Lcg,
    biome: Biome,
    center: Vec2,
    half_width: f32,
    half_depth: f32,
) {
    let Some(water_color) = palette.water else {
        return;
    };
    let water_material = materials.add(ColorMaterial::from_color(water_color));

    match biome {
        Biome::Jungle => {
            // River: overlapping circles along a sine path across the map.
            let steps = 22;
            for step in 0..steps {
                let t = step as f32 / (steps - 1) as f32;
                let x = center.x - half_width + t * half_width * 2.0;
                let y = center.y
                    + (t * std::f32::consts::TAU * 0.8).sin() * half_depth * 0.45
                    + half_depth * 0.25;
                spawn_blob(
                    commands,
                    circle,
                    &water_material,
                    Vec2::new(x, y),
                    rng.range(0.7, 1.1),
                    LAYER_WATER,
                    1.0,
                );
            }
        }
        Biome::Beach => {
            // Shoreline: a band of overlapping water blobs along the east edge.
            let steps = 16;
            for step in 0..steps {
                let t = step as f32 / (steps - 1) as f32;
                let y = center.y - half_depth + t * half_depth * 2.0;
                let x = center.x + half_width * 0.72 + (t * 9.0).sin() * 0.6;
                spawn_blob(
                    commands,
                    circle,
                    &water_material,
                    Vec2::new(x, y),
                    rng.range(1.6, 2.4),
                    LAYER_WATER,
                    1.0,
                );
            }
        }
        _ => {
            // Pond: a cluster of overlapping blobs off to one side.
            let pond_center = center + Vec2::new(half_width * 0.55, half_depth * 0.55);
            for _ in 0..5 {
                let offset = Vec2::new(rng.range(-1.2, 1.2), rng.range(-0.9, 0.9));
                spawn_blob(
                    commands,
                    circle,
                    &water_material,
                    pond_center + offset,
                    rng.range(1.0, 2.0),
                    LAYER_WATER,
                    rng.range(0.75, 1.0),
                );
            }
        }
    }
}

// ── Systems ──────────────────────────────────────────────────────────────────

/// Drift swaying entities around their base position (wind through trees,
/// fireflies wandering). The "dynamic" in dynamic landscape.
pub(crate) fn sway_vegetation(time: Res<Time>, mut swayers: Query<(&Sway, &mut Transform)>) {
    let elapsed = time.elapsed_secs();
    for (sway, mut transform) in &mut swayers {
        let offset = Vec2::new(
            (elapsed * 0.45 + sway.phase).sin(),
            (elapsed * 0.31 + sway.phase * 1.7).cos() * 0.6,
        ) * sway.amplitude;
        transform.translation.x = sway.base.x + offset.x;
        transform.translation.y = sway.base.y + offset.y;
    }
}

/// `B` cycles biome, `N` toggles day/night: retint the world and rebuild decor.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_theme_keys(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut theme: ResMut<LandscapeTheme>,
    mut clear_color: ResMut<ClearColor>,
    mut commands: Commands,
    decor: Query<Entity, With<LandscapeDecor>>,
    mut floor: Query<&mut Sprite, With<FloorSprite>>,
    description: Res<SceneDescription>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let mut changed = false;
    if keyboard.just_pressed(KeyCode::KeyB) {
        theme.biome = theme.biome.next();
        changed = true;
    }
    if keyboard.just_pressed(KeyCode::KeyN) {
        theme.time_of_day = theme.time_of_day.toggled();
        changed = true;
    }
    if !changed {
        return;
    }

    apply_theme(
        &mut commands,
        &mut clear_color,
        &decor,
        &mut floor,
        &description,
        &mut meshes,
        &mut materials,
        *theme,
    );
    info!(
        "Landscape theme: {:?} / {:?}",
        theme.biome, theme.time_of_day
    );
}

/// Retint the world for `theme` and rebuild the decorative landscape.
/// Shared by the theme hotkeys and the automated screenshot tour.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_theme(
    commands: &mut Commands,
    clear_color: &mut ClearColor,
    decor: &Query<Entity, With<LandscapeDecor>>,
    floor: &mut Query<&mut Sprite, With<FloorSprite>>,
    description: &SceneDescription,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<ColorMaterial>,
    theme: LandscapeTheme,
) {
    let palette = theme.palette();
    clear_color.0 = palette.background;
    for mut sprite in floor.iter_mut() {
        sprite.color = palette.floor;
    }
    for entity in decor.iter() {
        commands.entity(entity).despawn();
    }
    spawn_landscape(commands, meshes, materials, description, theme);
}
