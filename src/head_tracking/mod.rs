//! Head tracking: a tracker's head turn steers the listener, so the world
//! stays put when you turn your head.
//!
//! The engine listens for the OpenTrack UDP protocol, not for any one brand.
//! OpenTrack's "UDP over network" packet is the de facto interchange format for
//! head trackers: phone apps, webcam trackers, DIY IMU boards and bridges for
//! headphone sensors all emit it. A brand only appears here as an optional
//! convenience — starting its bridge for you — selected on the command line:
//!
//! - `--head-tracking`          listen; start your tracker yourself
//! - `--head-tracking=sony`     listen and start the Sony bridge (WH/WF-1000XM5/XM6)
//! - `--head-tracking=airpods`  listen and start the AirPods bridge (macOS 14+)
//!
//! Adding a brand: one bridge script under `scripts/` that sends OpenTrack
//! UDP with a left turn as negative yaw, and one `Tracker` variant naming it.

mod bridge;
mod opentrack;
#[cfg(test)]
mod test_support;

use std::sync::{Arc, Mutex};

use atrium_core::commands::Command;
use atrium_core::types::Vec3;

/// Which tracker the engine starts alongside its listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tracker {
    /// Started by the user; the engine only listens.
    External,
    /// Sony headphones via `scripts/sony-head-tracker.sh`.
    Sony,
    /// Apple headphones with head tracking (AirPods Pro, AirPods 3rd/4th
    /// generation, AirPods Max) via `scripts/airpods-head-tracker.sh`.
    AirPods,
}

impl Tracker {
    /// The bridge script the engine starts for this tracker, or `None` when
    /// the user starts the tracker.
    fn bridge_script(self) -> Option<&'static str> {
        match self {
            Self::External => None,
            Self::Sony => Some("sony-head-tracker.sh"),
            Self::AirPods => Some("airpods-head-tracker.sh"),
        }
    }
}

/// Read the `--head-tracking[=tracker]` flag. `Ok(None)` when it is absent.
pub fn tracker_from_args(args: &[String]) -> Result<Option<Tracker>, String> {
    let Some(flag) = args.iter().find(|a| a.starts_with("--head-tracking")) else {
        return Ok(None);
    };
    match flag.as_str() {
        "--head-tracking" => Ok(Some(Tracker::External)),
        "--head-tracking=sony" => Ok(Some(Tracker::Sony)),
        "--head-tracking=airpods" => Ok(Some(Tracker::AirPods)),
        other => Err(format!(
            "unknown head tracker in `{other}` — use --head-tracking, \
             --head-tracking=sony or --head-tracking=airpods"
        )),
    }
}

/// Where the scene puts the listener and which way it faces (radians from
/// +x). Head turns are added to this heading. It is shared: an editor that
/// moves the listener moves where the head turns from, while tracking runs.
#[derive(Clone, Debug)]
pub struct Placement(Arc<Mutex<Pose>>);

/// A listener's position (m) and heading (radians from +x).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub position: Vec3,
    pub yaw: f32,
}

impl Placement {
    pub fn new(pose: Pose) -> Self {
        Self(Arc::new(Mutex::new(pose)))
    }

    pub fn get(&self) -> Pose {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn set(&self, pose: Pose) {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = pose;
    }
}

/// Head tracking while it runs. Keep it alive for as long as the head should
/// steer the listener. Dropping it stops any tracker bridge the engine
/// started, stops listening, frees the port and turns the listener back to
/// the scene's heading, all before the drop returns.
pub struct HeadTracking {
    bridge: Option<bridge::Bridge>,
    listener: Option<opentrack::OpenTrackListener>,
    producer: Arc<Mutex<rtrb::Producer<Command>>>,
    placement: Placement,
}

impl HeadTracking {
    /// Whether the tracker is sending head turns right now. False while the
    /// headphones are off, out of reach or not exposing their sensor: the
    /// engine is listening and nothing arrives.
    pub fn is_receiving(&self) -> bool {
        self.listener
            .as_ref()
            .is_some_and(opentrack::OpenTrackListener::is_receiving)
    }
}

impl Drop for HeadTracking {
    fn drop(&mut self) {
        // Sender first, then receiver, so no packet can turn the head again
        // after the pose below.
        self.bridge = None;
        self.listener = None;
        if let Ok(mut producer) = self.producer.lock() {
            // Without this the listener would keep the last tracked heading:
            // someone who switched tracking off while looking left would hear
            // the place turned until the scene was restarted.
            let pose = self.placement.get();
            let _ = producer.push(Command::SetListenerPose {
                position: pose.position,
                yaw: pose.yaw,
            });
        }
    }
}

/// Start listening, and start the tracker's bridge if the engine owns it.
pub fn start(
    tracker: Tracker,
    producer: Arc<Mutex<rtrb::Producer<Command>>>,
    placement: Placement,
) -> std::io::Result<HeadTracking> {
    start_on(opentrack::DEFAULT_PORT, tracker, producer, placement)
}

fn start_on(
    port: u16,
    tracker: Tracker,
    producer: Arc<Mutex<rtrb::Producer<Command>>>,
    placement: Placement,
) -> std::io::Result<HeadTracking> {
    // Listen first: a bridge sending before the port is bound would lose packets,
    // and a port already in use should fail before a bridge is started.
    let listener = opentrack::spawn(port, producer.clone(), placement.clone())?;
    let bridge = tracker
        .bridge_script()
        .map(bridge::Bridge::start)
        .transpose()?;
    Ok(HeadTracking {
        bridge,
        listener: Some(listener),
        producer,
        placement,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn flag_selects_the_tracker() {
        assert_eq!(
            tracker_from_args(&args(&["atrium", "scene.yaml"])),
            Ok(None)
        );
        assert_eq!(
            tracker_from_args(&args(&["atrium", "--head-tracking", "scene.yaml"])),
            Ok(Some(Tracker::External))
        );
        assert_eq!(
            tracker_from_args(&args(&["atrium", "--head-tracking=sony"])),
            Ok(Some(Tracker::Sony))
        );
        assert_eq!(
            tracker_from_args(&args(&["atrium", "--head-tracking=airpods"])),
            Ok(Some(Tracker::AirPods))
        );
    }

    /// A script name that does not exist would only show as a bridge that
    /// never sends, so check every one is in `scripts/`.
    #[test]
    fn every_bridge_script_exists() {
        for tracker in [Tracker::External, Tracker::Sony, Tracker::AirPods] {
            if let Some(script) = tracker.bridge_script() {
                let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("scripts")
                    .join(script);
                assert!(
                    path.is_file(),
                    "{tracker:?} names a missing {}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn unknown_tracker_is_an_error_not_a_silent_fallback() {
        assert!(tracker_from_args(&args(&["atrium", "--head-tracking=apple"])).is_err());
    }

    /// Switching head tracking off while the scene plays: the head turn
    /// reaches the listener while it runs, and stopping puts the listener
    /// back on the scene's own pose as the last thing on the ring.
    #[test]
    fn stopping_turns_the_listener_back_to_the_scene_heading() {
        use test_support::{free_port, next_command, ports, send_yaw};

        let (producer, mut consumer) = rtrb::RingBuffer::<Command>::new(8);
        let _ports = ports();
        let port = free_port();
        let scene_position = Vec3::new(30.0, 30.0, 1.2);
        let scene_yaw = 90f32.to_radians();
        let head_tracking = start_on(
            port,
            Tracker::External,
            Arc::new(Mutex::new(producer)),
            Placement::new(Pose {
                position: scene_position,
                yaw: scene_yaw,
            }),
        )
        .expect("a free port can be bound");

        assert!(!head_tracking.is_receiving(), "nothing has been sent yet");
        send_yaw(port, 40.0);
        match next_command(&mut consumer) {
            Command::SetListenerPose { yaw, .. } => assert!(
                (yaw - scene_yaw).abs() > 0.5,
                "a 40° head turn (0.70 rad) reaches the listener, got {yaw} rad"
            ),
            other => panic!("expected a listener pose, got {other:?}"),
        }

        assert!(head_tracking.is_receiving(), "the tracker is sending");

        drop(head_tracking);
        match next_command(&mut consumer) {
            Command::SetListenerPose { position, yaw } => {
                assert_eq!(position, scene_position);
                assert_eq!(yaw, scene_yaw);
            }
            other => panic!("expected a listener pose, got {other:?}"),
        }
        assert!(
            consumer.pop().is_err(),
            "nothing steers the listener after the stop"
        );
    }
}
