//! The lab's stage-1 checks (`docs/generator-lab-plan.md`, "How we know it
//! is right"), on the soft wind as `sources/soft-wind.yaml` makes it.

use std::sync::Arc;

use atrium_analysis::{Column, Live};
use atrium_core::commands::SynthParam;
use atrium_core::source::SoundSource;
use atrium_world::{Entity, EntityId, Moves, Place, Shape, Sound, When};

use super::*;
use crate::audio::decode::AudioBuffer;
use crate::audio::spectral_profile::SpectralProfile;
use crate::config::{soft_wind_of, SceneConfig, SoundBuilder};
use crate::engine::scene::AudioScene;
use crate::engine::tap::{output_tap, TapReader};
use crate::synth::soft_wind::SoftWindSource;

const SAMPLE_RATE: f32 = 48_000.0;
const BLOCK: usize = 512;
const PRESET: &str = "sources/soft-wind.yaml";

fn entity() -> Entity {
    Entity {
        name: None,
        color: None,
        group: None,
        sound: Sound {
            preset: Some(PRESET.into()),
            audio: None,
            level: None,
        },
        shape: Shape::default(),
        place: Place {
            x: 0.0,
            y: 2.0,
            z: 1.2,
        },
        when: When::default(),
        follows: None,
        moves: Moves::Still,
    }
}

/// The soft wind from its preset, drawn from `seed`.
fn soft_wind(seed: u64) -> SoftWindSource {
    soft_wind_of(&entity(), seed).expect("the soft wind builds")
}

/// `seconds` of `generator`, a copy of it played on from where it is.
fn render(generator: &SoftWindSource, seconds: f64) -> Vec<f32> {
    let mut generator = generator.clone();
    (0..(seconds * SAMPLE_RATE as f64).round() as usize)
        .map(|_| generator.next_sample(SAMPLE_RATE))
        .collect()
}

fn recording(samples: Vec<f32>) -> Arc<AudioBuffer> {
    Arc::new(AudioBuffer {
        samples,
        sample_rate: SAMPLE_RATE as u32,
        rms: 0.0,
        spectral_profile: SpectralProfile::default(),
    })
}

fn mean_square(samples: &[f32]) -> f64 {
    samples
        .iter()
        .map(|sample| (*sample as f64).powi(2))
        .sum::<f64>()
        / samples.len() as f64
}

/// The untouched working candidate's live values.
fn untouched(preset: &SoftWindSource) -> [f32; 6] {
    CandidateSettings::default().live_values(preset)
}

/// Controls that play `heard`, looping.
fn playing(heard: Candidate, preset: &SoftWindSource) -> Arc<LabControls> {
    let controls = Arc::new(LabControls::new(heard, untouched(preset)));
    controls.set_playing(true);
    controls
}

/// A voice whose generators are both the untouched soft wind at seed 1, cued
/// at `cue` s, and whose reference is `reference` at `reference_gain`.
fn voice(
    controls: &Arc<LabControls>,
    reference: &Arc<AudioBuffer>,
    reference_gain: f32,
    cue: f64,
    loop_seconds: f64,
) -> LabVoice {
    let kept = state_at(soft_wind(1), cue, SAMPLE_RATE);
    LabVoice::new(VoiceParts {
        controls: controls.clone(),
        sample_rate: SAMPLE_RATE,
        reference: reference.clone(),
        reference_gain,
        reference_from_seconds: 0.0,
        loop_seconds,
        previous: kept.clone(),
        working: kept,
    })
}

/// Play `blocks` blocks of `voice` as the scene does: the clock, a tick,
/// then the block's samples.
fn play(voice: &mut LabVoice, clock: &mut u64, blocks: usize, into: &mut Vec<f32>) {
    for _ in 0..blocks {
        voice.set_clock(*clock);
        voice.tick(BLOCK as f32 / SAMPLE_RATE);
        for _ in 0..BLOCK {
            into.push(voice.next_sample(SAMPLE_RATE));
        }
        *clock += BLOCK as u64;
    }
}

/// Switching between two independent noises at the same level: the
/// reference (the soft wind at seed 2, brought to seed 1's level) and the
/// working wind at seed 1, 1 125 times over two minutes. Within the 10 ms
/// switches no sample step is larger than the largest step either noise
/// takes by itself, and their power is the noises' own (the mean of the
/// two over the same samples) within 0.1 dB. A straight-line fade would
/// dip by 1.8 dB here.
#[test]
fn switching_between_two_independent_noises_neither_clicks_nor_dips() {
    let cue = 20.0;
    let seconds = 121.0;
    let ours = render(&state_at(soft_wind(1), cue, SAMPLE_RATE), seconds);
    let other = render(&state_at(soft_wind(2), cue, SAMPLE_RATE), seconds);
    let gain = (mean_square(&ours) / mean_square(&other)).sqrt() as f32;
    let reference = recording(other);
    let preset = soft_wind(1);

    let reference_alone = playing(Candidate::Reference, &preset);
    let working_alone = playing(Candidate::Working, &preset);
    let switching = playing(Candidate::Reference, &preset);
    let mut voices = [&reference_alone, &working_alone, &switching]
        .map(|controls| voice(controls, &reference, gain, cue, seconds));
    let mut heard: [Vec<f32>; 3] = Default::default();
    let mut clocks = [0u64; 3];
    let blocks = (120.0 * SAMPLE_RATE as f64) as usize / BLOCK;
    let mut switches = Vec::new();
    for block in 0..blocks {
        // Every 10 blocks (107 ms), once the start's fade-in is over.
        if block >= 50 && block % 10 == 0 {
            switching.hear(if switches.len() % 2 == 0 {
                Candidate::Working
            } else {
                Candidate::Reference
            });
            switches.push(block * BLOCK);
        }
        for ((voice, clock), into) in voices.iter_mut().zip(&mut clocks).zip(&mut heard) {
            play(voice, clock, 1, into);
        }
    }
    let [reference_heard, working_heard, switched] = heard;
    let fade = (voice::SWITCH_FADE_SECONDS * SAMPLE_RATE) as usize;
    let largest_step = |samples: &[f32], from: usize| {
        samples[from..]
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0f32, f32::max)
    };
    let own_largest =
        largest_step(&reference_heard, 50 * BLOCK).max(largest_step(&working_heard, 50 * BLOCK));
    let (mut switched_power, mut own_power, mut switch_largest) = (0.0, 0.0, 0.0f32);
    for &start in &switches {
        let span = start..start + fade;
        switch_largest = switch_largest.max(largest_step(&switched[start - 1..start + fade], 0));
        switched_power += mean_square(&switched[span.clone()]);
        own_power +=
            (mean_square(&reference_heard[span.clone()]) + mean_square(&working_heard[span])) / 2.0;
    }
    let held_db = 10.0 * (switched_power / own_power).log10();
    println!(
        "{} switches; largest step {switch_largest:.5} against the noises' own {own_largest:.5}; \
         level across the switches {held_db:+.3} dB",
        switches.len()
    );
    assert_eq!(switches.len(), 1120);
    assert!(
        switch_largest <= own_largest,
        "{switch_largest} > {own_largest}"
    );
    assert!(held_db.abs() < 0.1, "{held_db} dB");
    // Away from the switches the output is the candidate asked for.
    let settled = switches[3] + fade;
    assert_eq!(
        switched[settled..switches[4]],
        reference_heard[settled..switches[4]]
    );
}

/// The two generators share their seed and so their noise: between them the
/// fade keeps the amplitude, and two untouched candidates switched back and
/// forth are heard as one, within 10⁻⁵ of its peak (−100 dB).
#[test]
fn switching_between_the_generators_keeps_their_shared_noise_whole() {
    let preset = soft_wind(1);
    let silence = recording(Vec::new());
    let working_alone = playing(Candidate::Working, &preset);
    let switching = playing(Candidate::Working, &preset);
    let mut alone = voice(&working_alone, &silence, 1.0, 20.0, 30.0);
    let mut switched = voice(&switching, &silence, 1.0, 20.0, 30.0);
    let (mut alone_heard, mut switched_heard) = (Vec::new(), Vec::new());
    let (mut alone_clock, mut switched_clock) = (0, 0);
    for block in 0..400 {
        if block % 7 == 0 {
            switching.hear(if block % 14 == 0 {
                Candidate::Previous
            } else {
                Candidate::Working
            });
        }
        play(&mut alone, &mut alone_clock, 1, &mut alone_heard);
        play(&mut switched, &mut switched_clock, 1, &mut switched_heard);
    }
    let largest = alone_heard
        .iter()
        .zip(&switched_heard)
        .map(|(alone, switched)| (alone - switched).abs())
        .fold(0.0f32, f32::max);
    let peak = alone_heard
        .iter()
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    assert!(largest <= peak * 1e-5, "{largest} against a peak of {peak}");
}

/// Recalling the state at the cue: the wind cued 30 s into its run, looped
/// over 2.56 s (240 blocks). Its first pass is the run from the seed through the cue and on,
/// sample for sample (past the 10 ms the start fades in over, and the 5 ms
/// at each edge); every later pass, and a replay pressed mid-pass, is the
/// first pass again, bit for bit.
#[test]
fn a_wrap_recalls_the_run_at_the_cue_sample_for_sample() {
    let (cue, loop_seconds) = (30.0, 2.56);
    let preset = soft_wind(1);
    let controls = playing(Candidate::Working, &preset);
    let silence = recording(Vec::new());
    let mut voice = voice(&controls, &silence, 1.0, cue, loop_seconds);
    let loop_frames = (loop_seconds * SAMPLE_RATE as f64) as usize;
    let from_seed = render(&soft_wind(1), cue + loop_seconds);
    let after_cue = &from_seed[(cue * SAMPLE_RATE as f64) as usize..];

    let mut heard = Vec::new();
    let mut clock = 0;
    play(&mut voice, &mut clock, 3 * loop_frames / BLOCK, &mut heard);
    let edge = (voice::EDGE_FADE_SECONDS * SAMPLE_RATE) as usize;
    let start = (voice::SWITCH_FADE_SECONDS * SAMPLE_RATE) as usize;
    assert_eq!(
        heard[start..loop_frames - edge],
        after_cue[start..loop_frames - edge]
    );
    assert_eq!(
        heard[start..loop_frames],
        heard[loop_frames + start..2 * loop_frames]
    );
    assert_eq!(
        heard[loop_frames..2 * loop_frames],
        heard[2 * loop_frames..3 * loop_frames]
    );

    // A replay mid-pass fades out over 5 ms and starts the pass again.
    play(&mut voice, &mut clock, 100, &mut heard);
    let before = heard.len();
    controls.replay();
    play(&mut voice, &mut clock, loop_frames / BLOCK, &mut heard);
    let replayed = &heard[before + edge..];
    assert_eq!(
        replayed[..],
        heard[loop_frames..loop_frames + replayed.len()]
    );
}

/// A level or colour setting leaves the gusts where they were; a timing
/// setting moves them. Over ten minutes of the wind at seed 1, its moves
/// start at the same samples with gust brightness at 0.30 instead of 0.18
/// and with the body layer 6 dB down, and the lab says they line up; with
/// the rest between moves halved they start elsewhere, and the lab says
/// they no longer line up. Every timing setting is flagged so.
#[test]
fn a_timing_setting_is_flagged_and_level_or_colour_keep_the_gusts() {
    let preset = soft_wind(1);
    let moves_of = |settings: &CandidateSettings| {
        let mut generator = soft_wind(1);
        settings.apply(&mut generator);
        run(&generator, 600.0, SAMPLE_RATE, 10).moves
    };
    let previous = CandidateSettings::default();
    let baseline = moves_of(&previous);
    assert!(
        baseline.len() >= 4,
        "{} moves in ten minutes",
        baseline.len()
    );

    let mut brighter = CandidateSettings::default();
    brighter.values.insert("gust_brightness".into(), 0.30);
    let mut quieter_body = CandidateSettings::default();
    quieter_body.mix.set(
        Layer::Body,
        Fader {
            level_db: -6.0,
            ..Fader::default()
        },
    );
    for working in [&brighter, &quieter_body] {
        assert_eq!(moves_of(working), baseline);
        assert!(working.lines_up_with(&previous, &preset));
    }

    let mut restless = CandidateSettings::default();
    restless.values.insert("move_rest".into(), 0.5);
    let moved = moves_of(&restless);
    assert_ne!(moved, baseline);
    assert_ne!(moved[0].seconds, baseline[0].seconds);
    assert!(!restless.lines_up_with(&previous, &preset));
    for setting in SETTINGS
        .iter()
        .filter(|setting| setting.kind == SettingKind::Timing)
    {
        let mut changed = CandidateSettings::default();
        changed
            .values
            .insert(setting.name().into(), setting.value_in(&preset) * 0.5);
        assert!(
            !changed.lines_up_with(&previous, &preset),
            "{}",
            setting.name()
        );
    }
    println!(
        "moves at {:?} s; with the rest halved at {:?} s",
        baseline
            .iter()
            .map(|start| start.seconds.round())
            .collect::<Vec<_>>(),
        moved
            .iter()
            .map(|start| start.seconds.round())
            .collect::<Vec<_>>()
    );
}

/// A fader at 0 dB is the layer as the generator gives it: the untouched
/// mix renders the preset's wind sample for sample. A muted layer is
/// absent, and a solo leaves only the soloed layers.
#[test]
fn the_untouched_mix_is_the_generator_as_it_was() {
    let preset = soft_wind(1);
    let mut untouched = soft_wind(1);
    CandidateSettings::default().apply(&mut untouched);
    assert_eq!(render(&untouched, 2.0), render(&preset, 2.0));

    let own = [1.0, 2.0, 3.0, 4.0, 5.0];
    let mut mix = LayerMix::default();
    mix.set(
        Layer::Mid,
        Fader {
            mute: true,
            ..Fader::default()
        },
    );
    assert_eq!(mix.gains(own), [1.0, 2.0, 0.0, 4.0, 5.0]);
    mix.set(
        Layer::Air,
        Fader {
            level_db: -6.0,
            solo: true,
            mute: false,
        },
    );
    let gains = mix.gains(own);
    assert_eq!(gains[..4], [0.0; 4]);
    assert!((gains[4] - 5.0 * 10f32.powf(-0.3)).abs() < 1e-6);
}

/// Undo steps back through mixer changes; one drag of a fader is one step.
#[test]
fn undo_steps_back_through_mixer_changes() {
    let mut history = MixHistory::default();
    let mut mix = LayerMix::default();
    let change = |history: &mut MixHistory, mix: &mut LayerMix, layer, fader| {
        let mut next = *mix;
        next.set(layer, fader);
        history.record(mix, &next);
        *mix = next;
    };
    for level_db in [-1.0, -2.0, -3.0] {
        let fader = Fader {
            level_db,
            ..Fader::default()
        };
        change(&mut history, &mut mix, Layer::Body, fader);
    }
    let soloed = Fader {
        level_db: -3.0,
        solo: true,
        mute: false,
    };
    change(&mut history, &mut mix, Layer::Body, soloed);
    assert_eq!(
        history.undo().map(|mix| mix.fader(Layer::Body).solo),
        Some(false)
    );
    assert_eq!(history.undo(), Some(LayerMix::default()));
    assert!(!history.can_undo());
}

/// What makes a trim stale, held and not held.
#[test]
fn edits_make_trims_stale_per_candidate() {
    let mut trims = Trims::default();
    trims.matched_to([0.0, -1.8, -1.6]);
    trims.after(Edit::Mixer);
    assert!(trims.rematch_needed(Candidate::Working));
    assert!(!trims.rematch_needed(Candidate::Previous));
    assert_eq!(trims.trim_db(Candidate::Working), -1.6);
    trims.after(Edit::Loop);
    assert!(trims.rematch_needed(Candidate::Previous));
    assert!(!trims.rematch_needed(Candidate::Reference));

    let mut loose = Trims::default();
    loose.hold = false;
    loose.matched_to([0.0, -1.8, -1.6]);
    loose.after(Edit::Setting);
    assert_eq!(loose.trim_db(Candidate::Working), 0.0);
    assert!(!loose.matched(Candidate::Working));
    assert_eq!(loose.trim_db(Candidate::Previous), -1.8);
}

/// An empty room with one voice for the lab, its output tapped and analysed
/// as the app's live analysis does.
struct Stage {
    scene: AudioScene,
    sounds: SoundBuilder,
    reader: TapReader,
    live: Live,
    channels: usize,
    block: Vec<f32>,
}

const EMPTY_SCENE: &str = "\
environment: environments/small_atrium.yaml
atmosphere: atmospheres/default.yaml
listener:
  position: [0.0, 0.0, 1.2]
  yaw_degrees: 90
speakers:
  layout: \"stereo\"
  render_mode: dbap
sounds: {}
";

impl Stage {
    fn new() -> Self {
        let config: SceneConfig = serde_yaml::from_str(EMPTY_SCENE).expect("parses");
        let full_scale_db_spl = config.normalization.full_scale_db_spl();
        let built = config.build().expect("builds");
        let mut scene = built.scene;
        scene.sample_rate = SAMPLE_RATE;
        scene.init_pipelines();
        let channels = scene.speaker_layout.total_channels();
        let (tap, reader) = output_tap(channels, 8 * BLOCK);
        scene.output_tap = Some(tap);
        Self {
            scene,
            sounds: built.sounds,
            reader,
            live: Live::new(SAMPLE_RATE as u32, channels, full_scale_db_spl),
            channels,
            block: Vec::new(),
        }
    }

    /// Put `voice` in the first voice, as the soft wind would be there.
    fn put(&mut self, voice: LabVoice) {
        let id = EntityId::new("lab").expect("an id");
        let built = self.sounds.build(&id, &entity(), 0).expect("builds");
        self.scene.sources[0] = placed_like(voice, &built);
    }

    /// Render `blocks` blocks and return the analysis columns they finished.
    fn render(&mut self, blocks: usize) -> Vec<Column> {
        let mut output = vec![0.0; BLOCK * self.channels];
        for _ in 0..blocks {
            self.scene.render(&mut output, self.channels);
            while let Some(block) = self.reader.next_block(&mut self.block) {
                self.live.push(&self.block, block.clock_sample);
            }
        }
        self.live.take_columns()
    }
}

/// The level of `columns` from `from` to `to` s on the clock.
fn level_of(columns: &[Column], from: f64, to: f64) -> f64 {
    let mut meter = matching::LevelMeter::default();
    for column in columns {
        let seconds = column.clock_sample as f64 / SAMPLE_RATE as f64;
        if seconds >= from && seconds + Live::COLUMN_SECONDS <= to {
            meter.push(column.leq_overall_decibels, true);
        }
    }
    meter.reading().expect("it sounded").level_db
}

/// The trim is exact: the working wind looped over 12 s, its first pass at
/// 0 dB and its second, the same samples, trimmed 6 dB up, read 6.00 ±
/// 0.01 dB apart at the output tap by the analyser's A-weighted level while
/// sounding, in place, at its preset's 42 dB SPL.
#[test]
fn a_trim_of_six_decibels_reads_six_decibels_at_the_output_tap() {
    let preset = soft_wind(1);
    let controls = playing(Candidate::Working, &preset);
    let mut stage = Stage::new();
    stage.put(voice(&controls, &recording(Vec::new()), 1.0, 20.0, 12.0));
    // 12 s is 1 125 blocks of 512 at 48 kHz.
    let mut columns = stage.render(1125);
    controls.set_trim(Candidate::Working, 6.0);
    columns.extend(stage.render(1125 + 40));
    let first = level_of(&columns, 0.5, 11.5);
    let second = level_of(&columns, 12.5, 23.5);
    println!(
        "first pass {first:.3} dB SPL, trimmed pass {second:.3}: {:.4} dB apart",
        second - first
    );
    assert!(
        (second - first - 6.0).abs() <= 0.01,
        "{} dB",
        second - first
    );
    assert!(first > 30.0 && first < 60.0, "{first} dB SPL");
}

/// Run a level match on `stage` as the lab does: each candidate in turn,
/// counted from the first sample it is heard alone, until its meter is done.
fn run_match(
    stage: &mut Stage,
    controls: &LabControls,
    loop_frames: u64,
    trims_db: [f32; 3],
) -> MatchOutcome {
    let mut level_match = LevelMatch::new(trims_db);
    for _ in 0..(200.0 * SAMPLE_RATE as f64) as usize / BLOCK {
        let Some(measuring) = level_match.measuring() else {
            break;
        };
        if controls.asked_to_hear() != measuring {
            controls.hear(measuring);
        }
        for column in stage.render(1) {
            let Some((sounding, from)) = controls.sounding() else {
                continue;
            };
            if level_match.measuring() == Some(sounding) && column.clock_sample >= from {
                let first_half =
                    in_first_half(column.clock_sample, controls.origin_clock(), loop_frames);
                level_match.push(column.leq_overall_decibels, first_half);
            }
        }
    }
    level_match.outcome().expect("the match finished")
}

/// Matching two noises 6 dB apart, in place and alone: the reference is
/// the working wind's own passage 6 dB down, so the generators measure 6 dB
/// louder and are trimmed by −6.0 ± 0.1 dB; matched again at those trims,
/// the output tap reads the reference and the working wind within 0.1 dB.
#[test]
fn matching_two_noises_six_decibels_apart_trims_by_six() {
    let (cue, loop_seconds) = (20.0, 12.0);
    let down = 10f32.powf(-6.0 / 20.0);
    let passage = render(&state_at(soft_wind(1), cue, SAMPLE_RATE), loop_seconds);
    let reference = recording(passage.iter().map(|sample| sample * down).collect());
    let preset = soft_wind(1);
    let controls = playing(Candidate::Reference, &preset);
    let mut stage = Stage::new();
    stage.put(voice(&controls, &reference, 1.0, cue, loop_seconds));
    let loop_frames = (loop_seconds * SAMPLE_RATE as f64) as u64;

    let first = run_match(&mut stage, &controls, loop_frames, [0.0; 3]);
    let trims = first.trims_db;
    for candidate in Candidate::ALL {
        controls.set_trim(candidate, trims[candidate.index()]);
    }
    let second = run_match(&mut stage, &controls, loop_frames, trims);
    let reading = |outcome: &MatchOutcome, candidate: Candidate| {
        outcome.readings[candidate.index()].expect("it sounded")
    };
    for candidate in [Candidate::Previous, Candidate::Working] {
        let trim = trims[candidate.index()];
        let after = second.difference_db(candidate).expect("both sounded");
        println!(
            "{}: trim {trim:+.3} dB; matched, {after:+.3} dB from the reference; halves {:.2} dB apart",
            candidate.name(),
            reading(&first, candidate).halves_apart_db().unwrap_or(f64::NAN)
        );
        assert!((trim + 6.0).abs() <= 0.1, "{trim}");
        assert!(after.abs() <= 0.1, "{after}");
    }
    assert!(reading(&first, Candidate::Reference).sounding_seconds >= SOUNDING_SECONDS);
}

/// Reopening a session restores the loop, the seed and the settings: what
/// is saved is read back the same, and the working candidate rebuilt from
/// the file renders the same passage as before, sample for sample.
#[test]
fn reopening_a_session_restores_loop_seed_and_settings() {
    let preset = soft_wind(1);
    let previous = CandidateSettings::default();
    let mut working = CandidateSettings::default();
    working.values.insert("gust_brightness".into(), 0.30);
    working.mix.set(
        Layer::Body,
        Fader {
            level_db: -2.0,
            solo: false,
            mute: false,
        },
    );
    let (seed, cue) = (7, 41.2);
    let mut working_saved = SessionCandidate::of(&working, seed, cue);
    working_saved.changed = working.changed_from(&previous, &preset);
    let mut session = Session {
        sound: "soft-wind".into(),
        question: "Does the gust fade naturally?".into(),
        reference: Some(ReferencePassage {
            file: "research papers/soft wind.mp3".into(),
            passage: "Gust 1".into(),
            from: 12.4,
            to: 19.0,
        }),
        candidates: SessionCandidates {
            previous: SessionCandidate::of(&previous, seed, cue),
            working: working_saved,
        },
        looping: true,
        trims_db: [("previous".to_owned(), -1.8), ("working".to_owned(), -1.6)].into(),
        hold_level_match: true,
        mode: "in place, alone".into(),
        notes: vec![SessionNote {
            at: 3.2,
            heard: "working".into(),
            moment: 44.4,
            text: "the air drops away".into(),
        }],
        verdict: "the fade sounds better, but the onset is still too sharp".into(),
        ..Session::default()
    };
    let path = std::env::temp_dir().join(format!("atrium-lab-{}/session.json", std::process::id()));
    session.save(&path).expect("saved");
    let reopened = Session::load(&path).expect("read back");
    let _ = std::fs::remove_dir_all(path.parent().expect("a folder"));
    assert_eq!(reopened, session);
    assert_eq!(
        reopened.candidates.working.changed["gust_brightness"],
        [0.18, 0.30]
    );
    assert_eq!(reopened.candidates.working.changed["body_db"], [0.0, -2.0]);
    let restored = reopened.candidates.working.settings();
    assert_eq!(restored, working);

    let passage = |settings: &CandidateSettings, seed: u64, cue: f64| {
        let mut generator = soft_wind(seed);
        settings.apply(&mut generator);
        render(&state_at(generator, cue, SAMPLE_RATE), 6.6)
    };
    let saved = &reopened.candidates.working;
    assert_eq!(
        passage(&restored, saved.seed, saved.cue),
        passage(&working, seed, cue)
    );
}

/// A setting the lab does not know is not one, and the ones it knows carry
/// their kind.
#[test]
fn settings_are_known_by_name() {
    assert!(setting_named("gust_brightness").is_some_and(|setting| setting.in_behaviour));
    assert_eq!(
        setting_named("move_rest").map(|setting| setting.kind),
        Some(SettingKind::Timing)
    );
    assert!(setting_named("low_gain").is_none());
    assert_eq!(
        SynthParam::named("gust_brightness"),
        Some(SynthParam::GustBrightness)
    );
}
