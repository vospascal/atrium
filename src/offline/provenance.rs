//! Which scene file and which state of the code made a render, so a file can
//! be made again and told apart from one made after a change.

use std::process::Command;

/// Where a render came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provenance {
    /// The scene file as it was named.
    pub(super) scene_path: String,
    /// FNV-1a (64-bit) of the scene file's bytes, in hex: enough to see the
    /// file changed, which is all it is for. The files the scene names
    /// (presets, recordings, environments) are not in it; the commit covers
    /// those that are tracked.
    pub(super) scene_hash: String,
    /// The commit checked out, if this is a git checkout.
    pub(super) commit: Option<String>,
    /// Whether tracked files differ from that commit. Untracked files do not
    /// count.
    pub(super) dirty: Option<bool>,
}

impl Provenance {
    /// The provenance of a render of `scene_path`, made from the working
    /// directory's checkout.
    pub fn of(scene_path: &str) -> std::io::Result<Self> {
        let bytes = std::fs::read(scene_path)?;
        let commit = git(&["rev-parse", "HEAD"]).map(|out| out.trim().to_owned());
        let dirty =
            git(&["status", "--porcelain", "--untracked-files=no"]).map(|out| !out.is_empty());
        Ok(Self {
            scene_path: scene_path.to_owned(),
            scene_hash: format!("{:016x}", fnv1a_64(&bytes)),
            commit,
            dirty,
        })
    }
}

/// What `git` prints for `arguments`, or `None` when it cannot say.
fn git(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git").args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// FNV-1a, 64-bit: a few lines and no dependency, and stable across Rust
/// versions, which `std`'s hasher is not.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published test vectors of FNV-1a 64.
    #[test]
    fn fnv1a_matches_its_reference_values() {
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x8594_4171_f739_67e8);
    }
}
