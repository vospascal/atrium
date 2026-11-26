//! The recipe as a preset writes it ([`RecipeDef`]) and as it plays
//! ([`Recipe`]): checked, every note in hertz, every name a chord's index.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::events::{Pings, PingsDef, Swoosh, SwooshDef};
use crate::note;
use crate::span::{Span, SpanDef};
use crate::RecipeError;

/// The most notes, colour notes included, one chord may have: what the
/// engine's voice holds room for.
pub const MOST_NOTES: usize = 12;

/// The most voices one note may have: what the engine's voice holds room for.
pub const MOST_VOICES: usize = 6;

const DEFAULT_A4_HERTZ: f64 = 440.0;
const DEFAULT_HOLD_SECONDS: f64 = 14.0;
const DEFAULT_SWELL_IN_SECONDS: f64 = 3.0;
const DEFAULT_SWELL_OUT_SECONDS: f64 = 8.0;
/// Calm's colour notes sit about 10 dB under its chords.
const DEFAULT_COLOUR_LEVEL_DB: f64 = -10.0;
/// One move of a voice's pitch drift: slow enough to be a living pitch, not
/// a vibrato.
const DEFAULT_DRIFT_SECONDS: (f64, f64) = (4.0, 12.0);

/// A recipe as a preset writes it. Every field may be left out but the
/// chords; see `docs/recipe-design.md`.
///
/// ```yaml
/// chords:
///   - { name: D, notes: [D2, A2, D3, A3, D4], colour: [E4, F4] }
///   - { name: C, notes: [G2, C3, G3, C4], colour: [D4, F4] }
/// change: alternate
/// hold: 12..16 s
/// swell: { in: 0.5..6 s, out: 5..24 s }
/// voice: { voices: 2, detune: 27 cents, drift: 2..11 cents }
/// ```
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
pub struct RecipeDef {
    /// The frequency of A4: `A = 449 Hz`. 440 Hz when left out.
    #[serde(default)]
    tuning: Option<String>,
    #[serde(default)]
    voice: Option<VoiceDef>,
    #[serde(default)]
    chords: Vec<ChordDef>,
    /// The chord it begins with; the first written when left out.
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    change: Option<ChangeDef>,
    #[serde(default)]
    hold: Option<HoldDef>,
    #[serde(default)]
    swell: Option<SwellDef>,
    /// How far under the chord its colour notes sound (dB).
    #[serde(default)]
    colour_level: Option<f64>,
    /// Struck notes on top, of one or more kinds (lower, higher, ...).
    #[serde(default)]
    pings: Vec<PingsDef>,
    /// A soft swell of noise now and then.
    #[serde(default)]
    swoosh: Option<SwooshDef>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
struct ChordDef {
    name: String,
    notes: Vec<String>,
    #[serde(default)]
    colour: Vec<String>,
    /// This chord's colour level, in place of the recipe's (dB).
    #[serde(default)]
    colour_level: Option<f64>,
}

/// `alternate` (each chord in the order written, round again) or a table of
/// chances: from each chord, how likely each other comes next.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
enum ChangeDef {
    Word(String),
    Table(BTreeMap<String, BTreeMap<String, f64>>),
}

/// One span for every chord, or one per chord by name.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
enum HoldDef {
    Every(SpanDef),
    Each(BTreeMap<String, SpanDef>),
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
struct SwellDef {
    #[serde(default, rename = "in")]
    swell_in: Option<SpanDef>,
    #[serde(default)]
    out: Option<SpanDef>,
    /// How much later than the chord each of its notes may begin (s), drawn
    /// per note, so a chord blooms note by note instead of arriving whole.
    #[serde(default)]
    stagger: Option<SpanDef>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
struct VoiceDef {
    /// 1 to 6: the voices of each note, spread evenly across the detune.
    #[serde(default)]
    voices: Option<u8>,
    /// How far the outermost voices are from the note (cents).
    #[serde(default)]
    detune: Option<SpanDef>,
    /// How far each voice's pitch wanders (cents) ...
    #[serde(default)]
    drift: Option<SpanDef>,
    /// ... and how long one move of it takes (s).
    #[serde(default)]
    drift_over: Option<SpanDef>,
    /// How far each note's level wanders either way (dB) ...
    #[serde(default)]
    wander: Option<SpanDef>,
    /// ... and how long one move of it takes (s).
    #[serde(default)]
    wander_over: Option<SpanDef>,
    /// 0 a soft, nearly pure tone; 1 a bright one.
    #[serde(default)]
    brightness: Option<f64>,
    /// A gentle low-pass on the whole (Hz): warm above it, 12 dB less an
    /// octave higher. None when left out.
    #[serde(default)]
    cutoff: Option<SpanDef>,
}

/// One chord: its notes and its colour notes, in hertz.
#[derive(Clone, Debug, PartialEq)]
pub struct Chord {
    name: String,
    notes: Vec<f64>,
    colour: Vec<f64>,
    colour_level_db: f64,
}

impl Chord {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn notes(&self) -> &[f64] {
        &self.notes
    }

    /// The notes on top, sounding [`Chord::colour_level_db`] under the chord.
    pub fn colour(&self) -> &[f64] {
        &self.colour
    }

    /// How far under the chord its colour notes sound (dB): its own level,
    /// else the recipe's.
    pub fn colour_level_db(&self) -> f64 {
        self.colour_level_db
    }
}

/// How each note sounds. The engine's voice plays it.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Voice {
    /// 1 to [`MOST_VOICES`].
    pub voices: usize,
    /// The outermost voices this far either side of the note, the others
    /// spread evenly between (cents).
    pub detune_cents: f64,
    /// How far each voice's pitch wanders (cents).
    pub drift_cents: Span,
    /// How long one move of the drift takes (s).
    pub drift_seconds: Span,
    /// How far each note's level wanders either way (dB).
    pub wander_db: f64,
    /// How long one move of the wander takes (s).
    pub wander_seconds: Span,
    /// 0 to 1.
    pub brightness: f64,
    /// The low-pass's corner (Hz), if it has one.
    pub cutoff_hertz: Option<f64>,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            voices: 2,
            detune_cents: 15.0,
            drift_cents: Span::fixed(3.0),
            drift_seconds: Span {
                low: DEFAULT_DRIFT_SECONDS.0,
                high: DEFAULT_DRIFT_SECONDS.1,
            },
            wander_db: 6.0,
            wander_seconds: Span::fixed(30.0),
            brightness: 0.3,
            cutoff_hertz: None,
        }
    }
}

/// A recipe ready to play: checked, notes in hertz, names resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct Recipe {
    pub(crate) chords: Vec<Chord>,
    pub(crate) start: usize,
    pub(crate) change: Change,
    pub(crate) hold: Vec<Span>,
    pub(crate) swell_in: Span,
    pub(crate) swell_out: Span,
    swell_stagger: Span,
    colour_level_db: f64,
    voice: Voice,
    pings: Vec<Pings>,
    swoosh: Option<Swoosh>,
}

/// How one chord follows another.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Change {
    /// The next in the order written, round again.
    InOrder,
    /// For each chord, the chords that may follow and their running share of
    /// the chance (the last is 1).
    Table(Vec<Vec<(usize, f64)>>),
}

impl Recipe {
    pub fn chords(&self) -> &[Chord] {
        &self.chords
    }

    pub fn voice(&self) -> &Voice {
        &self.voice
    }

    /// How much later than its chord each note may begin (s).
    pub fn swell_stagger(&self) -> Span {
        self.swell_stagger
    }

    pub fn colour_level_db(&self) -> f64 {
        self.colour_level_db
    }

    /// Each kind of ping, in the order written.
    pub fn pings(&self) -> &[Pings] {
        &self.pings
    }

    pub fn swoosh(&self) -> Option<&Swoosh> {
        self.swoosh.as_ref()
    }
}

/// A colour level as written, checked.
fn colour_level(value: Option<f64>, default: f64, field: &str) -> Result<f64, RecipeError> {
    let level = value.unwrap_or(default);
    if !level.is_finite() || level > 12.0 {
        return Err(RecipeError::new(field, "is a level in dB, at most +12"));
    }
    Ok(level)
}

impl RecipeDef {
    /// The recipe ready to play, or what is wrong with it.
    pub fn resolve(&self) -> Result<Recipe, RecipeError> {
        let a4 = match &self.tuning {
            Some(text) => note::tuning(text, "tuning")?,
            None => DEFAULT_A4_HERTZ,
        };
        if self.chords.is_empty() {
            return Err(RecipeError::new(
                "chords",
                "a recipe needs at least one chord",
            ));
        }
        let colour_level_db =
            colour_level(self.colour_level, DEFAULT_COLOUR_LEVEL_DB, "colour_level")?;
        let mut chords = Vec::with_capacity(self.chords.len());
        for (index, chord) in self.chords.iter().enumerate() {
            let field = format!("chords[{index}]");
            let name = chord.name.trim();
            if name.is_empty() {
                return Err(RecipeError::new(format!("{field}.name"), "is empty"));
            }
            if chords.iter().any(|known: &Chord| known.name == name) {
                return Err(RecipeError::new(
                    format!("{field}.name"),
                    format!("`{name}` is named twice"),
                ));
            }
            if chord.notes.is_empty() {
                return Err(RecipeError::new(
                    format!("{field}.notes"),
                    "a chord needs at least one note",
                ));
            }
            if chord.notes.len() + chord.colour.len() > MOST_NOTES {
                return Err(RecipeError::new(
                    &field,
                    format!(
                        "has {} notes and colour notes; at most {MOST_NOTES}",
                        chord.notes.len() + chord.colour.len()
                    ),
                ));
            }
            let hertz = |list: &[String], part: &str| -> Result<Vec<f64>, RecipeError> {
                list.iter()
                    .enumerate()
                    .map(|(at, text)| note::frequency(text, a4, &format!("{field}.{part}[{at}]")))
                    .collect()
            };
            chords.push(Chord {
                name: name.to_owned(),
                notes: hertz(&chord.notes, "notes")?,
                colour: hertz(&chord.colour, "colour")?,
                colour_level_db: colour_level(
                    chord.colour_level,
                    colour_level_db,
                    &format!("{field}.colour_level"),
                )?,
            });
        }
        let index_of = |name: &str, field: &str| -> Result<usize, RecipeError> {
            chords
                .iter()
                .position(|chord| chord.name == name.trim())
                .ok_or_else(|| {
                    RecipeError::new(field, format!("`{name}` is not one of the chords"))
                })
        };

        let start = match &self.start {
            Some(name) => index_of(name, "start")?,
            None => 0,
        };

        let change = match &self.change {
            None => Change::InOrder,
            Some(ChangeDef::Word(word)) if word.trim() == "alternate" => Change::InOrder,
            Some(ChangeDef::Word(word)) => {
                return Err(RecipeError::new(
                    "change",
                    format!("`{word}` is not `alternate` or a table of chances"),
                ))
            }
            Some(ChangeDef::Table(table)) => {
                let mut rows = vec![Vec::new(); chords.len()];
                for (from, targets) in table {
                    let from_index = index_of(from, "change")?;
                    let field = format!("change.{from}");
                    let mut total = 0.0;
                    for (to, weight) in targets {
                        let to_index = index_of(to, &field)?;
                        if !weight.is_finite() || *weight < 0.0 {
                            return Err(RecipeError::new(
                                format!("{field}.{to}"),
                                "a chance is a number from 0 up",
                            ));
                        }
                        total += weight;
                        rows[from_index].push((to_index, total));
                    }
                    if total <= 0.0 {
                        return Err(RecipeError::new(
                            field,
                            "no chord can follow: every chance is 0",
                        ));
                    }
                    for (_, running) in &mut rows[from_index] {
                        *running /= total;
                    }
                }
                if let Some(stuck) = rows.iter().position(Vec::is_empty) {
                    return Err(RecipeError::new(
                        "change",
                        format!("says nothing of what follows `{}`", chords[stuck].name),
                    ));
                }
                Change::Table(rows)
            }
        };

        let hold = match &self.hold {
            None => vec![Span::fixed(DEFAULT_HOLD_SECONDS); chords.len()],
            Some(HoldDef::Every(span)) => {
                vec![positive(span.resolve("hold", "s", 0.0)?, "hold")?; chords.len()]
            }
            Some(HoldDef::Each(each)) => {
                for name in each.keys() {
                    index_of(name, "hold")?;
                }
                chords
                    .iter()
                    .map(|chord| {
                        let field = format!("hold.{}", chord.name);
                        let span = each.get(&chord.name).ok_or_else(|| {
                            RecipeError::new(
                                &field,
                                "every chord needs a hold when holds are given per chord",
                            )
                        })?;
                        positive(span.resolve(&field, "s", 0.0)?, &field)
                    })
                    .collect::<Result<_, _>>()?
            }
        };

        let swell = self.swell.as_ref();
        let swell_in = match swell.and_then(|swell| swell.swell_in.as_ref()) {
            Some(span) => span.resolve("swell.in", "s", 0.0)?,
            None => Span::fixed(DEFAULT_SWELL_IN_SECONDS),
        };
        let swell_out = match swell.and_then(|swell| swell.out.as_ref()) {
            Some(span) => span.resolve("swell.out", "s", 0.0)?,
            None => Span::fixed(DEFAULT_SWELL_OUT_SECONDS),
        };
        let swell_stagger = match swell.and_then(|swell| swell.stagger.as_ref()) {
            Some(span) => span.resolve("swell.stagger", "s", 0.0)?,
            None => Span::fixed(0.0),
        };

        Ok(Recipe {
            chords,
            start,
            change,
            hold,
            swell_in,
            swell_out,
            swell_stagger,
            colour_level_db,
            voice: self
                .voice
                .as_ref()
                .map_or(Ok(Voice::default()), VoiceDef::resolve)?,
            pings: self
                .pings
                .iter()
                .enumerate()
                .map(|(kind, pings)| pings.resolve(a4, kind))
                .collect::<Result<_, _>>()?,
            swoosh: self.swoosh.as_ref().map(SwooshDef::resolve).transpose()?,
        })
    }
}

impl VoiceDef {
    fn resolve(&self) -> Result<Voice, RecipeError> {
        let defaults = Voice::default();
        let voices = match self.voices {
            None => defaults.voices,
            Some(count) if (1..=MOST_VOICES).contains(&(count as usize)) => count as usize,
            Some(_) => {
                return Err(RecipeError::new(
                    "voice.voices",
                    format!("is 1 to {MOST_VOICES}"),
                ))
            }
        };
        let fixed = |span: Option<&SpanDef>,
                     field: &str,
                     unit: &str,
                     default: f64|
         -> Result<f64, RecipeError> {
            match span {
                None => Ok(default),
                Some(span) => {
                    let span = span.resolve(field, unit, 0.0)?;
                    if span.low() != span.high() {
                        return Err(RecipeError::new(
                            field,
                            format!("is one value in {unit}, not a range"),
                        ));
                    }
                    Ok(span.low())
                }
            }
        };
        let brightness = self.brightness.unwrap_or(defaults.brightness);
        if !(0.0..=1.0).contains(&brightness) {
            return Err(RecipeError::new("voice.brightness", "runs from 0 to 1"));
        }
        let cutoff_hertz = match &self.cutoff {
            None => None,
            Some(span) => {
                let hertz = fixed(Some(span), "voice.cutoff", "Hz", 0.0)?;
                if hertz < 20.0 {
                    return Err(RecipeError::new("voice.cutoff", "is at least 20 Hz"));
                }
                Some(hertz)
            }
        };
        let drift_seconds = match &self.drift_over {
            Some(span) => positive(
                span.resolve("voice.drift_over", "s", 0.0)?,
                "voice.drift_over",
            )?,
            None => defaults.drift_seconds,
        };
        let wander_seconds = match &self.wander_over {
            Some(span) => positive(
                span.resolve("voice.wander_over", "s", 0.0)?,
                "voice.wander_over",
            )?,
            None => defaults.wander_seconds,
        };
        Ok(Voice {
            voices,
            detune_cents: fixed(
                self.detune.as_ref(),
                "voice.detune",
                "cents",
                defaults.detune_cents,
            )?,
            drift_cents: match &self.drift {
                Some(span) => span.resolve("voice.drift", "cents", 0.0)?,
                None => defaults.drift_cents,
            },
            drift_seconds,
            wander_db: fixed(
                self.wander.as_ref(),
                "voice.wander",
                "dB",
                defaults.wander_db,
            )?,
            wander_seconds,
            brightness,
            cutoff_hertz,
        })
    }
}

/// A span that cannot be zero: a hold or a wander's move of 0 s would never end.
fn positive(span: Span, field: &str) -> Result<Span, RecipeError> {
    if span.low() <= 0.0 {
        return Err(RecipeError::new(field, "must be longer than 0 s"));
    }
    Ok(span)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn recipe(yaml: &str) -> Result<Recipe, RecipeError> {
        serde_yaml::from_str::<RecipeDef>(yaml)
            .expect("parses")
            .resolve()
    }

    const CALM: &str = "
chords:
  - { name: Am7, notes: [A2, E3, G3, C4] }
  - { name: D, notes: [D2, A2, D3, A3, D4], colour: [E4, F4] }
  - { name: C, notes: [G2, C3, G3, C4], colour: [D4, F4] }
start: Am7
change: { Am7: { D: 1 }, D: { C: 1 }, C: { D: 1 } }
hold: { Am7: 12 s, D: 12..14 s, C: 10..12 s }
swell: { in: 0.5..6 s, out: 5..24 s }
voice: { voices: 2, detune: 27 cents, drift: 2..11 cents }
";

    #[test]
    fn calm_resolves_with_its_notes_in_hertz() {
        let calm = recipe(CALM).expect("valid");
        assert_eq!(calm.chords().len(), 3);
        let d = &calm.chords()[1];
        assert_eq!(d.name(), "D");
        assert!((d.notes()[2] - 146.83).abs() < 0.01, "D3 {}", d.notes()[2]);
        assert_eq!(d.colour().len(), 2);
        assert_eq!(calm.start, 0);
        assert_eq!(calm.hold[1].low(), 12.0);
        assert_eq!(calm.voice().detune_cents, 27.0);
        assert_eq!(calm.voice().drift_cents.high(), 11.0);
        assert_eq!(calm.colour_level_db(), -10.0);
    }

    #[test]
    fn mistakes_are_named_by_field() {
        let field = |yaml: &str| recipe(yaml).unwrap_err().field;
        assert_eq!(field("chords: []"), "chords");
        assert_eq!(
            field("chords: [{ name: D, notes: [D3, H3] }]"),
            "chords[0].notes[1]"
        );
        assert_eq!(
            field("chords: [{ name: D, notes: [D3] }]\nstart: E"),
            "start"
        );
        assert_eq!(
            field("chords: [{ name: D, notes: [D3] }, { name: C, notes: [C3] }]\nchange: { D: { C: 1 } }"),
            "change"
        );
        assert_eq!(
            field(
                "chords: [{ name: D, notes: [D3] }, { name: C, notes: [C3] }]\nhold: { D: 12 s }"
            ),
            "hold.C"
        );
        assert_eq!(field("chords: [{ name: D, notes: [D3] }]\nhold: 0"), "hold");
        assert_eq!(
            field("chords: [{ name: D, notes: [D3] }]\nvoice: { cutoff: 10 Hz }"),
            "voice.cutoff"
        );
        assert_eq!(
            field("chords: [{ name: D, notes: [D3] }]\nvoice: { voices: 7 }"),
            "voice.voices"
        );
        assert_eq!(
            field("chords: [{ name: D, notes: [D3, D3, D3, D3, D3, D3, D3], colour: [E4, E4, E4, E4, E4, E4] }]"),
            "chords[0]"
        );
    }

    #[test]
    fn defaults_fill_what_is_left_out() {
        let plain =
            recipe("chords: [{ name: D, notes: [D3] }, { name: C, notes: [C3] }]").expect("valid");
        assert_eq!(plain.change, Change::InOrder);
        assert_eq!(plain.hold[0], Span::fixed(14.0));
        assert_eq!(plain.voice(), &Voice::default());
    }
}
