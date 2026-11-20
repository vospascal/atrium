//! Head tracking: the trackers a UI can offer on this platform.

use atrium::head_tracking::Tracker;

/// A source of head turns. While one runs, the listener turns with the head,
/// so the place stays where it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum HeadTracker {
    /// Sony WH/WF-1000XM5 or XM6 headphones. The engine starts and stops
    /// their bridge.
    Sony,
    /// Any tracker that sends OpenTrack UDP to this machine. The user starts
    /// it; the engine only listens.
    External,
}

/// What head tracking is doing, so a UI can tell "switched on" from "working".
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum HeadTrackingStatus {
    /// No tracker is chosen.
    Off,
    /// A tracker is chosen and the engine is listening, but no head turn has
    /// arrived in the last two seconds: the headphones are off, out of reach,
    /// or not exposing their sensor.
    Waiting,
    /// Head turns are arriving and steering the listener.
    Receiving,
}

impl From<HeadTracker> for Tracker {
    fn from(tracker: HeadTracker) -> Self {
        match tracker {
            HeadTracker::Sony => Self::Sony,
            HeadTracker::External => Self::External,
        }
    }
}

/// The trackers that can work on this platform, in the order a UI lists them.
///
/// The Sony bridge is a macOS program. Nothing is offered on iOS: the engine
/// accepts tracker packets from the same device only, and no tracker on an
/// iPhone sends them.
#[uniffi::export]
pub fn head_trackers() -> Vec<HeadTracker> {
    if cfg!(target_os = "macos") {
        vec![HeadTracker::Sony, HeadTracker::External]
    } else if cfg!(target_os = "ios") {
        Vec::new()
    } else {
        vec![HeadTracker::External]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sony_is_offered_only_where_its_bridge_runs() {
        assert_eq!(
            head_trackers().contains(&HeadTracker::Sony),
            cfg!(target_os = "macos")
        );
    }

    #[test]
    fn each_tracker_is_offered_at_most_once() {
        let offered = head_trackers();
        for tracker in [HeadTracker::Sony, HeadTracker::External] {
            assert!(offered.iter().filter(|entry| **entry == tracker).count() <= 1);
        }
    }
}
