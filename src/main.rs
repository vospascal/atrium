use std::path::Path;
use std::sync::Arc;

use atrium::audio::decode::decode_file;
use atrium::audio::mixer::DistanceModel;
use atrium::audio::output::{AudioOutput, CpalOutput};
use atrium::engine::commands::Command;
use atrium::engine::scene::AudioScene;
use atrium::spatial::listener::Listener;
use atrium::spatial::source::TestNode;
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

    // Campfire: wider orbit, slower, opposite direction
    let mut campfire_node = TestNode::new(
        campfire,
        listener_pos,
        2.5,   // 2.5m radius — further out, so distance attenuation is audible
        -0.6,  // negative = counter-clockwise, slower
    );
    campfire_node.amplitude = 0.8;

    let scene = AudioScene {
        listener,
        sources: vec![Box::new(djembe_node), Box::new(campfire_node)],
        room: Box::new(room),
        master_gain: 0.7,
        sample_rate: 0.0, // set by output backend
        distance_model: DistanceModel::default(),
    };

    // 4. Start audio output
    let _handle = CpalOutput.start(scene, consumer)?;

    // 5. Report
    println!();
    println!("=== Atrium Phase 2 ===");
    println!("Room: 6x4m | Distance attenuation: inverse (ref=1m, max=10m)");
    println!("Listener: center ({}, {})", listener_pos.x, listener_pos.y);
    println!("Sources:");
    println!("  - Djembe:   1.5m radius, clockwise,        ~6.3s orbit");
    println!("  - Campfire: 2.5m radius, counter-clockwise, ~10.5s orbit");
    println!();
    println!("You should hear two sources orbiting independently in stereo.");
    println!("The campfire is further out — notice it's quieter due to distance.");
    println!("Press Ctrl+C to stop.");

    // Keep the producer alive (future: use for WebSocket commands)
    let _producer = producer;

    // Block until interrupted
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
