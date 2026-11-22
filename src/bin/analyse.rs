// Analyse recordings and renders: one report per file, in JSON for the
// comparison, as a one-screen summary and as a picture; or, with --compare,
// where other files differ from a reference. The measures and the comparison
// are `atrium_analysis`; this decodes, reads a render's sidecar, cuts, and
// writes the files.
//
//   cargo run --release --bin analyse -- <file.wav|mp3>... [options]
//   cargo run --release --bin analyse -- --compare <reference> <other>... [options]
//
// Options are listed by `--help` and in run.md ("Analyse a file", "Compare
// files").

use std::path::{Path, PathBuf};
use std::time::Instant;

use atrium::audio::decode::{decode_channels, DecodedChannels};
use atrium_analysis::{
    analyse, compare, comparison_picture, comparison_summary, picture, summary, Compared, Report,
    Settings, Signal, REPORT_VERSION,
};
use serde_json::{json, Value};

const USAGE: &str = "\
usage: analyse <file.wav|mp3>... [options]
       analyse --compare <reference> <other>... [options]

  <file>@<from>-<to>   only this stretch of the file, in seconds (file.mp3@30-60)
  --compare            compare: the first file is the reference; each file is
                       audio (analysed) or a report JSON (target/analysis/*.json)
  --from <s>           analyse from this time (default 0), where no @ says
  --to <s>             analyse up to this time (default: the end), where no @ says
  --output <path>      the JSON: the report (one file only; default
                       target/analysis/<stem>.json) or the comparison (default
                       target/analysis/<reference>-against-<others>.json)
  --picture[=<path>]   also draw the picture (default beside the JSON, .png;
                       a path only with one file, or with --compare)
  --summary <path>     also write the printed summaries to this file

A render's sidecar (<file>.json beside it) gives the channel names and the
calibration, so levels are in dB SPL; without one they are in dBFS.
";

/// A file and the stretch of it asked for.
#[derive(Debug, PartialEq)]
struct Input {
    path: PathBuf,
    /// From `@from-to`, in seconds.
    range: Option<(f64, f64)>,
}

/// What the command line asked for.
#[derive(Debug, PartialEq)]
struct Arguments {
    files: Vec<Input>,
    compare: bool,
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
    let result = if arguments.compare {
        run_comparison(&arguments)
    } else {
        run(&arguments)
    };
    if let Err(error) = result {
        eprintln!("analyse: {error}");
        std::process::exit(1);
    }
}

/// A decoded file, its sidecar, and the stretch of it analysed.
struct Audio {
    path: PathBuf,
    decoded: DecodedChannels,
    sidecar: Option<Value>,
    first: usize,
    end: usize,
    total_frames: usize,
}

impl Audio {
    fn load(input: &Input, arguments: &Arguments) -> Result<Self, Box<dyn std::error::Error>> {
        let file = &input.path;
        let decoded =
            decode_channels(file).map_err(|error| format!("{}: {error}", file.display()))?;
        let sidecar = read_sidecar(file)?;
        let rate = decoded.sample_rate as f64;
        let total_frames = decoded.interleaved.len() / decoded.channels;
        let (from, to) = match input.range {
            Some((from, to)) => (Some(from), Some(to)),
            None => (arguments.from, arguments.to),
        };
        let first = from
            .map_or(0, |seconds| (seconds * rate).round() as usize)
            .min(total_frames);
        let end = to
            .map_or(total_frames, |seconds| (seconds * rate).round() as usize)
            .min(total_frames);
        if end <= first {
            return Err(format!("{}: nothing in the stretch asked for", file.display()).into());
        }
        Ok(Self {
            path: file.clone(),
            decoded,
            sidecar,
            first,
            end,
            total_frames,
        })
    }

    fn seconds(&self, frame: usize) -> f64 {
        frame as f64 / self.decoded.sample_rate as f64
    }

    /// Frames `first` to `end` of the file, as the analyser takes them.
    fn signal(&self, first: usize, end: usize) -> Signal<'_> {
        let channels = self.decoded.channels;
        let channel_names = self
            .sidecar
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
        Signal {
            interleaved: &self.decoded.interleaved[first * channels..end * channels],
            channels,
            sample_rate: self.decoded.sample_rate,
            channel_names,
        }
    }

    fn cut(&self) -> Signal<'_> {
        self.signal(self.first, self.end)
    }

    fn settings(&self) -> Settings {
        Settings {
            full_scale_db_spl: self.sidecar.as_ref().and_then(full_scale_db_spl),
        }
    }

    fn heading(&self) -> String {
        heading(
            &self.path,
            self.sidecar.as_ref(),
            self.seconds(self.first),
            self.seconds(self.end),
            self.seconds(self.total_frames),
        )
    }

    /// The file's name, with the stretch when it is not the whole file.
    fn name(&self) -> String {
        let name = file_name(&self.path);
        if self.first > 0 || self.end < self.total_frames {
            format!(
                "{name}@{}-{}",
                trimmed(self.seconds(self.first)),
                trimmed(self.seconds(self.end))
            )
        } else {
            name
        }
    }

    /// How the report JSON says where it came from.
    fn input(&self) -> Value {
        json!({
            "file": self.path.display().to_string(),
            "from_seconds": self.seconds(self.first),
            "to_seconds": self.seconds(self.end),
            "sidecar": self.sidecar,
        })
    }
}

fn run(arguments: &Arguments) -> Result<(), Box<dyn std::error::Error>> {
    let mut summaries = String::new();
    for input in &arguments.files {
        let audio = Audio::load(input, arguments)?;
        let default = |extension: &str| default_path(&stem(&input.path), extension);
        let signal = audio.cut();
        let heading = audio.heading();

        let started = Instant::now();
        let report = analyse(&signal, &audio.settings());
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
            fields.insert("input".to_owned(), audio.input());
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

/// One file of a comparison: analysed from its samples, or read from its
/// report JSON.
struct Loaded {
    name: String,
    heading: String,
    timeline: Option<String>,
    report: Report,
    /// Each half analysed apart, for the reference: how far two stretches
    /// of its sound lie apart.
    halves: Option<[Report; 2]>,
    audio: Option<Audio>,
    /// Where it came from, for the comparison JSON.
    input: Value,
}

impl Loaded {
    fn compared(&self) -> Compared<'_> {
        Compared {
            name: self.name.clone(),
            heading: self.heading.clone(),
            report: &self.report,
            halves: self.halves.as_ref().map(|[first, second]| [first, second]),
            timeline: self.timeline.clone(),
            signal: self.audio.as_ref().map(Audio::cut),
        }
    }
}

fn load(
    input: &Input,
    arguments: &Arguments,
    with_halves: bool,
) -> Result<Loaded, Box<dyn std::error::Error>> {
    if is_report(&input.path) {
        if input.range.is_some() {
            return Err(format!(
                "{}: a report is already cut; give the range when analysing the file",
                input.path.display()
            )
            .into());
        }
        return read_report(&input.path);
    }
    let audio = Audio::load(input, arguments)?;
    let settings = audio.settings();
    let report = analyse(&audio.cut(), &settings);
    let halves = with_halves.then(|| {
        let middle = (audio.first + audio.end) / 2;
        [
            analyse(&audio.signal(audio.first, middle), &settings),
            analyse(&audio.signal(middle, audio.end), &settings),
        ]
    });
    Ok(Loaded {
        name: audio.name(),
        heading: audio.heading(),
        timeline: audio.sidecar.as_ref().and_then(|sidecar| {
            timeline(
                sidecar,
                audio.seconds(audio.first),
                audio.seconds(audio.end),
            )
        }),
        report,
        halves,
        input: audio.input(),
        audio: Some(audio),
    })
}

fn is_report(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

/// A report `analyse` wrote, with the file and stretch it says it is of.
fn read_report(path: &Path) -> Result<Loaded, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    let document: Value =
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    let version = document["report_version"].as_u64();
    if version != Some(u64::from(REPORT_VERSION)) {
        return Err(format!(
            "{}: report_version {version:?}, this analyser reads {REPORT_VERSION}",
            path.display()
        )
        .into());
    }
    let report: Report = serde_json::from_value(document.clone())
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let input = document["input"].clone();
    let sidecar = input.get("sidecar").filter(|sidecar| !sidecar.is_null());
    let from = input["from_seconds"].as_f64().unwrap_or(0.0);
    let to = input["to_seconds"]
        .as_f64()
        .unwrap_or(from + report.signal.seconds);
    let file = input["file"]
        .as_str()
        .map_or(path.to_owned(), PathBuf::from);
    let whole = sidecar
        .and_then(|sidecar| sidecar["seconds"].as_f64())
        .unwrap_or(to);
    Ok(Loaded {
        name: file_name(path),
        heading: format!(
            "report JSON of {}",
            heading(&file, sidecar, from, to, whole.max(to))
        ),
        timeline: sidecar.and_then(|sidecar| timeline(sidecar, from, to)),
        report,
        halves: None,
        audio: None,
        input: json!({ "report": path.display().to_string(), "of": input }),
    })
}

fn run_comparison(arguments: &Arguments) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let loaded: Vec<Loaded> = arguments
        .files
        .iter()
        .enumerate()
        .map(|(index, input)| load(input, arguments, index == 0))
        .collect::<Result<_, _>>()?;
    let analysed_in = started.elapsed().as_secs_f64();
    let compared: Vec<Compared> = loaded.iter().map(Loaded::compared).collect();
    let (reference, others) = compared.split_first().expect("two or more files");
    let comparison = compare(reference, others);

    let default = |extension: &str| {
        let others: Vec<String> = arguments.files[1..]
            .iter()
            .map(|input| stem(&input.path))
            .collect();
        default_path(
            &format!(
                "{}-against-{}",
                stem(&arguments.files[0].path),
                others.join("-")
            ),
            extension,
        )
    };
    let output = arguments.output.clone().unwrap_or_else(|| default("json"));
    let mut document = serde_json::to_value(&comparison)?;
    if let Value::Object(fields) = &mut document {
        fields.insert(
            "inputs".to_owned(),
            Value::Array(loaded.iter().map(|file| file.input.clone()).collect()),
        );
    }
    write(
        &output,
        (serde_json::to_string_pretty(&document)? + "\n").as_bytes(),
    )?;
    let picture_path = arguments
        .picture
        .as_ref()
        .map(|path| path.clone().unwrap_or_else(|| default("png")));
    if let Some(path) = &picture_path {
        write(path, &comparison_picture(reference, others))?;
    }

    let text = comparison_summary(&comparison);
    print!("{text}");
    println!(
        "{:<12}{} (analysed in {analysed_in:.2} s)",
        "wrote",
        output.display()
    );
    if let Some(path) = &picture_path {
        println!("{:<12}{}", "", path.display());
    }
    if let Some(path) = &arguments.summary {
        write(path, text.as_bytes())?;
        println!("{:<12}{}", "", path.display());
    }
    Ok(())
}

/// What fixes when things happen in a render: its scene, seed, start and
/// weather speed, and the stretch cut from it. The layout, mode, probe and
/// head turn change what is heard, not when, so two renders that differ
/// only in those line up. None for a file without a scene: a recording.
fn timeline(sidecar: &Value, from: f64, to: f64) -> Option<String> {
    let scene = sidecar["scene"]["path"].as_str()?;
    Some(format!(
        "{scene} | seed {} | start {} s | weather x{} | {from:.3}-{to:.3} s",
        sidecar["seed"],
        sidecar["start_seconds"].as_f64().unwrap_or(0.0),
        sidecar["weather_speed"].as_f64().unwrap_or(1.0),
    ))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "analysis".to_owned())
}

fn default_path(stem: &str, extension: &str) -> PathBuf {
    Path::new("target/analysis").join(format!("{stem}.{extension}"))
}

/// 28, 0.5, 12.25: seconds without trailing zeros.
fn trimmed(seconds: f64) -> String {
    let text = format!("{seconds:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
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

/// The file and how it was made, in one line; `from` to `to` of `whole`
/// seconds.
fn heading(file: &Path, sidecar: Option<&Value>, from: f64, to: f64, whole: f64) -> String {
    let name = file_name(file);
    // A stretch shorter than the file by more than a frame at 8 kHz.
    let cut = if from > 1e-4 || to < whole - 1e-4 {
        format!(" | {from:.1}-{to:.1} s of {whole:.1} s")
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

/// `file@from-to` as a file and a stretch; any other argument as a file. A
/// file whose own name ends in such a suffix is read as the file only when
/// the suffix is not a stretch.
fn input(argument: &str) -> Result<Input, String> {
    if let Some((path, stretch)) = argument.rsplit_once('@') {
        if let Some((from, to)) = stretch.split_once('-') {
            let seconds = |value: &str| value.parse::<f64>().ok().filter(|s| s.is_finite());
            if let (Some(from), Some(to)) = (seconds(from), seconds(to)) {
                if from < 0.0 || to <= from {
                    return Err(format!("{argument}: the stretch must run forwards from 0"));
                }
                return Ok(Input {
                    path: PathBuf::from(path),
                    range: Some((from, to)),
                });
            }
        }
    }
    Ok(Input {
        path: PathBuf::from(argument),
        range: None,
    })
}

fn parse(arguments: &[String]) -> Result<Arguments, String> {
    let mut parsed = Arguments {
        files: Vec::new(),
        compare: false,
        from: None,
        to: None,
        output: None,
        picture: None,
        summary: None,
    };
    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        if !argument.starts_with("--") {
            parsed.files.push(input(argument)?);
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
        if flag == "--compare" {
            parsed.compare = true;
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
    if parsed.compare {
        if parsed.files.len() < 2 {
            return Err("--compare needs a reference and at least one file to compare".into());
        }
    } else {
        if parsed.files.iter().any(|file| is_report(&file.path)) {
            return Err("a report JSON is read only with --compare".into());
        }
        if parsed.files.len() > 1 {
            if parsed.output.is_some() {
                return Err("--output names one report; give one file, or leave it out".into());
            }
            if matches!(parsed.picture, Some(Some(_))) {
                return Err(
                    "--picture=<path> names one picture; give one file, or leave the path out"
                        .into(),
                );
            }
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

    fn whole(path: &str) -> Input {
        Input {
            path: PathBuf::from(path),
            range: None,
        }
    }

    #[test]
    fn options_and_files() {
        let read =
            arguments("a.wav b.mp3 --from 2 --to=5.5 --picture --summary s.txt").expect("reads");
        assert_eq!(read.files, [whole("a.wav"), whole("b.mp3")]);
        assert_eq!((read.from, read.to), (Some(2.0), Some(5.5)));
        assert_eq!(read.picture, Some(None));
        assert_eq!(read.summary, Some(PathBuf::from("s.txt")));
        assert!(!read.compare);
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

    #[test]
    fn a_comparison_and_its_stretches() {
        let read =
            arguments("--compare ref.mp3@0-28 a.json b.wav@1.5-3 --picture=c.png").expect("reads");
        assert!(read.compare);
        assert_eq!(
            read.files,
            [
                Input {
                    path: PathBuf::from("ref.mp3"),
                    range: Some((0.0, 28.0)),
                },
                whole("a.json"),
                Input {
                    path: PathBuf::from("b.wav"),
                    range: Some((1.5, 3.0)),
                },
            ]
        );
        assert_eq!(read.picture, Some(Some(PathBuf::from("c.png"))));
        // A name with an @ that is not a stretch is a name.
        assert_eq!(
            arguments("take@home.wav").expect("reads").files,
            [whole("take@home.wav")]
        );
        assert!(arguments("a.wav@5-2").is_err());
        assert!(arguments("--compare a.wav").is_err());
        assert!(arguments("a.json").is_err());
    }

    /// The analyser undoes `--normalise`: 114 dB SPL at RMS 1.0 for the
    /// engine, a file raised by 40 dB reads RMS 1.0 as 74 dB SPL.
    #[test]
    fn the_normalising_gain_is_subtracted() {
        let sidecar = json!({"calibration": {"full_scale_db_spl": 114.0}, "gain_decibels": 40.0});
        assert_eq!(full_scale_db_spl(&sidecar), Some(74.0));
        assert_eq!(full_scale_db_spl(&json!({"gain_decibels": 40.0})), None);
    }

    /// Two renders of one scene and seed over one stretch line up whatever
    /// their layout, mode or probe; another seed or stretch does not; a
    /// recording has no timeline.
    #[test]
    fn what_fixes_the_timeline() {
        let render = |seed: u64, mode: &str| {
            json!({
                "scene": {"path": "scenes/meadow-birds.yaml"},
                "seed": seed, "mode": mode, "start_seconds": 0.0, "weather_speed": 1.0,
                "probe": {"ambient": true}
            })
        };
        let first = timeline(&render(1, "hrtf"), 0.0, 60.0);
        assert!(first.is_some());
        assert_eq!(first, timeline(&render(1, "vbap"), 0.0, 60.0));
        assert_ne!(first, timeline(&render(2, "hrtf"), 0.0, 60.0));
        assert_ne!(first, timeline(&render(1, "hrtf"), 0.0, 30.0));
        assert_eq!(timeline(&json!({"gain_decibels": 0.0}), 0.0, 60.0), None);
    }

    #[test]
    fn stretches_are_named_without_trailing_zeros() {
        assert_eq!(trimmed(28.0), "28");
        assert_eq!(trimmed(1.5), "1.5");
        assert_eq!(trimmed(0.0), "0");
    }
}
