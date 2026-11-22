//! The analyser on a stream: what the engine plays, measured as it plays,
//! a column every 50 ms, by the same definitions as the whole-file report.
//!
//! - `a_weighting`: the A-weighting as a linear-phase FIR, zero-phase again
//!   by reading late
//! - `spectrum`: a column's band levels, 96 rows from 20 Hz to 20 kHz
//! - `onsets`: the whole-file onset detector run forward
//!
//! Width and ILD are read by the whole-file analyser's own frames
//! (`crate::space::PairFrames`), over the 100 ms that end with the column.
//!
//! A column is finished once everything it needs has arrived: the
//! weighting reaches 43 ms past it, the spectrum's window 30 ms, the onset
//! picker's moving mean about 100 ms. Columns come about 0.15 s after the
//! sound, stamped with when it sounded.

mod a_weighting;
mod onsets;
mod spectrum;

use a_weighting::AWeighting;
use onsets::{frame_at, OnsetStream};
use spectrum::{row_edges, ColumnSpectrum, ROWS};

use crate::signal::{pair_of, usual_name, Calibration, Combination, Settings};
use crate::space::PairFrames;
use crate::statistics::mean;
use crate::weighting::Fast;

/// History is trimmed once this many seconds of it are no longer needed, so
/// it is not shifted on every push.
const TRIM_SECONDS: f64 = 1.0;

/// One 50 ms column of what was heard.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    /// The column's first sample, on the clock its samples were pushed with.
    pub clock_sample: u64,
    /// The band level (dB SPL) in each row, lowest first; row r runs from
    /// `Live::row_edges_hertz()[r]` to `[r + 1]`. The channels' powers
    /// combine as the report's overall level does: two ears averaged,
    /// speakers summed.
    pub spectrum_decibels: Vec<f64>,
    /// Per channel, A-weighted: the level over the column (LAeq, dB SPL)
    /// and the largest FAST level in it (LAF, dB SPL).
    pub leq_decibels: Vec<f64>,
    pub laf_decibels: Vec<f64>,
    /// The same for all channels together, as the report's overall level:
    /// the A-weighted powers combined sample by sample (two ears averaged,
    /// speakers summed), then LAeq and FAST of that.
    pub leq_overall_decibels: f64,
    pub laf_overall_decibels: f64,
    /// Whether an onset fell in the column.
    pub onset: bool,
    /// 1 − IACC over 500 Hz to 2 kHz and the ILD (R − L, dB) of the 100 ms
    /// that end with the column, for the ears or a layout's L and R; none
    /// without a pair, before 100 ms have sounded, or where either is silent.
    pub width: Option<f64>,
    pub ild_decibels: Option<f64>,
}

/// The analyser of one output stream. Feed it what the stream played with
/// [`Live::push`]; take what it has finished with [`Live::take_columns`].
pub struct Live {
    sample_rate: u32,
    channels: usize,
    calibration: Calibration,
    combination: Combination,
    pair: Option<(usize, usize)>,
    column_length: usize,
    /// The clock sample of the stream's first sample, once one has come.
    start_clock: Option<u64>,
    /// Frames received since the stream started.
    received: u64,
    /// Each channel's samples from `history_start` (a stream sample) on.
    history: Vec<Vec<f32>>,
    history_start: u64,
    next_column: u64,
    weighting: AWeighting,
    weighted: Vec<f64>,
    /// Each channel's FAST, from the first column's level.
    fast: Vec<Option<Fast>>,
    /// FAST of the channels' combined power, as the report's overall LAF.
    fast_overall: Option<Fast>,
    /// The channels' A-weighted powers combined, sample by sample.
    combined: Vec<f64>,
    spectrum: ColumnSpectrum,
    onsets: OnsetStream,
    pair_frames: Option<PairFrames>,
    columns: Vec<Column>,
}

impl Live {
    /// The time a column covers (s).
    pub const COLUMN_SECONDS: f64 = 0.05;

    /// An analyser of `channels` channels at `sample_rate`, whose RMS of 1.0
    /// is `full_scale_db_spl` dB SPL (the scene's `spl_reference −
    /// 20·log10(target_rms)`).
    ///
    /// # Panics
    ///
    /// If `channels` or `sample_rate` is zero.
    pub fn new(sample_rate: u32, channels: usize, full_scale_db_spl: f64) -> Self {
        assert!(channels > 0, "a stream needs at least one channel");
        assert!(sample_rate > 0, "a stream needs a sample rate");
        let column_length = ((Self::COLUMN_SECONDS * sample_rate as f64).round() as usize).max(1);
        let names: Vec<String> = (0..channels)
            .map(|index| usual_name(channels, index))
            .collect();
        let pair = pair_of(&names);
        Self {
            sample_rate,
            channels,
            calibration: Calibration::of(&Settings {
                full_scale_db_spl: Some(full_scale_db_spl),
            }),
            combination: Combination::for_channels(channels),
            pair,
            column_length,
            start_clock: None,
            received: 0,
            history: vec![Vec::new(); channels],
            history_start: 0,
            next_column: 0,
            weighting: AWeighting::new(sample_rate, column_length),
            weighted: vec![0.0; column_length],
            fast: (0..channels).map(|_| None).collect(),
            fast_overall: None,
            combined: vec![0.0; column_length],
            spectrum: ColumnSpectrum::new(sample_rate),
            onsets: OnsetStream::new(sample_rate),
            pair_frames: pair.map(|_| PairFrames::new(sample_rate)),
            columns: Vec::new(),
        }
    }

    /// The spectrum's row edges (Hz), lowest first: 97 edges for 96 rows,
    /// log-spaced from 20 Hz to 20 kHz.
    pub fn row_edges_hertz() -> Vec<f64> {
        row_edges()
    }

    /// Add `interleaved` frames that the stream played from
    /// `at_clock_sample` on, and finish every column they complete. Frames
    /// that do not follow on from the last push (a block was dropped, or the
    /// clock moved) start the analysis afresh from there: the columns still
    /// waiting for what would have followed are lost, and the onset picker
    /// and FAST begin again. A partial frame at the end is ignored.
    pub fn push(&mut self, interleaved: &[f32], at_clock_sample: u64) {
        let frames = interleaved.len() / self.channels;
        if frames == 0 {
            return;
        }
        match self.start_clock {
            Some(start) if start + self.received == at_clock_sample => {}
            _ => self.restart(at_clock_sample),
        }
        for frame in interleaved.chunks_exact(self.channels) {
            for (history, sample) in self.history.iter_mut().zip(frame) {
                history.push(*sample);
            }
        }
        self.received += frames as u64;

        let history = &self.history;
        let history_start = self.history_start;
        self.onsets
            .advance(self.received as usize, self.channels, |channel, index| {
                sample_from(history, history_start, channel, index)
            });
        while self.column_ready() {
            let column = self.finish_column();
            self.columns.push(column);
        }
        self.trim();
    }

    /// The columns finished since the last call, oldest first.
    pub fn take_columns(&mut self) -> Vec<Column> {
        std::mem::take(&mut self.columns)
    }

    fn restart(&mut self, at_clock_sample: u64) {
        self.start_clock = Some(at_clock_sample);
        self.received = 0;
        self.history.iter_mut().for_each(Vec::clear);
        self.history_start = 0;
        self.next_column = 0;
        self.fast.iter_mut().for_each(|fast| *fast = None);
        self.fast_overall = None;
        self.onsets = OnsetStream::new(self.sample_rate);
    }

    /// How far past a column's end the input must reach to finish it.
    fn lookahead(&self) -> u64 {
        (self.weighting.delay() as i64)
            .max(self.spectrum.reach_after(self.column_length))
            .max(0) as u64
    }

    fn column_ready(&self) -> bool {
        let end = (self.next_column + 1) * self.column_length as u64;
        self.received >= end + self.lookahead()
            && self.onsets.decided_before(frame_at(end as usize))
    }

    fn finish_column(&mut self) -> Column {
        let length = self.column_length;
        let start = self.next_column * length as u64;
        let end = start + length as u64;
        let history = &self.history;
        let history_start = self.history_start;

        let mut leq_decibels = Vec::with_capacity(self.channels);
        let mut laf_decibels = Vec::with_capacity(self.channels);
        let mut rows = vec![0.0; ROWS];
        let share = self.combination.scale(self.channels);
        self.combined.fill(0.0);
        for channel in 0..self.channels {
            let sample = |index: i64| sample_from(history, history_start, channel, index);
            self.weighting
                .weigh(sample, start as i64, &mut self.weighted);
            let powers: Vec<f64> = self.weighted.iter().map(|value| value * value).collect();
            for (sum, power) in self.combined.iter_mut().zip(&powers) {
                *sum += power;
            }
            let leq = mean(&powers);
            let fast = self.fast[channel].get_or_insert_with(|| Fast::new(self.sample_rate, leq));
            let laf = powers
                .iter()
                .fold(0.0f64, |largest, power| largest.max(fast.step(*power)));
            leq_decibels.push(self.calibration.level(leq));
            laf_decibels.push(self.calibration.level(laf));
            self.spectrum
                .add(sample, start as i64, length, share, &mut rows);
        }

        // As the report: the channels' powers summed, then scaled (by one
        // half for two ears).
        self.combined.iter_mut().for_each(|power| *power *= share);
        let leq_overall = mean(&self.combined);
        let fast_overall = self
            .fast_overall
            .get_or_insert_with(|| Fast::new(self.sample_rate, leq_overall));
        let laf_overall = self.combined.iter().fold(0.0f64, |largest, power| {
            largest.max(fast_overall.step(*power))
        });

        let (mut width, mut ild_decibels) = (None, None);
        if let (Some((left, right)), Some(frames)) = (self.pair, self.pair_frames.as_mut()) {
            let frame_length = frames.frame_length() as u64;
            if end >= frame_length {
                let span =
                    (end - frame_length - history_start) as usize..(end - history_start) as usize;
                (width, ild_decibels) =
                    frames.width_and_ild(&history[left][span.clone()], &history[right][span]);
            }
        }

        let onset = self.onsets.take_before(frame_at(end as usize));
        self.next_column += 1;
        Column {
            clock_sample: self.start_clock.unwrap_or(0) + start,
            spectrum_decibels: rows
                .iter()
                .map(|power| self.calibration.level(*power))
                .collect(),
            leq_decibels,
            laf_decibels,
            leq_overall_decibels: self.calibration.level(leq_overall),
            laf_overall_decibels: self.calibration.level(laf_overall),
            onset,
            width,
            ild_decibels,
        }
    }

    /// Drop history nothing will read again, a second or more at a time.
    fn trim(&mut self) {
        let column_start = (self.next_column * self.column_length as u64) as i64;
        let frame_length = self
            .pair_frames
            .as_ref()
            .map_or(0, |frames| frames.frame_length()) as i64;
        let reach_back = (self.weighting.delay() as i64)
            .max(self.spectrum.reach_before(self.column_length))
            .max(frame_length - self.column_length as i64);
        let oldest_needed = (column_start - reach_back)
            .min(self.onsets.oldest_needed())
            .max(0) as u64;
        let unneeded = oldest_needed.saturating_sub(self.history_start);
        if unneeded as f64 >= TRIM_SECONDS * self.sample_rate as f64 {
            for history in &mut self.history {
                history.drain(..unneeded as usize);
            }
            self.history_start += unneeded;
        }
    }
}

/// A channel's sample at stream sample `index`: zero before the stream
/// began.
fn sample_from(history: &[Vec<f32>], history_start: u64, channel: usize, index: i64) -> f32 {
    if index < 0 {
        return 0.0;
    }
    debug_assert!(
        index as u64 >= history_start,
        "sample {index} was trimmed (history from {history_start})"
    );
    history[channel]
        .get((index as u64).wrapping_sub(history_start) as usize)
        .copied()
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests;
