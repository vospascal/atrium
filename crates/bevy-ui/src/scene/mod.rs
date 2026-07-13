//! Scene setup, 2D top-down sound emission map, and real-time updates.
//!
//! Reads `SceneDescription` to spawn the map entities (source badges, speaker
//! dots, listener badge, procedural landscape) plus screen-space info cards,
//! then updates them each frame from telemetry. Dynamic overlays — icon
//! glyphs, intensity ripples, dashed connection lines, listener halo,
//! directivity, room outline — are drawn with 2D gizmos.

pub mod export;
pub(crate) mod icons;
pub mod import;
pub(crate) mod landscape;
pub mod reload;
pub mod save;
pub mod schema;

pub use schema::SceneDescription;

use std::f32::consts::{PI, TAU};

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use atrium_core::commands::Command;
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3 as AtriumVec3;

use crate::camera::TopDownCamera;
use crate::ecs::*;
use crate::telemetry::{LatestTelemetry, TelemetryMessage};
use atrium_behavior::CommandSender;
use icons::SourceIcon;
use landscape::LandscapeTheme;

// ── Draw-order layers (z in the 2D plane; higher = in front) ─────────────────

pub(crate) const LAYER_SPEAKER: f32 = 5.0;
pub(crate) const LAYER_SOURCE: f32 = 10.0;
pub(crate) const LAYER_LISTENER: f32 = 15.0;

/// Radius of the source/listener badge discs (meters, world space).
pub(crate) const BADGE_RADIUS: f32 = 0.36;

// ── Rendering-only markers (not scene data) ─────────────────────────────────

/// Root node of a source's screen-space info card.
#[derive(Component)]
pub(crate) struct SourceCard {
    pub index: usize,
}

/// The live "distance  bearing  level" text inside a source card.
#[derive(Component)]
pub(crate) struct SourceCardMetrics {
    pub index: usize,
}

/// The "Listener" pill below the listener badge.
#[derive(Component)]
pub(crate) struct ListenerTag;

/// Marker for a screen-space label that tracks a speaker's position.
#[derive(Component)]
pub(crate) struct SpeakerLabel {
    pub channel: usize,
}

/// Marker for the listener ear labels ("L" / "R").
#[derive(Component)]
pub(crate) struct EarLabel {
    /// true = right ear, false = left ear
    pub is_right: bool,
}

// ── Coordinate mapping ───────────────────────────────────────────────────────
//
// Atrium: X = left/right, Y = front/back, Z = up/down (height).
// World (Bevy 2D): X = right, Y = up (= atrium front). Height (Z) is flattened;
// the top-down view is purely a ground-plane schematic.

/// Atrium [x, y, z] → world ground-plane position (height dropped).
pub fn atrium_to_world(position: [f32; 3]) -> Vec2 {
    Vec2::new(position[0], position[1])
}

// ── Setup ────────────────────────────────────────────────────────────────────

pub fn setup_scene(
    mut commands: Commands,
    description: Res<SceneDescription>,
    theme: Res<LandscapeTheme>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    import::spawn_scene(
        &mut commands,
        &mut meshes,
        &mut materials,
        &description,
        *theme,
    );
    landscape::spawn_landscape(
        &mut commands,
        &mut meshes,
        &mut materials,
        &description,
        *theme,
    );
    info!("Theme keys: B = cycle biome, N = toggle day/night");
}

// ── Per-frame updates ────────────────────────────────────────────────────────

/// Update source positions from telemetry (skips sources being dragged).
pub(crate) fn update_sources(
    mut sources: Query<(&SoundSourceIndex, &mut Transform)>,
    mut messages: MessageReader<TelemetryMessage>,
    drag: Res<SourceDragState>,
) {
    let Some(msg) = messages.read().last() else {
        return;
    };
    let frame = &msg.frame;

    for (index, mut transform) in &mut sources {
        if drag.dragging == Some(index.0) {
            continue;
        }
        if index.0 < frame.source_count as usize {
            let source = &frame.sources[index.0];
            let target = atrium_to_world([source.x, source.y, source.z]);
            let current = transform.translation.truncate();
            let next = current.lerp(target, 0.3);
            transform.translation.x = next.x;
            transform.translation.y = next.y;
        }
    }
}

/// Latest gain for a source, treating muted sources as silent.
fn telemetry_gain(telemetry: &LatestTelemetry, index: usize) -> f32 {
    let frame = &telemetry.frame;
    if index >= frame.source_count as usize {
        return 0.2;
    }
    let source = &frame.sources[index];
    if source.is_muted {
        0.0
    } else {
        source.gain_total.clamp(0.0, 1.0)
    }
}

// ── Gizmo overlays ───────────────────────────────────────────────────────────

/// Draw the environment and atrium outlines on the ground plane.
///
/// Both are fixed in world space, matching the audio engine: `SetListenerPose`
/// moves only the listener, never the speakers, so the physical atrium room
/// (and its speakers) stays put while the listener moves within it. The atrium
/// outline is anchored at the spawn point — the world position the physical
/// room is centered on (speaker positions were placed relative to it).
///
/// In HRTF the atrium square is hidden alongside the speaker dots (headphones
/// use no speaker room). The environment square stays — the virtual acoustic
/// space is still simulated on headphones (reflections + reverb).
pub(crate) fn draw_room_bounds(
    mut gizmos: Gizmos,
    description: Res<SceneDescription>,
    theme: Res<LandscapeTheme>,
    telemetry: Res<LatestTelemetry>,
) {
    let outline = theme.palette().outline;
    let env = &description.environment;
    gizmos.rect_2d(
        Isometry2d::from_translation(Vec2::new(env.width * 0.5, env.depth * 0.5)),
        Vec2::new(env.width, env.depth),
        outline.with_alpha(0.25),
    );

    if telemetry.frame.render_mode == RenderMode::Hrtf {
        return;
    }
    let atrium = &description.atrium;
    gizmos.rect_2d(
        Isometry2d::from_translation(atrium_to_world(env.spawn)),
        Vec2::new(atrium.width, atrium.depth),
        outline.with_alpha(0.55),
    );
}

/// Draw a dashed line whose dashes march from `from` toward `to`.
fn draw_dashed_line_2d(
    gizmos: &mut Gizmos,
    from: Vec2,
    to: Vec2,
    dash: f32,
    gap: f32,
    march_offset: f32,
    color: Color,
) {
    let delta = to - from;
    let length = delta.length();
    if length < 1e-3 {
        return;
    }
    let direction = delta / length;
    let period = dash + gap;
    // Start one period early so dashes flow in from the source end.
    let mut start = (march_offset % period) - period;
    while start < length {
        let segment_start = start.max(0.0);
        let segment_end = (start + dash).min(length);
        if segment_end > segment_start {
            gizmos.line_2d(
                from + direction * segment_start,
                from + direction * segment_end,
                color,
            );
        }
        start += period;
    }
}

/// Dashed connection lines from each source badge to the listener badge.
/// Dashes march toward the listener (sound traveling); alpha tracks gain.
pub(crate) fn draw_source_links(
    mut gizmos: Gizmos,
    time: Res<Time>,
    theme: Res<LandscapeTheme>,
    sources: Query<(&SoundSourceIndex, &Transform), With<SoundSource>>,
    listener: Query<&Transform, With<SoundListener>>,
    telemetry: Res<LatestTelemetry>,
) {
    let Ok(listener_transform) = listener.single() else {
        return;
    };
    let listener_position = listener_transform.translation.truncate();
    let link_color = theme.palette().link_line;
    let march = time.elapsed_secs() * 0.9;

    for (index, source_transform) in &sources {
        let gain = telemetry_gain(&telemetry, index.0);
        let source_position = source_transform.translation.truncate();

        let delta = listener_position - source_position;
        let distance = delta.length();
        // Trim so the line starts at the badge halo and stops at the listener rings.
        let start_trim = BADGE_RADIUS + 0.18;
        let end_trim = BADGE_RADIUS + 0.45;
        if distance <= start_trim + end_trim {
            continue;
        }
        let direction = delta / distance;
        draw_dashed_line_2d(
            &mut gizmos,
            source_position + direction * start_trim,
            listener_position - direction * end_trim,
            0.26,
            0.20,
            march,
            link_color.with_alpha(0.15 + gain * 0.55),
        );
    }
}

/// Animated intensity ripples: expanding rings around each source badge whose
/// speed, reach, and brightness track the source's current gain.
pub(crate) fn draw_source_ripples(
    mut gizmos: Gizmos,
    time: Res<Time>,
    sources: Query<(&SoundSourceIndex, &SoundSource, &Transform)>,
    telemetry: Res<LatestTelemetry>,
) {
    const RIPPLES: usize = 3;
    let elapsed = time.elapsed_secs();

    for (index, source, transform) in &sources {
        let gain = telemetry_gain(&telemetry, index.0);
        let center = transform.translation.truncate();
        let speed = 0.35 + gain * 0.45;
        let reach = 0.7 + gain * 1.3;

        for ripple in 0..RIPPLES {
            let progress = (elapsed * speed + ripple as f32 / RIPPLES as f32).fract();
            let radius = BADGE_RADIUS + 0.15 + progress * reach;
            let alpha = (1.0 - progress).powi(2) * (0.08 + gain * 0.45);
            let color = Color::srgba(source.color[0], source.color[1], source.color[2], alpha);
            gizmos.circle_2d(center, radius, color);
        }
    }
}

/// Draw each source's icon glyph on its badge.
pub(crate) fn draw_source_icons(
    mut gizmos: Gizmos,
    theme: Res<LandscapeTheme>,
    sources: Query<(&SourceIcon, &Transform)>,
) {
    let icon_color = theme.palette().icon;
    for (icon, transform) in &sources {
        icons::draw_icon(
            &mut gizmos,
            icon.0,
            transform.translation.truncate(),
            BADGE_RADIUS * 0.8,
            icon_color,
        );
    }
}

/// Speaker visuals that depend on the active render mode:
///  - HRTF is headphone-only (no speakers used), so hide the speaker dots.
///  - Otherwise, glow each speaker by its real output level (`channel_peaks`),
///    which is what actually shows how the mode routes to the speakers: VBAP
///    lights the active pair, DBAP/Ambisonics light them all, WorldLocked
///    lights by proximity, and masked channels (e.g. quad) stay dark.
pub(crate) fn update_speaker_visuals(
    mut gizmos: Gizmos,
    telemetry: Res<LatestTelemetry>,
    theme: Res<LandscapeTheme>,
    mut speakers: Query<
        (&SoundSpeaker, &Transform, &mut Visibility),
        Without<landscape::FloorSprite>,
    >,
    mut floor: Query<&mut Visibility, (With<landscape::FloorSprite>, Without<SoundSpeaker>)>,
) {
    let frame = &telemetry.frame;
    let hrtf = frame.render_mode == RenderMode::Hrtf;
    let glow = theme.palette().listener_ring;

    // The atrium floor fill represents the physical speaker room — hide it in
    // HRTF alongside the speakers and the atrium outline.
    for mut floor_visibility in &mut floor {
        *floor_visibility = if hrtf {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
    }

    for (speaker, transform, mut visibility) in &mut speakers {
        *visibility = if hrtf {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
        if hrtf {
            continue;
        }

        let peak = frame
            .channel_peaks
            .get(speaker.channel)
            .copied()
            .unwrap_or(0.0);
        // Map peak amplitude to a 0..1 "activity" on a dB scale (−50 dB floor),
        // so relative speaker use is visible even when absolute output is quiet.
        let activity = if peak > 1e-5 {
            ((20.0 * peak.log10() + 50.0) / 50.0).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if activity <= 0.02 {
            continue;
        }
        let center = transform.translation.truncate();
        gizmos.circle_2d(
            center,
            0.20 + activity * 0.30,
            glow.with_alpha(0.2 + activity * 0.6),
        );
    }
}

/// Draw the audible radius (SPL threshold) as a faint ring per source.
pub(crate) fn draw_audible_rings(mut gizmos: Gizmos, sources: Query<(&SoundSource, &Transform)>) {
    const SPL_THRESHOLD: f32 = 20.0;

    for (source, transform) in &sources {
        let db_above = source.spl - SPL_THRESHOLD;
        if db_above <= 0.0 {
            continue;
        }
        let radius = source.ref_distance * 10.0_f32.powf(db_above / 20.0);
        let color = Color::srgba(source.color[0], source.color[1], source.color[2], 0.10);
        gizmos.circle_2d(transform.translation.truncate(), radius, color);
    }
}

/// Draw directivity polar patterns around each source.
pub(crate) fn draw_directivity_patterns(
    mut gizmos: Gizmos,
    sources: Query<(&SoundSourceIndex, &SoundSource, &Transform)>,
    mut messages: MessageReader<TelemetryMessage>,
) {
    const SEGMENTS: usize = 48;
    const PATTERN_RADIUS: f32 = 1.2;
    let latest = messages.read().last();

    for (index, source, transform) in &sources {
        if source.directivity == "omni" {
            continue;
        }
        let color = Color::srgba(source.color[0], source.color[1], source.color[2], 0.6);

        let (orientation_x, orientation_y) = latest
            .and_then(|msg| {
                (index.0 < msg.frame.source_count as usize).then(|| {
                    let s = &msg.frame.sources[index.0];
                    (s.orientation_x, s.orientation_y)
                })
            })
            .unwrap_or((1.0, 0.0));
        let source_yaw = orientation_y.atan2(orientation_x);

        let center = transform.translation.truncate();
        let mut points = Vec::with_capacity(SEGMENTS + 1);
        for step in 0..=SEGMENTS {
            let theta = (step as f32 / SEGMENTS as f32) * TAU - PI;
            let gain = pattern_gain(&source.directivity, source.directivity_alpha, theta.abs());
            let radius = gain * PATTERN_RADIUS;
            let angle = source_yaw + theta;
            points.push(center + Vec2::new(angle.cos(), angle.sin()) * radius);
        }
        gizmos.linestrip_2d(points, color);

        // Forward pointer.
        let forward_gain = pattern_gain(&source.directivity, source.directivity_alpha, 0.0);
        let forward =
            center + Vec2::new(source_yaw.cos(), source_yaw.sin()) * forward_gain * PATTERN_RADIUS;
        gizmos.line_2d(center, forward, color);
    }
}

/// Evaluate directivity gain at an angle off the forward axis.
fn pattern_gain(directivity: &str, alpha: f32, angle: f32) -> f32 {
    match directivity {
        "omni" => 1.0,
        "polar" | "cardioid" | "supercardioid" => (alpha + (1.0 - alpha) * angle.cos()).max(0.0),
        _ => 1.0,
    }
}

/// Concentric halo rings around the listener (gently pulsing) and the
/// "person" glyph on the listener badge.
pub(crate) fn draw_listener_rings(
    mut gizmos: Gizmos,
    time: Res<Time>,
    theme: Res<LandscapeTheme>,
    listener: Query<&Transform, With<SoundListener>>,
) {
    let Ok(listener_transform) = listener.single() else {
        return;
    };
    let center = listener_transform.translation.truncate();
    let palette = theme.palette();
    let elapsed = time.elapsed_secs();

    for (ring, radius) in [0.65_f32, 1.05, 1.5, 2.0].into_iter().enumerate() {
        let pulse = 1.0 + 0.02 * (elapsed * 1.3 + ring as f32 * 0.7).sin();
        let alpha = 0.26 - ring as f32 * 0.06;
        gizmos.circle_2d(
            center,
            radius * pulse,
            palette.listener_ring.with_alpha(alpha),
        );
    }

    // Person glyph: head + shoulder arc.
    let icon_color = palette.icon;
    let head_center = center + Vec2::new(0.0, 0.10);
    gizmos.circle_2d(head_center, 0.085, icon_color);
    gizmos.circle_2d(head_center, 0.045, icon_color);

    const ARC_STEPS: usize = 12;
    let shoulder_center = center + Vec2::new(0.0, -0.26);
    let shoulder_radius = 0.24;
    let (start_angle, end_angle) = (30.0_f32.to_radians(), 150.0_f32.to_radians());
    let mut points = Vec::with_capacity(ARC_STEPS + 2);
    for step in 0..=ARC_STEPS {
        let t = step as f32 / ARC_STEPS as f32;
        let angle = start_angle + t * (end_angle - start_angle);
        points.push(shoulder_center + Vec2::new(angle.cos(), angle.sin()) * shoulder_radius);
    }
    // Close the base of the shoulders.
    points.push(points[0]);
    gizmos.linestrip_2d(points, icon_color);
}

/// Draw the listener's facing cone (inner + outer wedges) on the ground plane.
pub(crate) fn draw_listener_direction(
    mut gizmos: Gizmos,
    listener: Query<&Transform, With<SoundListener>>,
    state: Res<crate::camera::ListenerState>,
    theme: Res<LandscapeTheme>,
) {
    let Ok(listener_transform) = listener.single() else {
        return;
    };
    let center = listener_transform.translation.truncate();
    let yaw = state.yaw;
    let ring_color = theme.palette().listener_ring;

    let cone_length = 1.5;
    let inner = 15.0_f32.to_radians();
    let outer = 45.0_f32.to_radians();
    let inner_color = ring_color.with_alpha(0.55);
    let outer_color = ring_color.with_alpha(0.22);

    let dir = |angle: f32| Vec2::new(angle.cos(), angle.sin());

    gizmos.line_2d(center, center + dir(yaw) * cone_length, inner_color);
    for sign in [-1.0_f32, 1.0] {
        gizmos.line_2d(
            center,
            center + dir(yaw + sign * inner) * cone_length,
            inner_color,
        );
        gizmos.line_2d(
            center,
            center + dir(yaw + sign * outer) * cone_length,
            outer_color,
        );
    }

    // Inner arc cap.
    const ARC: usize = 12;
    let mut prev = None;
    for step in 0..=ARC {
        let t = step as f32 / ARC as f32;
        let angle = yaw - inner + t * 2.0 * inner;
        let point = center + dir(angle) * cone_length;
        if let Some(p) = prev {
            gizmos.line_2d(p, point, inner_color);
        }
        prev = Some(point);
    }
}

// ── Screen-space info cards & labels ─────────────────────────────────────────

/// Position each source's info card next to its badge and refresh the live
/// "distance  bearing  level" line from telemetry.
pub(crate) fn update_source_cards(
    camera: Query<(&Camera, &GlobalTransform), With<TopDownCamera>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    sources: Query<(&SoundSourceIndex, &GlobalTransform)>,
    listener_state: Res<crate::camera::ListenerState>,
    telemetry: Res<LatestTelemetry>,
    mut cards: Query<(&SourceCard, &mut Node)>,
    mut metrics: Query<(&SourceCardMetrics, &mut Text)>,
) {
    let Ok((camera, camera_global)) = camera.single() else {
        return;
    };
    let window_width = windows.single().map(|window| window.width()).unwrap_or(0.0);
    let listener_position = Vec2::new(listener_state.position[0], listener_state.position[1]);
    let frame = &telemetry.frame;

    for (card, mut node) in &mut cards {
        let Some((_, source_global)) = sources.iter().find(|(idx, _)| idx.0 == card.index) else {
            continue;
        };
        let Ok(viewport) = camera.world_to_viewport(camera_global, source_global.translation())
        else {
            node.display = Display::None;
            continue;
        };
        node.display = Display::Flex;
        // Card sits on the side of the badge facing away from the listener so
        // clustered sources don't stack their cards; screen edges override.
        let mut place_right = source_global.translation().x >= listener_position.x;
        if window_width > 0.0 {
            if viewport.x > window_width - 300.0 {
                place_right = false;
            } else if viewport.x < 260.0 {
                place_right = true;
            }
        }
        node.left = Val::Px(if place_right {
            viewport.x + 30.0
        } else {
            viewport.x - 230.0
        });
        node.top = Val::Px(viewport.y - 26.0);
    }

    for (metric, mut text) in &mut metrics {
        let Some((_, source_global)) = sources.iter().find(|(idx, _)| idx.0 == metric.index) else {
            continue;
        };
        let source_position = source_global.translation().truncate();
        let delta = source_position - listener_position;

        // Compass bearing from the listener: north (+Y) = 0°, clockwise.
        let mut bearing = delta.x.atan2(delta.y).to_degrees();
        if bearing < 0.0 {
            bearing += 360.0;
        }

        let (distance, level) = if metric.index < frame.source_count as usize {
            let source = &frame.sources[metric.index];
            let level = if source.is_muted {
                "muted".to_string()
            } else if source.gain_total <= 1e-4 {
                "silent".to_string()
            } else {
                format!("{:+.1} dB", source.gain_db.clamp(-99.9, 20.0))
            };
            (source.distance, level)
        } else {
            (delta.length(), "—".to_string())
        };

        // "deg" instead of "°": Bevy's default font subset renders ° as tofu.
        **text = format!("{distance:.1} m   {bearing:.0} deg   {level}");
    }
}

/// Keep the "Listener" pill centered below the listener badge.
pub(crate) fn update_listener_tag(
    camera: Query<(&Camera, &GlobalTransform), With<TopDownCamera>>,
    listener: Query<&GlobalTransform, With<SoundListener>>,
    mut tags: Query<&mut Node, With<ListenerTag>>,
) {
    let Ok((camera, camera_global)) = camera.single() else {
        return;
    };
    let Ok(listener_global) = listener.single() else {
        return;
    };
    for mut node in &mut tags {
        place_label(
            camera,
            camera_global,
            listener_global.translation(),
            &mut node,
            36.0,
            -30.0,
        );
    }
}

/// Keep speaker/ear label colors readable when the theme changes: they sit on
/// the landscape (unlike cards, which have their own dark background), so they
/// take the palette's line color — light at night, dark by day.
pub(crate) fn retint_labels_on_theme_change(
    theme: Res<LandscapeTheme>,
    mut speaker_labels: Query<&mut TextColor, With<SpeakerLabel>>,
    mut ear_labels: Query<&mut TextColor, (With<EarLabel>, Without<SpeakerLabel>)>,
) {
    if !theme.is_changed() {
        return;
    }
    let color = theme.palette().link_line;
    for mut text_color in &mut speaker_labels {
        text_color.0 = color.with_alpha(0.85);
    }
    for mut text_color in &mut ear_labels {
        text_color.0 = color.with_alpha(0.9);
    }
}

/// Position speaker labels above their world positions (hidden in HRTF, which
/// uses headphones rather than the speakers).
pub(crate) fn billboard_speaker_labels(
    camera: Query<(&Camera, &GlobalTransform), With<TopDownCamera>>,
    speakers: Query<(&SoundSpeaker, &GlobalTransform)>,
    telemetry: Res<LatestTelemetry>,
    mut labels: Query<(&SpeakerLabel, &mut Node)>,
) {
    let Ok((camera, camera_global)) = camera.single() else {
        return;
    };
    let hrtf = telemetry.frame.render_mode == RenderMode::Hrtf;
    for (label, mut node) in &mut labels {
        if hrtf {
            node.display = Display::None;
            continue;
        }
        let Some((_, speaker_global)) = speakers.iter().find(|(s, _)| s.channel == label.channel)
        else {
            continue;
        };
        place_label(
            camera,
            camera_global,
            speaker_global.translation(),
            &mut node,
            10.0,
            20.0,
        );
    }
}

/// Position "L"/"R" labels at the listener's ears (rotated with facing).
pub(crate) fn update_ear_labels(
    camera: Query<(&Camera, &GlobalTransform), With<TopDownCamera>>,
    listener: Query<&Transform, With<SoundListener>>,
    state: Res<crate::camera::ListenerState>,
    mut labels: Query<(&EarLabel, &mut Node)>,
) {
    let Ok((camera, camera_global)) = camera.single() else {
        return;
    };
    let Ok(listener_transform) = listener.single() else {
        return;
    };
    let center = listener_transform.translation.truncate();
    let facing = Vec2::new(state.yaw.cos(), state.yaw.sin());
    // Right of facing (clockwise 90°): (x, y) → (y, -x).
    let right = Vec2::new(facing.y, -facing.x);
    let ear_offset = 0.55;

    for (ear, mut node) in &mut labels {
        let sign = if ear.is_right { 1.0 } else { -1.0 };
        let world = center + right * sign * ear_offset;
        place_label(
            camera,
            camera_global,
            world.extend(LAYER_LISTENER),
            &mut node,
            6.0,
            8.0,
        );
    }
}

/// Shared helper: project a world point to the viewport and place a UI node,
/// or hide it if off-screen/behind the camera.
fn place_label(
    camera: &Camera,
    camera_global: &GlobalTransform,
    world: Vec3,
    node: &mut Node,
    offset_x: f32,
    offset_y: f32,
) {
    if let Ok(viewport) = camera.world_to_viewport(camera_global, world) {
        node.left = Val::Px(viewport.x - offset_x);
        node.top = Val::Px(viewport.y - offset_y);
        node.display = Display::Flex;
    } else {
        node.display = Display::None;
    }
}

// ── Source dragging ─────────────────────────────────────────────────────────

/// Tracks which source is being dragged (if any).
#[derive(Resource, Default)]
pub(crate) struct SourceDragState {
    pub dragging: Option<usize>,
}

const PICK_RADIUS: f32 = 40.0;

/// Left-click to pick a source, drag to move it on the ground plane.
pub(crate) fn drag_sources(
    camera: Query<(&Camera, &GlobalTransform), With<TopDownCamera>>,
    mut sources: Query<(&SoundSourceIndex, &mut Transform, &AtriumHeight)>,
    mut drag: ResMut<SourceDragState>,
    mut selected: ResMut<crate::synth_panel::SelectedSource>,
    mut command_sender: ResMut<CommandSender>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok((camera, camera_global)) = camera.single() else {
        return;
    };
    let Ok(window) = windows.single() else {
        return;
    };

    if mouse_buttons.just_released(MouseButton::Left) {
        drag.dragging = None;
        return;
    }
    let Some(cursor) = window.cursor_position() else {
        return;
    };

    if mouse_buttons.just_pressed(MouseButton::Left) {
        let mut best = PICK_RADIUS;
        let mut picked = None;
        for (index, transform, _) in &sources {
            if let Ok(screen) = camera.world_to_viewport(camera_global, transform.translation) {
                let dist = screen.distance(cursor);
                if dist < best {
                    best = dist;
                    picked = Some(index.0);
                }
            }
        }
        drag.dragging = picked;
        // Clicking a source selects it for the live synth panel (sticky).
        if picked.is_some() {
            selected.0 = picked;
        }
    }

    let Some(dragging) = drag.dragging else {
        return;
    };
    if !mouse_buttons.pressed(MouseButton::Left) {
        drag.dragging = None;
        return;
    }

    let Ok(world) = camera.viewport_to_world_2d(camera_global, cursor) else {
        return;
    };
    for (index, mut transform, height) in &mut sources {
        if index.0 == dragging {
            transform.translation.x = world.x;
            transform.translation.y = world.y;
            command_sender.send(Command::SetSourcePosition {
                index: dragging as u16,
                position: AtriumVec3::new(world.x, world.y, height.0),
            });
            break;
        }
    }
}
