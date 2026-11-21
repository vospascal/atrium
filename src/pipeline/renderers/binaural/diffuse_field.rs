//! Ambient sound at the two ears, as the measured head hears a diffuse field.
//!
//! An ambient sound has no position: it is the sound around the listener.
//! What two ears receive of sound from every direction at once is known
//! from the measured head itself (`statistics`): how loud each ear hears
//! each frequency, and how alike the ears are, nearly the same in the bass
//! and unrelated from about 500 Hz up. Every ambient sound joins one mono
//! bus, and two fixed filters (`ear_filters`) give the left and right ear
//! exactly that colour and that coherence. They are convolved by the same
//! convolver the point sources use, with one forward transform shared by
//! both ears.
//!
//! A diffuse field is the same from every side, so nothing here depends on
//! where the listener faces: turning the head changes nothing, as it changes
//! nothing in a real one.

mod ear_filters;
mod statistics;

use std::sync::{Arc, Mutex, OnceLock};

use realfft::num_complex::Complex;
use sofar::reader::Sofar;

use crate::audio::convolver::Convolver;
use crate::pipeline::renderer::OutputBuffer;

use ear_filters::{design_ear_filters, design_grid_size, EarFilters};
use statistics::DiffuseStatistics;

/// Frames convolved at a time. The filters are 60 ms long, so the
/// transform is long whatever the block; a block as long as a typical
/// callback makes it one transform per callback.
const BLOCK_FRAMES: usize = 512;

pub(super) struct DiffuseField {
    left: Convolver,
    right: Convolver,
    /// The bus's spectrum, shared by both ears' convolvers.
    spectrum: Vec<Complex<f32>>,
    left_block: Vec<f32>,
    right_block: Vec<f32>,
    /// How long the filters ring after their input stops, in frames.
    filter_frames: usize,
    /// Frames the filters still ring for since the bus last carried sound;
    /// at zero nothing is left to play and nothing is computed.
    ringing_frames: usize,
}

impl DiffuseField {
    /// The ears' filters for the responses in `sofa`, read from
    /// `hrtf_path`, at `sample_rate`. Called when the HRIRs are loaded, never
    /// on the audio thread.
    pub(super) fn new(hrtf_path: &str, sofa: &Sofar, sample_rate: f32) -> Self {
        let filters = shared_ear_filters(hrtf_path, sofa, sample_rate);
        let mut left = Convolver::new();
        let mut right = Convolver::new();
        left.init(BLOCK_FRAMES, &filters.left);
        right.init(BLOCK_FRAMES, &filters.right);
        let spectrum = vec![Complex::new(0.0, 0.0); left.freq_len()];
        Self {
            left,
            right,
            spectrum,
            left_block: vec![0.0; BLOCK_FRAMES],
            right_block: vec![0.0; BLOCK_FRAMES],
            filter_frames: filters.left.len(),
            ringing_frames: 0,
        }
    }

    /// Add the two ears of the ambient `bus` to `out`. `fed` says whether
    /// any ambient sound wrote to the bus this callback; when none did and
    /// the filters have rung out, this returns at once.
    pub(super) fn render(&mut self, bus: &[f32], fed: bool, out: &mut OutputBuffer) {
        if fed {
            self.ringing_frames = self.filter_frames;
        } else if self.ringing_frames == 0 {
            return;
        } else {
            self.ringing_frames = self.ringing_frames.saturating_sub(out.num_frames);
        }
        let mut start = 0;
        while start < out.num_frames {
            let length = (out.num_frames - start).min(BLOCK_FRAMES);
            self.left
                .forward_fft(&bus[start..start + length], &mut self.spectrum);
            self.left
                .process_with_spectrum(&self.spectrum, length, &mut self.left_block[..length]);
            self.right.process_with_spectrum(
                &self.spectrum,
                length,
                &mut self.right_block[..length],
            );
            for index in 0..length {
                let base = (start + index) * out.channels;
                out.buffer[base] += self.left_block[index];
                if out.channels > 1 {
                    out.buffer[base + 1] += self.right_block[index];
                }
            }
            start += length;
        }
    }

    pub(super) fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
        self.ringing_frames = 0;
    }
}

/// The ears' filters for one HRIR file at one rate, designed the first time
/// they are asked for. They depend on nothing else, so every renderer of
/// that file at that rate shares them: building the pipelines again for
/// another scene does not measure the head again.
fn shared_ear_filters(hrtf_path: &str, sofa: &Sofar, sample_rate: f32) -> Arc<EarFilters> {
    type Designed = Vec<(String, u32, Arc<EarFilters>)>;
    static DESIGNED: OnceLock<Mutex<Designed>> = OnceLock::new();
    let mut designed = DESIGNED
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let rate = sample_rate.to_bits();
    if let Some((_, _, filters)) = designed
        .iter()
        .find(|(path, designed_rate, _)| path == hrtf_path && *designed_rate == rate)
    {
        return Arc::clone(filters);
    }
    let statistics = DiffuseStatistics::measure(sofa, sample_rate, design_grid_size(sample_rate));
    let filters = Arc::new(design_ear_filters(&statistics, sample_rate));
    designed.push((hrtf_path.to_owned(), rate, Arc::clone(&filters)));
    filters
}

#[cfg(test)]
mod tests;
