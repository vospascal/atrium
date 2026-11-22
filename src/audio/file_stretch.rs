//! A file and the stretch of it a command line asks for, written
//! `file@from-to` in seconds (`frog.mp3@30-60`), as `analyse` and `cut` read
//! their files.

use std::path::PathBuf;

/// A file and, from `@from-to`, the stretch of it asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct FileStretch {
    pub path: PathBuf,
    /// From `@from-to`, in seconds.
    pub range: Option<(f64, f64)>,
}

impl FileStretch {
    /// `file@from-to` as a file and a stretch; any other argument as a file.
    /// A file whose own name ends in such a suffix is read as the file only
    /// when the suffix is not a stretch.
    pub fn parse(argument: &str) -> Result<Self, String> {
        if let Some((path, stretch)) = argument.rsplit_once('@') {
            if let Some((from, to)) = stretch.split_once('-') {
                let seconds = |value: &str| value.parse::<f64>().ok().filter(|s| s.is_finite());
                if let (Some(from), Some(to)) = (seconds(from), seconds(to)) {
                    if from < 0.0 || to <= from {
                        return Err(format!("{argument}: the stretch must run forwards from 0"));
                    }
                    return Ok(Self {
                        path: PathBuf::from(path),
                        range: Some((from, to)),
                    });
                }
            }
        }
        Ok(Self::whole(argument))
    }

    /// All of the file at `path`.
    pub fn whole(path: &str) -> Self {
        Self {
            path: PathBuf::from(path),
            range: None,
        }
    }
}

/// The frames from `from` (default the start) to `to` (default the end)
/// seconds of a file of `total_frames` at `sample_rate`, cut to the file;
/// None when nothing is left.
pub fn frame_range(
    from: Option<f64>,
    to: Option<f64>,
    sample_rate: u32,
    total_frames: usize,
) -> Option<(usize, usize)> {
    let rate = sample_rate as f64;
    let first = from
        .map_or(0, |seconds| (seconds * rate).round() as usize)
        .min(total_frames);
    let end = to
        .map_or(total_frames, |seconds| (seconds * rate).round() as usize)
        .min(total_frames);
    (end > first).then_some((first, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stretch_and_a_name_with_an_at() {
        assert_eq!(
            FileStretch::parse("ref.mp3@0-28").expect("reads"),
            FileStretch {
                path: PathBuf::from("ref.mp3"),
                range: Some((0.0, 28.0)),
            }
        );
        assert_eq!(
            FileStretch::parse("b.wav@1.5-3").expect("reads").range,
            Some((1.5, 3.0))
        );
        // A name with an @ that is not a stretch is a name.
        assert_eq!(
            FileStretch::parse("take@home.wav").expect("reads"),
            FileStretch::whole("take@home.wav")
        );
        assert!(FileStretch::parse("a.wav@5-2").is_err());
    }

    #[test]
    fn frames_are_cut_to_the_file() {
        assert_eq!(frame_range(None, None, 100, 500), Some((0, 500)));
        assert_eq!(
            frame_range(Some(1.0), Some(9.0), 100, 500),
            Some((100, 500))
        );
        assert_eq!(frame_range(Some(6.0), None, 100, 500), None);
    }
}
