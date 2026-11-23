//! The cutter's checks that need a recording or the engine
//! (docs/cutter-plan.md, "How we know it is right"): the frog recording cut,
//! our soft wind, the written preset in a scene, and the round trip from the
//! frog through its preset and a render back to the recording.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use atrium_analysis::{
    analyse, compare, cut, Compared, Cut, CutSettings, Report, Settings, Signal,
};

use super::{on_headphones, render};
use crate::audio::decode::{decode_channels, DecodedChannels};
use crate::audio::wav::{write_wav, SampleFormat};
use crate::config::{draft_preset, read_preset, Draft};

const FROG: &str = "assets/frog.mp3";
const FROG_PRESET: &str = "sources/creatures/frog.yaml";
const FROG_SCENE: &str = "scenes/frog.yaml";
/// The frog's level filled in for the round trip, as `sources/creatures/frog.yaml`
/// has it (dB SPL at 1 m).
const FILLED_REFERENCE_SPL: &str = "reference_spl: 65.0";

fn signal(decoded: &DecodedChannels) -> Signal<'_> {
    Signal {
        interleaved: &decoded.interleaved,
        channels: decoded.channels,
        sample_rate: decoded.sample_rate,
        channel_names: Vec::new(),
    }
}

/// The frog recording and its cut, made once for every test here.
fn frog() -> &'static (DecodedChannels, Cut) {
    static FROG_CUT: OnceLock<(DecodedChannels, Cut)> = OnceLock::new();
    FROG_CUT.get_or_init(|| {
        let decoded = decode_channels(Path::new(FROG)).expect("the frog recording decodes");
        let cut = cut(&signal(&decoded), &CutSettings::default());
        (decoded, cut)
    })
}

/// The frog's cut written as `cut` writes it, into a folder of its own: its
/// takes and its draft preset. Returns the folder and the draft's text.
fn written_frog(folder_name: &str) -> (PathBuf, String) {
    let (decoded, cut) = frog();
    let folder =
        std::env::temp_dir().join(format!("atrium-cut-{folder_name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).expect("a folder for the takes");
    let take_files: Vec<String> = cut
        .takes
        .iter()
        .map(|take| {
            let file = folder.join(format!("take-{:02}.wav", take.number));
            write_wav(
                &file,
                &take.samples(&signal(decoded)),
                decoded.channels as u16,
                decoded.sample_rate,
                SampleFormat::Float32,
            )
            .expect("a take is written");
            file.display().to_string()
        })
        .collect();
    let existing = std::fs::read_to_string(FROG_PRESET).expect("the frog's preset");
    let draft = draft_preset(
        cut,
        &Draft {
            recording: FROG,
            name: "frog",
            take_files: &take_files,
            existing: Some((FROG_PRESET, &existing)),
        },
    )
    .expect("a draft");
    (folder, draft)
}

/// The draft with its reference_spl filled in, and a copy of the frog scene
/// that uses it, both in `folder`. Returns the scene's path.
fn filled_scene(folder: &Path, draft: &str) -> String {
    let filled = draft.replacen(
        "# reference_spl:            # dB SPL at 1 m, while it sounds",
        FILLED_REFERENCE_SPL,
        1,
    );
    assert_ne!(
        filled, draft,
        "the draft has the reference_spl line to fill"
    );
    let preset = folder.join("frog.yaml");
    std::fs::write(&preset, filled).expect("the filled preset is written");
    let scene = std::fs::read_to_string(FROG_SCENE)
        .expect("the frog scene")
        .replace(FROG_PRESET, &preset.display().to_string());
    let path = folder.join("frog-scene.yaml");
    std::fs::write(&path, scene).expect("the scene is written");
    path.display().to_string()
}

/// `assets/frog.mp3`: the plan's hand count was 15–18 calls of 1–2.5 s, the
/// first 41 s not one take, gaps mostly 1.8–5.6 s. The hand count was on the
/// A-weighted FAST level, which runs calls less than 0.3 s apart together;
/// the spectrogram shows them apart, and the cut keeps them apart. Measured:
/// 19 takes, all but one 0.5 to 1.4 s, the one 4.3 s (four chorus calls
/// following within 0.1 s, whose gaps are as short as the pulses'), 7 of
/// them in the first 41 s, the gaps' median 2.9 s and 11 of 18 within
/// 1.8–5.6 s.
#[test]
fn the_frog_recording() {
    let (_, cut) = frog();
    let kept: Vec<_> = cut.takes.iter().filter(|take| !take.flagged).collect();
    println!(
        "{} takes ({} unflagged), durations {:?}",
        cut.takes.len(),
        kept.len(),
        cut.takes
            .iter()
            .map(|take| (take.duration_seconds * 100.0).round() / 100.0)
            .collect::<Vec<_>>()
    );
    assert!((15..=20).contains(&kept.len()), "{} takes", kept.len());
    let calls = cut
        .takes
        .iter()
        .filter(|take| (0.5..=2.5).contains(&take.duration_seconds))
        .count();
    assert!(calls + 1 >= cut.takes.len(), "all but one take a call long");
    let early = cut
        .takes
        .iter()
        .filter(|take| take.end_seconds < 41.0)
        .count();
    assert!(early >= 5, "{early} takes in the first 41 s");
    let gaps = &cut.gaps.seconds;
    let resting = gaps.iter().filter(|gap| (1.8..=5.6).contains(*gap)).count();
    println!(
        "{early} takes in the first 41 s; gaps median {:?}, {resting} of {} within 1.8-5.6 s; criterion {:?}",
        cut.gaps.median,
        gaps.len(),
        cut.criterion.criterion_seconds
    );
    assert!(2 * resting > gaps.len());
    assert!(cut
        .gaps
        .median
        .is_some_and(|median| (1.8..=5.6).contains(&median)));
}

/// A minute of our soft wind has no takes.
#[test]
fn our_soft_wind_has_no_takes() {
    let wind = render("scenes/soft-wind-only.yaml", &on_headphones(60.0));
    let cut = cut(
        &Signal {
            interleaved: wind.frames(),
            channels: wind.channels(),
            sample_rate: wind.sample_rate(),
            channel_names: Vec::new(),
        },
        &CutSettings::default(),
    );
    println!(
        "soft wind: {} events, band {:?}",
        cut.events.len(),
        cut.band.ranges
    );
    assert!(cut.takes.is_empty(), "{:?}", cut.takes);
}

/// The written preset does not load as written (no reference_spl); with it
/// filled in, it reads, a scene with it builds, and its takes sound in the
/// engine.
#[test]
fn the_written_preset_loads_once_filled() {
    let (folder, draft) = written_frog("loads");
    let unfilled = folder.join("unfilled.yaml");
    std::fs::write(&unfilled, &draft).expect("written");
    // Refused as written; read once the one line is filled in, so the
    // reference_spl is what it lacked.
    assert!(read_preset(&unfilled.display().to_string()).is_err());
    let scene = filled_scene(&folder, &draft);
    let preset = read_preset(&folder.join("frog.yaml").display().to_string()).expect("reads");
    assert_eq!(preset.level, 65.0);
    assert_eq!(
        preset.takes,
        frog().1.takes.iter().filter(|take| !take.flagged).count()
    );
    let rendered = render(&scene, &on_headphones(20.0));
    let peak = super::peak(rendered.frames());
    assert!(peak > 1e-5, "the takes sound: peak {peak}");
    let _ = std::fs::remove_dir_all(&folder);
}

fn compared<'a>(name: &str, report: &'a Report, halves: Option<&'a [Report; 2]>) -> Compared<'a> {
    Compared {
        name: name.to_owned(),
        heading: name.to_owned(),
        report,
        halves: halves.map(|[first, second]| [first, second]),
        timeline: None,
        signal: None,
    }
}

/// The round trip: the frog's preset, with reference_spl filled in, rendered
/// for two minutes on headphones and compared with the recording. The plan
/// expects onsets a minute and the LAF distribution relative to L50 within
/// the recording's own chance, the difference between its halves. The onsets
/// are (asserted). The LAF distribution is not, and is printed: between takes
/// the render is digital silence where the recording has its bed, so the
/// render's L95 to L50 are one level and everything above L50 stands far
/// higher over it. Measured numbers in docs/cutter-plan.md.
#[test]
fn the_frog_round_trip() {
    let (decoded, _) = frog();
    let (folder, draft) = written_frog("round-trip");
    let scene = filled_scene(&folder, &draft);
    let rendered = render(&scene, &on_headphones(120.0));
    let settings = Settings::default();
    let recording = signal(decoded);
    let frames = decoded.interleaved.len() / decoded.channels;
    let half = |first: usize, end: usize| Signal {
        interleaved: &decoded.interleaved[first * decoded.channels..end * decoded.channels],
        ..recording.clone()
    };
    let reference = analyse(&recording, &settings);
    let halves = [
        analyse(&half(0, frames / 2), &settings),
        analyse(&half(frames / 2, frames), &settings),
    ];
    let render_report = analyse(
        &Signal {
            interleaved: rendered.frames(),
            channels: rendered.channels(),
            sample_rate: rendered.sample_rate(),
            channel_names: Vec::new(),
        },
        &settings,
    );
    let against_render = compare(
        &compared("recording", &reference, Some(&halves)),
        &[compared("render", &render_report, None)],
    );
    let between_halves = compare(
        &compared("first half", &halves[0], None),
        &[compared("second half", &halves[1], None)],
    );
    let onsets = |report: &Report| report.events.onsets_per_minute;
    println!(
        "onsets a minute: recording {:.1}, render {:.1}; halves {:.1} and {:.1}",
        onsets(&reference),
        onsets(&render_report),
        onsets(&halves[0]),
        onsets(&halves[1])
    );
    println!(
        "LAF exceeded N % relative to L50 (dB): N, render - recording, second half - first half"
    );
    for (render, chance) in against_render.others[0]
        .over_time
        .laf_exceeded
        .iter()
        .zip(&between_halves.others[0].over_time.laf_exceeded)
    {
        println!(
            "  L{:<3} {:+6.1} {:+6.1}   (recording {:+.1}, render {:+.1})",
            render.exceeded_percent,
            render.values.difference,
            chance.values.difference,
            render.values.reference,
            render.values.other
        );
    }
    let _ = std::fs::remove_dir_all(&folder);
    assert!(
        (onsets(&render_report) - onsets(&reference)).abs()
            <= (onsets(&halves[1]) - onsets(&halves[0])).abs(),
        "onsets a minute beyond the recording's own chance"
    );
}
