//! What the engine's tests share.

use std::time::{Duration, Instant};

use super::Engine;
use crate::api::Telemetry;

/// The newest telemetry for which `ready` holds, waiting up to two seconds.
pub(super) fn telemetry_when(engine: &Engine, ready: impl Fn(&Telemetry) -> bool) -> Telemetry {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(telemetry) = engine.latest_telemetry() {
            if ready(&telemetry) {
                return telemetry;
            }
        }
        assert!(Instant::now() < deadline, "the engine never reported it");
        std::thread::sleep(Duration::from_millis(20));
    }
}
