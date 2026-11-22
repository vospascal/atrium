//! The generator lab: the soft wind tuned against a reference recording,
//! from the audition voice, with the rest of the scene muted
//! (`docs/generator-lab-plan.md`, stage 1).
//!
//! The lab's voice plays the reference and the two generator candidates in
//! step; `atrium::lab` decides and plays, this module keeps the session and
//! hands its changes over. Building the voice runs the generators from their
//! seed to the cue, which takes a moment: the calls that do are marked, and
//! belong off a UI thread. `lab_state` is cheap, and is called every frame:
//! it also moves a level match along.

mod records;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use atrium::audio::decode::{decode_file, AudioBuffer};
use atrium::config::{read_preset, soft_wind_of};
use atrium::engine::edit::SceneEdit;
use atrium::lab::{
    in_first_half, placed_like, run, session_path, setting_named, state_at, Candidate,
    CandidateSettings, Edit, GeneratorRun, LabControls, LabVoice, Layer, LevelMatch, MatchOutcome,
    MixHistory, ReferencePassage, Session, SessionCandidate, SessionCandidates, SessionNote, Trims,
    VoiceParts, Waveform, SETTINGS,
};
use atrium::synth::soft_wind::SoftWindSource;
use atrium_core::commands::Command;
use atrium_world::{Entity, EntityId};

use super::{lock, Engine, AUDITION_SLOT};
use crate::api::EngineError;

use records::{
    LabCandidate, LabFader, LabNote, LabPassage, LabRecording, LabReopened, LabRun, LabSetting,
    LabState, LabTrim,
};

/// The sound stage 1 tunes, its preset and the name its lab folder takes.
const PRESET: &str = "sources/soft-wind.yaml";
const SOUND: &str = "soft-wind";
/// How long a seeded run is (s): the soft wind moves every half minute to a
/// minute and a half, so five minutes hold several gusts to choose from.
const RUN_SECONDS: f64 = 300.0;
/// How many stretches a waveform is drawn in.
const WAVEFORM_COLUMNS: usize = 1200;
/// The shortest passage (s).
const SHORTEST_PASSAGE: f64 = 0.25;
/// How long a playing voice about to be replaced is given to fade out.
const FADE_OUT_WAIT: Duration = Duration::from_millis(25);
/// How the candidates are heard in stage 1.
const MODE: &str = "in place, alone";

#[uniffi::export]
impl Engine {
    /// The lab as it is now. Call it every frame: a level match under way
    /// moves on in it.
    pub fn lab_state(&self) -> LabState {
        let columns = lock(&self.analysis).lab_columns();
        let voice_in_place = lock(&self.edits).lab_voice;
        let mut lab = lock(&self.lab);
        let controls = lab.controls.clone();
        let loop_frames = lab.loop_frames;
        // A preset heard from the audition voice took the lab's place: a
        // match under way would wait for it for ever.
        if !voice_in_place {
            lab.cancel_match(&self.analysis);
        }
        if let Some(level_match) = &mut lab.level_match {
            for column in columns {
                let Some((sounding, from)) = controls.sounding() else {
                    continue;
                };
                if level_match.measuring() == Some(sounding) && column.clock_sample >= from {
                    let first_half =
                        in_first_half(column.clock_sample, controls.origin_clock(), loop_frames);
                    level_match.push(column.leq_overall_decibels, first_half);
                }
            }
            match level_match.measuring() {
                Some(candidate) if controls.asked_to_hear() != candidate => {
                    controls.hear(candidate)
                }
                Some(_) => {}
                None => {
                    let outcome = level_match.outcome();
                    lab.finish_match(outcome);
                    lock(&self.analysis).listen_for_lab(false);
                }
            }
        }
        // A pass played once has ended: the scene comes back.
        if lab.alone && lab.level_match.is_none() && !controls.asked_to_play() {
            self.set_alone(&mut lab, false);
        }
        lab.state(voice_in_place)
    }

    /// Open `file` as the reference recording. A passage marked in another
    /// recording is let go. Decoding takes a moment.
    pub fn lab_open_recording(&self, file: String) -> Result<LabRecording, EngineError> {
        let recording = Recording::open(&file)?;
        let opened = LabRecording {
            file,
            waveform: (&recording.waveform).into(),
        };
        let mut lab = lock(&self.lab);
        lab.recording = Some(recording);
        lab.passage = None;
        lab.trims.after(Edit::Reference);
        lab.cancel_match(&self.analysis);
        lab.controls.set_playing(false);
        self.set_alone(&mut lab, false);
        Ok(opened)
    }

    /// Run the working generator from `seed`, offline, and report its
    /// waveform and the moves its gust driver made: where the generator's
    /// markers snap to. A marked passage plays from the new seed. Takes a
    /// moment.
    pub fn lab_generator_run(&self, seed: u32) -> Result<LabRun, EngineError> {
        let working = lock(&self.lab).working.clone();
        let run = self.render_run(seed, &working)?;
        let replace = {
            let mut lab = lock(&self.lab);
            let changed = lab.seed != seed;
            lab.seed = seed;
            lab.wind_power = Some(run.power_while_sounding);
            if changed {
                lab.trims.after(Edit::Loop);
                lab.cancel_match(&self.analysis);
            }
            changed && lab.passage.is_some()
        };
        if replace {
            self.place_lab_voice()?;
        }
        Ok(LabRun {
            seed,
            waveform: (&run.waveform).into(),
            moves: run.moves.iter().map(Into::into).collect(),
        })
    }

    /// Mark `passage` and loop it: the recording's stretch, and the same
    /// length of the seeded run from `generator_cue`. The generators are run
    /// from their seed to the cue, which takes a moment.
    pub fn lab_mark_passage(&self, passage: LabPassage) -> Result<(), EngineError> {
        {
            let mut lab = lock(&self.lab);
            let Some(recording) = &lab.recording else {
                return Err(lab_error("open a recording before marking a passage"));
            };
            let seconds = recording.seconds();
            let from = (passage.reference_from as f64).clamp(0.0, seconds);
            let to = (passage.reference_to as f64).clamp(0.0, seconds);
            let numbers = [
                passage.reference_from,
                passage.reference_to,
                passage.generator_cue,
            ];
            if numbers.iter().any(|number| !number.is_finite()) || to - from < SHORTEST_PASSAGE {
                return Err(lab_error(
                    "a passage needs its end at least a quarter second after its start",
                ));
            }
            let name = match passage.name.trim() {
                "" => "Passage".to_owned(),
                name => name.to_owned(),
            };
            lab.passage = Some(Passage {
                name,
                from,
                to,
                cue: (passage.generator_cue as f64).clamp(0.0, RUN_SECONDS),
            });
            lab.trims.after(Edit::Loop);
            lab.cancel_match(&self.analysis);
        }
        self.place_lab_voice()
    }

    /// Play the passage, the rest of the scene muted, or pause it there and
    /// let the scene back in.
    pub fn lab_play(&self, playing: bool) -> Result<(), EngineError> {
        if playing && !lock(&self.edits).lab_voice {
            self.place_lab_voice()?;
        }
        let mut lab = lock(&self.lab);
        if !playing {
            lab.cancel_match(&self.analysis);
        }
        lab.controls.set_playing(playing);
        self.set_alone(&mut lab, playing);
        Ok(())
    }

    /// Repeat the passage, or play it once.
    pub fn lab_set_looping(&self, looping: bool) {
        let mut lab = lock(&self.lab);
        match &mut lab.before_match {
            Some((_, after_match)) => *after_match = looping,
            None => lab.controls.set_looping(looping),
        }
    }

    /// Start the passage again from its cues, playing, even mid-pass.
    pub fn lab_replay(&self) -> Result<(), EngineError> {
        if !lock(&self.edits).lab_voice {
            self.place_lab_voice()?;
        }
        let mut lab = lock(&self.lab);
        lab.controls.replay();
        self.set_alone(&mut lab, true);
        Ok(())
    }

    /// Hear `candidate`, over a 10 ms fade. A level match under way stops.
    pub fn lab_hear(&self, candidate: LabCandidate) {
        let mut lab = lock(&self.lab);
        lab.cancel_match(&self.analysis);
        lab.controls.hear(candidate.into());
    }

    /// Set one strip of the working generator's mixer, heard at once.
    pub fn lab_set_fader(&self, fader: LabFader) -> Result<(), EngineError> {
        let mut lab = lock(&self.lab);
        let before = lab.working.mix;
        let mut next = before;
        next.set(fader.layer.into(), fader.fader());
        if next == before {
            return Ok(());
        }
        lab.mix_history.record(&before, &next);
        lab.working.mix = next;
        lab.after_working_change(Edit::Mixer, &self.analysis)
    }

    /// Take back the last mixer change.
    pub fn lab_undo_mix(&self) -> Result<(), EngineError> {
        let mut lab = lock(&self.lab);
        let Some(mix) = lab.mix_history.undo() else {
            return Ok(());
        };
        lab.working.mix = mix;
        lab.after_working_change(Edit::Mixer, &self.analysis)
    }

    /// Set the working generator's setting `name` to `value`, within its
    /// range. A level or colour setting is heard at once; a timing setting
    /// runs the generator from its seed to the cue again, which takes a
    /// moment, and its gusts no longer line up with the marked one. A name
    /// the lab does not know is an error.
    pub fn lab_set_setting(&self, name: String, value: f32) -> Result<(), EngineError> {
        let Some(setting) = setting_named(&name) else {
            return Err(lab_error(&format!(
                "the soft wind has no setting `{name}` the lab changes"
            )));
        };
        if !value.is_finite() {
            return Ok(());
        }
        let timing = {
            let mut lab = lock(&self.lab);
            lab.working
                .values
                .insert(name, value.clamp(setting.low, setting.high));
            let timing = setting.kind == atrium::lab::SettingKind::Timing;
            if timing {
                lab.trims.after(Edit::Setting);
                lab.cancel_match(&self.analysis);
            } else {
                lab.after_working_change(Edit::Setting, &self.analysis)?;
            }
            timing && lab.passage.is_some()
        };
        if timing {
            self.place_lab_voice()?;
        }
        Ok(())
    }

    /// Let the generator run on over the rest of the recording instead of
    /// the marked gust, to compare spacing and variation; or loop the marked
    /// passage again. Takes a moment.
    pub fn lab_set_run_freely(&self, run_freely: bool) -> Result<(), EngineError> {
        let replace = {
            let mut lab = lock(&self.lab);
            if lab.run_freely == run_freely {
                return Ok(());
            }
            lab.run_freely = run_freely;
            lab.trims.after(Edit::Loop);
            lab.cancel_match(&self.analysis);
            lab.passage.is_some()
        };
        if replace {
            self.place_lab_voice()?;
        }
        Ok(())
    }

    /// Measure every candidate alone at the output tap and trim the
    /// generators to the reference. It plays the passage, looping, each
    /// candidate in turn until 10 s of it have sounded (60 s at most), and
    /// then goes back to the candidate that was heard. `lab_state` shows it
    /// under way and moves it along.
    pub fn lab_match_levels(&self) -> Result<(), EngineError> {
        if !lock(&self.edits).lab_voice {
            self.place_lab_voice()?;
        }
        let mut lab = lock(&self.lab);
        lab.cancel_match(&self.analysis);
        let trims = Candidate::ALL.map(|candidate| lab.trims.trim_db(candidate));
        lab.level_match = Some(LevelMatch::new(trims));
        lab.before_match = Some((lab.controls.asked_to_hear(), lab.controls.looping()));
        lab.controls.set_looping(true);
        lab.controls.set_playing(true);
        self.set_alone(&mut lab, true);
        lock(&self.analysis).listen_for_lab(true);
        Ok(())
    }

    /// Keep the trims through edits, marking them for a rematch, or drop a
    /// trim an edit makes stale.
    pub fn lab_set_hold_level_match(&self, hold: bool) {
        lock(&self.lab).trims.hold = hold;
    }

    /// Pin `text` to the moment heard now: the lab's playhead, and the same
    /// moment in the recording or the seeded run.
    pub fn lab_pin_note(&self, text: String) {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let mut lab = lock(&self.lab);
        let at = lab.controls.position_frames() as f64 / lab.sample_rate as f64;
        let heard = lab
            .controls
            .sounding()
            .map_or(lab.controls.asked_to_hear(), |(candidate, _)| candidate);
        let start = lab.passage.as_ref().map_or(0.0, |passage| match heard {
            Candidate::Reference => passage.from,
            _ => passage.cue,
        });
        lab.notes.push(SessionNote {
            at,
            heard: heard.name().to_owned(),
            moment: start + at,
            text,
        });
    }

    /// What this round listens for.
    pub fn lab_set_question(&self, question: String) {
        lock(&self.lab).question = question;
    }

    /// The round's verdict, in the listener's own words.
    pub fn lab_set_verdict(&self, verdict: String) {
        lock(&self.lab).verdict = verdict;
    }

    /// Save the session as `lab/soft-wind/session.json`, so it can be
    /// reopened with its loop, seed, settings, trims, notes and verdict.
    /// Returns the file written.
    pub fn lab_save(&self) -> Result<String, EngineError> {
        let mut lab = lock(&self.lab);
        let preset = lab.preset()?.clone();
        let mut session = lab.session(&preset);
        let path = session_path(SOUND);
        session
            .save(&path)
            .map_err(|error| lab_error(&error.to_string()))?;
        lab.saved_session = true;
        Ok(path.to_string_lossy().into_owned())
    }

    /// Open the saved session again: its recording and passage, seed,
    /// settings, trims, notes and verdict, the voice built and paused.
    /// Takes a moment.
    pub fn lab_reopen(&self) -> Result<LabReopened, EngineError> {
        let session =
            Session::load(&session_path(SOUND)).map_err(|error| lab_error(&error.to_string()))?;
        let recording = match &session.reference {
            Some(reference) => Some(Recording::open(&reference.file)?),
            None => None,
        };
        let previous = session.candidates.previous.settings();
        let working = session.candidates.working.settings();
        let seed = session.candidates.working.seed as u32;
        let run = self.render_run(seed, &working)?;
        let opened = recording.as_ref().map(|recording| LabRecording {
            file: recording.file.clone(),
            waveform: (&recording.waveform).into(),
        });
        let has_passage = {
            let mut lab = lock(&self.lab);
            lab.cancel_match(&self.analysis);
            lab.controls.set_playing(false);
            self.set_alone(&mut lab, false);
            lab.restore(&session, recording, previous, working, seed);
            lab.wind_power = Some(run.power_while_sounding);
            lab.passage.is_some()
        };
        if has_passage {
            self.place_lab_voice()?;
        }
        Ok(LabReopened {
            recording: opened,
            run: Some(LabRun {
                seed,
                waveform: (&run.waveform).into(),
                moves: run.moves.iter().map(Into::into).collect(),
            }),
        })
    }
}

impl Engine {
    /// The soft wind as the lab plays it: its preset, just ahead of the
    /// listener.
    fn lab_entity(&self) -> Entity {
        self.sound_ahead(PRESET.into(), None, None)
    }

    /// The working generator, as `working` sets it up, run from `seed`.
    fn render_run(
        &self,
        seed: u32,
        working: &CandidateSettings,
    ) -> Result<GeneratorRun, EngineError> {
        let sample_rate = lock(&self.lab).sample_rate as f32;
        let mut generator = soft_wind_of(&self.lab_entity(), seed as u64)
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        working.apply(&mut generator);
        Ok(run(&generator, RUN_SECONDS, sample_rate, WAVEFORM_COLUMNS))
    }

    /// Build the lab's voice for the marked passage and put it in the
    /// audition voice, in place of whatever plays there; a voice of the lab
    /// already playing fades out first. Takes a moment.
    fn place_lab_voice(&self) -> Result<(), EngineError> {
        if self.audition_voice_taken() {
            return Err(EngineError::NoFreeVoice);
        }
        // Built before the edit ring is locked: the lab's state is read every
        // frame and asks the ring whether the voice is in place.
        let (parts, sample_rate) = {
            let mut lab = lock(&self.lab);
            let Some(parts) = lab.parts() else {
                return Err(lab_error("mark a passage before playing it"));
            };
            (parts, lab.sample_rate)
        };
        let entity = self.lab_entity();
        let built = self
            .sounds
            .build(&EntityId::unused("lab", []), &entity, AUDITION_SLOT)
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        let preset = soft_wind_of(&entity, parts.seed as u64)
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        let rate = sample_rate as f32;
        let mut previous = preset.clone();
        parts.previous.apply(&mut previous);
        let previous = state_at(previous, parts.passage.cue, rate);
        // The same timing makes the same run: only the colour and level
        // differ, and the voice puts those on top of the kept state.
        let working = if parts.working.lines_up_with(&parts.previous, &preset) {
            previous.clone()
        } else {
            let mut working = preset.clone();
            parts.working.apply(&mut working);
            state_at(working, parts.passage.cue, rate)
        };
        let loop_seconds = if parts.run_freely {
            parts.recording.seconds() - parts.passage.from
        } else {
            parts.passage.to - parts.passage.from
        }
        .max(SHORTEST_PASSAGE);
        let reference_gain = match parts.wind_power {
            Some(wind) if wind > 0.0 && parts.recording.power > 0.0 => {
                (wind / parts.recording.power).sqrt() as f32
            }
            _ => 1.0,
        };

        let controls = parts.controls;
        controls.set_working(parts.working.live_values(&preset));
        let mut edits = lock(&self.edits);
        let was_playing = edits.lab_voice && controls.asked_to_play();
        if was_playing {
            controls.set_playing(false);
            std::thread::sleep(FADE_OUT_WAIT);
        } else if !edits.lab_voice {
            // A preset auditioned there, and its plan, go.
            self.end_audition(&mut edits)?;
        }
        let voice = LabVoice::new(VoiceParts {
            controls: controls.clone(),
            sample_rate: rate,
            reference: parts.recording.buffer.clone(),
            reference_gain,
            reference_from_seconds: parts.passage.from,
            loop_seconds,
            previous,
            working,
        });
        edits.send(SceneEdit::AddSource {
            slot: AUDITION_SLOT as u16,
            source: placed_like(voice, &built),
            bands: built.bands,
            amplitude: built.amplitude,
        })?;
        edits.auditioned = true;
        edits.lab_voice = true;
        if was_playing {
            controls.set_playing(true);
        }
        lock(&self.lab).loop_frames = (loop_seconds * rate as f64).round() as u64;
        Ok(())
    }

    /// Mute every sound of the scene for the lab, or give each back the mute
    /// the listener gave it.
    fn set_alone(&self, lab: &mut Lab, alone: bool) {
        if lab.alone == alone {
            return;
        }
        lab.alone = alone;
        let scene = lock(&self.scene);
        for source in &scene.description.sources {
            if source.slot as usize == AUDITION_SLOT {
                continue;
            }
            let muted = alone || scene.muted.contains(&source.slot);
            // A full queue drops one mute; the next lab call sends them again.
            if self
                .command(Command::SetSourceMuted {
                    index: source.slot,
                    muted,
                })
                .is_err()
            {
                lab.alone = !alone;
            }
        }
    }
}

/// The lab's session, kept by the engine for the scene's lifetime.
pub(super) struct Lab {
    /// The rate the output plays at, and so the rate runs are made at.
    sample_rate: u32,
    sound_name: String,
    controls: Arc<LabControls>,
    /// The generator as its preset makes it, once read.
    preset: Option<SoftWindSource>,
    question: String,
    recording: Option<Recording>,
    seed: u32,
    /// The working generator's level while it sounds in its last run, in
    /// its own units: what brings the recording to the same level.
    wind_power: Option<f64>,
    passage: Option<Passage>,
    previous: CandidateSettings,
    working: CandidateSettings,
    mix_history: MixHistory,
    trims: Trims,
    run_freely: bool,
    level_match: Option<LevelMatch>,
    last_match: Option<MatchOutcome>,
    /// What a match changed, to put back after it: the candidate heard and
    /// whether the passage looped.
    before_match: Option<(Candidate, bool)>,
    notes: Vec<SessionNote>,
    verdict: String,
    /// Whether the rest of the scene is muted for the lab.
    alone: bool,
    /// How long the passage of the voice in place is (output frames).
    loop_frames: u64,
    saved_session: bool,
}

/// The reference recording, decoded.
struct Recording {
    file: String,
    buffer: Arc<AudioBuffer>,
    /// Its level while it sounds, as a power.
    power: f64,
    waveform: Waveform,
}

impl Recording {
    fn open(file: &str) -> Result<Self, EngineError> {
        let buffer =
            decode_file(Path::new(file)).map_err(|error| lab_error(&format!("{file}: {error}")))?;
        let power = atrium_analysis::power_while_sounding(&[(&buffer.samples, buffer.sample_rate)]);
        let waveform = Waveform::of(&buffer.samples, buffer.sample_rate as f32, WAVEFORM_COLUMNS);
        Ok(Self {
            file: file.to_owned(),
            buffer: Arc::new(buffer),
            power,
            waveform,
        })
    }

    fn seconds(&self) -> f64 {
        self.buffer.samples.len() as f64 / self.buffer.sample_rate.max(1) as f64
    }
}

/// The marked passage.
#[derive(Clone)]
struct Passage {
    name: String,
    /// In the recording (s).
    from: f64,
    to: f64,
    /// In the seeded run (s).
    cue: f64,
}

/// What building a voice needs, taken from the lab at once.
struct Parts {
    controls: Arc<LabControls>,
    recording: RecordingParts,
    passage: Passage,
    seed: u32,
    wind_power: Option<f64>,
    previous: CandidateSettings,
    working: CandidateSettings,
    run_freely: bool,
}

struct RecordingParts {
    buffer: Arc<AudioBuffer>,
    power: f64,
}

impl RecordingParts {
    fn seconds(&self) -> f64 {
        self.buffer.samples.len() as f64 / self.buffer.sample_rate.max(1) as f64
    }
}

impl Lab {
    pub(super) fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1),
            sound_name: read_preset(PRESET)
                .map_or_else(|_| "Soft wind".to_owned(), |preset| preset.name),
            controls: Arc::new(LabControls::new(Candidate::Working, [0.0; 6])),
            preset: None,
            question: String::new(),
            recording: None,
            seed: 1,
            wind_power: None,
            passage: None,
            previous: CandidateSettings::default(),
            working: CandidateSettings::default(),
            mix_history: MixHistory::default(),
            trims: Trims::default(),
            run_freely: false,
            level_match: None,
            last_match: None,
            before_match: None,
            notes: Vec::new(),
            verdict: String::new(),
            alone: false,
            loop_frames: 1,
            saved_session: session_path(SOUND).exists(),
        }
    }

    /// The generator as its preset makes it: what a fader at 0 dB and an
    /// untouched setting keep.
    fn preset(&mut self) -> Result<&SoftWindSource, EngineError> {
        if self.preset.is_none() {
            let entity = Entity {
                name: None,
                color: None,
                group: None,
                sound: atrium_world::Sound {
                    preset: Some(PRESET.into()),
                    audio: None,
                    level: None,
                },
                shape: Default::default(),
                place: atrium_world::Place {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
                when: Default::default(),
                follows: None,
                moves: atrium_world::Moves::Still,
            };
            let preset = soft_wind_of(&entity, self.seed as u64)
                .map_err(|error| EngineError::Scene(error.to_string()))?;
            self.preset = Some(preset);
        }
        Ok(self.preset.as_ref().expect("read just now"))
    }

    /// Note an edit of the working generator's level or colour, and let the
    /// voice hear it at once.
    fn after_working_change(
        &mut self,
        edit: Edit,
        analysis: &std::sync::Mutex<super::analysis::Analysis>,
    ) -> Result<(), EngineError> {
        self.trims.after(edit);
        self.cancel_match(analysis);
        let live = {
            let working = self.working.clone();
            working.live_values(self.preset()?)
        };
        self.controls.set_working(live);
        Ok(())
    }

    /// Stop a level match under way, putting back what it changed.
    fn cancel_match(&mut self, analysis: &std::sync::Mutex<super::analysis::Analysis>) {
        if self.level_match.take().is_some() {
            lock(analysis).listen_for_lab(false);
        }
        if let Some((heard, looping)) = self.before_match.take() {
            self.controls.hear(heard);
            self.controls.set_looping(looping);
        }
    }

    /// Take what a finished match found: its trims, fresh.
    fn finish_match(&mut self, outcome: Option<MatchOutcome>) {
        self.level_match = None;
        if let Some(outcome) = outcome {
            // A candidate that never sounded above the threshold of hearing
            // has no level to match; the trims stay as they were.
            if outcome.readings.iter().all(Option::is_some) {
                self.trims.matched_to(outcome.trims_db);
            }
            for candidate in Candidate::ALL {
                self.controls
                    .set_trim(candidate, self.trims.trim_db(candidate));
            }
            self.last_match = Some(outcome);
        }
        if let Some((heard, looping)) = self.before_match.take() {
            self.controls.hear(heard);
            self.controls.set_looping(looping);
        }
    }

    fn parts(&mut self) -> Option<Parts> {
        let recording = self.recording.as_ref()?;
        Some(Parts {
            controls: self.controls.clone(),
            recording: RecordingParts {
                buffer: recording.buffer.clone(),
                power: recording.power,
            },
            passage: self.passage.clone()?,
            seed: self.seed,
            wind_power: self.wind_power,
            previous: self.previous.clone(),
            working: self.working.clone(),
            run_freely: self.run_freely,
        })
    }

    fn state(&mut self, voice_in_place: bool) -> LabState {
        let rate = self.sample_rate as f32;
        let controls = self.controls.clone();
        let playing = voice_in_place && controls.asked_to_play();
        let preset = self.preset().ok().cloned();
        let (behaviour, lines_up) = match &preset {
            Some(preset) => (
                SETTINGS
                    .iter()
                    .filter(|setting| setting.in_behaviour)
                    .map(|setting| LabSetting::of(setting, self.working.value(setting, preset)))
                    .collect(),
                self.working.lines_up_with(&self.previous, preset),
            ),
            None => (Vec::new(), true),
        };
        let trims = Candidate::ALL
            .into_iter()
            .map(|candidate| {
                let reading = self
                    .last_match
                    .and_then(|outcome| outcome.readings[candidate.index()]);
                LabTrim {
                    candidate: candidate.into(),
                    trim_db: self.trims.trim_db(candidate),
                    matched: self.trims.matched(candidate),
                    rematch_needed: self.trims.rematch_needed(candidate),
                    difference_db: self
                        .last_match
                        .and_then(|outcome| outcome.difference_db(candidate))
                        .map(|difference| difference as f32),
                    halves_apart_db: reading
                        .and_then(|reading| reading.halves_apart_db())
                        .map(|apart| apart as f32),
                }
            })
            .collect();
        LabState {
            sound: self.sound_name.clone(),
            question: self.question.clone(),
            recording: self
                .recording
                .as_ref()
                .map(|recording| recording.file.clone()),
            passage: self.passage.as_ref().map(|passage| LabPassage {
                name: passage.name.clone(),
                reference_from: passage.from as f32,
                reference_to: passage.to as f32,
                generator_cue: passage.cue as f32,
            }),
            seed: self.seed,
            playing,
            looping: self
                .before_match
                .map_or(controls.looping(), |(_, looping)| looping),
            run_freely: self.run_freely,
            playhead_seconds: controls.position_frames() as f32 / rate,
            loop_seconds: self.loop_frames as f32 / rate,
            asked: self
                .before_match
                .map_or(controls.asked_to_hear(), |(heard, _)| heard)
                .into(),
            heard: controls
                .sounding()
                .filter(|_| playing)
                .map(|(candidate, _)| candidate.into()),
            trims,
            hold_level_match: self.trims.hold,
            measuring: self
                .level_match
                .as_ref()
                .and_then(LevelMatch::measuring)
                .map(Into::into),
            match_progress: self.level_match.as_ref().map_or(0.0, LevelMatch::progress),
            mix: Layer::ALL
                .into_iter()
                .map(|layer| LabFader::of(layer, self.working.mix.fader(layer)))
                .collect(),
            can_undo_mix: self.mix_history.can_undo(),
            behaviour,
            lines_up,
            notes: self
                .notes
                .iter()
                .map(|note| LabNote {
                    at_seconds: note.at as f32,
                    heard: Candidate::named(&note.heard)
                        .unwrap_or(Candidate::Working)
                        .into(),
                    moment_seconds: note.moment as f32,
                    text: note.text.clone(),
                })
                .collect(),
            verdict: self.verdict.clone(),
            saved_session: self.saved_session,
        }
    }

    /// The session as a file holds it.
    fn session(&self, preset: &SoftWindSource) -> Session {
        let cue = self.passage.as_ref().map_or(0.0, |passage| passage.cue);
        let mut working = SessionCandidate::of(&self.working, self.seed as u64, cue);
        working.changed = self.working.changed_from(&self.previous, preset);
        let named = |candidates: Vec<Candidate>| -> Vec<String> {
            candidates
                .into_iter()
                .map(|candidate| candidate.name().to_owned())
                .collect()
        };
        let generators = [Candidate::Previous, Candidate::Working];
        Session {
            sound: SOUND.to_owned(),
            question: self.question.clone(),
            reference: self.recording.as_ref().map(|recording| {
                let passage = self.passage.clone();
                ReferencePassage {
                    file: recording.file.clone(),
                    passage: passage
                        .as_ref()
                        .map_or_else(String::new, |passage| passage.name.clone()),
                    from: passage.as_ref().map_or(0.0, |passage| passage.from),
                    to: passage.as_ref().map_or(0.0, |passage| passage.to),
                }
            }),
            candidates: SessionCandidates {
                previous: SessionCandidate::of(&self.previous, self.seed as u64, cue),
                working,
            },
            looping: self
                .before_match
                .map_or(self.controls.looping(), |(_, looping)| looping),
            run_freely: self.run_freely,
            trims_db: generators
                .into_iter()
                .filter(|candidate| self.trims.matched(*candidate))
                .map(|candidate| (candidate.name().to_owned(), self.trims.trim_db(candidate)))
                .collect(),
            rematch_needed: named(
                generators
                    .into_iter()
                    .filter(|candidate| self.trims.rematch_needed(*candidate))
                    .collect(),
            ),
            hold_level_match: self.trims.hold,
            mode: MODE.to_owned(),
            notes: self.notes.clone(),
            verdict: self.verdict.clone(),
            ..Session::default()
        }
    }

    /// Take everything `session` holds, with its recording opened.
    fn restore(
        &mut self,
        session: &Session,
        recording: Option<Recording>,
        previous: CandidateSettings,
        working: CandidateSettings,
        seed: u32,
    ) {
        self.question = session.question.clone();
        self.verdict = session.verdict.clone();
        self.notes = session.notes.clone();
        self.seed = seed;
        self.previous = previous;
        self.working = working;
        self.mix_history = MixHistory::default();
        self.run_freely = session.run_freely;
        self.controls.set_looping(session.looping);
        self.passage = match (&recording, &session.reference) {
            (Some(_), Some(reference)) if reference.to - reference.from >= SHORTEST_PASSAGE => {
                Some(Passage {
                    name: reference.passage.clone(),
                    from: reference.from,
                    to: reference.to,
                    cue: session.candidates.working.cue,
                })
            }
            _ => None,
        };
        self.recording = recording;
        let trim_of = |candidate: Candidate| session.trims_db.get(candidate.name()).copied();
        let stale = |candidate: Candidate| {
            session
                .rematch_needed
                .iter()
                .any(|name| name == candidate.name())
        };
        self.trims = Trims::restored(
            Candidate::ALL.map(|candidate| trim_of(candidate).unwrap_or(0.0)),
            Candidate::ALL.map(|candidate| match candidate {
                Candidate::Reference => !session.trims_db.is_empty(),
                _ => trim_of(candidate).is_some(),
            }),
            Candidate::ALL.map(stale),
            session.hold_level_match,
        );
        for candidate in Candidate::ALL {
            self.controls
                .set_trim(candidate, self.trims.trim_db(candidate));
        }
        self.last_match = None;
    }
}

fn lab_error(reason: &str) -> EngineError {
    EngineError::Scene(reason.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use atrium::audio::spectral_profile::SpectralProfile;
    use atrium::lab::Fader;

    /// Reopening a session gives the lab back what it was: the question,
    /// the recording's passage, the seed and cue, both candidates' settings
    /// and mixes, the loop, the trims as they were held, the notes and the
    /// verdict. Without a device: the lab's own state, saved as a session
    /// and restored into a fresh lab.
    #[test]
    fn a_session_reopens_as_it_was_saved() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        let recording = || Recording {
            file: "research papers/soft wind.mp3".into(),
            buffer: Arc::new(AudioBuffer {
                samples: vec![0.1; 48_000 * 30],
                sample_rate: 48_000,
                rms: 0.1,
                spectral_profile: SpectralProfile::default(),
            }),
            power: 0.01,
            waveform: Waveform::default(),
        };
        let mut lab = Lab::new(48_000);
        lab.question = "Does the gust fade naturally?".into();
        lab.recording = Some(recording());
        lab.passage = Some(Passage {
            name: "Gust 1".into(),
            from: 12.4,
            to: 19.0,
            cue: 41.2,
        });
        lab.seed = 3;
        lab.working.values.insert("gust_brightness".into(), 0.3);
        let mut mix = lab.working.mix;
        mix.set(
            Layer::Body,
            Fader {
                level_db: -2.0,
                solo: false,
                mute: true,
            },
        );
        lab.working.mix = mix;
        lab.trims.matched_to([0.0, -1.8, -1.6]);
        lab.trims.after(Edit::Mixer);
        lab.controls.set_looping(false);
        lab.notes.push(SessionNote {
            at: 3.2,
            heard: "working".into(),
            moment: 44.4,
            text: "the air drops away too soon".into(),
        });
        lab.verdict = "the fade sounds better, but the onset is still too sharp".into();
        let preset = lab.preset().expect("the preset reads").clone();
        let session = lab.session(&preset);
        let mut session = session;
        let path = std::env::temp_dir().join(format!(
            "atrium-ffi-lab-{}/session.json",
            std::process::id()
        ));
        session.save(&path).expect("saved");
        let read = Session::load(&path).expect("read back");
        let _ = std::fs::remove_dir_all(path.parent().expect("a folder"));

        let mut reopened = Lab::new(48_000);
        reopened.restore(
            &read,
            Some(recording()),
            read.candidates.previous.settings(),
            read.candidates.working.settings(),
            read.candidates.working.seed as u32,
        );
        let (before, after) = (lab.state(true), reopened.state(true));
        assert_eq!(after.question, before.question);
        assert_eq!(after.passage, before.passage);
        assert_eq!(after.seed, 3);
        assert_eq!(after.mix, before.mix);
        assert_eq!(after.behaviour, before.behaviour);
        assert!(!after.looping);
        assert_eq!(after.trims, before.trims);
        assert!(after.trims[2].rematch_needed && after.trims[2].trim_db == -1.6);
        assert_eq!(after.notes, before.notes);
        assert_eq!(after.verdict, before.verdict);
        assert_eq!(reopened.working, lab.working);
        assert_eq!(reopened.previous, lab.previous);
        assert_eq!(read.candidates.working.changed["body_mute"], [0.0, 1.0]);
    }

    /// The lab on a playing scene, at zero volume: a recording opened, a
    /// gust marked at a move of the seeded run, the candidates switched, a
    /// match started and stopped, a fader moved and taken back, and a timing
    /// setting changed. Needs the default output device
    /// and the recording, which is not in git:
    /// `cargo test -p atrium-ffi lab -- --ignored --test-threads=1`.
    #[test]
    #[ignore = "opens the default output device"]
    fn the_lab_tunes_the_soft_wind_on_a_playing_scene() {
        use std::time::Instant;
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        let engine = Engine::new("scenes/frog.yaml".into()).expect("scene should play");
        engine.set_volume(0.0).expect("volume");
        let recording = engine
            .lab_open_recording("research papers/soft wind.mp3".into())
            .expect("the recording opens");
        assert!(recording.waveform.seconds > 20.0);
        let run = engine.lab_generator_run(1).expect("the run renders");
        let gust = run.moves.first().expect("a move in five minutes");
        engine
            .lab_mark_passage(LabPassage {
                name: "Gust 1".into(),
                reference_from: 2.0,
                reference_to: 14.0,
                generator_cue: (gust.seconds - 2.0).max(0.0),
            })
            .expect("the passage builds");
        engine.lab_play(true).expect("plays");
        let state_when = |ready: &dyn Fn(&LabState) -> bool, seconds: u64| {
            let deadline = Instant::now() + Duration::from_secs(seconds);
            loop {
                let state = engine.lab_state();
                if ready(&state) {
                    return state;
                }
                assert!(Instant::now() < deadline, "never: {state:?}");
                std::thread::sleep(Duration::from_millis(16));
            }
        };
        state_when(
            &|state| state.playing && state.heard == Some(LabCandidate::Working),
            2,
        );
        engine.lab_hear(LabCandidate::Reference);
        state_when(&|state| state.heard == Some(LabCandidate::Reference), 2);

        // At zero volume nothing reaches the threshold of hearing at the
        // tap, so a match is only started here, and hearing another
        // candidate stops it.
        engine.lab_match_levels().expect("a match starts");
        state_when(&|state| state.measuring == Some(LabCandidate::Reference), 2);
        engine.lab_hear(LabCandidate::Working);
        let stopped = state_when(&|state| state.heard == Some(LabCandidate::Working), 2);
        assert_eq!(stopped.measuring, None);

        engine
            .lab_set_fader(LabFader {
                layer: records::LabLayer::Body,
                level_db: -2.0,
                solo: false,
                mute: false,
            })
            .expect("a fader moves");
        let moved = engine.lab_state();
        assert!(moved.can_undo_mix && moved.lines_up);
        engine.lab_undo_mix().expect("undone");
        assert_eq!(engine.lab_state().mix[1].level_db, 0.0);
        engine
            .lab_set_setting("move_rest".into(), 0.5)
            .expect("a timing setting");
        assert!(
            !engine.lab_state().lines_up,
            "the marked gust no longer lines up"
        );
        assert!(engine.lab_set_setting("no_such".into(), 1.0).is_err());
        engine.lab_play(false).expect("pauses");
        assert!(!engine.lab_state().playing);
    }
}
