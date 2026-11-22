//! Step 3: events. One starts when the band level rises
//! [`START_ABOVE_BED_DECIBELS`] above the local bed and ends when it falls
//! below bed + [`END_ABOVE_BED_DECIBELS`]: hysteresis, so a call that decays
//! is not cut in two by its own decay.
//!
//! Both thresholds are set from a measurement, not a publication. On the
//! frog recording (its A-weighted FAST level) the count of calls holds steady
//! with a start threshold of 15 or 20 dB above the bed (17 and 16 events)
//! and breaks apart at 10 dB, where the pulses inside a call become events of
//! their own.

pub(super) const START_ABOVE_BED_DECIBELS: f64 = 15.0;
pub(super) const END_ABOVE_BED_DECIBELS: f64 = 6.0;

/// An event as readings: the first above the start threshold and the last
/// at or above the end threshold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Found {
    pub(super) first: usize,
    pub(super) last: usize,
    pub(super) peak_above_bed: f64,
}

/// The events in `levels` over `bed` (dB, reading by reading).
pub(super) fn find(levels: &[f64], bed: &[f64]) -> Vec<Found> {
    let mut found = Vec::new();
    let mut open: Option<Found> = None;
    for (index, (level, bed)) in levels.iter().zip(bed).enumerate() {
        let above = level - bed;
        match open.as_mut() {
            None => {
                if above > START_ABOVE_BED_DECIBELS {
                    open = Some(Found {
                        first: index,
                        last: index,
                        peak_above_bed: above,
                    });
                }
            }
            Some(event) => {
                if above >= END_ABOVE_BED_DECIBELS {
                    event.last = index;
                    event.peak_above_bed = event.peak_above_bed.max(above);
                } else {
                    found.extend(open.take());
                }
            }
        }
    }
    found.extend(open);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A call that rises to 30 dB, dips to 10 dB (above the end threshold)
    /// and decays is one event; a rise to 12 dB starts none.
    #[test]
    fn hysteresis_keeps_a_call_whole() {
        let levels = [0.0, 30.0, 10.0, 25.0, 8.0, 3.0, 12.0, 0.0, 20.0, 0.0];
        let bed = [0.0; 10];
        assert_eq!(
            find(&levels, &bed),
            [
                Found {
                    first: 1,
                    last: 4,
                    peak_above_bed: 30.0
                },
                Found {
                    first: 8,
                    last: 8,
                    peak_above_bed: 20.0
                }
            ]
        );
    }
}
