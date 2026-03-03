use std::path::Path;
use std::sync::Arc;

use atrium::audio::decode::decode_file;
use atrium::audio::output::{AudioOutput, CpalOutput};
use atrium::engine::commands::Command;
use atrium::engine::scene::AudioScene;
use atrium::spatial::listener::Listener;
use atrium::spatial::source::TestNode;
use atrium::world::room::BoxRoom;
use atrium::world::types::Vec3;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Decode audio asset
    let audio_path = Path::new("assets/djembe.mp3");
    println!("Loading {}...", audio_path.display());
    let buffer = Arc::new(decode_file(audio_path)?);
    println!(
        "Loaded: {:.1}s of audio at {}Hz",
        buffer.samples.len() as f32 / buffer.sample_rate as f32,
        buffer.sample_rate
    );

    // 2. Create the command queue
    let (producer, consumer) = rtrb::RingBuffer::<Command>::new(256);

    // 3. Build the scene
    let room = BoxRoom::new(6.0, 4.0, 3.0); // 6x4x3m room
    let listener_pos = Vec3::new(3.0, 2.0, 0.0); // center of room
    let listener = Listener::new(listener_pos, 0.0); // facing +X

    let test_node = TestNode::new(
        buffer,
        listener_pos, // orbit around listener
        1.5,          // 1.5m orbit radius
        1.0,          // 1.0 rad/s (~6.3 second orbit)
    );

    let scene = AudioScene {
        listener,
        sources: vec![Box::new(test_node)],
        room: Box::new(room),
        master_gain: 0.7,
        sample_rate: 0.0, // set by output backend
    };

    // 4. Start audio output
    let _handle = CpalOutput.start(scene, consumer)?;

    // 5. Report
    println!();
    println!("=== Atrium Phase 1 ===");
    println!("Room: 6x4m");
    println!("Listener: center ({}, {})", listener_pos.x, listener_pos.y);
    println!("TestNode: djembe orbiting at 1.5m radius, ~6.3s per orbit");
    println!();
    println!("You should hear the djembe rotating around you in stereo.");
    println!("Press Ctrl+C to stop.");

    // Keep the producer alive (future: use for WebSocket commands)
    let _producer = producer;

    // Block until interrupted
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
