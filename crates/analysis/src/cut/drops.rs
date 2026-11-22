//! Step 6: what is dropped. A take shorter than [`SHORTEST_TAKE_SECONDS`]
//! is a click, not a call; a take already sounding when the recording starts,
//! or still sounding when it ends, is not whole; a take that clips is
//! distorted.
//!
//! Short takes are dropped before the edges are placed and the gaps measured:
//! they are not the animal. Takes cut off or clipping are dropped after: the
//! animal rested around them, and the gaps keep them.

use super::edges::Placed;
use super::record::DropReason;
use crate::signal::Signal;

pub(super) const SHORTEST_TAKE_SECONDS: f64 = 0.05;
/// A sample at or beyond this magnitude is at full scale.
const FULL_SCALE: f32 = 0.999;
/// A run of this many samples at full scale in one channel is a clipped
/// flat top; one alone can be a true peak. Ours, by convention, not from a
/// publication.
const CLIPPED_RUN: usize = 3;

/// Whether a take sounding for `seconds` (its first event's start to its
/// last event's end) is too short to keep.
pub(super) fn too_short(seconds: f64) -> bool {
    seconds < SHORTEST_TAKE_SECONDS
}

/// Why a placed take is not kept, if it is not.
pub(super) fn reason(placed: &Placed, signal: &Signal) -> Option<DropReason> {
    if placed.cut_off_at_start {
        Some(DropReason::CutOffAtStart)
    } else if placed.cut_off_at_end {
        Some(DropReason::CutOffAtEnd)
    } else if clips(signal, placed.first_frame, placed.end_frame) {
        Some(DropReason::Clips)
    } else {
        None
    }
}

/// Whether any channel of frames `first` to `end` holds a run of
/// [`CLIPPED_RUN`] samples at full scale.
fn clips(signal: &Signal, first: usize, end: usize) -> bool {
    let channels = signal.channels;
    (0..channels).any(|channel| {
        let mut run = 0;
        for frame in first..end {
            if signal.interleaved[frame * channels + channel].abs() >= FULL_SCALE {
                run += 1;
                if run >= CLIPPED_RUN {
                    return true;
                }
            } else {
                run = 0;
            }
        }
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_top_clips_and_a_peak_does_not() {
        let mut samples = vec![0.5f32; 100];
        samples[40] = 1.0;
        fn signal(samples: &[f32]) -> Signal<'_> {
            Signal {
                interleaved: samples,
                channels: 1,
                sample_rate: 1000,
                channel_names: Vec::new(),
            }
        }
        assert!(!clips(&signal(&samples), 0, 100));
        samples[41] = -1.0;
        samples[42] = 1.0;
        assert!(clips(&signal(&samples), 0, 100));
        assert!(!clips(&signal(&samples), 0, 42));
    }
}
