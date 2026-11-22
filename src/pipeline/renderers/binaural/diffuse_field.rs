//! Ambient sound at the two ears, as the measured head hears a diffuse field.
//!
//! An ambient sound has no position: it is the sound around the listener.
//! What two ears receive of sound from every direction at once is known
//! from the measured head itself (`statistics`): how loud each ear hears
//! each frequency, and how alike the ears are, nearly the same in the bass
//! and unrelated from about 500 Hz up. Every ambient sound joins one mono
//! bus, and two fixed filters (`ear_filters`) give the left and right ear
//! that coherence, and, unequalised, that colour. They are convolved by the same
//! convolver the point sources use, with one forward transform shared by
//! both ears.
//!
//! A diffuse field is the same from every side, so nothing here depends on
//! where the listener faces: turning the head changes nothing, as it changes
//! nothing in a real one.
//!
//! The same diffuse-field response is what headphones must not hear twice:
//! it holds the measurement loudspeaker and the dummy head's ear canal. The
//! headphone equaliser (`inverse`, at work in `equaliser`) divides it out of
//! the point sources' direct sound, and the ambient filters then leave the
//! colour out altogether, so a diffuse field reaches each ear flat. With the
//! probe's equalisation off both are heard as measured.

mod ear_filters;
mod equaliser;
mod inverse;
mod minimum_phase;
mod statistics;
mod third_octave;

use std::sync::{Arc, Mutex, OnceLock};

use realfft::num_complex::Complex;
use sofar::reader::Sofar;

use super::equalised_share::EqualisedShare;
use crate::audio::convolver::Convolver;
use crate::pipeline::renderer::OutputBuffer;

use ear_filters::{design_ear_filters, design_grid_size, AmbientEarFilters};
pub(super) use equaliser::HeadphoneEqualiser;
use inverse::{design_equaliser, EqualiserFilters};
use statistics::DiffuseStatistics;

/// Frames convolved at a time. The filters are 60 ms long, so the
/// transform is long whatever the block; a block as long as a typical
/// callback makes it one transform per callback.
const BLOCK_FRAMES: usize = 512;

pub(super) struct DiffuseField {
    /// The ears with the head's colour, as the measured responses hear a
    /// diffuse field.
    left: Convolver,
    right: Convolver,
    /// The ears without it, as the equalised responses hear one. Both pairs
    /// always run, so either is whole the moment the probe switches to it.
    equalised_left: Convolver,
    equalised_right: Convolver,
    /// The bus's spectrum, shared by all four convolvers.
    spectrum: Vec<Complex<f32>>,
    left_block: Vec<f32>,
    right_block: Vec<f32>,
    equalised_left_block: Vec<f32>,
    equalised_right_block: Vec<f32>,
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
        let filters = shared_head_filters(hrtf_path, sofa, sample_rate).ambient;
        let convolver = |response: &[f32]| {
            let mut convolver = Convolver::new();
            convolver.init(BLOCK_FRAMES, response);
            convolver
        };
        let left = convolver(&filters.as_measured.left);
        let spectrum = vec![Complex::new(0.0, 0.0); left.freq_len()];
        Self {
            left,
            right: convolver(&filters.as_measured.right),
            equalised_left: convolver(&filters.equalised.left),
            equalised_right: convolver(&filters.equalised.right),
            spectrum,
            left_block: vec![0.0; BLOCK_FRAMES],
            right_block: vec![0.0; BLOCK_FRAMES],
            equalised_left_block: vec![0.0; BLOCK_FRAMES],
            equalised_right_block: vec![0.0; BLOCK_FRAMES],
            filter_frames: filters
                .as_measured
                .left
                .len()
                .max(filters.equalised.left.len()),
            ringing_frames: 0,
        }
    }

    /// Add the two ears of the ambient `bus` to `out`, `share` of them
    /// equalised. `fed` says whether any ambient sound wrote to the bus this
    /// callback; when none did and the filters have rung out, this returns
    /// at once.
    pub(super) fn render(
        &mut self,
        bus: &[f32],
        fed: bool,
        share: EqualisedShare,
        out: &mut OutputBuffer,
    ) {
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
            self.equalised_left.process_with_spectrum(
                &self.spectrum,
                length,
                &mut self.equalised_left_block[..length],
            );
            self.equalised_right.process_with_spectrum(
                &self.spectrum,
                length,
                &mut self.equalised_right_block[..length],
            );
            match share {
                EqualisedShare::None => {
                    for index in 0..length {
                        let base = (start + index) * out.channels;
                        out.buffer[base] += self.left_block[index];
                        if out.channels > 1 {
                            out.buffer[base + 1] += self.right_block[index];
                        }
                    }
                }
                EqualisedShare::All => {
                    for index in 0..length {
                        let base = (start + index) * out.channels;
                        out.buffer[base] += self.equalised_left_block[index];
                        if out.channels > 1 {
                            out.buffer[base + 1] += self.equalised_right_block[index];
                        }
                    }
                }
                EqualisedShare::Gliding { .. } => {
                    let inverse_frames = 1.0 / out.num_frames as f32;
                    for index in 0..length {
                        let frame = start + index;
                        let equalised = share.at(frame as f32 * inverse_frames);
                        let as_measured = 1.0 - equalised;
                        let base = frame * out.channels;
                        out.buffer[base] += self.left_block[index] * as_measured
                            + self.equalised_left_block[index] * equalised;
                        if out.channels > 1 {
                            out.buffer[base + 1] += self.right_block[index] * as_measured
                                + self.equalised_right_block[index] * equalised;
                        }
                    }
                }
            }
            start += length;
        }
    }

    pub(super) fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
        self.equalised_left.reset();
        self.equalised_right.reset();
        self.ringing_frames = 0;
    }
}

impl HeadphoneEqualiser {
    /// The equaliser for the responses in `sofa`, read from `hrtf_path`, at
    /// `sample_rate`. Called when the HRIRs are loaded, never on the audio
    /// thread.
    pub(super) fn for_head(hrtf_path: &str, sofa: &Sofar, sample_rate: f32) -> Self {
        Self::new(shared_head_filters(hrtf_path, sofa, sample_rate).equaliser)
    }
}

/// Everything designed from one head's diffuse field.
#[derive(Clone)]
struct HeadFilters {
    ambient: Arc<AmbientEarFilters>,
    equaliser: Arc<EqualiserFilters>,
}

/// The filters for one HRIR file at one rate, designed the first time they
/// are asked for. They depend on nothing else, so every renderer of that
/// file at that rate shares them: building the pipelines again for another
/// scene does not measure the head again.
fn shared_head_filters(hrtf_path: &str, sofa: &Sofar, sample_rate: f32) -> HeadFilters {
    type Designed = Vec<(String, u32, HeadFilters)>;
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
        return filters.clone();
    }
    let grid_size = design_grid_size(sample_rate);
    let statistics = DiffuseStatistics::measure(sofa, sample_rate, grid_size);
    let filters = HeadFilters {
        ambient: Arc::new(design_ear_filters(&statistics, sample_rate)),
        equaliser: Arc::new(design_equaliser(&statistics, sample_rate, grid_size)),
    };
    designed.push((hrtf_path.to_owned(), rate, filters.clone()));
    filters
}

#[cfg(test)]
mod tests;
