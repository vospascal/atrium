//! Writing a scene back to a file, as the editor saves it.
//!
//! A saved file is rewritten from the scene's values, so the comments of a
//! hand-written file would be lost. A hand-written file is therefore never
//! overwritten: the editor saves it under a new name, and the file it writes
//! starts with [`EDITOR_MARK`] so that it can be saved over again later.

use std::path::Path;

use super::scene::SceneConfig;

/// The first line of every scene file the editor writes.
pub const EDITOR_MARK: &str = "# Saved by the Atrium editor. Comments are not kept.";

impl SceneConfig {
    /// The scene as the text of a scene file: the editor's mark, short
    /// number lists on one line, and a blank line between sections and
    /// between sounds.
    pub fn to_file_text(&self) -> Result<String, Box<dyn std::error::Error>> {
        let yaml = serde_yaml::to_string(self)?;
        Ok(format!(
            "{EDITOR_MARK}\n\n{}",
            spaced(&numbers_on_one_line(&yaml))
        ))
    }

    /// Write the scene to `path`. A file already there is replaced only if
    /// the editor wrote it; a hand-written one is left alone and refused.
    /// The file is written beside the target first and then moved over it,
    /// so a failed write never leaves half a scene.
    pub fn save(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        let target = Path::new(path);
        if target.exists() && !saved_by_editor(path) {
            return Err(format!(
                "{path} was written by hand and keeps its comments; save under a new name"
            )
            .into());
        }
        let text = self.to_file_text()?;
        let staging = target.with_extension("yaml.saving");
        std::fs::write(&staging, text).map_err(|error| format!("{path}: {error}"))?;
        std::fs::rename(&staging, target).map_err(|error| format!("{path}: {error}"))?;
        Ok(())
    }
}

/// Whether the scene file at `path` was written by the editor and may be
/// saved over. A file that cannot be read was not.
pub fn saved_by_editor(path: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|text| text.lines().next() == Some(EDITOR_MARK))
        .unwrap_or(false)
}

/// serde_yaml writes `[x, y, z]` as three lines of `- x`. A key whose value
/// is a list of numbers only is written on one line instead, as a person
/// writes a position.
fn numbers_on_one_line(yaml: &str) -> String {
    let lines: Vec<&str> = yaml.lines().collect();
    let mut out = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if let Some(key) = line.strip_suffix(':') {
            let indent = &line[..line.len() - line.trim_start().len()];
            let item = format!("{indent}- ");
            let numbers: Vec<&str> = lines[index + 1..]
                .iter()
                .map_while(|next| {
                    next.strip_prefix(item.as_str())
                        .filter(|value| value.parse::<f64>().is_ok())
                })
                .collect();
            if !numbers.is_empty() {
                out.push(format!("{key}: [{}]", numbers.join(", ")));
                index += 1 + numbers.len();
                continue;
            }
        }
        out.push(line.to_string());
        index += 1;
    }
    out.join("\n") + "\n"
}

/// A blank line before each top-level section after the first, and before
/// each sound after the first, so a long scene reads in blocks.
fn spaced(yaml: &str) -> String {
    let mut out = String::with_capacity(yaml.len() + 64);
    let mut in_sounds = false;
    let mut first_sound = true;
    for (index, line) in yaml.lines().enumerate() {
        let top_level = !line.starts_with(' ') && !line.is_empty();
        if top_level {
            if index > 0 {
                out.push('\n');
            }
            in_sounds = line == "sounds:";
            first_sound = true;
        } else if in_sounds && line.starts_with("  ") && !line.starts_with("   ") {
            if !first_sound {
                out.push('\n');
            }
            first_sound = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse::load_yaml;

    fn scratch(name: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("atrium-scene-file-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("scratch directory");
        directory.join(name)
    }

    /// Every scene in the repository, written by the editor, reads back as
    /// the same scene: the same sounds in the same order, every part kept.
    #[test]
    fn every_repository_scene_survives_a_save() {
        for entry in std::fs::read_dir("scenes").expect("scenes directory") {
            let path = entry.expect("entry").path();
            if path.extension().is_none_or(|extension| extension != "yaml") {
                continue;
            }
            let path = path.to_str().expect("utf-8 path").to_owned();
            let scene: SceneConfig = load_yaml(&path).expect("a repository scene reads");
            let text = scene.to_file_text().expect("it writes");
            let again: SceneConfig = serde_yaml::from_str(&text)
                .unwrap_or_else(|error| panic!("{path} written:\n{text}\n{error}"));
            assert_eq!(again.sounds, scene.sounds, "{path}");
            assert_eq!(again.listener.position, scene.listener.position, "{path}");
            assert_eq!(again.title, scene.title, "{path}");
            assert_eq!(again.environment, scene.environment, "{path}");
            assert!(
                !text.contains("null"),
                "{path} writes no empty parts:\n{text}"
            );
        }
    }

    #[test]
    fn a_saved_scene_reads_like_a_hand_written_one() {
        let scene: SceneConfig = load_yaml("scenes/meadow-birds.yaml").expect("meadow");
        let text = scene.to_file_text().expect("it writes");
        assert!(text.starts_with(EDITOR_MARK), "{text}");
        assert!(text.contains("position: [0.0, 0.0, 1.2]"), "{text}");
        assert!(text.contains("place: [9.0, 11.0, 7.0]"), "{text}");
        assert!(
            text.contains("\n\n  robin:\n"),
            "a blank line between sounds:\n{text}"
        );
        assert!(
            text.contains("\n\nsounds:\n  wind:\n"),
            "but not before the first:\n{text}"
        );
    }

    /// A hand-written file keeps its comments: it is never saved over. A
    /// file the editor wrote is.
    #[test]
    fn only_a_file_the_editor_wrote_is_saved_over() {
        let scene: SceneConfig = load_yaml("scenes/frog.yaml").expect("frog");
        let hand_written = scratch("hand-written.yaml");
        std::fs::write(&hand_written, "# a person's notes\n").expect("write");
        let hand_written = hand_written.to_str().expect("utf-8");
        assert!(scene.save(hand_written).is_err());
        assert_eq!(
            std::fs::read_to_string(hand_written).expect("still there"),
            "# a person's notes\n"
        );

        let saved = scratch("saved.yaml");
        let saved = saved.to_str().expect("utf-8");
        let _ = std::fs::remove_file(saved);
        scene.save(saved).expect("a new file");
        assert!(saved_by_editor(saved));
        scene.save(saved).expect("the editor's own file again");
        let again: SceneConfig = load_yaml(saved).expect("it reads");
        assert_eq!(again.sounds, scene.sounds);
    }

    #[test]
    fn only_lists_of_numbers_go_on_one_line() {
        let yaml = "a:\n- 1.0\n- -2.5\nb:\n  c:\n  - 3\nd:\n- x\n- y\n";
        assert_eq!(
            numbers_on_one_line(yaml),
            "a: [1.0, -2.5]\nb:\n  c: [3]\nd:\n- x\n- y\n"
        );
    }
}
