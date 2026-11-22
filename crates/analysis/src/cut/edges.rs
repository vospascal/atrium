//! Step 5: a take's edges and fades.
//!
//! The start moves back from where the event started to where the band level
//! left bed + [`EDGE_ABOVE_BED_DECIBELS`], at most
//! [`LONGEST_ATTACK_SECONDS`], so the attack is kept: back down the attack
//! while the level falls, and no further than where it stops falling. A
//! reading of the bed alone is above bed + 3 dB nearly half the time (the bed
//! is its 10th percentile, 3 dB under its mean at ten degrees of freedom), so
//! walking back to the first reading under bed + 3 dB wandered on into the
//! bed: one start in twenty came 11 ms early. The end moves on until
//! the level reaches bed + 3 dB, at most [`LONGEST_DECAY_SECONDS`], so the
//! decay and the recorded space's reverb are kept. Neither crosses into the
//! take before or after.
//!
//! A reading averages a frame centred on it, so an edge is the outer end of
//! the frame of the last reading above bed + 3 dB: the start the frame's
//! beginning, the end its end. The sound starts somewhere in that frame, not
//! before it.
//!
//! Then the file gets a 5 ms raised-cosine fade in before the start and out
//! after the end, so the fades lie over material at the bed and a take
//! starts and ends at zero, without a click.

use crate::signal::Signal;

pub(super) const EDGE_ABOVE_BED_DECIBELS: f64 = 3.0;
pub(super) const LONGEST_ATTACK_SECONDS: f64 = 0.3;
pub(super) const LONGEST_DECAY_SECONDS: f64 = 2.0;
pub(super) const FADE_SECONDS: f64 = 0.005;

/// Where a take lies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Placed {
    /// The readings at its edges, the first and the last kept.
    pub(super) first_reading: usize,
    pub(super) last_reading: usize,
    /// The file: its first frame, the frame after its last, the fades' length.
    pub(super) first_frame: usize,
    pub(super) end_frame: usize,
    pub(super) fade_frames: usize,
    /// The level was above bed + 3 dB at the recording's first or last reading.
    pub(super) cut_off_at_start: bool,
    pub(super) cut_off_at_end: bool,
}

/// What the edges are found in.
pub(super) struct Readings<'a> {
    pub(super) levels: &'a [f64],
    pub(super) bed: &'a [f64],
    pub(super) step_seconds: f64,
    pub(super) frame_seconds: f64,
    pub(super) sample_rate: u32,
    pub(super) frames: usize,
}

impl Readings<'_> {
    fn above_edge(&self, reading: usize) -> bool {
        self.levels[reading] > self.bed[reading] + EDGE_ABOVE_BED_DECIBELS
    }
}

/// The takes whose sounding runs over readings `spans` (first and last, in
/// order), placed.
pub(super) fn place(spans: &[(usize, usize)], readings: &Readings) -> Vec<Placed> {
    let last_reading = readings.levels.len().saturating_sub(1);
    let attack = (LONGEST_ATTACK_SECONDS / readings.step_seconds).round() as usize;
    let decay = (LONGEST_DECAY_SECONDS / readings.step_seconds).round() as usize;
    let starts: Vec<usize> = spans
        .iter()
        .enumerate()
        .map(|(index, (first, _))| {
            let earliest = if index == 0 {
                0
            } else {
                spans[index - 1].1 + 1
            };
            let mut reading = *first;
            while reading > earliest
                && first - (reading - 1) <= attack
                && readings.above_edge(reading - 1)
                && readings.levels[reading - 1] < readings.levels[reading]
            {
                reading -= 1;
            }
            reading
        })
        .collect();
    let rate = readings.sample_rate as f64;
    let fade_frames = (FADE_SECONDS * rate).round() as usize;
    spans
        .iter()
        .enumerate()
        .map(|(index, (_, last))| {
            let latest = starts
                .get(index + 1)
                .map_or(last_reading, |start| start.saturating_sub(1).max(*last));
            let mut reading = *last;
            while reading < latest
                && reading + 1 - last <= decay
                && readings.above_edge(reading + 1)
            {
                reading += 1;
            }
            let first_reading = starts[index];
            let start_seconds =
                first_reading as f64 * readings.step_seconds - readings.frame_seconds / 2.0;
            let end_seconds = reading as f64 * readings.step_seconds + readings.frame_seconds / 2.0;
            let first_frame = (((start_seconds - FADE_SECONDS) * rate).round().max(0.0) as usize)
                .min(readings.frames);
            let end_frame = (((end_seconds + FADE_SECONDS) * rate).round().max(0.0) as usize)
                .min(readings.frames)
                .max(first_frame);
            Placed {
                first_reading,
                last_reading: reading,
                first_frame,
                end_frame,
                fade_frames: fade_frames.min((end_frame - first_frame) / 2),
                cut_off_at_start: first_reading == 0 && readings.above_edge(0),
                cut_off_at_end: reading == last_reading && readings.above_edge(last_reading),
            }
        })
        .collect()
}

/// Frames `first` to `end` of `signal`, interleaved, with a raised-cosine fade
/// of `fade` frames in and out: the first sample's gain is 0 and the last's
/// is 0.
pub(super) fn faded(signal: &Signal, first: usize, end: usize, fade: usize) -> Vec<f32> {
    let channels = signal.channels;
    let mut samples = signal.interleaved[first * channels..end * channels].to_vec();
    let frames = end - first;
    for frame in 0..fade.min(frames) {
        let gain = (0.5 - 0.5 * (std::f64::consts::PI * frame as f64 / fade as f64).cos()) as f32;
        for channel in 0..channels {
            samples[frame * channels + channel] *= gain;
            samples[(frames - 1 - frame) * channels + channel] *= gain;
        }
    }
    samples
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A take's ends start and stop at zero, and its middle is untouched.
    #[test]
    fn fades_start_and_end_at_zero() {
        let samples = vec![1.0f32; 2 * 100];
        let signal = Signal {
            interleaved: &samples,
            channels: 2,
            sample_rate: 1000,
            channel_names: Vec::new(),
        };
        let take = faded(&signal, 10, 60, 5);
        assert_eq!(take.len(), 100);
        assert_eq!((take[0], take[1]), (0.0, 0.0));
        assert_eq!((take[98], take[99]), (0.0, 0.0));
        assert_eq!(take[50], 1.0);
        assert!(take[2] > 0.0 && take[2] < 0.5);
    }

    /// The edges move out to bed + 3 dB, no further than allowed and not
    /// into the next take.
    #[test]
    fn edges_move_out_to_the_bed() {
        //          0    1    2     3     4     5    6    7     8     9
        let levels = [0.0, 4.0, 20.0, 20.0, 10.0, 4.0, 0.0, 5.0, 20.0, 4.0];
        let bed = [0.0; 10];
        let readings = Readings {
            levels: &levels,
            bed: &bed,
            step_seconds: 0.1,
            frame_seconds: 0.1,
            sample_rate: 1000,
            frames: 1000,
        };
        let placed = place(&[(2, 4), (8, 8)], &readings);
        assert_eq!((placed[0].first_reading, placed[0].last_reading), (1, 5));
        assert_eq!((placed[1].first_reading, placed[1].last_reading), (7, 9));
        // From 0.05 s (the start of reading 1's frame) less the fade.
        assert_eq!(placed[0].first_frame, 45);
        assert!(placed[1].cut_off_at_end);
        assert!(!placed[0].cut_off_at_start);
    }
}
