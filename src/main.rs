use std::path::Path;
use std::sync::Arc;

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
use atrium::synth::rain_v2::RainSourceV2;
use atrium::world::ray::RayPool;
use atrium::world::room::BoxRoom;
use atrium::world::types::Vec3;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse rain intensity from CLI: cargo run -- light|medium|heavy|0.5
    let intensity = match std::env::args().nth(1).as_deref() {
        Some("light") => 0.2,
        Some("heavy") => 0.9,
        Some("medium") | None => 0.5,
        Some(s) => s.parse::<f32>().unwrap_or(0.5).clamp(0.0, 1.0),
    };

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

    // Campfire: wider orbit, slower, opposite direction
    let mut campfire_node = TestNode::new(
        campfire,
        listener_pos,
        2.5,   // 2.5m radius — further out, so distance attenuation is audible
        -0.6,  // negative = counter-clockwise, slower
    );
    campfire_node.amplitude = 0.8;

    // Rain v2: physically-based (impact + Minnaert bubble per drop)
    let mut rain = RainSourceV2::new(
        Vec3::new(3.0, 2.0, 0.5), // just overhead — close to listener
        intensity,
        0xDEAD_BEEF,               // PRNG seed
    );
    rain.master_gain = 3.0;

    let scene = AudioScene {
        listener,
        sources: vec![
            // Box::new(djembe_node),
            // Box::new(campfire_node),
            Box::new(rain),
        ],
        room: Box::new(room),
        master_gain: 1.0,
        sample_rate: 0.0, // set by output backend
        distance_model: DistanceModel::default(),
        processors: vec![
            // Disabled for rain audition — re-enable for full scene
            // Box::new(EarlyReflections::new(0.5, 0.9)),
            // Box::new(FdnReverb::new(0.2, 0.8, 0.3)),
        ],
        ray_pool: RayPool::new(),
    };

    // 4. Start audio output
    let _handle = CpalOutput.start(scene, consumer)?;

    // 5. Report
    println!();
    println!("=== Rain v2 Audition ===");
    println!("Intensity: {intensity} ({})", match intensity {
        i if i <= 0.3 => "light",
        i if i <= 0.7 => "medium",
        _ => "heavy",
    });
    println!("Room: 6x4x3m | Reverb: FDN");
    println!();
    println!("Usage: cargo run -- light|medium|heavy|0.0-1.0");
    println!("Press Ctrl+C to stop.");

    // Keep the producer alive (future: use for WebSocket commands)
    let _producer = producer;

    // Block until interrupted
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
