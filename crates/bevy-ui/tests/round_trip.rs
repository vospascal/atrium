//! Round-trip test: SceneDescription → (simulated ECS) → export → SceneDescription.
//!
//! Verifies that export_scene produces a SceneDescription that matches the input,
//! exercising the coordinate conversion (atrium_to_bevy → bevy_to_atrium) and
//! color conversion (hex → float → hex) paths.

use atrium_bevy::ecs::*;
use atrium_bevy::scene::export::export_scene;
use atrium_bevy::scene::schema::*;
use bevy::math::Vec3;

fn sample_description() -> SceneDescription {
    SceneDescription {
        version: 1,
        environment: EnvironmentDescription {
            width: 20.0,
            depth: 15.0,
            height: 8.0,
            spawn: [2.0, 3.0, 0.0],
        },
        atrium: AtriumDescription {
            width: 6.0,
            depth: 4.0,
            height: 3.0,
        },
        listener: ListenerDescription {
            position: [0.0, 0.0, 1.5],
            yaw_degrees: 90.0,
        },
        sources: vec![
            SourceDescription {
                id: "source_0".into(),
                name: "Djembe".into(),
                color: "#ff6b35".into(),
                position: [3.0, 2.0, 0.0],
                spl: 75.0,
                ref_distance: 1.0,
                directivity: "omni".into(),
                directivity_alpha: 1.0,
                spread: 0.0,
                orbit_radius: 2.5,
                orbit_speed: 0.8,
            },
            SourceDescription {
                id: "source_1".into(),
                name: "Campfire".into(),
                color: "#4ecdc4".into(),
                position: [-1.0, 4.0, 0.0],
                spl: 55.0,
                ref_distance: 1.0,
                directivity: "polar".into(),
                directivity_alpha: 0.5,
                spread: 0.3,
                orbit_radius: 0.0,
                orbit_speed: 0.0,
            },
        ],
        speakers: SpeakerLayoutDescription {
            layout: "stereo".into(),
            speakers: vec![
                SpeakerDescription {
                    id: "fl".into(),
                    label: "FL".into(),
                    position: [-1.5, 2.0, 1.2],
                    channel: 0,
                },
                SpeakerDescription {
                    id: "fr".into(),
                    label: "FR".into(),
                    position: [1.5, 2.0, 1.2],
                    channel: 1,
                },
            ],
            dbap_rolloff_db: 6.0,
        },
        render_mode: "vbap".into(),
        master_gain: 0.8,
        distance_model: DistanceModelDescription {
            model: "inverse".into(),
            ref_distance: 1.0,
            max_distance: 20.0,
            rolloff: 1.0,
        },
        atmosphere: AtmosphereDescription {
            temperature_c: 22.0,
            humidity_pct: 55.0,
            pressure_kpa: 101.325,
        },
    }
}

/// Simulate what atrium_to_bevy does: Bevy.X = Atrium.X, Bevy.Y = Atrium.Z, Bevy.Z = -Atrium.Y
fn atrium_to_bevy(pos: [f32; 3]) -> Vec3 {
    Vec3::new(pos[0], pos[2], -pos[1])
}

/// Build the intermediate data that would exist in ECS, then export it.
fn round_trip(input: &SceneDescription) -> SceneDescription {
    // Simulate what import::spawn_scene does: convert positions to Bevy coords
    let source_data: Vec<(SoundSourceIndex, SoundSource, Vec3)> = input
        .sources
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let rgb = parse_hex_color(&s.color);
            (
                SoundSourceIndex(i),
                SoundSource {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    color: rgb,
                    spl: s.spl,
                    ref_distance: s.ref_distance,
                    directivity: s.directivity.clone(),
                    directivity_alpha: s.directivity_alpha,
                    spread: s.spread,
                    orbit_radius: s.orbit_radius,
                    orbit_speed: s.orbit_speed,
                },
                atrium_to_bevy(s.position),
            )
        })
        .collect();

    let speaker_data: Vec<(SoundSpeaker, Vec3)> = input
        .speakers
        .speakers
        .iter()
        .map(|s| {
            (
                SoundSpeaker {
                    id: s.id.clone(),
                    label: s.label.clone(),
                    channel: s.channel,
                },
                atrium_to_bevy(s.position),
            )
        })
        .collect();

    let listener_component = SoundListener {
        id: "listener".into(),
        yaw_degrees: input.listener.yaw_degrees,
    };
    let listener_pos = atrium_to_bevy(input.listener.position);

    let env = SoundEnvironment {
        id: "environment".into(),
        width: input.environment.width,
        depth: input.environment.depth,
        height: input.environment.height,
        spawn: input.environment.spawn,
    };

    let atr = SoundAtrium {
        id: "atrium".into(),
        width: input.atrium.width,
        depth: input.atrium.depth,
        height: input.atrium.height,
    };

    export_scene(
        input,
        &source_data,
        &speaker_data,
        Some((&listener_component, listener_pos)),
        Some(&env),
        Some(&atr),
    )
}

#[test]
fn round_trip_preserves_structure() {
    let input = sample_description();
    let output = round_trip(&input);

    assert_eq!(output.version, input.version);
    assert_eq!(output.render_mode, input.render_mode);
    assert_eq!(output.master_gain, input.master_gain);
    assert_eq!(output.sources.len(), input.sources.len());
    assert_eq!(
        output.speakers.speakers.len(),
        input.speakers.speakers.len()
    );
    assert_eq!(output.speakers.layout, input.speakers.layout);
}

#[test]
fn round_trip_preserves_positions() {
    let input = sample_description();
    let output = round_trip(&input);

    // Positions go through atrium→bevy→atrium, must survive exactly
    for (src_in, src_out) in input.sources.iter().zip(output.sources.iter()) {
        assert_eq!(
            src_in.position, src_out.position,
            "source {} position",
            src_in.name
        );
    }

    for (spk_in, spk_out) in input
        .speakers
        .speakers
        .iter()
        .zip(output.speakers.speakers.iter())
    {
        assert_eq!(
            spk_in.position, spk_out.position,
            "speaker {} position",
            spk_in.label
        );
    }

    assert_eq!(
        input.listener.position, output.listener.position,
        "listener position"
    );
    assert_eq!(
        input.listener.yaw_degrees, output.listener.yaw_degrees,
        "listener yaw"
    );
}

#[test]
fn round_trip_preserves_source_properties() {
    let input = sample_description();
    let output = round_trip(&input);

    for (src_in, src_out) in input.sources.iter().zip(output.sources.iter()) {
        assert_eq!(src_in.id, src_out.id);
        assert_eq!(src_in.name, src_out.name);
        assert_eq!(src_in.spl, src_out.spl);
        assert_eq!(src_in.ref_distance, src_out.ref_distance);
        assert_eq!(src_in.directivity, src_out.directivity);
        assert_eq!(src_in.directivity_alpha, src_out.directivity_alpha);
        assert_eq!(src_in.spread, src_out.spread);
        assert_eq!(src_in.orbit_radius, src_out.orbit_radius);
        assert_eq!(src_in.orbit_speed, src_out.orbit_speed);
    }
}

#[test]
fn round_trip_preserves_colors() {
    let input = sample_description();
    let output = round_trip(&input);

    // Hex → float → hex may lose sub-u8 precision, but should round-trip
    // through the same u8 quantization consistently.
    for (src_in, src_out) in input.sources.iter().zip(output.sources.iter()) {
        assert_eq!(
            src_in.color.to_lowercase(),
            src_out.color.to_lowercase(),
            "color for source {}",
            src_in.name,
        );
    }
}

#[test]
fn round_trip_preserves_environment() {
    let input = sample_description();
    let output = round_trip(&input);

    assert_eq!(input.environment.width, output.environment.width);
    assert_eq!(input.environment.depth, output.environment.depth);
    assert_eq!(input.environment.height, output.environment.height);
    assert_eq!(input.environment.spawn, output.environment.spawn);
    assert_eq!(input.atrium.width, output.atrium.width);
    assert_eq!(input.atrium.depth, output.atrium.depth);
    assert_eq!(input.atrium.height, output.atrium.height);
}

#[test]
fn round_trip_preserves_atmosphere() {
    let input = sample_description();
    let output = round_trip(&input);

    assert_eq!(
        input.atmosphere.temperature_c,
        output.atmosphere.temperature_c
    );
    assert_eq!(
        input.atmosphere.humidity_pct,
        output.atmosphere.humidity_pct
    );
    assert_eq!(
        input.atmosphere.pressure_kpa,
        output.atmosphere.pressure_kpa
    );
}

#[test]
fn round_trip_json_serialization() {
    let input = sample_description();
    let output = round_trip(&input);

    // Both should produce identical JSON
    let input_json = serde_json::to_string_pretty(&input).unwrap();
    let output_json = serde_json::to_string_pretty(&output).unwrap();
    assert_eq!(input_json, output_json, "JSON round-trip mismatch");
}
