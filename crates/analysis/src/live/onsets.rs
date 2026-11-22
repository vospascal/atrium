//! Onsets as they happen: the whole-file method (`crate::onsets`) run
//! forward, frame by frame, on the same mel frames, flux and peak picker.
//!
//! Three things the whole file knows that a stream does not, each replaced
//! by what has been heard so far:
//!
//! - The decibels are relative to the loudest mel cell **so far**, not of
//!   the whole file. The flux is a difference of decibels, so this only
//!   matters where a cell lies at the −80 dB floor.
//! - The peak picker's 0.07 is of the flux scaled to the largest flux **so
//!   far**. In the first seconds, before anything loud, it is a lower bar.
//! - The median rise a peak must stand 0.75 dB clear of is the median of
//!   the **past** second, not of the second centred on it. A rise that
//!   comes after a loud stretch is held to that stretch.
//!
//! The moving mean looks 100 ms ahead, as librosa's; the detector waits for
//! those frames, so a frame's verdict comes about 0.1 s after it.

use crate::onsets::{
    decibels, is_peak, median_span_frames, rise, stands_clear, MelFrames, PeakWindows, DELTA,
    FFT_SIZE, FIRST_WHOLE_FLUX_FRAME, FLUX_DELAY_FRAMES, HOP, MEL_BANDS,
};

/// The envelope is trimmed when it holds this many frames.
const ENVELOPE_TRIM_FRAMES: usize = 4096;

pub(super) struct OnsetStream {
    mel: MelFrames,
    windows: PeakWindows,
    /// Frames the median rise is taken over, ending at the frame.
    median_span: usize,
    /// The next mel frame to compute.
    next_mel_frame: usize,
    /// The newest mel frame, as power, to take the next rise from.
    previous_mel: Vec<f64>,
    current_mel: Vec<f64>,
    loudest: f64,
    /// Flux, one per frame from `envelope_start`.
    envelope: Vec<f64>,
    envelope_start: usize,
    highest: f64,
    /// The next frame to decide, and the first that may be a peak after the
    /// last one's wait.
    next_decision: usize,
    next_candidate: usize,
    /// Onset frames decided and not yet taken.
    found: std::collections::VecDeque<usize>,
    scratch: Vec<f64>,
}

impl OnsetStream {
    pub(super) fn new(sample_rate: u32) -> Self {
        Self {
            mel: MelFrames::new(sample_rate),
            windows: PeakWindows::new(sample_rate),
            median_span: median_span_frames(sample_rate),
            next_mel_frame: 0,
            previous_mel: vec![0.0; MEL_BANDS],
            current_mel: vec![0.0; MEL_BANDS],
            loudest: 0.0,
            envelope: Vec::new(),
            envelope_start: 0,
            highest: 0.0,
            next_decision: 0,
            next_candidate: 0,
            found: std::collections::VecDeque::new(),
            scratch: Vec::new(),
        }
    }

    /// The sample (from the stream's start) up to which the next mel frame
    /// reaches: it can be computed once that many samples have arrived.
    fn next_frame_needs(&self) -> usize {
        self.next_mel_frame * HOP + FFT_SIZE / 2
    }

    /// The first sample the next mel frame reads; nothing before it is read
    /// again.
    pub(super) fn oldest_needed(&self) -> i64 {
        (self.next_mel_frame * HOP) as i64 - (FFT_SIZE / 2) as i64
    }

    /// Everything `received` samples allow: mel frames, flux, verdicts.
    /// `sample(channel, index)` is a channel's sample at `index`, zero
    /// before the stream began.
    pub(super) fn advance(
        &mut self,
        received: usize,
        channels: usize,
        sample: impl Fn(usize, i64) -> f32,
    ) {
        while self.next_frame_needs() <= received {
            let frame = self.next_mel_frame;
            std::mem::swap(&mut self.previous_mel, &mut self.current_mel);
            self.current_mel.fill(0.0);
            for channel in 0..channels {
                let first = (frame * HOP) as i64 - (FFT_SIZE / 2) as i64;
                self.mel.add(
                    |offset| sample(channel, first + offset as i64),
                    channels,
                    &mut self.current_mel,
                );
            }
            self.loudest = self
                .current_mel
                .iter()
                .fold(self.loudest, |loudest, value| loudest.max(*value));
            if frame == 0 {
                // Frames before the flux's delay compare nothing: zero, as
                // the whole-file envelope starts.
                self.envelope.extend([0.0; FLUX_DELAY_FRAMES]);
            } else {
                let flux = if self.loudest > 0.0 {
                    let to_decibels = |bands: &[f64]| -> Vec<f64> {
                        bands
                            .iter()
                            .map(|value| decibels(*value, self.loudest))
                            .collect()
                    };
                    rise(
                        &to_decibels(&self.previous_mel),
                        &to_decibels(&self.current_mel),
                    )
                } else {
                    0.0
                };
                self.highest = self.highest.max(flux);
                self.envelope.push(flux);
            }
            self.next_mel_frame += 1;
            self.decide();
        }
    }

    /// Decide every frame whose moving mean's window is all known.
    fn decide(&mut self) {
        let known = self.envelope_start + self.envelope.len();
        while self.next_decision + self.windows.after_mean <= known {
            let frame = self.next_decision;
            let index = frame - self.envelope_start;
            if frame >= self.next_candidate
                && self.highest > 0.0
                && is_peak(&self.envelope, index, &self.windows, DELTA * self.highest)
            {
                self.next_candidate = frame + self.windows.wait + 1;
                let past = index + 1 - self.median_span.min(index + 1);
                if frame >= FIRST_WHOLE_FLUX_FRAME
                    && stands_clear(
                        self.envelope[index],
                        &self.envelope[past..=index],
                        &mut self.scratch,
                    )
                {
                    self.found.push_back(frame);
                }
            }
            self.next_decision += 1;
        }
        // Keep what the next verdicts look back over: the median's second.
        if self.envelope.len() >= ENVELOPE_TRIM_FRAMES {
            let keep_from = self.next_decision - self.envelope_start - self.median_span;
            self.envelope.drain(..keep_from);
            self.envelope_start += keep_from;
        }
    }

    /// Whether every frame before `frame` has been decided.
    pub(super) fn decided_before(&self, frame: usize) -> bool {
        self.next_decision >= frame
    }

    /// Whether an onset was found in frames before `frame`; those are
    /// forgotten.
    pub(super) fn take_before(&mut self, frame: usize) -> bool {
        let mut any = false;
        while self.found.front().is_some_and(|found| *found < frame) {
            self.found.pop_front();
            any = true;
        }
        any
    }
}

/// The first onset frame at or after sample `sample` (from the stream's
/// start): frame t is timed at sample t·hop, as the whole-file onsets are.
pub(super) fn frame_at(sample: usize) -> usize {
    sample.div_ceil(HOP)
}
