// Render a scene to a WAV file instead of the speakers, through the real
// engine and faster than real time, with a JSON sidecar beside it saying how
// it was made. The rendering is `atrium::offline`; this only reads the
// arguments and writes the files.
//
//   cargo run --release --bin render -- <scene.yaml> [options]
//
// Options are listed by `--help` and in run.md ("Render a scene to a file").

use std::path::{Path, PathBuf};
use std::time::Instant;

use atrium::audio::wav::SampleFormat;
use atrium::config::SceneConfig;
use atrium::offline::{write_files, Output, Provenance, RenderSettings, Rendering};
use atrium_core::probe::Probe;
use atrium_core::speaker::{channel_name, RenderMode, MAX_CHANNELS};

const USAGE: &str = "\
usage: render <scene.yaml> [options]

  --seconds <n>          how long to render (default 30)
  --start <seconds>      start this far into the scene, as the timeline seeks (default 0)
  --sample-rate <hz>     output rate (default 48000)
  --block <frames>       frames per engine call, as the audio callback (default 512)
  --headphones           binaural on two channels, whatever speakers the scene names
  --mode <name>          world_locked, vbap, hrtf, dbap or ambisonics on the scene's speakers
  --seed <n>             the session seed (default: the scene's seed:, else 1)
  --probe <list>         listen to parts alone, comma-separated:
                           direct, reflections, reverb, ambient   only these parts
                           none                                   no part at all
                           no-air-absorption, no-moving-air, no-ground-effect,
                           no-wall-colour, no-near-field, no-headphone-equalisation
                           channel=<L|R|C|LFE|Ls|Rs|index>         one channel alone
  --turn <deg/s>         the head turning steadily, positive to the left
  --weather-speed <n>    the weather this many times as fast (default 1)
  --format <f32|s16>     32-bit float (default) or 16-bit
  --normalise[=<dBFS>]   one gain on every channel so the loudest sample peaks
                         at -1 dBFS (or the level given); the gain is in the
                         sidecar. Without it the file is at the engine's own
                         level, often -50 dBFS or lower
  --output <path>        default target/renders/<scene>-<output>-seed<seed>….wav
";

/// Where `--normalise` puts the loudest sample when it names no level: just
/// under full scale, so a 16-bit file does not clip.
const NORMALISED_PEAK_DECIBELS: f64 = -1.0;

/// What the command line asked for.
struct Arguments {
    scene: String,
    settings: RenderSettings,
    probe_text: Option<String>,
    format: SampleFormat,
    output: Option<PathBuf>,
    /// The peak (dBFS) `--normalise` brings the render to, if asked.
    normalise: Option<f64>,
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
            eprintln!("render: {message}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(error) = render(arguments) {
        eprintln!("render: {error}");
        std::process::exit(1);
    }
}

fn render(arguments: Arguments) -> Result<(), Box<dyn std::error::Error>> {
    let Arguments {
        scene,
        settings,
        probe_text,
        format,
        output,
        normalise,
    } = arguments;
    let provenance = Provenance::of(&scene).map_err(|error| format!("{scene}: {error}"))?;
    println!("Loading scene: {scene}");
    let config = SceneConfig::load(&scene)?;

    let built_at = Instant::now();
    let rendering = Rendering::prepare(config, &settings)?;
    let built_in = built_at.elapsed().as_secs_f64();
    let output_name = match settings.output {
        Output::Headphones => "headphones".to_owned(),
        Output::Scene | Output::Mode(_) => rendering.render_mode().as_str().to_owned(),
    };
    let wav = output.unwrap_or_else(|| {
        default_output(
            &scene,
            &output_name,
            rendering.seed(),
            &settings,
            probe_text.as_deref(),
            normalise.is_some(),
        )
    });
    println!(
        "Rendering {} s from {} s, {output_name}, seed {}, {} Hz, {}-frame blocks",
        settings.seconds,
        settings.start,
        rendering.seed(),
        settings.sample_rate,
        settings.block_frames
    );

    let rendered_at = Instant::now();
    let mut rendered = rendering.run();
    if let Some(peak_decibels) = normalise {
        rendered.normalise(peak_decibels);
    }
    let rendered_in = rendered_at.elapsed().as_secs_f64();
    println!(
        "Built the scene in {built_in:.2} s; rendered {} s in {rendered_in:.2} s, {:.1}× real time",
        settings.seconds,
        settings.seconds / rendered_in.max(1e-9)
    );

    let written = write_files(&rendered, &provenance, &wav, format)?;
    println!("Wrote {}", written.wav.display());
    println!("      {}", written.sidecar.display());
    if format == SampleFormat::Pcm16 {
        println!("{} samples clipped", written.clipped);
    }
    if normalise.is_some() {
        println!(
            "Normalised: raised {:+.1} dB; the engine's level is the file's minus that",
            rendered.gain_decibels()
        );
    }
    let decibels = |level: f64| {
        if level > 0.0 {
            format!("{:7.1} dBFS", 20.0 * level.log10())
        } else {
            "silent".to_owned()
        }
    };
    for (name, statistics) in rendered.channel_names().iter().zip(rendered.statistics()) {
        println!(
            "  {name:>3}  peak {}  rms {}",
            decibels(statistics.peak as f64),
            decibels(statistics.rms)
        );
    }
    Ok(())
}

/// `target/renders/<scene>-<output>-seed<seed>`, with the start, the probe
/// and the turn when they are not the defaults, so different renders of one
/// scene do not overwrite each other.
fn default_output(
    scene: &str,
    output_name: &str,
    seed: u64,
    settings: &RenderSettings,
    probe_text: Option<&str>,
    normalised: bool,
) -> PathBuf {
    let stem = Path::new(scene)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "scene".to_owned());
    let mut name = format!("{stem}-{output_name}-seed{seed}");
    if settings.start > 0.0 {
        name += &format!("-from{}s", settings.start);
    }
    if let Some(probe) = probe_text {
        name += &format!("-{}", probe.replace([',', '='], "-"));
    }
    if settings.turn_degrees_per_second != 0.0 {
        name += &format!("-turn{}", settings.turn_degrees_per_second);
    }
    if normalised {
        name += "-normalised";
    }
    Path::new("target/renders").join(format!("{name}.wav"))
}

fn parse(arguments: &[String]) -> Result<Arguments, String> {
    let mut scene = None;
    let mut settings = RenderSettings::default();
    let mut probe_text = None;
    let mut format = SampleFormat::Float32;
    let mut output = None;
    let mut normalise = None;
    let mut headphones = false;
    let mut mode = None;

    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        if !argument.starts_with("--") {
            if scene.replace(argument.clone()).is_some() {
                return Err(format!("one scene only, and `{argument}` is a second"));
            }
            continue;
        }
        let (flag, inline) = match argument.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_owned())),
            None => (argument.as_str(), None),
        };
        if flag == "--headphones" {
            headphones = true;
            continue;
        }
        // Its level only with `=`, so `--normalise -3` is never misread.
        if flag == "--normalise" {
            let peak: f64 = match inline {
                Some(value) => number(flag, &value)?,
                None => NORMALISED_PEAK_DECIBELS,
            };
            if !peak.is_finite() || peak > 0.0 {
                return Err(format!(
                    "--normalise={peak} is not a peak at or below full scale; give 0 dBFS or less"
                ));
            }
            normalise = Some(peak);
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
            "--seconds" => settings.seconds = number(flag, &value)?,
            "--start" => settings.start = number(flag, &value)?,
            "--sample-rate" => settings.sample_rate = number(flag, &value)?,
            "--block" => settings.block_frames = number(flag, &value)?,
            "--mode" => {
                mode = Some(
                    RenderMode::ALL
                        .into_iter()
                        .find(|mode| mode.as_str() == value)
                        .ok_or_else(|| {
                            format!(
                                "--mode `{value}` is not one of world_locked, vbap, hrtf, dbap, ambisonics"
                            )
                        })?,
                )
            }
            "--seed" => settings.seed = Some(number(flag, &value)?),
            "--probe" => {
                settings.probe = parse_probe(&value)?;
                probe_text = Some(value);
            }
            "--turn" => settings.turn_degrees_per_second = number(flag, &value)?,
            "--weather-speed" => settings.weather_speed = number(flag, &value)?,
            "--format" => {
                format = match value.as_str() {
                    "f32" => SampleFormat::Float32,
                    "s16" => SampleFormat::Pcm16,
                    other => return Err(format!("--format `{other}` is not f32 or s16")),
                }
            }
            "--output" => output = Some(PathBuf::from(value)),
            other => return Err(format!("unknown option {other}")),
        }
    }
    settings.output = match (headphones, mode) {
        (true, Some(_)) => return Err("--headphones or --mode, not both".into()),
        (true, None) => Output::Headphones,
        (false, Some(mode)) => Output::Mode(mode),
        (false, None) => Output::Scene,
    };
    Ok(Arguments {
        scene: scene.ok_or("which scene? give a scene file")?,
        settings,
        probe_text,
        format,
        output,
        normalise,
    })
}

fn number<T: std::str::FromStr>(flag: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} `{value}` is not a number of the right kind"))
}

/// The probe a comma-separated list names. Naming parts lets only those
/// through; `none` lets no part through; `no-…` switches one effect off;
/// `channel=` leaves one channel alone.
fn parse_probe(text: &str) -> Result<Probe, String> {
    let mut probe = Probe::WHOLE;
    let mut parts_named = false;
    let mut part = |probe: &mut Probe, set: fn(&mut Probe)| {
        if !parts_named {
            parts_named = true;
            probe.direct = false;
            probe.reflections = false;
            probe.reverb = false;
            probe.ambient = false;
        }
        set(probe);
    };
    for word in text
        .split(',')
        .map(str::trim)
        .filter(|word| !word.is_empty())
    {
        match word {
            "direct" => part(&mut probe, |probe| probe.direct = true),
            "reflections" => part(&mut probe, |probe| probe.reflections = true),
            "reverb" => part(&mut probe, |probe| probe.reverb = true),
            "ambient" => part(&mut probe, |probe| probe.ambient = true),
            "none" => part(&mut probe, |_| {}),
            "no-air-absorption" => probe.air_absorption = false,
            "no-moving-air" => probe.moving_air = false,
            "no-ground-effect" => probe.ground_effect = false,
            "no-wall-colour" => probe.wall_colouring = false,
            "no-near-field" => probe.near_field = false,
            "no-headphone-equalisation" => probe.headphone_equalisation = false,
            _ => match word.strip_prefix("channel=") {
                Some(channel) => probe.only_channel = Some(parse_channel(channel)?),
                None => return Err(format!("--probe does not know `{word}`")),
            },
        }
    }
    Ok(probe)
}

/// A channel by its name (L, R, C, LFE, Ls, Rs, any case) or its index.
fn parse_channel(text: &str) -> Result<u8, String> {
    if let Ok(index) = text.parse::<u8>() {
        return Ok(index);
    }
    (0..MAX_CHANNELS)
        .map(|channel| (channel, channel_name(channel)))
        .find(|(_, name)| *name != "?" && name.eq_ignore_ascii_case(text))
        .map(|(channel, _)| channel)
        .map(|channel| channel as u8)
        .ok_or_else(|| format!("`{text}` is not a channel: L, R, C, LFE, Ls, Rs or an index"))
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
    fn options_are_read_with_a_space_or_an_equals_sign() {
        let read = arguments(
            "scenes/a.yaml --seconds 12.5 --start=20 --headphones --seed 7 --format s16 --turn -30",
        )
        .expect("reads");
        assert_eq!(read.scene, "scenes/a.yaml");
        assert_eq!(read.settings.seconds, 12.5);
        assert_eq!(read.settings.start, 20.0);
        assert_eq!(read.settings.output, Output::Headphones);
        assert_eq!(read.settings.seed, Some(7));
        assert_eq!(read.settings.turn_degrees_per_second, -30.0);
        assert_eq!(read.format, SampleFormat::Pcm16);
        assert_eq!(
            arguments("a.yaml --mode dbap")
                .expect("reads")
                .settings
                .output,
            Output::Mode(RenderMode::Dbap)
        );
        assert!(arguments("a.yaml --mode stereo").is_err());
        assert!(arguments("a.yaml --headphones --mode vbap").is_err());
        assert!(arguments("--seconds 3").is_err(), "no scene");
        assert!(arguments("a.yaml --seconds").is_err(), "no value");
    }

    #[test]
    fn normalise_takes_its_level_only_after_an_equals_sign() {
        assert_eq!(arguments("a.yaml").expect("reads").normalise, None);
        let plain = arguments("a.yaml --normalise --seconds 3").expect("reads");
        assert_eq!(plain.normalise, Some(NORMALISED_PEAK_DECIBELS));
        assert_eq!(plain.settings.seconds, 3.0);
        assert_eq!(
            arguments("a.yaml --normalise=-6").expect("reads").normalise,
            Some(-6.0)
        );
        assert!(arguments("a.yaml --normalise=2").is_err(), "above 0 dBFS");
        assert!(arguments("a.yaml --normalise=loud").is_err());
    }

    #[test]
    fn a_probe_names_parts_effects_and_a_channel() {
        let probe = parse_probe("reverb,ambient,no-air-absorption,channel=Ls").expect("reads");
        assert_eq!(
            probe,
            Probe {
                direct: false,
                reflections: false,
                air_absorption: false,
                only_channel: Some(4),
                ..Probe::WHOLE
            }
        );
        let none = parse_probe("none").expect("reads");
        assert!(!none.direct && !none.reflections && !none.reverb && !none.ambient);
        assert_eq!(
            parse_probe("no-near-field,channel=1").expect("reads"),
            Probe {
                near_field: false,
                only_channel: Some(1),
                ..Probe::WHOLE
            }
        );
        assert!(parse_probe("loud").is_err());
        assert!(parse_probe("channel=Q").is_err());
    }
}
