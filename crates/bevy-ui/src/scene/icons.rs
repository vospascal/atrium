//! Schematic source icons, drawn as gizmo polylines on the badge circles.
//!
//! Each source gets an `IconKind` inferred from its name/id at import time
//! (a campfire shows a flame, a cricket shows a bug, …). Glyphs are hand-coded
//! polylines in a unit space of roughly [-0.7, 0.7], scaled at draw time.

use bevy::prelude::*;

/// Which glyph a source badge shows. Inferred once at import.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct SourceIcon(pub IconKind);

#[derive(Clone, Copy, Debug)]
pub(crate) enum IconKind {
    Bird,
    Frog,
    Water,
    Waterfall,
    Wind,
    Fire,
    Insect,
    Cat,
    Drum,
    Note,
}

impl IconKind {
    /// Keyword-match the source name/id. Order matters: "waterfall" must win
    /// over "water", "seagull" must hit Bird before "sea" hits Water.
    pub fn infer(id: &str, name: &str) -> Self {
        let text = format!("{} {}", id.to_lowercase(), name.to_lowercase());
        let matches = |keywords: &[&str]| keywords.iter().any(|keyword| text.contains(keyword));

        if matches(&["waterfall"]) {
            IconKind::Waterfall
        } else if matches(&["fire", "flame", "camp"]) {
            IconKind::Fire
        } else if matches(&["bird", "gull", "crow", "owl", "sparrow"]) {
            IconKind::Bird
        } else if matches(&["frog", "toad"]) {
            IconKind::Frog
        } else if matches(&["cricket", "cicada", "insect", "bug", "bee"]) {
            IconKind::Insect
        } else if matches(&["cat", "purr", "kitten"]) {
            IconKind::Cat
        } else if matches(&["djembe", "drum", "conga", "bongo"]) {
            IconKind::Drum
        } else if matches(&["wind", "breeze", "leaves", "tree"]) {
            IconKind::Wind
        } else if matches(&[
            "stream", "river", "creek", "water", "rain", "wave", "ocean", "sea",
        ]) {
            IconKind::Water
        } else {
            IconKind::Note
        }
    }
}

/// Draw one glyph centered at `center`, `scale` = glyph half-height in meters.
pub(crate) fn draw_icon(
    gizmos: &mut Gizmos,
    kind: IconKind,
    center: Vec2,
    scale: f32,
    color: Color,
) {
    let stroke = |gizmos: &mut Gizmos, raw: &[(f32, f32)]| {
        let points: Vec<Vec2> = raw
            .iter()
            .map(|&(x, y)| center + Vec2::new(x, y) * scale)
            .collect();
        gizmos.linestrip_2d(points, color);
    };
    let ring = |gizmos: &mut Gizmos, offset: (f32, f32), radius: f32| {
        gizmos.circle_2d(
            center + Vec2::new(offset.0, offset.1) * scale,
            radius * scale,
            color,
        );
    };

    match kind {
        IconKind::Fire => {
            // Teardrop with a concave lick on the left edge.
            stroke(
                gizmos,
                &[
                    (0.04, 0.60),
                    (0.20, 0.36),
                    (0.32, 0.10),
                    (0.34, -0.16),
                    (0.22, -0.38),
                    (0.0, -0.48),
                    (-0.22, -0.38),
                    (-0.34, -0.14),
                    (-0.28, 0.08),
                    (-0.14, 0.02),
                    (-0.20, 0.26),
                    (-0.08, 0.38),
                    (-0.12, 0.50),
                    (0.04, 0.60),
                ],
            );
            // Inner tongue.
            stroke(
                gizmos,
                &[
                    (0.0, 0.14),
                    (0.10, 0.0),
                    (0.13, -0.14),
                    (0.06, -0.26),
                    (0.0, -0.29),
                    (-0.06, -0.26),
                    (-0.13, -0.14),
                    (-0.10, 0.0),
                    (0.0, 0.14),
                ],
            );
        }
        IconKind::Bird => {
            // Side-profile songbird: body, head, beak, wing, and feet.
            stroke(
                gizmos,
                &[
                    (-0.58, -0.06),
                    (-0.30, 0.10),
                    (-0.16, 0.30),
                    (0.12, 0.26),
                    (0.31, 0.08),
                    (0.32, -0.16),
                    (0.12, -0.34),
                    (-0.22, -0.32),
                    (-0.45, -0.18),
                    (-0.58, -0.06),
                ],
            );
            ring(gizmos, (0.28, 0.26), 0.15);
            stroke(gizmos, &[(0.40, 0.29), (0.62, 0.22), (0.41, 0.16)]);
            stroke(gizmos, &[(-0.23, 0.10), (0.06, 0.04), (0.17, -0.20)]);
            stroke(gizmos, &[(-0.02, -0.32), (-0.08, -0.53), (-0.19, -0.57)]);
            stroke(gizmos, &[(0.12, -0.32), (0.14, -0.52), (0.03, -0.57)]);
        }
        IconKind::Frog => {
            ring(gizmos, (-0.28, 0.32), 0.14);
            ring(gizmos, (0.28, 0.32), 0.14);
            stroke(
                gizmos,
                &[
                    (-0.50, 0.18),
                    (-0.48, -0.15),
                    (-0.25, -0.34),
                    (0.0, -0.38),
                    (0.25, -0.34),
                    (0.48, -0.15),
                    (0.50, 0.18),
                ],
            );
            stroke(gizmos, &[(-0.26, -0.10), (0.0, -0.18), (0.26, -0.10)]);
        }
        IconKind::Water => {
            for wave_y in [0.16, -0.16] {
                let points: Vec<(f32, f32)> = (0..=12)
                    .map(|step| {
                        let x = -0.55 + step as f32 / 12.0 * 1.10;
                        (x, wave_y + 0.10 * (x * 9.0).sin())
                    })
                    .collect();
                stroke(gizmos, &points);
            }
        }
        IconKind::Waterfall => {
            stroke(gizmos, &[(-0.55, 0.42), (-0.05, 0.42)]);
            for x in [-0.32, -0.08, 0.16] {
                stroke(gizmos, &[(x, 0.40), (x + 0.08, -0.30)]);
            }
            stroke(
                gizmos,
                &[
                    (-0.45, -0.46),
                    (-0.22, -0.38),
                    (0.02, -0.46),
                    (0.26, -0.38),
                    (0.50, -0.46),
                ],
            );
        }
        IconKind::Wind => {
            stroke(
                gizmos,
                &[
                    (-0.55, 0.28),
                    (0.05, 0.28),
                    (0.30, 0.34),
                    (0.38, 0.46),
                    (0.28, 0.54),
                    (0.18, 0.48),
                ],
            );
            stroke(
                gizmos,
                &[
                    (-0.60, 0.0),
                    (0.25, 0.0),
                    (0.48, 0.04),
                    (0.56, 0.16),
                    (0.48, 0.26),
                    (0.38, 0.20),
                ],
            );
            stroke(
                gizmos,
                &[
                    (-0.50, -0.28),
                    (0.15, -0.28),
                    (0.32, -0.34),
                    (0.38, -0.44),
                    (0.30, -0.52),
                    (0.20, -0.46),
                ],
            );
        }
        IconKind::Insect => {
            ring(gizmos, (0.0, -0.04), 0.22);
            // Antennae.
            stroke(gizmos, &[(0.10, 0.16), (0.26, 0.46)]);
            stroke(gizmos, &[(-0.10, 0.16), (-0.26, 0.46)]);
            // Legs, three per side.
            for (leg_y, reach) in [(0.08, 0.14), (-0.04, 0.02), (-0.16, -0.14)] {
                stroke(gizmos, &[(0.20, leg_y), (0.48, reach)]);
                stroke(gizmos, &[(-0.20, leg_y), (-0.48, reach)]);
            }
        }
        IconKind::Cat => {
            ring(gizmos, (0.0, 0.02), 0.30);
            // Ears.
            stroke(gizmos, &[(-0.26, 0.18), (-0.34, 0.50), (-0.06, 0.32)]);
            stroke(gizmos, &[(0.26, 0.18), (0.34, 0.50), (0.06, 0.32)]);
            // Whiskers.
            stroke(gizmos, &[(-0.30, -0.02), (-0.56, 0.04)]);
            stroke(gizmos, &[(-0.30, -0.10), (-0.56, -0.12)]);
            stroke(gizmos, &[(0.30, -0.02), (0.56, 0.04)]);
            stroke(gizmos, &[(0.30, -0.10), (0.56, -0.12)]);
        }
        IconKind::Drum => {
            // Djembe goblet.
            stroke(
                gizmos,
                &[
                    (-0.38, 0.30),
                    (0.38, 0.30),
                    (0.28, -0.02),
                    (0.16, -0.20),
                    (0.16, -0.44),
                    (-0.16, -0.44),
                    (-0.16, -0.20),
                    (-0.28, -0.02),
                    (-0.38, 0.30),
                ],
            );
            stroke(
                gizmos,
                &[
                    (-0.38, 0.30),
                    (-0.20, 0.42),
                    (0.0, 0.46),
                    (0.20, 0.42),
                    (0.38, 0.30),
                ],
            );
        }
        IconKind::Note => {
            stroke(gizmos, &[(0.14, 0.48), (0.14, -0.16)]);
            stroke(gizmos, &[(0.14, 0.48), (0.38, 0.32), (0.34, 0.12)]);
            ring(gizmos, (0.02, -0.26), 0.13);
            ring(gizmos, (0.02, -0.26), 0.06);
        }
    }
}
