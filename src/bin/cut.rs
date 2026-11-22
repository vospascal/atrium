// Cut a long field recording into its takes: the takes as files, the rests
// between them, their types and bouts, and a draft preset that uses them, all
// measured from the recording. The cut is `atrium_analysis::cut` and the
// draft `atrium::config::draft_preset`; this decodes, and writes the files.
//
//   cargo run --release --bin cut -- <recording>[@from-to] [options]
//
// Options are listed by `--help` and in run.md ("Cut a recording").

use std::path::Path;
use std::time::Instant;

use atrium::audio::decode::decode_channels;
use atrium::audio::file_stretch::{frame_range, FileStretch};
use atrium::audio::wav::{write_wav, SampleFormat};
use atrium::config::{draft_preset, Draft};
use atrium_analysis::{cut, cut_picture, cut_summary, CutSettings, Signal};
use serde_json::{json, Value};

const USAGE: &str = "\
usage: cut <recording>[@<from>-<to>] [options]

  <recording>@<from>-<to>  only this stretch of it, in seconds (frog.mp3@30-60)
  --name <kind>            the folder and the kind (default: the file's stem);
                           a preset sources/<kind>.yaml lends the draft its
                           name, group, width, vary.pitch and follows
  --band <low>-<high>      find the events between these frequencies (Hz)
                           instead of in the third octaves where they stand
                           furthest above the bed
  --picture                also draw cut.png

Writes only target/cut/<kind>/, and refuses a folder that is already there:
take-01.wav ... (32-bit float, the recording's rate and channels), cut.json
(every decision, cut_version 1), summary.txt (as printed) and preset.yaml (a
draft that does not load until its reference_spl is filled in).
";

/// What the command line asked for.
#[derive(Debug, PartialEq)]
struct Arguments {
    recording: FileStretch,
    name: Option<String>,
    band: Option<(f64, f64)>,
    picture: bool,
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        print!("{USAGE}");
        return;
    }
    let arguments = match parse(&arguments) {
        Ok(arguments) => arguments,
        Err(message) => {
            eprintln!("cut: {message}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(error) = run(&arguments) {
        eprintln!("cut: {error}");
        std::process::exit(1);
    }
}

fn run(arguments: &Arguments) -> Result<(), Box<dyn std::error::Error>> {
    let path = &arguments.recording.path;
    let name = arguments.name.clone().unwrap_or_else(|| stem(path));
    let folder = Path::new("target/cut").join(&name);
    if folder.exists() {
        return Err(format!(
            "{} is already there; move it away or give another --name",
            folder.display()
        )
        .into());
    }

    let decoded = decode_channels(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let total_frames = decoded.interleaved.len() / decoded.channels;
    let (from, to) = arguments
        .recording
        .range
        .map_or((None, None), |(from, to)| (Some(from), Some(to)));
    let (first, end) = frame_range(from, to, decoded.sample_rate, total_frames)
        .ok_or_else(|| format!("{}: nothing in the stretch asked for", path.display()))?;
    let signal = Signal {
        interleaved: &decoded.interleaved[first * decoded.channels..end * decoded.channels],
        channels: decoded.channels,
        sample_rate: decoded.sample_rate,
        channel_names: Vec::new(),
    };
    let rate = decoded.sample_rate as f64;
    let recording = match arguments.recording.range {
        Some(_) => format!(
            "{}@{}-{}",
            path.display(),
            trimmed(first as f64 / rate),
            trimmed(end as f64 / rate)
        ),
        None => path.display().to_string(),
    };
    let heading = format!(
        "{recording} | {:.1}-{:.1} s of {:.1} s",
        first as f64 / rate,
        end as f64 / rate,
        total_frames as f64 / rate
    );

    let started = Instant::now();
    let cut = cut(
        &signal,
        &CutSettings {
            band: arguments.band,
        },
    );
    let cut_in = started.elapsed().as_secs_f64();

    std::fs::create_dir_all(&folder)?;
    let mut take_files = Vec::new();
    for take in &cut.takes {
        let file = folder.join(format!("take-{:02}.wav", take.number));
        write_wav(
            &file,
            &take.samples(&signal),
            decoded.channels as u16,
            decoded.sample_rate,
            SampleFormat::Float32,
        )?;
        take_files.push(file.display().to_string());
    }

    let mut document = serde_json::to_value(&cut)?;
    if let Value::Object(fields) = &mut document {
        fields.insert(
            "input".to_owned(),
            json!({
                "file": path.display().to_string(),
                "from_seconds": first as f64 / rate,
                "to_seconds": end as f64 / rate,
            }),
        );
    }
    std::fs::write(
        folder.join("cut.json"),
        serde_json::to_string_pretty(&document)? + "\n",
    )?;

    let existing_path = format!("sources/{name}.yaml");
    let existing_text = std::fs::read_to_string(&existing_path).ok();
    let preset = draft_preset(
        &cut,
        &Draft {
            recording: &recording,
            name: &name,
            take_files: &take_files,
            existing: existing_text
                .as_deref()
                .map(|text| (existing_path.as_str(), text)),
        },
    )?;
    std::fs::write(folder.join("preset.yaml"), preset)?;

    if arguments.picture {
        std::fs::write(folder.join("cut.png"), cut_picture(&heading, &signal, &cut))?;
    }

    let text = cut_summary(&heading, &cut);
    std::fs::write(folder.join("summary.txt"), &text)?;
    print!("{text}");
    println!(
        "{:<12}{} ({} takes, cut.json, summary.txt, preset.yaml{}; cut in {cut_in:.2} s)",
        "wrote",
        folder.display(),
        cut.takes.len(),
        if arguments.picture { ", cut.png" } else { "" }
    );
    if existing_text.is_some() {
        println!(
            "{:<12}name, group, width, vary.pitch and follows from {existing_path}",
            ""
        );
    }
    Ok(())
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording".to_owned())
}

/// 28, 0.5, 12.25: seconds without trailing zeros.
fn trimmed(seconds: f64) -> String {
    let text = format!("{seconds:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn parse(arguments: &[String]) -> Result<Arguments, String> {
    let mut recording = None;
    let mut name = None;
    let mut band = None;
    let mut picture = false;
    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        if !argument.starts_with("--") {
            if recording.is_some() {
                return Err(format!("one recording at a time; {argument} is a second"));
            }
            recording = Some(FileStretch::parse(argument)?);
            continue;
        }
        let (flag, inline) = match argument.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_owned())),
            None => (argument.as_str(), None),
        };
        if flag == "--picture" {
            picture = true;
            continue;
        }
        let value = match inline {
            Some(value) => value,
            None => remaining
                .next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))?,
        };
        match flag {
            "--name" => {
                let valid = !value.is_empty()
                    && value
                        .chars()
                        .all(|character| character.is_alphanumeric() || "-_".contains(character));
                if !valid {
                    return Err(format!(
                        "--name `{value}`: letters, digits, - and _ only, as a folder and a preset are named"
                    ));
                }
                name = Some(value);
            }
            "--band" => {
                let hertz = |text: &str| text.parse::<f64>().ok().filter(|value| *value > 0.0);
                band = match value
                    .split_once('-')
                    .map(|(low, high)| (hertz(low), hertz(high)))
                {
                    Some((Some(low), Some(high))) if high > low => Some((low, high)),
                    _ => {
                        return Err(format!(
                            "--band `{value}`: two frequencies in Hz, low-high (2000-8000)"
                        ))
                    }
                };
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(Arguments {
        recording: recording.ok_or("which recording?")?,
        name,
        band,
        picture,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn arguments(text: &str) -> Result<Arguments, String> {
        parse(
            &text
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn a_recording_its_stretch_and_options() {
        let read = arguments("assets/frog.mp3@10-40 --name frog --band 500-3000 --picture")
            .expect("reads");
        assert_eq!(
            read,
            Arguments {
                recording: FileStretch {
                    path: PathBuf::from("assets/frog.mp3"),
                    range: Some((10.0, 40.0)),
                },
                name: Some("frog".into()),
                band: Some((500.0, 3000.0)),
                picture: true,
            }
        );
        assert_eq!(
            arguments("a.wav --band=2000-8000").expect("reads").band,
            Some((2000.0, 8000.0))
        );
        assert!(arguments("a.wav b.wav").is_err());
        assert!(arguments("a.wav --band 3000-500").is_err());
        assert!(arguments("a.wav --name ../sources").is_err());
        assert!(arguments("--picture").is_err());
    }
}
