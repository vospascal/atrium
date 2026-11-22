//! One arc's filters for every way the head can face it.
//!
//! An arc is fixed in the world and the head turns, so the arc's direction
//! relative to the head changes some 25 times a second while head tracking
//! runs. Designing filters takes milliseconds of transforms, far too long
//! for the audio thread, so an arc of one width is designed once for every
//! relative bearing in steps of `BEARING_STEP_DEGREES`, and the audio thread
//! only picks the nearest and crossfades to it.

use super::super::ear_filters::EarFilters;
use super::design::ArcHead;

/// How finely the head's turn is followed, in degrees. The nearest bearing
/// is at most half a step away; for the narrowest arc, 10°, half a step
/// moves it by a quarter of its width, and it is crossfaded to over one
/// callback.
pub(in crate::pipeline::renderers::binaural) const BEARING_STEP_DEGREES: usize = 5;

const BEARINGS: usize = 360 / BEARING_STEP_DEGREES;

/// The two ears' filters of an arc of one width, for every relative bearing.
pub(in crate::pipeline::renderers::binaural) struct ArcTable {
    width_degrees: u16,
    /// Index `k` is the arc centred `k · BEARING_STEP_DEGREES` anticlockwise
    /// from straight ahead.
    bearings: Vec<EarFilters>,
}

impl ArcTable {
    /// Every bearing of an arc `width_degrees` wide. Takes some tenths of a
    /// second: never on the audio thread. The bearings are designed side by
    /// side on the machine's cores.
    pub(in crate::pipeline::renderers::binaural) fn design(
        head: &ArcHead,
        width_degrees: u16,
    ) -> Self {
        let workers = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1)
            .clamp(1, BEARINGS);
        let mut bearings: Vec<Option<EarFilters>> = (0..BEARINGS).map(|_| None).collect();
        std::thread::scope(|scope| {
            for (worker, chunk) in bearings.chunks_mut(BEARINGS.div_ceil(workers)).enumerate() {
                let first = worker * BEARINGS.div_ceil(workers);
                scope.spawn(move || {
                    for (offset, slot) in chunk.iter_mut().enumerate() {
                        let centre = ((first + offset) * BEARING_STEP_DEGREES) as f64;
                        *slot = Some(head.design(centre, width_degrees as f64));
                    }
                });
            }
        });
        Self {
            width_degrees,
            bearings: bearings
                .into_iter()
                .map(|filters| filters.expect("every bearing designed"))
                .collect(),
        }
    }

    pub(in crate::pipeline::renderers::binaural) fn width_degrees(&self) -> u16 {
        self.width_degrees
    }

    /// The bearing nearest to `relative_azimuth` radians anticlockwise from
    /// straight ahead.
    pub(in crate::pipeline::renderers::binaural) fn nearest(relative_azimuth: f32) -> usize {
        let steps = relative_azimuth.to_degrees() / BEARING_STEP_DEGREES as f32;
        (steps.round() as i64).rem_euclid(BEARINGS as i64) as usize
    }

    pub(in crate::pipeline::renderers::binaural) fn ears(&self, bearing: usize) -> &EarFilters {
        &self.bearings[bearing]
    }
}
