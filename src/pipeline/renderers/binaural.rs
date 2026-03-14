//! HrtfRenderer — binaural rendering to stereo headphones via HRTF convolution.
//!
//! The **direct path** gets full HRTF convolution: the HRIR for the source's
//! direction is loaded from a SOFA file and applied via overlap-add FFT.
//! Two convolver pairs (A/B double-buffer) enable click-free IR crossfading
//! when the source direction changes.
//!
//! **Reflection paths** use cheap equal-power stereo panning from their
//! azimuth angle — no FFT convolution. This keeps CPU cost manageable
//! (reflections contribute spaciousness, not precise localization).

use realfft::num_complex::Complex;
use sofar::reader::{Filter, OpenOptions, Sofar};

use crate::audio::convolver::Convolver;

use atrium_core::directivity::directivity_gain;
use atrium_core::listener::Listener;
use atrium_core::panner::distance_gain_at_model;
use atrium_core::speaker::SpeakerLayout;
use atrium_core::types::Vec3;

use crate::pipeline::path::{PathEffectChain, PathKind, PathSet, MAX_PATHS};
use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::source_stage::{SourceContext, SourceOutput};
use crate::pipeline::SourceStageBank;

const BLOCK_SIZE: usize = 128;
const FILTER_UPDATE_INTERVAL: usize = 4;
/// Ring buffer size for per-ear ITD delay. Max human ITD ≈ 0.7ms = 34 samples @ 48kHz.
/// Perceptual reference: ITD JND ≈ 90μs, ILD JND ≈ 2.5 dB (BBC WHP254).
const DELAY_BUF_SIZE: usize = 64;
const DELAY_BUF_MASK: usize = DELAY_BUF_SIZE - 1;

struct ConvPair {
    left: Convolver,
    right: Convolver,
}

/// Per-ear fractional delay line for ITD rendering.
struct ItdDelay {
    buf: [f32; DELAY_BUF_SIZE],
    write_pos: usize,
    /// Current delay in fractional samples, smoothed toward target.
    delay_samples: f32,
    /// Target delay in fractional samples (from SOFA).
    target_delay_samples: f32,
}

impl ItdDelay {
    fn new() -> Self {
        Self {
            buf: [0.0; DELAY_BUF_SIZE],
            write_pos: 0,
            delay_samples: 0.0,
            target_delay_samples: 0.0,
        }
    }

    /// Write a sample and read with fractional delay (linear interpolation).
    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        self.buf[self.write_pos] = input;
        self.write_pos = (self.write_pos + 1) & DELAY_BUF_MASK;

        // Smooth delay changes to avoid clicks
        self.delay_samples += (self.target_delay_samples - self.delay_samples) * 0.01;

        let d = self.delay_samples;
        let int_d = d as usize;
        let frac = d - int_d as f32;

        let idx0 = (self.write_pos + DELAY_BUF_SIZE - 1 - int_d) & DELAY_BUF_MASK;
        let idx1 = (idx0 + DELAY_BUF_SIZE - 1) & DELAY_BUF_MASK;

        self.buf[idx0] * (1.0 - frac) + self.buf[idx1] * frac
    }

    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write_pos = 0;
        self.delay_samples = 0.0;
        self.target_delay_samples = 0.0;
    }
}

struct HrtfPath {
    /// Double-buffered convolver pairs for crossfading IR changes.
    conv: [ConvPair; 2],
    /// Which slot (0 or 1) has the most recent IR.
    active: usize,
    /// Samples remaining in crossfade (0 = no crossfade active).
    xfade_remaining: usize,
    /// Per-ear ITD delay lines (from SOFA delay metadata).
    itd_left: ItdDelay,
    itd_right: ItdDelay,
    prev_gain: f32,
}

struct HrtfSource {
    paths: Vec<HrtfPath>,
}

pub struct HrtfRenderer {
    sofa: Option<Sofar>,
    sources: Vec<HrtfSource>,
    filter: Option<Filter>,
    base_buf: Vec<f32>,
    mono_buf: Vec<f32>,
    left_buf: Vec<f32>,
    right_buf: Vec<f32>,
    /// Extra buffers for the retiring convolver during crossfade.
    xfade_left_buf: Vec<f32>,
    xfade_right_buf: Vec<f32>,
    /// Shared input spectrum buffer — one forward FFT per block shared by L/R convolvers.
    input_spectrum: Vec<Complex<f32>>,
    update_counter: usize,
    sample_rate: f32,
}

impl HrtfRenderer {
    pub fn new(hrtf_path: &str, sample_rate: f32) -> Self {
        match Self::try_load(hrtf_path, sample_rate) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("HRTF not available: {e}");
                Self {
                    sofa: None,
                    sources: Vec::new(),
                    filter: None,
                    base_buf: vec![0.0; BLOCK_SIZE],
                    mono_buf: vec![0.0; BLOCK_SIZE],
                    left_buf: vec![0.0; BLOCK_SIZE],
                    right_buf: vec![0.0; BLOCK_SIZE],
                    xfade_left_buf: vec![0.0; BLOCK_SIZE],
                    xfade_right_buf: vec![0.0; BLOCK_SIZE],
                    input_spectrum: Vec::new(),
                    update_counter: 0,
                    sample_rate,
                }
            }
        }
    }

    fn try_load(hrtf_path: &str, sample_rate: f32) -> Result<Self, Box<dyn std::error::Error>> {
        let sofa = OpenOptions::new()
            .sample_rate(sample_rate)
            .open(hrtf_path)?;

        let filt_len = sofa.filter_len();
        let filter = Filter::new(filt_len);

        // Compute shared FFT buffer size from the first source's convolver.
        let fft_size = (BLOCK_SIZE + filt_len - 1).next_power_of_two();
        let freq_len = fft_size / 2 + 1;

        Ok(Self {
            sofa: Some(sofa),
            sources: Vec::new(),
            filter: Some(filter),
            base_buf: vec![0.0; BLOCK_SIZE],
            mono_buf: vec![0.0; BLOCK_SIZE],
            left_buf: vec![0.0; BLOCK_SIZE],
            right_buf: vec![0.0; BLOCK_SIZE],
            xfade_left_buf: vec![0.0; BLOCK_SIZE],
            xfade_right_buf: vec![0.0; BLOCK_SIZE],
            input_spectrum: vec![Complex::new(0.0, 0.0); freq_len],
            update_counter: 0,
            sample_rate,
        })
    }

    fn new_source(sofa: &Sofar) -> HrtfSource {
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
            });
        }

        HrtfSource { paths }
    }
}

/// Convert source position to SOFA listener-relative coordinates.
/// NOTE: Only yaw rotation is applied. Pitch and roll are ignored — the
/// atrium installation assumes a horizontal listener plane.
fn to_sofa_coords(source_pos: Vec3, listener: &Listener) -> (f32, f32, f32) {
    let d = source_pos - listener.position;
    let yaw = listener.yaw;
    let forward = d.x * yaw.cos() + d.y * yaw.sin();
    let right = d.x * yaw.sin() - d.y * yaw.cos();
    (forward, -right, d.z)
}

impl Renderer for HrtfRenderer {
    #[allow(clippy::needless_range_loop)]
    fn render_source(
        &mut self,
        source_idx: usize,
        source: &mut dyn atrium_core::source::SoundSource,
        source_stages: &mut SourceStageBank,
        ctx: &SourceContext,
        src_out: &SourceOutput,
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
            };
        }

        // Update HRTF filter for the direct path only — reflections use cheap
        // stereo panning and don't need convolution.
        let should_update = self.update_counter.is_multiple_of(FILTER_UPDATE_INTERVAL);
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
                    let new_active = 1 - hpath.active;
                    hpath.conv[new_active].left.set_response(&filter.left);
                    hpath.conv[new_active].right.set_response(&filter.right);
                    hpath.active = new_active;
                    hpath.xfade_remaining = BLOCK_SIZE;

                    let sr = self.sample_rate;
                    hpath.itd_left.target_delay_samples = filter.ldelay * sr;
                    hpath.itd_right.target_delay_samples = filter.rdelay * sr;
                }
            }
        }

        let inv_frames = 1.0 / out.num_frames as f32;

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
                let sample = source_stages.process_sample_all(source_idx, raw);
                self.base_buf[i] = sample * src_out.gain_modifier;
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

                    // Only the direct path feeds the late reverb send.
                    if path.kind == PathKind::Direct {
                        if let Some(ref mut rev) = out.reverb_send {
                            let base = (frame + i) * out.channels;
                            rev[base] += self.mono_buf[i] * src_out.reverb_send;
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

                        let xfade_len = xfade_rem.min(block_len);
                        let inv_xfade = 1.0 / xfade_rem as f32;
                        for i in 0..xfade_len {
                            let new_weight = (i + 1) as f32 * inv_xfade;
                            let old_weight = 1.0 - new_weight;
                            self.left_buf[i] =
                                self.left_buf[i] * new_weight + self.xfade_left_buf[i] * old_weight;
                            self.right_buf[i] = self.right_buf[i] * new_weight
                                + self.xfade_right_buf[i] * old_weight;
                        }
                        self.sources[source_idx].paths[pi].xfade_remaining =
                            xfade_rem.saturating_sub(block_len);
                    }

                    // ITD delay + accumulate into stereo output
                    let itd = &mut self.sources[source_idx].paths[pi];
                    for i in 0..block_len {
                        let base = (frame + i) * out.channels;
                        out.buffer[base] += itd.itd_left.process(self.left_buf[i]);
                        if out.channels > 1 {
                            out.buffer[base + 1] += itd.itd_right.process(self.right_buf[i]);
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
                    let gain_left = (std::f32::consts::FRAC_PI_2 * (1.0 - pan)).cos();
                    let gain_right = (std::f32::consts::FRAC_PI_2 * pan).cos();

                    // Accumulate directly into stereo output (no convolution)
                    for i in 0..block_len {
                        let base = (frame + i) * out.channels;
                        out.buffer[base] += self.mono_buf[i] * gain_left;
                        if out.channels > 1 {
                            out.buffer[base + 1] += self.mono_buf[i] * gain_right;
                        }
                    }
                }
            }

            frame += block_len;
        }

        // Store target gains as prev for next buffer
        for pi in 0..path_slice.len() {
            self.sources[source_idx].paths[pi].prev_gain = target_gains[pi];
        }

        self.update_counter += 1;
    }

    fn name(&self) -> &str {
        "hrtf"
    }

    fn ensure_topology(&mut self, source_count: usize, _layout: &SpeakerLayout, sample_rate: f32) {
        self.sample_rate = sample_rate;
        if let Some(ref sofa) = self.sofa {
            while self.sources.len() < source_count {
                self.sources.push(Self::new_source(sofa));
            }
        }
    }

    fn reset(&mut self) {
        for src in &mut self.sources {
            for path in &mut src.paths {
                path.prev_gain = 0.0;
                path.xfade_remaining = 0;
                path.itd_left.reset();
                path.itd_right.reset();
                for slot in &mut path.conv {
                    slot.left.reset();
                    slot.right.reset();
                }
            }
        }
        self.update_counter = 0;
    }
}
