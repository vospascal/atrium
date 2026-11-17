//! Head tracking: a tracker's head turn steers the listener, so the world
//! stays put when you turn your head.
//!
//! The engine listens for the OpenTrack UDP protocol, not for any one brand.
//! OpenTrack's "UDP over network" packet is the de facto interchange format for
//! head trackers: phone apps, webcam trackers, DIY IMU boards and bridges for
//! headphone sensors all emit it. A brand only appears here as an optional
//! convenience — starting its bridge for you — selected on the command line:
//!
//! - `--head-tracking`        listen; start your tracker yourself
//! - `--head-tracking=sony`   listen and start the Sony bridge (WH/WF-1000XM5/XM6)
//!
//! Adding a brand: one file that starts its bridge, one `Tracker` variant.

mod opentrack;
mod sony;

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
}

/// Read the `--head-tracking[=tracker]` flag. `Ok(None)` when it is absent.
pub fn tracker_from_args(args: &[String]) -> Result<Option<Tracker>, String> {
    let Some(flag) = args.iter().find(|a| a.starts_with("--head-tracking")) else {
        return Ok(None);
    };
    match flag.as_str() {
        "--head-tracking" => Ok(Some(Tracker::External)),
        "--head-tracking=sony" => Ok(Some(Tracker::Sony)),
        other => Err(format!(
            "unknown head tracker in `{other}` — use --head-tracking or --head-tracking=sony"
        )),
    }
}

/// Head tracking while it runs. Keep it alive for the engine's lifetime;
/// dropping it stops any tracker bridge the engine started.
pub struct HeadTracking {
    _sony_bridge: Option<sony::SonyBridge>,
}

/// Start listening, and start the tracker's bridge if the engine owns it.
pub fn start(
    tracker: Tracker,
    producer: Arc<Mutex<rtrb::Producer<Command>>>,
    scene_position: Vec3,
    scene_yaw_radians: f32,
) -> std::io::Result<HeadTracking> {
    // Listen first: a bridge sending before the port is bound would lose packets,
    // and a port already in use should fail before a bridge is started.
    opentrack::spawn(
        opentrack::DEFAULT_PORT,
        producer,
        scene_position,
        scene_yaw_radians,
    )?;
    let sony_bridge = match tracker {
        Tracker::External => None,
        Tracker::Sony => Some(sony::SonyBridge::start()?),
    };
    Ok(HeadTracking {
        _sony_bridge: sony_bridge,
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
    }

    #[test]
    fn unknown_tracker_is_an_error_not_a_silent_fallback() {
        assert!(tracker_from_args(&args(&["atrium", "--head-tracking=apple"])).is_err());
    }
}
