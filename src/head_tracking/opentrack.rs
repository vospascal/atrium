//! OpenTrack UDP listener: decodes head angles and steers the listener.
//!
//! Packet: exactly 48 bytes, six little-endian `f64` — `x, y, z` (cm) then
//! `yaw, pitch, roll` (degrees). Only yaw is used: the listener model has a
//! single heading (`Listener::yaw`), so tilt and roll are dropped until it
//! gains pitch and roll.

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use atrium_core::commands::Command;

/// OpenTrack's own default input port; every tracker bridge defaults to it.
pub(super) const DEFAULT_PORT: u16 = 4242;

/// How long one read waits before the thread checks whether it should stop.
/// It is also the longest that dropping an [`OpenTrackListener`] blocks.
const STOP_CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// Silence after which the tracker is reported as gone.
const SILENCE_BEFORE_HOLDING: Duration = Duration::from_secs(2);

/// Size of one OpenTrack UDP packet: six `f64`.
const PACKET_BYTES: usize = 6 * std::mem::size_of::<f64>();

/// Direction convention between the tracker and the listener model.
///
/// `Listener::yaw` grows counter-clockwise seen from above (0 = facing +X,
/// π/2 = facing +Y), so turning the head LEFT must increase it. This constant
/// maps the tracker's yaw onto that: −1.0 means the tracker reports a left turn
/// as negative degrees. Verified by ear 2026-11-17 with a Sony WH-1000XM6 via
/// `sony-head-tracker` (default axis map): in `scenes/frog.yaml` the
/// front-right frog moves to straight ahead on a right turn. A tracker that
/// turns the world the wrong way has the opposite convention — re-run that test.
const TRACKER_YAW_SIGN: f32 = -1.0;

/// Head angles carried by one tracker packet, in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HeadAngles {
    yaw_degrees: f64,
    pitch_degrees: f64,
    roll_degrees: f64,
}

/// Decode one OpenTrack UDP packet. Returns `None` for any other size or for
/// non-finite angles, so a stray or corrupt packet can never move the listener.
fn decode_opentrack_packet(packet: &[u8]) -> Option<HeadAngles> {
    if packet.len() != PACKET_BYTES {
        return None;
    }
    let value = |index: usize| {
        let start = index * 8;
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&packet[start..start + 8]);
        f64::from_le_bytes(bytes)
    };
    let angles = HeadAngles {
        yaw_degrees: value(3),
        pitch_degrees: value(4),
        roll_degrees: value(5),
    };
    let all_finite = angles.yaw_degrees.is_finite()
        && angles.pitch_degrees.is_finite()
        && angles.roll_degrees.is_finite();
    all_finite.then_some(angles)
}

/// Listener heading for a head turn: the scene's heading (which way the room
/// faces when the head is centred) plus the tracked turn. Radians.
fn listener_yaw(scene_yaw_radians: f32, tracker_yaw_degrees: f64) -> f32 {
    scene_yaw_radians + TRACKER_YAW_SIGN * (tracker_yaw_degrees as f32).to_radians()
}

/// The running listener thread. Dropping it stops the thread and frees the
/// port before the drop returns, so a new listener can bind it right away.
pub(super) struct OpenTrackListener {
    stop: Arc<AtomicBool>,
    receiving: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl OpenTrackListener {
    /// Whether a tracker is sending: a valid packet arrived within the last
    /// two seconds.
    pub(super) fn is_receiving(&self) -> bool {
        self.receiving.load(Ordering::Relaxed)
    }
}

impl Drop for OpenTrackListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            // The thread owns the socket; joining is what frees the port.
            let _ = thread.join();
        }
    }
}

/// Start the head tracking listener on its own thread.
///
/// Binds `127.0.0.1:port` (loopback only — a tracker on the network is not
/// trusted to steer the listener) and sends a `SetListenerPose` for every
/// valid packet: where `placement` puts the listener, heading = its heading +
/// head turn. Returns an error only if the port cannot be bound.
pub(super) fn spawn(
    port: u16,
    producer: Arc<Mutex<rtrb::Producer<Command>>>,
    placement: super::Placement,
) -> std::io::Result<OpenTrackListener> {
    let socket = UdpSocket::bind(("127.0.0.1", port))?;
    socket.set_read_timeout(Some(STOP_CHECK_INTERVAL))?;
    println!("Head tracking: listening for OpenTrack UDP on 127.0.0.1:{port}");

    let stop = Arc::new(AtomicBool::new(false));
    let stop_requested = stop.clone();
    let receiving = Arc::new(AtomicBool::new(false));
    let receiving_now = receiving.clone();
    let thread = std::thread::Builder::new()
        .name("head-tracking".into())
        .spawn(move || {
            let mut buffer = [0u8; 64];
            // When the newest valid packet arrived; `None` while no tracker
            // is sending.
            let mut last_packet: Option<Instant> = None;
            while !stop_requested.load(Ordering::Relaxed) {
                match socket.recv(&mut buffer) {
                    Ok(length) => {
                        let Some(angles) = decode_opentrack_packet(&buffer[..length]) else {
                            continue;
                        };
                        if last_packet.is_none() {
                            println!("Head tracking: receiving");
                        }
                        last_packet = Some(Instant::now());
                        receiving_now.store(true, Ordering::Relaxed);
                        let placed = placement.get();
                        let command = Command::SetListenerPose {
                            position: placed.position,
                            yaw: listener_yaw(placed.yaw, angles.yaw_degrees),
                        };
                        if let Ok(mut producer) = producer.lock() {
                            // A full ring means the audio thread is behind; the
                            // next packet (40 ms later) carries a newer pose.
                            let _ = producer.push(command);
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        let silent = last_packet
                            .is_some_and(|arrived| arrived.elapsed() >= SILENCE_BEFORE_HOLDING);
                        if silent {
                            last_packet = None;
                            receiving_now.store(false, Ordering::Relaxed);
                            println!("Head tracking: no packets for 2 s, holding last pose");
                        }
                    }
                    Err(error) => {
                        eprintln!("Head tracking: socket error: {error}");
                        return;
                    }
                }
            }
        })?;
    Ok(OpenTrackListener {
        stop,
        receiving,
        thread: Some(thread),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::head_tracking::test_support::{free_port, next_command, packet, send_yaw};
    use atrium_core::types::Vec3;

    #[test]
    fn decodes_yaw_pitch_roll_from_the_last_three_doubles() {
        let angles = decode_opentrack_packet(&packet([1.0, 2.0, 3.0, -52.8, 8.3, -11.3])).unwrap();
        assert_eq!(
            angles,
            HeadAngles {
                yaw_degrees: -52.8,
                pitch_degrees: 8.3,
                roll_degrees: -11.3
            }
        );
    }

    #[test]
    fn rejects_wrong_sizes_and_non_finite_angles() {
        assert_eq!(decode_opentrack_packet(&[0u8; 47]), None);
        assert_eq!(decode_opentrack_packet(&[0u8; 49]), None);
        assert_eq!(decode_opentrack_packet(b"{\"version\":2}"), None);
        assert_eq!(
            decode_opentrack_packet(&packet([0.0, 0.0, 0.0, f64::NAN, 0.0, 0.0])),
            None
        );
    }

    #[test]
    fn centred_head_keeps_the_scene_heading() {
        let scene_yaw = 90f32.to_radians();
        assert!((listener_yaw(scene_yaw, 0.0) - scene_yaw).abs() < 1e-6);
    }

    #[test]
    fn a_left_turn_turns_the_listener_counter_clockwise() {
        // Facing +Y (north). A 90° left turn must face −X (west): yaw 180°.
        let scene_yaw = 90f32.to_radians();
        let left_turn_as_reported = 90.0 * TRACKER_YAW_SIGN as f64;
        let yaw = listener_yaw(scene_yaw, left_turn_as_reported);
        assert!(
            (yaw.to_degrees() - 180.0).abs() < 1e-3,
            "got {}°",
            yaw.to_degrees()
        );
    }

    /// OpenTrack angles are degrees; the listener wants radians. One radian is
    /// 57.29578°, so a reported −57.29578° (a left turn, under the −1 sign)
    /// adds exactly +1 rad to the scene heading.
    #[test]
    fn tracker_degrees_become_listener_radians() {
        let scene_yaw = 0.25;
        let yaw = listener_yaw(scene_yaw, -57.295_78);
        assert!((yaw - 1.25).abs() < 1e-5, "got {yaw} rad");
    }

    /// A full head turn (360°) lands on the same heading: the listener uses
    /// yaw only through cos/sin, so no wrapping is needed.
    #[test]
    fn a_full_turn_returns_to_the_same_heading() {
        let scene_yaw = 30f32.to_radians();
        let turned = listener_yaw(scene_yaw, 360.0);
        assert!((turned.cos() - scene_yaw.cos()).abs() < 1e-5);
        assert!((turned.sin() - scene_yaw.sin()).abs() < 1e-5);
    }

    /// Only the angles are read: the position doubles (cm in OpenTrack) do
    /// not affect the decoded head angles.
    #[test]
    fn position_doubles_do_not_affect_the_angles() {
        let still = decode_opentrack_packet(&packet([0.0, 0.0, 0.0, 12.5, -3.0, 4.0])).unwrap();
        let moved = decode_opentrack_packet(&packet([25.0, -10.0, 8.0, 12.5, -3.0, 4.0])).unwrap();
        assert_eq!(still, moved);
    }

    /// The whole path on a real socket: a packet becomes one listener pose at
    /// the scene's position, and dropping the listener gives the port back
    /// before the drop returns. An app that changes scene binds it again at
    /// once, so a port still held would refuse the next scene's tracker.
    #[test]
    fn a_packet_steers_the_listener_and_dropping_frees_the_port() {
        let (producer, mut consumer) = rtrb::RingBuffer::<Command>::new(8);
        let port = free_port();
        let scene_position = Vec3::new(30.0, 30.0, 1.2);
        let scene_yaw = 90f32.to_radians();
        let listener = spawn(
            port,
            Arc::new(Mutex::new(producer)),
            super::super::Placement::new(super::super::Pose {
                position: scene_position,
                yaw: scene_yaw,
            }),
        )
        .expect("a free port can be bound");

        assert!(
            !listener.is_receiving(),
            "nothing has been sent, so no tracker is reported"
        );
        send_yaw(port, 90.0 * TRACKER_YAW_SIGN as f64);
        match next_command(&mut consumer) {
            Command::SetListenerPose { position, yaw } => {
                assert_eq!(position, scene_position);
                assert!(
                    (yaw.to_degrees() - 180.0).abs() < 1e-3,
                    "a 90° left turn from north faces west, got {}°",
                    yaw.to_degrees()
                );
            }
            other => panic!("expected a listener pose, got {other:?}"),
        }

        assert!(
            listener.is_receiving(),
            "a tracker that has just sent is reported"
        );
        assert!(
            UdpSocket::bind(("127.0.0.1", port)).is_err(),
            "the listener holds its port while it runs"
        );
        drop(listener);
        UdpSocket::bind(("127.0.0.1", port)).expect("dropping the listener frees its port");
    }

    /// An editor moves the listener while the head is tracked: the next head
    /// turn is from the new place and heading, not the one tracking began at.
    #[test]
    fn a_moved_listener_turns_from_where_it_was_moved() {
        let (producer, mut consumer) = rtrb::RingBuffer::<Command>::new(8);
        let port = free_port();
        let placement = super::super::Placement::new(super::super::Pose {
            position: Vec3::ZERO,
            yaw: 0.0,
        });
        let _listener = spawn(port, Arc::new(Mutex::new(producer)), placement.clone())
            .expect("a free port can be bound");
        let moved = super::super::Pose {
            position: Vec3::new(4.0, -2.0, 1.2),
            yaw: 1.0,
        };
        placement.set(moved);
        send_yaw(port, 0.0);
        match next_command(&mut consumer) {
            Command::SetListenerPose { position, yaw } => {
                assert_eq!(position, moved.position);
                assert!((yaw - moved.yaw).abs() < 1e-6);
            }
            other => panic!("expected a listener pose, got {other:?}"),
        }
    }

    /// Headphones that are switched off stop sending. After two seconds of
    /// silence the tracker is no longer reported, so a UI can say it is
    /// waiting instead of showing a frozen head.
    #[test]
    fn a_tracker_that_falls_silent_is_no_longer_reported() {
        let (producer, mut consumer) = rtrb::RingBuffer::<Command>::new(8);
        let port = free_port();
        let still = super::super::Pose {
            position: Vec3::ZERO,
            yaw: 0.0,
        };
        let listener = spawn(
            port,
            Arc::new(Mutex::new(producer)),
            super::super::Placement::new(still),
        )
        .expect("a free port can be bound");
        send_yaw(port, 10.0);
        next_command(&mut consumer);
        assert!(listener.is_receiving());

        let deadline = Instant::now() + SILENCE_BEFORE_HOLDING + Duration::from_secs(1);
        while listener.is_receiving() {
            assert!(
                Instant::now() < deadline,
                "still reported a second after the silence limit"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_source_ahead_moves_to_the_right_ear_after_a_left_turn() {
        // The point of head tracking: the world stays put. A source due north
        // is straight ahead at rest; after turning left 90° it is on the right.
        let scene_yaw = 90f32.to_radians();
        let yaw = listener_yaw(scene_yaw, 90.0 * TRACKER_YAW_SIGN as f64);
        let (forward_x, forward_y) = (yaw.cos(), yaw.sin());
        let (to_source_x, to_source_y) = (0.0f32, 1.0f32);
        // Cross product z: positive = source is to the left, negative = right.
        let side = forward_x * to_source_y - forward_y * to_source_x;
        assert!(side < -0.99, "source should be hard right, side = {side}");
    }
}
