//! The virtual acoustic space a scene lives in: dimensions, ground, and the
//! per-wall materials that drive reflections and reverb.

use serde::Deserialize;

/// Environment geometry + acoustic properties (loaded from `environments/*.yaml`).
/// Defines the virtual acoustic space where sources live — dimensions, wall
/// materials, ground surface, and broadband reflectivity.
#[derive(Deserialize)]
pub(super) struct EnvironmentConfig {
    pub width: f32,
    pub depth: f32,
    pub height: f32,
    /// Spawn point — where the atrium center is placed within the environment.
    /// Scene positions (listener, sources, speakers) are atrium-local and get
    /// offset by this point to become world coordinates.
    /// Defaults to the center of the environment: [width/2, depth/2, 0].
    pub spawn: Option<[f32; 3]>,
    /// Ground surface factor for ISO 9613-2 ground effect (0.0 = hard, 1.0 = porous).
    /// Default: 0.0 (hard reflective floor like concrete or tile).
    #[serde(default)]
    pub ground_factor: f32,
    /// Material of each face. Reflections and the reverb's decay time both
    /// come from these, so the two always describe the same room.
    #[serde(default)]
    pub walls: WallsConfig,
    /// Explicit late-reverb decay (mid-band RT60, seconds). When set, overrides
    /// the geometry/material-derived Sabine decay in the FDN — lets you dial a
    /// target "atrium tail" (e.g. 3.5 s) that room dimensions alone can't hit.
    #[serde(default)]
    pub rt60_seconds: Option<f32>,
    /// Explicit late-reverb pre-delay (milliseconds). When set, overrides the
    /// mean-free-path pre-delay (e.g. 20 ms for clarity before the wash).
    #[serde(default)]
    pub pre_delay_ms: Option<f32>,
}

/// Per-wall material configuration. Each wall can specify a material name
/// (e.g. "stone", "wood", "open"). Unspecified walls use `default`.
#[derive(Deserialize)]
pub(super) struct WallsConfig {
    /// Fallback material for walls not individually specified.
    #[serde(default = "default_wall_name")]
    pub default: String,
    pub floor: Option<String>,   // -Z
    pub ceiling: Option<String>, // +Z
    pub north: Option<String>,   // +Y
    pub south: Option<String>,   // -Y
    pub east: Option<String>,    // +X
    pub west: Option<String>,    // -X
}

impl Default for WallsConfig {
    fn default() -> Self {
        Self {
            default: default_wall_name(),
            floor: None,
            ceiling: None,
            north: None,
            south: None,
            east: None,
            west: None,
        }
    }
}

fn default_wall_name() -> String {
    "hard_wall".into()
}

/// Map a material name string to a `WallMaterial` preset.
fn parse_wall_material(name: &str) -> crate::pipeline::wall_material::WallMaterial {
    use crate::pipeline::wall_material::WallMaterial;
    match name {
        "hard_wall" => WallMaterial::hard_wall(),
        "stone" => WallMaterial::stone(),
        "wood" => WallMaterial::wood(),
        "glass" => WallMaterial::glass(),
        "carpet" => WallMaterial::carpet(),
        "ceiling_tile" => WallMaterial::ceiling_tile(),
        "grass" => WallMaterial::grass(),
        "open" => WallMaterial::open(),
        other => {
            eprintln!("warning: unknown wall material '{other}', using hard_wall");
            WallMaterial::hard_wall()
        }
    }
}

/// Build the 6-wall material array from an `EnvironmentConfig`.
/// Order: [-X (west), +X (east), -Y (south), +Y (north), -Z (floor), +Z (ceiling)].
pub(super) fn build_wall_materials(
    env: &EnvironmentConfig,
) -> [crate::pipeline::wall_material::WallMaterial; 6] {
    let default = &env.walls.default;
    let get = |wall: &Option<String>| -> crate::pipeline::wall_material::WallMaterial {
        parse_wall_material(wall.as_deref().unwrap_or(default))
    };
    [
        get(&env.walls.west),    // -X
        get(&env.walls.east),    // +X
        get(&env.walls.south),   // -Y
        get(&env.walls.north),   // +Y
        get(&env.walls.floor),   // -Z
        get(&env.walls.ceiling), // +Z
    ]
}

#[cfg(test)]
mod tests {
    use atrium_core::types::Vec3;

    use crate::config::build::load_yaml;

    use super::*;

    /// The default scene must load a *reflective* environment. This guards the
    /// root-cause fix: the old default (`riverside`) was an open field whose
    /// walls reflect nothing, so the engine produced dry panning with no tail.
    #[test]
    fn atrium_environment_is_reflective_unlike_open_field() {
        let atrium: EnvironmentConfig =
            load_yaml("environments/atrium.yaml").expect("atrium.yaml should load");
        let riverside: EnvironmentConfig =
            load_yaml("environments/riverside.yaml").expect("riverside.yaml should load");

        // Every atrium wall reflects a meaningful fraction of energy.
        let atrium_walls = build_wall_materials(&atrium);
        for (i, wall) in atrium_walls.iter().enumerate() {
            assert!(
                wall.broadband_reflection_gain() > 0.5,
                "atrium wall {i} should be reflective, got gain {}",
                wall.broadband_reflection_gain()
            );
        }

        // The open field's default walls reflect essentially nothing.
        let riverside_walls = build_wall_materials(&riverside);
        assert!(
            riverside_walls[0].broadband_reflection_gain() < 0.01,
            "riverside default walls should be non-reflecting (open air), got {}",
            riverside_walls[0].broadband_reflection_gain()
        );

        // The atrium exposes explicit tail knobs; the open field leaves them unset.
        assert_eq!(atrium.rt60_seconds, Some(1.6));
        assert_eq!(atrium.pre_delay_ms, Some(12.0));
        assert_eq!(riverside.rt60_seconds, None);
    }

    /// Outdoors the ground is modelled by the ISO 9613-2 ground effect, which
    /// already contains the bounce off the ground. So the outdoor environments
    /// must give the floor no image (no face reflects at all), or that bounce
    /// is rendered twice and `ground_factor` is switched off. Both are porous
    /// ground: G = 1.
    #[test]
    fn outdoor_environments_leave_the_ground_to_the_iso_ground_effect() {
        for path in ["environments/meadow.yaml", "environments/riverside.yaml"] {
            let environment: EnvironmentConfig = load_yaml(path).expect("environment loads");
            for (wall_index, wall) in build_wall_materials(&environment).iter().enumerate() {
                assert_eq!(
                    wall.broadband_reflection_gain(),
                    0.0,
                    "{path}: face {wall_index} ({}) must not reflect",
                    wall.name
                );
            }
            assert_eq!(environment.ground_factor, 1.0, "{path}: porous ground");
        }
    }

    /// The YAML wall names and the image-source walls must agree on which
    /// face is which: west −X (0), east +X (1), south −Y (2), north +Y (3),
    /// floor −Z (4), ceiling +Z (5). Checked through the resolver itself: the
    /// reflection whose image lies beyond +X must carry the *east* material's
    /// broadband gain √(1 − mean α₂₅₀…₄ₖ).
    #[test]
    fn wall_names_map_to_the_image_source_walls() {
        use crate::pipeline::environment::Environment;
        use crate::pipeline::path::{PathKind, PathResolver, PathSet, ResolveContext};
        use crate::pipeline::path_resolvers::ImageSourceResolver;
        use crate::pipeline::wall_material::WallMaterial;

        let environment: EnvironmentConfig = serde_yaml::from_str(
            "width: 10\ndepth: 8\nheight: 4\n\
             walls:\n  default: hard_wall\n  west: stone\n  east: wood\n  south: glass\n  \
             north: carpet\n  floor: grass\n  ceiling: ceiling_tile\n",
        )
        .unwrap();
        let materials = build_wall_materials(&environment);
        let names: Vec<&str> = materials.iter().map(|wall| wall.name).collect();
        assert_eq!(
            names,
            ["stone", "wood", "glass", "carpet", "grass", "ceiling_tile"]
        );

        let resolver = ImageSourceResolver;
        let resolve_environment = Environment {
            wall_materials: materials,
            ..Environment::anechoic(Vec3::ZERO, Vec3::new(10.0, 8.0, 4.0))
        };
        let context = ResolveContext {
            source_pos: Vec3::new(4.0, 3.0, 1.5),
            target_pos: Vec3::new(6.0, 5.0, 1.2),
            environment: &resolve_environment,
        };
        let mut paths = PathSet::new();
        resolver.resolve(&context, &mut paths);
        let expected_axis = [
            (0, Vec3::new(-1.0, 0.0, 0.0), WallMaterial::stone()),
            (1, Vec3::new(1.0, 0.0, 0.0), WallMaterial::wood()),
            (2, Vec3::new(0.0, -1.0, 0.0), WallMaterial::glass()),
            (3, Vec3::new(0.0, 1.0, 0.0), WallMaterial::carpet()),
            (4, Vec3::new(0.0, 0.0, -1.0), WallMaterial::grass()),
            (5, Vec3::new(0.0, 0.0, 1.0), WallMaterial::ceiling_tile()),
        ];
        for (wall_index, outward, material) in expected_axis {
            let path = paths
                .as_slice()
                .iter()
                .find(|path| {
                    path.kind == PathKind::Reflection && path.wall_index == Some(wall_index)
                })
                .unwrap_or_else(|| panic!("no reflection off wall {wall_index}"));
            assert!(
                path.direction.dot(outward) > 0.0,
                "wall {wall_index}: image direction {:?} does not point through {outward:?}",
                path.direction
            );
            assert!(
                (path.gain - material.broadband_reflection_gain()).abs() < 1e-6,
                "wall {wall_index}: gain {} is not {}'s {}",
                path.gain,
                material.name,
                material.broadband_reflection_gain()
            );
        }
    }

    /// Material names resolve to octave-band absorption rows (125 Hz–4 kHz).
    /// Glass and wood are the standard textbook rows ("glass, ordinary
    /// window"; "wood floor"); the others are pinned as shipped. `open` is
    /// fully absorptive, so its reflection gain is exactly 0 and the FDN gate
    /// treats an all-open environment as non-reverberant.
    #[test]
    fn wall_material_names_resolve_to_their_absorption_rows() {
        let rows: [(&str, [f32; 6]); 8] = [
            ("hard_wall", [0.02, 0.02, 0.03, 0.04, 0.05, 0.05]),
            ("stone", [0.02, 0.02, 0.03, 0.03, 0.04, 0.05]),
            ("wood", [0.15, 0.11, 0.10, 0.07, 0.06, 0.07]),
            ("glass", [0.35, 0.25, 0.18, 0.12, 0.07, 0.04]),
            ("carpet", [0.02, 0.04, 0.08, 0.20, 0.35, 0.40]),
            ("ceiling_tile", [0.20, 0.40, 0.70, 0.80, 0.60, 0.40]),
            ("grass", [0.10, 0.20, 0.40, 0.55, 0.60, 0.60]),
            ("open", [1.0; 6]),
        ];
        for (name, alpha) in rows {
            let material = parse_wall_material(name);
            assert_eq!(material.name, name);
            assert_eq!(material.alpha, alpha, "{name}");
        }
        assert_eq!(parse_wall_material("open").broadband_reflection_gain(), 0.0);
        // Unknown names fall back to hard_wall (with a warning on stderr).
        assert_eq!(parse_wall_material("marble").name, "hard_wall");
    }
}
