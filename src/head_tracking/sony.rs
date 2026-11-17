//! Sony headphone tracker: runs `scripts/sony-head-tracker.sh` as a child.
//!
//! The script owns everything Sony-specific (building the pinned
//! `sony-head-tracker` bridge, wrapping it as an app for macOS permissions,
//! starting it). It streams OpenTrack UDP to the listener in `opentrack.rs`, so
//! this file only starts and stops it.

use std::path::PathBuf;
use std::process::{Child, Command};

/// The bridge script, located from the crate root so it works from any
/// working directory.
fn script_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/sony-head-tracker.sh")
}

/// A running Sony bridge. Dropping it stops the bridge.
pub(super) struct SonyBridge {
    child: Child,
}

impl SonyBridge {
    pub(super) fn start() -> std::io::Result<Self> {
        let child = Command::new("sh").arg(script_path()).spawn()?;
        Ok(Self { child })
    }
}

impl Drop for SonyBridge {
    fn drop(&mut self) {
        // SIGTERM, not `Child::kill` (SIGKILL): the script traps TERM to stop
        // the bridge app, which `open` launched outside this process group.
        // On Ctrl+C the terminal sends SIGINT to the whole foreground group, so
        // the script's trap runs then too, without this Drop.
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(self.child.id().to_string())
            .status();
        let _ = self.child.wait();
    }
}
