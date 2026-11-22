// Analyse recordings and renders: one report per file, in JSON for the
// comparison tool, as a one-screen summary and as a picture. The measures are
// `atrium_analysis`; this decodes, reads a render's sidecar, cuts, and writes
// the files.
//
//   cargo run --release --bin analyse -- <file.wav|mp3>... [options]
//
// Options are listed by `--help` and in run.md ("Analyse a file").

use std::path::{Path, PathBuf};
use std::time::Instant;

use atrium::audio::decode::decode_channels;
use atrium_analysis::{analyse, picture, summary, Settings, Signal};
use serde_json::{json, Value};

const USAGE: &str = "\
usage: analyse <file.wav|mp3>... [options]

  --from <s>           analyse from this time (default 0)
  --to <s>             analyse up to this time (default: the end)
  --output <path>      the JSON report (one file only; default target/analysis/<stem>.json)
  --picture[=<path>]   also draw the picture (default target/analysis/<stem>.png;
                       a path only with one file)
  --summary <path>     also write the printed summaries to this file

A render's sidecar (<file>.json beside it) gives the channel names and the
calibration, so levels are in dB SPL; without one they are in dBFS.
";

/// What the command line asked for.
#[derive(Debug, PartialEq)]
struct Arguments {
    files: Vec<PathBuf>,
    from: Option<f64>,
    to: Option<f64>,
    output: Option<PathBuf>,
    /// Some(None): the default path.
    picture: Option<Option<PathBuf>>,
    summary: Option<PathBuf>,
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
            eprintln!("analyse: {message}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(error) = run(&arguments) {
        eprintln!("analyse: {error}");
        std::process::exit(1);
    }
}

fn run(arguments: &Arguments) -> Result<(), Box<dyn std::error::Error>> {
    let mut summaries = String::new();
    for file in &arguments.files {
        let stem = file
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "analysis".to_owned());
        let default =
            |extension: &str| Path::new("target/analysis").join(format!("{stem}.{extension}"));

        let decoded =
            decode_channels(file).map_err(|error| format!("{}: {error}", file.display()))?;
        let sidecar = read_sidecar(file)?;
        let channels = decoded.channels;
        let rate = decoded.sample_rate;
        let total_frames = decoded.interleaved.len() / channels;
        let first = arguments
            .from
            .map_or(0, |seconds| (seconds * rate as f64).round() as usize)
            .min(total_frames);
        let end = arguments
            .to
            .map_or(total_frames, |seconds| {
                (seconds * rate as f64).round() as usize
            })
            .min(total_frames);
        if end <= first {
            return Err(format!("{}: nothing between --from and --to", file.display()).into());
        }
        let channel_names = sidecar
            .as_ref()
            .and_then(|sidecar| sidecar["channels"].as_array())
            .map(|names| {
                names
                    .iter()
                    .filter_map(|name| name.as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .filter(|names| names.len() == channels)
            .unwrap_or_default();
        let settings = Settings {
            full_scale_db_spl: sidecar.as_ref().and_then(full_scale_db_spl),
        };
        let signal = Signal {
            interleaved: &decoded.interleaved[first * channels..end * channels],
            channels,
            sample_rate: rate,
            channel_names,
        };
        let heading = heading(file, sidecar.as_ref(), first, end, total_frames, rate);

        let started = Instant::now();
        let report = analyse(&signal, &settings);
        let analysed_in = started.elapsed().as_secs_f64();
        let picture_path = arguments
            .picture
            .as_ref()
            .map(|path| path.clone().unwrap_or_else(|| default("png")));
        let mut drawn_in = None;
        if let Some(path) = &picture_path {
            let started = Instant::now();
            let png = picture(&heading, &signal, &report);
            drawn_in = Some(started.elapsed().as_secs_f64());
            write(path, &png)?;
        }

        let mut document = serde_json::to_value(&report)?;
        if let Value::Object(fields) = &mut document {
            fields.insert(
                "input".to_owned(),
                json!({
                    "file": file.display().to_string(),
                    "from_seconds": first as f64 / rate as f64,
                    "to_seconds": end as f64 / rate as f64,
                    "sidecar": sidecar,
                }),
            );
        }
        let output = arguments.output.clone().unwrap_or_else(|| default("json"));
        write(
            &output,
            (serde_json::to_string_pretty(&document)? + "\n").as_bytes(),
        )?;

        let text = summary(&heading, &report);
        print!("{text}");
        let timing = match drawn_in {
            Some(drawn) => format!("analysed in {analysed_in:.2} s, drawn in {drawn:.2} s"),
            None => format!("analysed in {analysed_in:.2} s"),
        };
        println!("{:<12}{} ({timing})", "wrote", output.display());
        if let Some(path) = &picture_path {
            println!("{:<12}{}", "", path.display());
        }
        println!();
        summaries += &text;
        summaries += "\n";
    }
    if let Some(path) = &arguments.summary {
        write(path, summaries.as_bytes())?;
        println!("Summaries written to {}", path.display());
    }
    Ok(())
}

fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(directory) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(directory)?;
    }
    std::fs::write(path, bytes)
}

/// The render's sidecar beside `file`, if there is one.
fn read_sidecar(file: &Path) -> Result<Option<Value>, Box<dyn std::error::Error>> {
    let path = file.with_extension("json");
    if path == file || !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)?;
    let value: Value =
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(Some(value))
}

/// dB SPL of RMS 1.0 in the file: the engine's calibration less the gain
/// `--normalise` raised the file by. None for a sidecar written before the
/// calibration was.
fn full_scale_db_spl(sidecar: &Value) -> Option<f64> {
    let engine = sidecar["calibration"]["full_scale_db_spl"].as_f64()?;
    let gain = sidecar["gain_decibels"].as_f64().unwrap_or(0.0);
    Some(engine - gain)
}

/// The file and how it was made, in one line.
fn heading(
    file: &Path,
    sidecar: Option<&Value>,
    first: usize,
    end: usize,
    total_frames: usize,
    rate: u32,
) -> String {
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cut = if first > 0 || end < total_frames {
        format!(
            " | {:.1}-{:.1} s of {:.1} s",
            first as f64 / rate as f64,
            end as f64 / rate as f64,
            total_frames as f64 / rate as f64
        )
    } else {
        String::new()
    };
    let Some(sidecar) = sidecar else {
        return format!("{name} | a recording (no sidecar){cut}");
    };
    let scene = sidecar["scene"]["path"].as_str().unwrap_or("?");
    let probe = &sidecar["probe"];
    let parts: Vec<&str> = ["direct", "reflections", "reverb", "ambient"]
        .into_iter()
        .filter(|part| probe[*part].as_bool().unwrap_or(true))
        .collect();
    let parts = if parts.len() == 4 {
        "whole".to_owned()
    } else if parts.is_empty() {
        "none".to_owned()
    } else {
        parts.join("+")
    };
    let gain = sidecar["gain_decibels"].as_f64().unwrap_or(0.0);
    let gain = if gain != 0.0 {
        format!(", normalised {gain:+.1} dB")
    } else {
        String::new()
    };
    let calibration = if sidecar["calibration"].is_null() {
        ", no calibration (old sidecar)"
    } else {
        ""
    };
    format!(
        "{name} | render of {scene}, {} {}, seed {}, probe {parts}{gain}{calibration}{cut}",
        sidecar["layout"].as_str().unwrap_or("?"),
        sidecar["mode"].as_str().unwrap_or("?"),
        sidecar["seed"],
    )
}

fn parse(arguments: &[String]) -> Result<Arguments, String> {
    let mut parsed = Arguments {
        files: Vec::new(),
        from: None,
        to: None,
        output: None,
        picture: None,
        summary: None,
    };
    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        if !argument.starts_with("--") {
            parsed.files.push(PathBuf::from(argument));
            continue;
        }
        let (flag, inline) = match argument.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_owned())),
            None => (argument.as_str(), None),
        };
        // Its path only with `=`, so a file after it is never taken for it.
        if flag == "--picture" {
            parsed.picture = Some(inline.map(PathBuf::from));
            continue;
        }
        let value = match inline {
            Some(value) => value,
            None => remaining
                .next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))?,
        };
        let seconds = |value: &str| -> Result<f64, String> {
            value
                .parse::<f64>()
                .ok()
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                .ok_or_else(|| format!("{flag} `{value}` is not a time in seconds"))
        };
        match flag {
            "--from" => parsed.from = Some(seconds(&value)?),
            "--to" => parsed.to = Some(seconds(&value)?),
            "--output" => parsed.output = Some(PathBuf::from(value)),
            "--summary" => parsed.summary = Some(PathBuf::from(value)),
            other => return Err(format!("unknown option {other}")),
        }
    }
    if parsed.files.is_empty() {
        return Err("which file? give one or more".into());
    }
    if parsed.files.len() > 1 {
        if parsed.output.is_some() {
            return Err("--output names one report; give one file, or leave it out".into());
        }
        if matches!(parsed.picture, Some(Some(_))) {
            return Err(
                "--picture=<path> names one picture; give one file, or leave the path out".into(),
            );
        }
    }
    if let (Some(from), Some(to)) = (parsed.from, parsed.to) {
        if to <= from {
            return Err(format!("--to {to} is not after --from {from}"));
        }
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(text: &str) -> Result<Arguments, String> {
        parse(
            &text
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn options_and_files() {
        let read =
            arguments("a.wav b.mp3 --from 2 --to=5.5 --picture --summary s.txt").expect("reads");
        assert_eq!(read.files, [PathBuf::from("a.wav"), PathBuf::from("b.mp3")]);
        assert_eq!((read.from, read.to), (Some(2.0), Some(5.5)));
        assert_eq!(read.picture, Some(None));
        assert_eq!(read.summary, Some(PathBuf::from("s.txt")));
        assert_eq!(
            arguments("a.wav --picture=p.png").expect("reads").picture,
            Some(Some(PathBuf::from("p.png")))
        );
        assert!(arguments("a.wav b.wav --output r.json").is_err());
        assert!(arguments("a.wav b.wav --picture=p.png").is_err());
        assert!(arguments("a.wav --from 3 --to 2").is_err());
        assert!(arguments("--from 3").is_err());
        assert!(arguments("a.wav --from soon").is_err());
    }

    /// The analyser undoes `--normalise`: 114 dB SPL at RMS 1.0 for the
    /// engine, a file raised by 40 dB reads RMS 1.0 as 74 dB SPL.
    #[test]
    fn the_normalising_gain_is_subtracted() {
        let sidecar = json!({"calibration": {"full_scale_db_spl": 114.0}, "gain_decibels": 40.0});
        assert_eq!(full_scale_db_spl(&sidecar), Some(74.0));
        assert_eq!(full_scale_db_spl(&json!({"gain_decibels": 40.0})), None);
    }
}
