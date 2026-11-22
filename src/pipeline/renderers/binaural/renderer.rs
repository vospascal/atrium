//! The HRTF renderer: SOFA loading, the per-source render loop, the
//! headphone equalisation of the direct sound, and the ambient bus heard
//! through the head's diffuse field.

use realfft::num_complex::Complex;
use sofar::reader::{Filter, OpenOptions, Sofar};

use atrium_core::directivity::directivity_gain;
use atrium_core::panner::distance_gain_at_model;
use atrium_core::probe::Probe;
use atrium_core::speaker::SpeakerLayout;

use super::diffuse_field::{DiffuseField, HeadphoneEqualiser};
use super::direction::{equal_power_pan, to_sofa_coords};
use super::equalised_share::EqualisedShare;
use super::itd_delay::DELAY_BUF_SIZE;
use super::near_field::{near_field_corrections, shared_table};
use super::source_state::{HrtfSource, BLOCK_SIZE, FILTER_UPDATE_INTERVAL};
use crate::audio::near_field::NearFieldTable;
use crate::pipeline::path::{PathEffectChain, PathKind, PathSet, MAX_PATHS};
use crate::pipeline::renderer::SourceContext;
use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::renderers::field::FieldEncoding;

pub struct HrtfRenderer {
    sofa: Option<Sofar>,
    sources: Vec<HrtfSource>,
    filter: Option<Filter>,
    /// The near-field corrections, with the HRIRs; `None` while there are
    /// none.
    near_field: Option<&'static NearFieldTable>,
    /// What the two ears hear of the ambient bus, from the same HRIRs;
    /// `None` while there are none.
    diffuse_field: Option<DiffuseField>,
    /// The headphone equaliser for the direct sound of every source, from
    /// the same HRIRs; `None` while there are none.
    equaliser: Option<HeadphoneEqualiser>,
    /// How much of the equalised render the last callback ended on, so the
    /// probe's switch glides; `None` after a reset, when there is nothing to
    /// glide from.
    equalisation_heard: Option<f32>,
    base_buf: Vec<f32>,
    mono_buf: Vec<f32>,
    left_buf: Vec<f32>,
    right_buf: Vec<f32>,
    /// Extra buffers for the retiring convolver during crossfade.
    xfade_left_buf: Vec<f32>,
    xfade_right_buf: Vec<f32>,
    /// Shared input spectrum buffer — one forward FFT per block shared by L/R convolvers.
    input_spectrum: Vec<Complex<f32>>,
    /// Rate the loaded HRIRs are resampled to; 0 until the device is opened.
    sample_rate: f32,
    hrtf_path: String,
}

impl HrtfRenderer {
    /// Create a renderer for the SOFA file at `hrtf_path`.
    ///
    /// The file is not read yet. HRIRs must be resampled to the rate the
    /// device actually runs at, and that is only known once the device is
    /// opened, so loading happens in `ensure_topology`. Loading at a guessed
    /// rate played 48 kHz HRIRs at 96 kHz, an octave too high.
    pub fn new(hrtf_path: &str) -> Self {
        Self {
            hrtf_path: hrtf_path.to_owned(),
            sofa: None,
            sources: Vec::new(),
            filter: None,
            near_field: None,
            diffuse_field: None,
            equaliser: None,
            equalisation_heard: None,
            base_buf: vec![0.0; BLOCK_SIZE],
            mono_buf: vec![0.0; BLOCK_SIZE],
            left_buf: vec![0.0; BLOCK_SIZE],
            right_buf: vec![0.0; BLOCK_SIZE],
            xfade_left_buf: vec![0.0; BLOCK_SIZE],
            xfade_right_buf: vec![0.0; BLOCK_SIZE],
            input_spectrum: Vec::new(),
            sample_rate: 0.0,
        }
    }

    /// Load the SOFA file resampled to `sample_rate`, replacing any set loaded
    /// for another rate. Leaves the renderer without HRTFs if the file cannot
    /// be read. Called when the device is opened, never while audio runs.
    fn load(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.sources.clear();
        match OpenOptions::new()
            .sample_rate(sample_rate)
            .open(&self.hrtf_path)
        {
            Ok(sofa) => {
                let filter_length = sofa.filter_len();
                // One forward FFT per block is shared by both ears' convolvers.
                let fft_size = (BLOCK_SIZE + filter_length - 1).next_power_of_two();
                self.input_spectrum = vec![Complex::new(0.0, 0.0); fft_size / 2 + 1];
                self.filter = Some(Filter::new(filter_length));
                self.diffuse_field = Some(DiffuseField::new(&self.hrtf_path, &sofa, sample_rate));
                self.equaliser = Some(HeadphoneEqualiser::for_head(
                    &self.hrtf_path,
                    &sofa,
                    sample_rate,
                ));
                self.equalisation_heard = None;
                self.sofa = Some(sofa);
                self.near_field = Some(shared_table());
            }
            Err(error) => {
                eprintln!("HRTF not available: {error}");
                self.sofa = None;
                self.filter = None;
                self.near_field = None;
                self.diffuse_field = None;
                self.equaliser = None;
            }
        }
    }
}

impl Renderer for HrtfRenderer {
    #[allow(clippy::needless_range_loop)]
    fn render_source(
        &mut self,
        source_idx: usize,
        source: &mut dyn atrium_core::source::SoundSource,
        ctx: &SourceContext,
        paths: &PathSet,
        path_effects: &mut [PathEffectChain],
        out: &mut OutputBuffer,
    ) {
        let sofa = match &self.sofa {
            Some(s) => s,
            None => return,
        };

        if source_idx >= self.sources.len() {
            return;
        }

        let path_slice = paths.as_slice();

        // Compute per-path target gains (buffer-rate)
        let mut target_gains = [0.0f32; MAX_PATHS];
        for (pi, path) in path_slice.iter().enumerate() {
            target_gains[pi] = match path.kind {
                PathKind::Direct => {
                    let dist_gain = distance_gain_at_model(
                        ctx.listener.position,
                        ctx.source_pos,
                        ctx.source_ref_distance,
                        ctx.distance_model.max_distance,
                        ctx.distance_model.rolloff,
                        ctx.distance_model.model,
                    );
                    let dir_gain = directivity_gain(
                        ctx.source_pos,
                        ctx.source_orientation,
                        ctx.listener.position,
                        ctx.source_directivity,
                    );
                    dist_gain * dir_gain
                }
                _ => {
                    // Distance model for reflections — same law as the direct path.
                    let dist_gain = distance_gain_at_model(
                        ctx.listener.position,
                        ctx.listener.position + path.direction * path.distance,
                        ctx.source_ref_distance,
                        ctx.distance_model.max_distance,
                        ctx.distance_model.rolloff,
                        ctx.distance_model.model,
                    );
                    path.gain * dist_gain
                }
            } * ctx.heard(path.kind);
        }

        // Update HRTF filter for the direct path only — reflections use cheap
        // stereo panning and don't need convolution.
        let should_update = self.sources[source_idx]
            .update_counter
            .is_multiple_of(FILTER_UPDATE_INTERVAL);
        if should_update {
            if let Some(ref mut filter) = self.filter {
                // Find the direct path index (always first, but be safe)
                for (pi, path) in path_slice.iter().enumerate() {
                    if path.kind != PathKind::Direct {
                        continue;
                    }
                    let (sx, sy, sz) = to_sofa_coords(ctx.source_pos, ctx.listener);
                    sofa.filter(sx, sy, sz, filter);

                    let hpath = &mut self.sources[source_idx].paths[pi];
                    // Hand the stream to the other slot. It last ran several
                    // buffers ago, so its own overlap tail is stale: it takes
                    // over the outgoing slot's tail instead. Both slots then
                    // agree on everything already emitted, and the crossfade
                    // below only blends how the new input block is filtered.
                    let new_active = 1 - hpath.active;
                    let (first, second) = hpath.conv.split_at_mut(1);
                    let (incoming, outgoing) = if new_active == 0 {
                        (&mut first[0], &second[0])
                    } else {
                        (&mut second[0], &first[0])
                    };
                    incoming.left.set_response(&filter.left);
                    incoming.right.set_response(&filter.right);
                    incoming.left.adopt_overlap_from(&outgoing.left);
                    incoming.right.adopt_overlap_from(&outgoing.right);
                    hpath.active = new_active;
                    hpath.xfade_remaining = BLOCK_SIZE;

                    // Clamp to buffer capacity: SOFA delays × high sample rates
                    // (e.g. 0.7 ms × 96 kHz = 67 samples) can exceed the 64-sample
                    // ring; unclamped they would wrap and read future samples.
                    let sr = self.sample_rate;
                    let max_delay = (DELAY_BUF_SIZE - 2) as f32;
                    hpath.itd_left.target_delay_samples = (filter.ldelay * sr).min(max_delay);
                    hpath.itd_right.target_delay_samples = (filter.rdelay * sr).min(max_delay);
                }
            }
        }

        // The near-field corrections, aimed at once a buffer and glided to.
        // With the probe's near field off they glide home and leave the
        // signal, exactly as when the source is far.
        let corrections = match self.near_field {
            Some(table) if ctx.probe.near_field => {
                near_field_corrections(ctx.source_pos, ctx.listener, table)
            }
            _ => None,
        };
        {
            let source_state = &mut self.sources[source_idx];
            source_state.near_field_left.glide_to(
                corrections.map(|ears| ears.left),
                out.num_frames,
                out.sample_rate,
            );
            source_state.near_field_right.glide_to(
                corrections.map(|ears| ears.right),
                out.num_frames,
                out.sample_rate,
            );
        }

        let inv_frames = 1.0 / out.num_frames as f32;

        // How much of this source's direct sound is heard through the
        // equaliser, which every source's direct sound joins (`equaliser`),
        // and how much as the measured responses give it, straight into the
        // output as before there was one.
        let equalised_share =
            EqualisedShare::between(self.equalisation_heard, ctx.probe.headphone_equalisation);

        // Process in blocks of BLOCK_SIZE for FFT convolution
        let mut frame = 0;
        while frame < out.num_frames {
            let block_len = (out.num_frames - frame).min(BLOCK_SIZE);

            // Ensure buffers are large enough
            if self.base_buf.len() < block_len {
                self.base_buf.resize(block_len, 0.0);
                self.mono_buf.resize(block_len, 0.0);
                self.left_buf.resize(block_len, 0.0);
                self.right_buf.resize(block_len, 0.0);
                self.xfade_left_buf.resize(block_len, 0.0);
                self.xfade_right_buf.resize(block_len, 0.0);
            }

            // 1. Fill base samples (consumed once from source)
            for i in 0..block_len {
                let raw = source.next_sample(out.sample_rate);
                self.base_buf[i] = raw;
            }

            // 2. For each path: gain-ramp → render → accumulate
            for (pi, path) in path_slice.iter().enumerate() {
                let prev_gain = self.sources[source_idx].paths[pi].prev_gain;
                let tgt = target_gains[pi];

                // Gain-ramp the base samples into mono_buf, applying per-path effects.
                for i in 0..block_len {
                    let t = (frame + i) as f32 * inv_frames;
                    let gain = prev_gain + (tgt - prev_gain) * t;
                    let filtered = path_effects[pi].process_sample(self.base_buf[i]);
                    self.mono_buf[i] = filtered * gain;

                    // Only the direct path feeds the late reverb send, and it
                    // is taken before the distance and directivity gain: the
                    // send level already carries the distance law, exactly as
                    // in the speaker renderers.
                    if path.kind == PathKind::Direct {
                        if let Some(ref mut rev) = out.reverb_send {
                            let base = (frame + i) * out.channels;
                            rev[base] += filtered * ctx.reverb_send;
                        }
                    }
                }

                if path.kind == PathKind::Direct {
                    // ── Direct path: full HRTF convolution ──
                    let active = self.sources[source_idx].paths[pi].active;

                    // Forward FFT of mono_buf — shared between L/R convolvers.
                    self.sources[source_idx].paths[pi].conv[active]
                        .left
                        .forward_fft(&self.mono_buf[..block_len], &mut self.input_spectrum);

                    self.sources[source_idx].paths[pi].conv[active]
                        .left
                        .process_with_spectrum(
                            &self.input_spectrum,
                            block_len,
                            &mut self.left_buf[..block_len],
                        );
                    self.sources[source_idx].paths[pi].conv[active]
                        .right
                        .process_with_spectrum(
                            &self.input_spectrum,
                            block_len,
                            &mut self.right_buf[..block_len],
                        );

                    // Crossfade if IR was just swapped
                    let xfade_rem = self.sources[source_idx].paths[pi].xfade_remaining;
                    if xfade_rem > 0 {
                        let retiring = 1 - active;
                        self.sources[source_idx].paths[pi].conv[retiring]
                            .left
                            .process_with_spectrum(
                                &self.input_spectrum,
                                block_len,
                                &mut self.xfade_left_buf[..block_len],
                            );
                        self.sources[source_idx].paths[pi].conv[retiring]
                            .right
                            .process_with_spectrum(
                                &self.input_spectrum,
                                block_len,
                                &mut self.xfade_right_buf[..block_len],
                            );

                        // The crossfade is BLOCK_SIZE samples long in total and
                        // may span several calls when the device buffer is
                        // shorter than a block, so the weight continues from
                        // how much of it is already done.
                        let xfade_len = xfade_rem.min(block_len);
                        let already_faded = BLOCK_SIZE - xfade_rem;
                        let inv_xfade = 1.0 / BLOCK_SIZE as f32;
                        for i in 0..xfade_len {
                            let new_weight = (already_faded + i + 1) as f32 * inv_xfade;
                            let old_weight = 1.0 - new_weight;
                            self.left_buf[i] =
                                self.left_buf[i] * new_weight + self.xfade_left_buf[i] * old_weight;
                            self.right_buf[i] = self.right_buf[i] * new_weight
                                + self.xfade_right_buf[i] * old_weight;
                        }
                        self.sources[source_idx].paths[pi].xfade_remaining =
                            xfade_rem.saturating_sub(block_len);
                    }

                    // Near-field correction, ITD delay, then into the
                    // equaliser's bus, and as much as is heard unequalised
                    // into the stereo output.
                    let source_state = &mut self.sources[source_idx];
                    let itd = &mut source_state.paths[pi];
                    // Loaded with the HRIRs: there whenever a source is
                    // convolved at all.
                    let Some(equaliser) = self.equaliser.as_mut() else {
                        continue;
                    };
                    let (bus_left, bus_right) = equaliser.bus(out.num_frames);
                    for i in 0..block_len {
                        let base = (frame + i) * out.channels;
                        let left = itd
                            .itd_left
                            .process(source_state.near_field_left.process(self.left_buf[i]));
                        let right = itd
                            .itd_right
                            .process(source_state.near_field_right.process(self.right_buf[i]));
                        bus_left[frame + i] += left;
                        bus_right[frame + i] += right;
                        let unequalised = match equalised_share {
                            EqualisedShare::None => {
                                // Exactly as before the equaliser: added,
                                // not weighed.
                                out.buffer[base] += left;
                                if out.channels > 1 {
                                    out.buffer[base + 1] += right;
                                }
                                continue;
                            }
                            EqualisedShare::All => continue,
                            EqualisedShare::Gliding { .. } => {
                                1.0 - equalised_share.at((frame + i) as f32 * inv_frames)
                            }
                        };
                        out.buffer[base] += left * unequalised;
                        if out.channels > 1 {
                            out.buffer[base + 1] += right * unequalised;
                        }
                    }
                } else {
                    // ── Reflection paths: cheap stereo panning (no FFT) ──
                    // Compute L/R pan from the reflection's azimuth relative to listener.
                    let apparent_pos = ctx.listener.position + path.direction * path.distance;
                    let (sx, _sy, _sz) = to_sofa_coords(apparent_pos, ctx.listener);
                    let right_component = -_sy; // positive = right ear
                    let dist = (sx * sx + _sy * _sy + _sz * _sz).sqrt().max(0.001);
                    // Equal-power pan: angle → [0, 1] where 0.5 = center
                    let pan = 0.5 + 0.5 * (right_component / dist).clamp(-1.0, 1.0);
                    let (target_left, target_right) = equal_power_pan(pan);
                    // Ramp from where the previous buffer ended, like every
                    // other gain here. A pan that jumps at the buffer boundary
                    // is zipper noise on every head turn.
                    let (previous_left, previous_right) = self.sources[source_idx].paths[pi]
                        .previous_pan_gains
                        .unwrap_or((target_left, target_right));

                    // Accumulate directly into stereo output (no convolution)
                    for i in 0..block_len {
                        let t = (frame + i) as f32 * inv_frames;
                        let gain_left = previous_left + (target_left - previous_left) * t;
                        let gain_right = previous_right + (target_right - previous_right) * t;
                        let base = (frame + i) * out.channels;
                        out.buffer[base] += self.mono_buf[i] * gain_left;
                        if out.channels > 1 {
                            out.buffer[base + 1] += self.mono_buf[i] * gain_right;
                        }
                    }
                    if frame + block_len >= out.num_frames {
                        self.sources[source_idx].paths[pi].previous_pan_gains =
                            Some((target_left, target_right));
                    }
                }
            }

            frame += block_len;
        }

        // Store target gains as prev for next buffer
        for pi in 0..path_slice.len() {
            self.sources[source_idx].paths[pi].prev_gain = target_gains[pi];
        }

        self.sources[source_idx].update_counter += 1;
    }

    fn name(&self) -> &str {
        "hrtf"
    }

    /// Ambient sound goes through the head: one mono bus, given each ear's
    /// diffuse field. Without HRIRs there is no head, and ambient sound is
    /// written to the two channels as the speaker modes write it.
    fn field_encoding(&self, _layout: &SpeakerLayout, _device_channels: usize) -> FieldEncoding {
        if self.diffuse_field.is_some() {
            FieldEncoding::Ears
        } else {
            FieldEncoding::Speaker
        }
    }

    /// The direct sound of every source through the headphone equaliser,
    /// then the ambient bus through the head's diffuse field.
    fn finish_ears(
        &mut self,
        ambient_bus: &[f32],
        ambient_fed: bool,
        probe: &Probe,
        out: &mut OutputBuffer,
    ) {
        let share = EqualisedShare::between(self.equalisation_heard, probe.headphone_equalisation);
        if let Some(equaliser) = &mut self.equaliser {
            equaliser.render(share, out);
        }
        if let Some(diffuse_field) = &mut self.diffuse_field {
            diffuse_field.render(ambient_bus, ambient_fed, share, out);
        }
        self.equalisation_heard = Some(share.end());
    }

    fn ensure_topology(&mut self, source_count: usize, _layout: &SpeakerLayout, sample_rate: f32) {
        if sample_rate != self.sample_rate {
            self.load(sample_rate);
        }
        if let Some(ref sofa) = self.sofa {
            while self.sources.len() < source_count {
                let source_idx = self.sources.len();
                self.sources.push(HrtfSource::new(sofa, source_idx));
            }
        }
    }

    fn reset(&mut self) {
        if let Some(diffuse_field) = &mut self.diffuse_field {
            diffuse_field.reset();
        }
        if let Some(equaliser) = &mut self.equaliser {
            equaliser.reset();
        }
        self.equalisation_heard = None;
        for (source_idx, src) in self.sources.iter_mut().enumerate() {
            src.update_counter = source_idx % FILTER_UPDATE_INTERVAL;
            src.near_field_left.reset();
            src.near_field_right.reset();
            for path in &mut src.paths {
                path.prev_gain = 0.0;
                path.previous_pan_gains = None;
                path.xfade_remaining = 0;
                path.itd_left.reset();
                path.itd_right.reset();
                for slot in &mut path.conv {
                    slot.left.reset();
                    slot.right.reset();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
