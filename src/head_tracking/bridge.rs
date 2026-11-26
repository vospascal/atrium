//! A tracker's bridge: a script that turns one brand's sensor into OpenTrack
//! UDP, run as a child for as long as head tracking is on.
//!
//! Each script owns everything brand-specific (building its bridge program,
//! wrapping it as an app for macOS permissions, starting it). It streams
//! OpenTrack UDP to the listener in `opentrack.rs`, so this file only starts
//! and stops it. Every bridge script has the same shape — a TERM trap that
//! stops the app, then a log pipeline that never ends — which is what the
//! stop below is written for.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

/// A bridge script under `scripts/`, located from the crate root so it works
/// from any working directory.
fn script_path(script_name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join(script_name)
}

/// A running bridge. Dropping it stops the bridge.
pub(super) struct Bridge {
    child: Child,
}

impl Bridge {
    /// Start `scripts/<script_name>`.
    pub(super) fn start(script_name: &str) -> std::io::Result<Self> {
        Self::start_script(&script_path(script_name))
    }

    fn start_script(script: &Path) -> std::io::Result<Self> {
        let child = Command::new("sh").arg(script).spawn()?;
        Ok(Self { child })
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let script = self.child.id().to_string();
        // SIGTERM, not `Child::kill` (SIGKILL): the script traps TERM to stop
        // the bridge app, which `open` launched outside this process group.
        // On Ctrl+C the terminal sends SIGINT to the whole foreground group, so
        // the script's trap runs then too, without this Drop.
        let _ = Command::new("kill").arg("-TERM").arg(&script).status();
        // The shell runs a trap only once its foreground command has ended,
        // and the script ends in `tail -f … | grep …`, which never does. So
        // end the script's children as well. Measured 2026-11-20 with a script
        // of the same shape: TERM to the script alone left it running with the
        // trap unrun, and the `wait` below never returned; with its children
        // terminated it ran the trap and exited in 65 ms. The order matters:
        // if the children went first, the script would simply reach its end
        // without a signal and leave the bridge app running.
        let _ = Command::new("pkill")
            .arg("-TERM")
            .arg("-P")
            .arg(&script)
            .status();
        let _ = self.child.wait();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// The app turns head tracking off while it keeps running, so a drop has
    /// to return and the script's TERM trap has to run: the trap is what
    /// stops the bridge app. The stand-in ends like the real script does: a
    /// TERM trap, then a log pipeline that never finishes by itself.
    #[test]
    fn dropping_the_bridge_ends_a_script_blocked_in_its_log_pipeline() {
        let directory =
            std::env::temp_dir().join(format!("atrium-bridge-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let script = directory.join("stand-in.sh");
        let log = directory.join("bridge.log");
        let reached_pipeline = directory.join("reached-pipeline");
        let trap_ran = directory.join("trap-ran");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 : > '{log}'\n\
                 trap ': > \"{trap_ran}\"; exit 0' INT TERM\n\
                 : > '{reached_pipeline}'\n\
                 tail -f '{log}' | grep --line-buffered -v -E '^YPR' || true\n",
                log = log.display(),
                trap_ran = trap_ran.display(),
                reached_pipeline = reached_pipeline.display(),
            ),
        )
        .expect("the stand-in script is written");

        let bridge = Bridge::start_script(&script).expect("sh starts");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !reached_pipeline.exists() {
            assert!(Instant::now() < deadline, "the stand-in never started");
            std::thread::sleep(Duration::from_millis(10));
        }
        // Let the shell get from the marker into the pipeline.
        std::thread::sleep(Duration::from_millis(100));

        // Drop on another thread: a regression here is a hang, and the test
        // should fail rather than block the suite.
        let (dropped, drop_finished) = mpsc::channel();
        std::thread::spawn(move || {
            drop(bridge);
            let _ = dropped.send(());
        });
        let returned = drop_finished.recv_timeout(Duration::from_secs(5)).is_ok();
        let trap_ran_in_time = trap_ran.exists();
        if !returned {
            // Do not leave the stand-in's `tail` behind after a failure.
            let _ = Command::new("pkill")
                .arg("-f")
                .arg(log.display().to_string())
                .status();
        }
        let _ = std::fs::remove_dir_all(&directory);

        assert!(returned, "dropping the bridge never returned");
        assert!(
            trap_ran_in_time,
            "the script ended without running its TERM trap, so the bridge app would keep running"
        );
    }
}
