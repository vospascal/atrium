use std::path::Path;
use std::sync::{Arc, Mutex};

use atrium::audio::decode::decode_file;
use atrium::audio::atmosphere::AtmosphericParams;
use atrium::audio::mixer::{DistanceModel, MixerState};
use atrium::audio::output::{AudioOutput, CpalOutput};
use atrium::engine::commands::Command;
use atrium::engine::scene::AudioScene;
use atrium::processors::early_reflections::EarlyReflections;
use atrium::processors::fdn_reverb::FdnReverb;
use atrium::spatial::directivity::DirectivityPattern;
use atrium::spatial::listener::Listener;
use atrium::spatial::source::TestNode;

use atrium::server::websocket::run_server;
use atrium::world::room::BoxRoom;
use atrium::world::types::Vec3;
use atrium_core::speaker::SpeakerLayout;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Decode audio assets
    let djembe = Arc::new(decode_file(Path::new("assets/djembe.mp3"))?);
    let campfire = Arc::new(decode_file(Path::new("assets/campfire.mp3"))?);
    let purring = Arc::new(decode_file(Path::new("assets/purring.mp3"))?);

    // 2. Create the command queue
    let (producer, consumer) = rtrb::RingBuffer::<Command>::new(256);

    // 3. Build the scene
    let room = BoxRoom::new(6.0, 4.0, 3.0); // 6x4x3m room
    let listener_pos = Vec3::new(3.0, 2.0, 0.0); // center of room
    let listener = Listener::new(listener_pos, std::f32::consts::FRAC_PI_2); // facing +Y (toward center speaker)

    // Djembe: close orbit, moderate speed
    let mut djembe_node = TestNode::new(
        djembe,
        listener_pos,
        1.5,  // 1.5m radius
        1.0,  // 1.0 rad/s (~6.3s orbit)
    );
    djembe_node.amplitude = 0.6;
    djembe_node.pattern = DirectivityPattern::cardioid();

    // Campfire: stationary, placed in corner
    let mut campfire_node = TestNode::new(
        campfire,
        Vec3::new(1.0, 1.0, 0.0), // near corner of room
        0.0,   // stationary
        0.0,
    );
    campfire_node.amplitude = 0.8;
    campfire_node.spread = 0.3; // wider image — campfire is not a point source

    // Purring cat: stationary, opposite corner from campfire
    let mut purring_node = TestNode::new(
        purring,
        Vec3::new(5.0, 3.0, 0.0), // front-right area
        0.0,   // stationary
        0.0,
    );
    purring_node.amplitude = 0.5;

    // Speaker layout: 5.1 surround with speakers at room corners + front center.
    // Audience faces front wall (+Y). Left = -X (x=0), Right = +X (x=6).
    //   FL ──── C ──── FR     (front wall, y=4)
    //   │              │
    //   │              │      6×4m room
    //   │              │
    //   RL ─────────── RR     (rear wall, y=0)
    let speaker_layout = SpeakerLayout::surround_5_1(
        Vec3::new(0.0, 4.0, 0.0),  // FL: front-left  (x=0)
        Vec3::new(6.0, 4.0, 0.0),  // FR: front-right (x=6)
        Vec3::new(3.0, 4.0, 0.0),  // C:  front-center
        Vec3::new(0.0, 0.0, 0.0),  // RL: rear-left   (x=0)
        Vec3::new(6.0, 0.0, 0.0),  // RR: rear-right  (x=6)
    );

    // Build speaker layout JSON for the UI (before scene takes ownership)
    let channel_labels = ["FL", "FR", "C", "LFE", "RL", "RR"];
    let speaker_json = {
        let mut speakers = Vec::new();
        for i in 0..speaker_layout.speaker_count() {
            if let Some(sp) = speaker_layout.speaker_by_index(i) {
                let label = channel_labels.get(sp.channel).unwrap_or(&"?");
                speakers.push(serde_json::json!({
                    "label": label,
                    "x": sp.position.x,
                    "y": sp.position.y,
                    "z": sp.position.z,
                    "channel": sp.channel,
                }));
            }
        }
        serde_json::json!({
            "type": "speaker_layout",
            "speakers": speakers,
            "total_channels": speaker_layout.total_channels(),
            "lfe_channel": speaker_layout.lfe_channel(),
        }).to_string()
    };

    let num_sources = 3;
    let scene = AudioScene {
        listener,
        sources: vec![
            Box::new(djembe_node),
            Box::new(campfire_node),
            Box::new(purring_node),
        ],
        room: Box::new(room),
        master_gain: 1.0,
        sample_rate: 0.0, // set by output backend
        distance_model: DistanceModel::default(),
        processors: vec![
            Box::new(EarlyReflections::new(0.5, 0.9)),
            Box::new(FdnReverb::new(0.2, 0.8, 0.3)),
        ],
        speaker_layout,
        mixer_state: MixerState::new(num_sources),
        atmosphere: AtmosphericParams::default(),
    };

    // 4. Start audio output
    let _handle = CpalOutput.start(scene, consumer)?;

    // 5. Report
    println!();
    println!("=== Atrium Spatial Audio ===");
    println!("Room: 6x4x3m | Reverb: FDN 8-line");
    println!();

    // 6. Start WebSocket server (blocks on main thread, keeps _handle alive)
    let producer = Arc::new(Mutex::new(producer));
    run_server("0.0.0.0:3333", producer, speaker_json)?;

    Ok(())
}
