//! The headphone equaliser at work: one filter per ear on the sum of every
//! point source's direct sound.
//!
//! The equaliser (`inverse`) is the same for every direction, so filtering
//! each source's two ears by it and adding them is filtering their sum.
//! Every source adds its convolved direct sound to a stereo bus; once all
//! have, the bus is filtered once and added to the output. The reflections
//! are panned, not convolved with the measured responses, and the late
//! reverb never was: neither carries the head's colour, so neither passes
//! through here.
//!
//! The filters run whether the equalisation is heard or not. Switched on,
//! the equalised sound is then already right from its first sample, and
//! the switch glides between two renders that are both whole.

use std::sync::Arc;

use realfft::num_complex::Complex;

use super::super::equalised_share::EqualisedShare;
use super::inverse::EqualiserFilters;
use crate::audio::convolver::Convolver;
use crate::pipeline::renderer::OutputBuffer;

/// Frames filtered at a time: one transform per typical callback.
const BLOCK_FRAMES: usize = 512;

/// Frames the bus holds without growing, as the pipeline's own buffers.
const LARGEST_EXPECTED_CALLBACK_FRAMES: usize = 8192;

pub(in crate::pipeline::renderers::binaural) struct HeadphoneEqualiser {
    left: Convolver,
    right: Convolver,
    spectrum: Vec<Complex<f32>>,
    /// Each ear's direct sound from every source this callback, before the
    /// equaliser. Silent between callbacks.
    bus_left: Vec<f32>,
    bus_right: Vec<f32>,
    left_block: Vec<f32>,
    right_block: Vec<f32>,
    /// Whether any source added to the bus this callback.
    fed: bool,
    /// How long the filters ring after their input stops, in frames.
    filter_frames: usize,
    /// Frames the filters still ring for; at zero nothing is computed.
    ringing_frames: usize,
}

impl HeadphoneEqualiser {
    /// Built when the HRIRs are loaded, never on the audio thread.
    pub(super) fn new(filters: Arc<EqualiserFilters>) -> Self {
        let mut left = Convolver::new();
        let mut right = Convolver::new();
        left.init(BLOCK_FRAMES, &filters.left);
        right.init(BLOCK_FRAMES, &filters.right);
        let spectrum = vec![Complex::new(0.0, 0.0); left.freq_len()];
        Self {
            left,
            right,
            spectrum,
            bus_left: Vec::with_capacity(LARGEST_EXPECTED_CALLBACK_FRAMES),
            bus_right: Vec::with_capacity(LARGEST_EXPECTED_CALLBACK_FRAMES),
            left_block: vec![0.0; BLOCK_FRAMES],
            right_block: vec![0.0; BLOCK_FRAMES],
            fed: false,
            filter_frames: filters.left.len(),
            ringing_frames: 0,
        }
    }

    /// The two ears of the bus for a callback of `num_frames`, for a source
    /// to add its direct sound to.
    pub(in crate::pipeline::renderers::binaural) fn bus(
        &mut self,
        num_frames: usize,
    ) -> (&mut [f32], &mut [f32]) {
        self.bus_left.resize(num_frames, 0.0);
        self.bus_right.resize(num_frames, 0.0);
        self.fed = true;
        (&mut self.bus_left, &mut self.bus_right)
    }

    /// Filter the bus and add `share` of it to `out`, then empty it for the
    /// next callback. When nothing was added and the filters have rung out,
    /// this returns at once.
    pub(in crate::pipeline::renderers::binaural) fn render(
        &mut self,
        share: EqualisedShare,
        out: &mut OutputBuffer,
    ) {
        if std::mem::take(&mut self.fed) {
            self.ringing_frames = self.filter_frames;
        } else if self.ringing_frames == 0 {
            return;
        } else {
            self.ringing_frames = self.ringing_frames.saturating_sub(out.num_frames);
        }
        self.bus_left.resize(out.num_frames, 0.0);
        self.bus_right.resize(out.num_frames, 0.0);
        let inverse_frames = 1.0 / out.num_frames as f32;
        let mut start = 0;
        while start < out.num_frames {
            let length = (out.num_frames - start).min(BLOCK_FRAMES);
            let end = start + length;
            self.left
                .forward_fft(&self.bus_left[start..end], &mut self.spectrum);
            self.left
                .process_with_spectrum(&self.spectrum, length, &mut self.left_block[..length]);
            self.right
                .forward_fft(&self.bus_right[start..end], &mut self.spectrum);
            self.right.process_with_spectrum(
                &self.spectrum,
                length,
                &mut self.right_block[..length],
            );
            if share != EqualisedShare::None {
                for index in 0..length {
                    let frame = start + index;
                    let weight = share.at(frame as f32 * inverse_frames);
                    let base = frame * out.channels;
                    out.buffer[base] += self.left_block[index] * weight;
                    if out.channels > 1 {
                        out.buffer[base + 1] += self.right_block[index] * weight;
                    }
                }
            }
            start = end;
        }
        self.bus_left.fill(0.0);
        self.bus_right.fill(0.0);
    }

    pub(in crate::pipeline::renderers::binaural) fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
        self.bus_left.fill(0.0);
        self.bus_right.fill(0.0);
        self.fed = false;
        self.ringing_frames = 0;
    }
}
