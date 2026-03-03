use std::path::Path;
use std::sync::{Arc, Mutex};

use atrium::audio::decode::decode_file;
use atrium::audio::mixer::DistanceModel;
use atrium::audio::output::{AudioOutput, CpalOutput};
use atrium::engine::commands::Command;
use atrium::engine::scene::AudioScene;
use atrium::processors::early_reflections::EarlyReflections;
use atrium::processors::fdn_reverb::FdnReverb;
use atrium::spatial::directivity::DirectivityPattern;
use atrium::spatial::listener::Listener;
use atrium::spatial::source::TestNode;
use atrium::world::ray::RayPool;
use atrium::server::websocket::run_server;
use atrium::world::room::BoxRoom;
use atrium::world::types::Vec3;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Decode audio assets
    let djembe = Arc::new(decode_file(Path::new("assets/djembe.mp3"))?);
    let campfire = Arc::new(decode_file(Path::new("assets/campfire.mp3"))?);

    // 2. Create the command queue
    let (producer, consumer) = rtrb::RingBuffer::<Command>::new(256);

    // 3. Build the scene
    let room = BoxRoom::new(6.0, 4.0, 3.0); // 6x4x3m room
    let listener_pos = Vec3::new(3.0, 2.0, 0.0); // center of room
    let listener = Listener::new(listener_pos, 0.0); // facing +X

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

    let scene = AudioScene {
        listener,
        sources: vec![
            Box::new(djembe_node),
            Box::new(campfire_node),
        ],
        room: Box::new(room),
        master_gain: 1.0,
        sample_rate: 0.0, // set by output backend
        distance_model: DistanceModel::default(),
        processors: vec![
            Box::new(EarlyReflections::new(0.5, 0.9)),
            Box::new(FdnReverb::new(0.2, 0.8, 0.3)),
        ],
        ray_pool: RayPool::new(),
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
    run_server("0.0.0.0:8080", producer)?;

    Ok(())
}
