//! Convolution state the renderer keeps per source and per path.

use sofar::reader::{Filter, Sofar};

use super::itd_delay::ItdDelay;
use crate::audio::convolver::Convolver;
use crate::pipeline::path::MAX_PATHS;

pub(super) const BLOCK_SIZE: usize = 128;
pub(super) const FILTER_UPDATE_INTERVAL: usize = 4;

pub(super) struct ConvPair {
    pub(super) left: Convolver,
    pub(super) right: Convolver,
}

pub(super) struct HrtfPath {
    /// Double-buffered convolver pairs for crossfading IR changes.
    pub(super) conv: [ConvPair; 2],
    /// Which slot (0 or 1) has the most recent IR.
    pub(super) active: usize,
    /// Samples remaining in crossfade (0 = no crossfade active).
    pub(super) xfade_remaining: usize,
    /// Per-ear ITD delay lines (from SOFA delay metadata).
    pub(super) itd_left: ItdDelay,
    pub(super) itd_right: ItdDelay,
    pub(super) prev_gain: f32,
    /// Left and right pan gains this path ended the previous buffer on, when
    /// it is a reflection. `None` until it has been panned once, so the first
    /// buffer starts on its target instead of fading in from nowhere.
    pub(super) previous_pan_gains: Option<(f32, f32)>,
}

pub(super) struct HrtfSource {
    pub(super) paths: Vec<HrtfPath>,
    /// Per-source buffer counter gating HRTF filter updates. Must be
    /// per-source: a single renderer-wide counter incremented once per
    /// `render_source` call only ever hits `% FILTER_UPDATE_INTERVAL == 0`
    /// for some sources when the source count shares a factor with the
    /// interval — with 2 sources, source 1 would never update its direction.
    pub(super) update_counter: usize,
}

impl HrtfSource {
    pub(super) fn new(sofa: &Sofar, source_idx: usize) -> HrtfSource {
        let filt_len = sofa.filter_len();
        let mut init_filter = Filter::new(filt_len);
        sofa.filter(0.0, 1.0, 0.0, &mut init_filter);
        let mut paths = Vec::with_capacity(MAX_PATHS);
        for _ in 0..MAX_PATHS {
            let mut a_left = Convolver::new();
            let mut a_right = Convolver::new();
            let mut b_left = Convolver::new();
            let mut b_right = Convolver::new();
            a_left.init(BLOCK_SIZE, &init_filter.left);
            a_right.init(BLOCK_SIZE, &init_filter.right);
            b_left.init(BLOCK_SIZE, &init_filter.left);
            b_right.init(BLOCK_SIZE, &init_filter.right);
            paths.push(HrtfPath {
                conv: [
                    ConvPair {
                        left: a_left,
                        right: a_right,
                    },
                    ConvPair {
                        left: b_left,
                        right: b_right,
                    },
                ],
                active: 0,
                xfade_remaining: 0,
                itd_left: ItdDelay::new(),
                itd_right: ItdDelay::new(),
                prev_gain: 0.0,
                previous_pan_gains: None,
            });
        }

        HrtfSource {
            paths,
            // Stagger initial phases so at most one source recomputes its
            // HRTF filter per buffer (for source counts ≤ the interval).
            update_counter: source_idx % FILTER_UPDATE_INTERVAL,
        }
    }
}
