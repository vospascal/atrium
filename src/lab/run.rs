//! A seeded generator run, made offline: fast, and the same for the same
//! seed. Its waveform is drawn under the recording's, the moves its gust
//! driver reports are where the generator's markers snap to, and the state
//! it reaches at a cue is what every wrap of the loop starts from.

use atrium_core::source::SoundSource;

use crate::synth::soft_wind::SoftWindSource;
use crate::synth::wind_behaviour::WindMove;

/// Where one of the wind's moves began.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveStart {
    /// From the start of the run (s).
    pub seconds: f64,
    pub kind: WindMove,
}

/// A waveform for drawing: the lowest and highest sample in each of a
/// number of equal stretches.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Waveform {
    /// How long the whole is (s).
    pub seconds: f64,
    pub minima: Vec<f32>,
    pub maxima: Vec<f32>,
}

impl Waveform {
    /// `samples` at `sample_rate` in `columns` stretches.
    pub fn of(samples: &[f32], sample_rate: f32, columns: usize) -> Self {
        let columns = columns.max(1);
        let mut waveform = Self {
            seconds: samples.len() as f64 / sample_rate.max(1.0) as f64,
            minima: Vec::with_capacity(columns),
            maxima: Vec::with_capacity(columns),
        };
        for column in 0..columns {
            let start = column * samples.len() / columns;
            let end = ((column + 1) * samples.len() / columns).max(start);
            let span = &samples[start..end];
            waveform
                .minima
                .push(span.iter().copied().fold(0.0, f32::min));
            waveform
                .maxima
                .push(span.iter().copied().fold(0.0, f32::max));
        }
        waveform
    }
}

/// A run of the generator from its seed.
#[derive(Clone, Debug)]
pub struct GeneratorRun {
    pub waveform: Waveform,
    pub moves: Vec<MoveStart>,
    /// Its level while it sounds, as a power in the generator's own units
    /// (`atrium_analysis::power_while_sounding`).
    pub power_while_sounding: f64,
}

/// Run a copy of `generator`, as it is (fresh, at its seed), for `seconds`
/// at `sample_rate`, and draw it in `columns` stretches.
pub fn run(
    generator: &SoftWindSource,
    seconds: f64,
    sample_rate: f32,
    columns: usize,
) -> GeneratorRun {
    let mut generator = generator.clone();
    let length = (seconds * sample_rate as f64).max(0.0) as usize;
    let mut samples = Vec::with_capacity(length);
    let mut moves = Vec::new();
    for index in 0..length {
        samples.push(generator.next_sample(sample_rate));
        if let Some(kind) = generator.move_began() {
            moves.push(MoveStart {
                seconds: index as f64 / sample_rate as f64,
                kind,
            });
        }
    }
    GeneratorRun {
        waveform: Waveform::of(&samples, sample_rate, columns),
        moves,
        power_while_sounding: atrium_analysis::power_while_sounding(&[(
            &samples,
            sample_rate.round() as u32,
        )]),
    }
}

/// `generator` run from where it is for `cue_seconds` at `sample_rate`: its
/// whole state at the cue.
pub fn state_at(
    mut generator: SoftWindSource,
    cue_seconds: f64,
    sample_rate: f32,
) -> SoftWindSource {
    let frames = (cue_seconds * sample_rate as f64).round().max(0.0) as usize;
    for _ in 0..frames {
        generator.next_sample(sample_rate);
    }
    generator
}
