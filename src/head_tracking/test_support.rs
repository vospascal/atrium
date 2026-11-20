//! Helpers shared by the head tracking tests: a stand-in tracker that sends
//! OpenTrack packets over loopback, and a reader for the command ring.

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use atrium_core::commands::Command;

/// One OpenTrack packet: six little-endian `f64`, `x, y, z, yaw, pitch, roll`.
pub(super) fn packet(values: [f64; 6]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// A port nothing is bound to, found by letting the system pick one.
pub(super) fn free_port() -> u16 {
    UdpSocket::bind(("127.0.0.1", 0))
        .and_then(|socket| socket.local_addr())
        .expect("loopback is available")
        .port()
}

/// Send one head turn to a listener on `port`.
pub(super) fn send_yaw(port: u16, yaw_degrees: f64) {
    let sender = UdpSocket::bind(("127.0.0.1", 0)).expect("loopback is available");
    sender
        .send_to(
            &packet([0.0, 0.0, 0.0, yaw_degrees, 0.0, 0.0]),
            ("127.0.0.1", port),
        )
        .expect("the packet is sent");
}

/// The next command on the ring, waiting up to two seconds for it.
pub(super) fn next_command(consumer: &mut rtrb::Consumer<Command>) -> Command {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(command) = consumer.pop() {
            return command;
        }
        assert!(Instant::now() < deadline, "no command arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
}
