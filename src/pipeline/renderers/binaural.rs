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
use crate::pipeline::renderer::SourceContext;
use crate::pipeline::renderer::{OutputBuffer, Renderer};

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
    /// Left and right pan gains this path ended the previous buffer on, when
    /// it is a reflection. `None` until it has been panned once, so the first
    /// buffer starts on its target instead of fading in from nowhere.
    previous_pan_gains: Option<(f32, f32)>,
}

struct HrtfSource {
    paths: Vec<HrtfPath>,
    /// Per-source buffer counter gating HRTF filter updates. Must be
    /// per-source: a single renderer-wide counter incremented once per
    /// `render_source` call only ever hits `% FILTER_UPDATE_INTERVAL == 0`
    /// for some sources when the source count shares a factor with the
    /// interval — with 2 sources, source 1 would never update its direction.
    update_counter: usize,
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
                self.sofa = Some(sofa);
            }
            Err(error) => {
                eprintln!("HRTF not available: {error}");
                self.sofa = None;
                self.filter = None;
            }
        }
    }

    fn new_source(sofa: &Sofar, source_idx: usize) -> HrtfSource {
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

/// Equal-power stereo gains from a lateral pan position.
/// `pan` ∈ [0, 1]: 0 = full left, 0.5 = center, 1 = full right.
/// Invariant: gain_left² + gain_right² = 1 for all pan values.
#[inline]
fn equal_power_pan(pan: f32) -> (f32, f32) {
    let gain_left = (std::f32::consts::FRAC_PI_2 * pan).cos();
    let gain_right = (std::f32::consts::FRAC_PI_2 * (1.0 - pan)).cos();
    (gain_left, gain_right)
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
            };
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

    fn ensure_topology(&mut self, source_count: usize, _layout: &SpeakerLayout, sample_rate: f32) {
        if sample_rate != self.sample_rate {
            self.load(sample_rate);
        }
        if let Some(ref sofa) = self.sofa {
            while self.sources.len() < source_count {
                let source_idx = self.sources.len();
                self.sources.push(Self::new_source(sofa, source_idx));
            }
        }
    }

    fn reset(&mut self) {
        for (source_idx, src) in self.sources.iter_mut().enumerate() {
            src.update_counter = source_idx % FILTER_UPDATE_INTERVAL;
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
mod tests {
    use super::*;
    use atrium_core::directivity::DirectivityPattern;
    use atrium_core::panner::DistanceModelType;
    use atrium_core::source::SoundSource;

    use crate::audio::distance::DistanceModel;
    use crate::pipeline::environment::Environment;
    use crate::pipeline::path::PathContribution;

    const SAMPLE_RATE: f32 = 48000.0;

    fn sofa_file() -> String {
        format!("{}/assets/hrtf/default.sofa", env!("CARGO_MANIFEST_DIR"))
    }

    /// A test signal: sample `n` is `signal_at(n)`. Position is supplied to the
    /// renderer through the context, not read from here.
    struct TestSignal {
        sample_index: usize,
        signal_at: fn(usize) -> f32,
    }

    impl SoundSource for TestSignal {
        fn next_sample(&mut self, _sample_rate: f32) -> f32 {
            let sample = (self.signal_at)(self.sample_index);
            self.sample_index += 1;
            sample
        }
        fn position(&self) -> Vec3 {
            Vec3::ZERO
        }
        fn tick(&mut self, _dt: f32) {}
    }

    fn noise(sample_index: usize) -> f32 {
        let mut state = (sample_index as u32).wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
        state ^= state >> 15;
        state = state.wrapping_mul(0x2C1B_3C6D);
        state ^= state >> 12;
        (state as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    fn direct_path(listener: &Listener, source_position: Vec3) -> PathSet {
        let offset = source_position - listener.position;
        let mut paths = PathSet::new();
        paths.push(PathContribution {
            kind: PathKind::Direct,
            direction: offset.normalize(),
            distance: offset.length(),
            delay_seconds: 0.0,
            gain: 1.0,
            wall_index: None,
        });
        paths
    }

    /// Everything a `SourceContext` borrows, for an anechoic free field with
    /// the inverse distance law (reference 1 m) and an omni source.
    struct Scene {
        environment: Environment,
        distance_model: DistanceModel,
        directivity: DirectivityPattern,
        layout: SpeakerLayout,
    }

    impl Scene {
        fn new() -> Self {
            Self {
                environment: Environment::anechoic(
                    Vec3::new(-500.0, -500.0, 0.0),
                    Vec3::new(500.0, 500.0, 500.0),
                ),
                distance_model: DistanceModel {
                    model: DistanceModelType::Inverse,
                    ref_distance: 1.0,
                    max_distance: 1000.0,
                    rolloff: 1.0,
                },
                directivity: DirectivityPattern::Omni,
                layout: SpeakerLayout::stereo(Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, -1.0, 0.0)),
            }
        }

        /// Render one buffer; returns (interleaved stereo, interleaved reverb send).
        #[allow(clippy::too_many_arguments)]
        fn render(
            &self,
            renderer: &mut HrtfRenderer,
            signal: &mut TestSignal,
            listener: &Listener,
            source_position: Vec3,
            paths: &PathSet,
            reverb_send_level: f32,
            num_frames: usize,
        ) -> (Vec<f32>, Vec<f32>) {
            let ctx = SourceContext {
                listener,
                source_pos: source_position,
                source_orientation: Vec3::new(1.0, 0.0, 0.0),
                source_directivity: &self.directivity,
                source_spread: 0.0,
                source_ref_distance: 1.0,
                path_target: listener.position,
                environment: &self.environment,
                sample_rate: SAMPLE_RATE,
                distance_model: &self.distance_model,
                layout: &self.layout,
                reverb_send: reverb_send_level,
            };
            let mut chains: Vec<PathEffectChain> = (0..MAX_PATHS)
                .map(|_| PathEffectChain::new(Vec::new()))
                .collect();
            let mut block = vec![0.0f32; num_frames * 2];
            let mut reverb = vec![0.0f32; num_frames * 2];
            let mut out = OutputBuffer {
                buffer: &mut block,
                reverb_send: Some(&mut reverb),
                channels: 2,
                num_frames,
                sample_rate: SAMPLE_RATE,
            };
            renderer.render_source(0, signal, &ctx, paths, &mut chains, &mut out);
            (block, reverb)
        }
    }

    fn loaded_renderer(scene: &Scene) -> HrtfRenderer {
        let mut renderer = HrtfRenderer::new(&sofa_file());
        renderer.ensure_topology(1, &scene.layout, SAMPLE_RATE);
        assert!(renderer.sofa.is_some(), "default.sofa must load");
        renderer
    }

    fn split_channels(interleaved: &[f32]) -> (Vec<f32>, Vec<f32>) {
        (
            interleaved.iter().step_by(2).copied().collect(),
            interleaved.iter().skip(1).step_by(2).copied().collect(),
        )
    }

    fn energy(samples: &[f32]) -> f32 {
        samples.iter().map(|sample| sample * sample).sum()
    }

    /// Lag (in samples) of `right` relative to `left` that maximises their
    /// cross-correlation. Positive = right ear hears it later.
    fn interaural_lag(left: &[f32], right: &[f32], max_lag: i32) -> i32 {
        let correlation = |lag: i32| -> f32 {
            (0..left.len() as i32)
                .filter_map(|index| {
                    let other = index + lag;
                    (other >= 0 && (other as usize) < right.len())
                        .then(|| left[index as usize] * right[other as usize])
                })
                .sum()
        };
        (-max_lag..=max_lag)
            .max_by(|a, b| correlation(*a).total_cmp(&correlation(*b)))
            .unwrap()
    }

    /// Impulse response of the HRTF renderer for a source 1 m away in
    /// `direction` (world frame), listener at the origin with `yaw`.
    ///
    /// The first 1024-sample buffer is silent: it loads this direction's HRIR
    /// (crossfading away from the initial one) and settles the gain ramp at
    /// unity (1 m = reference distance). The impulse is the first sample of
    /// the second buffer, which has no filter update and no ramp.
    fn impulse_response(yaw: f32, direction: Vec3) -> (Vec<f32>, Vec<f32>) {
        let scene = Scene::new();
        let mut renderer = loaded_renderer(&scene);
        let listener = Listener::new(Vec3::ZERO, yaw);
        let source_position = direction.normalize();
        let paths = direct_path(&listener, source_position);
        let mut signal = TestSignal {
            sample_index: 0,
            signal_at: |index| if index == 1024 { 1.0 } else { 0.0 },
        };
        let mut render = || {
            scene.render(
                &mut renderer,
                &mut signal,
                &listener,
                source_position,
                &paths,
                0.0,
                1024,
            )
        };
        let (silent, _) = render();
        assert!(silent.iter().all(|sample| *sample == 0.0));
        let (block, _) = render();
        split_channels(&block)
    }

    fn level_difference_db(louder: &[f32], quieter: &[f32]) -> f32 {
        10.0 * (energy(louder) / energy(quieter)).log10()
    }

    /// SOFA frame: x forward, y LEFT, z up. World: yaw 0 faces +X and +Y is
    /// to the listener's left (right-handed, z up), so at yaw 0 the two frames
    /// coincide. Hand-derived cases, source 2 m from the listener at (1, 1, 0):
    #[test]
    fn sofa_coordinates_rotate_with_listener_yaw() {
        let close = |actual: (f32, f32, f32), expected: (f32, f32, f32)| {
            (actual.0 - expected.0).abs() < 1e-5
                && (actual.1 - expected.1).abs() < 1e-5
                && (actual.2 - expected.2).abs() < 1e-5
        };
        let origin = Vec3::new(1.0, 1.0, 0.0);
        let facing = |yaw: f32| Listener::new(origin, yaw);
        let north = origin + Vec3::new(0.0, 2.0, 0.5);
        let east = origin + Vec3::new(2.0, 0.0, 0.0);
        // Yaw 0 (facing +X): north is to the left → SOFA (0, +2), height passes through.
        assert!(close(to_sofa_coords(north, &facing(0.0)), (0.0, 2.0, 0.5)));
        // Yaw 90° (facing +Y): north is straight ahead.
        let quarter = std::f32::consts::FRAC_PI_2;
        assert!(close(
            to_sofa_coords(north, &facing(quarter)),
            (2.0, 0.0, 0.5)
        ));
        // Facing north, east is on the right → SOFA y = −2.
        assert!(close(
            to_sofa_coords(east, &facing(quarter)),
            (0.0, -2.0, 0.0)
        ));
        // Yaw 180° (facing −X): north, formerly left, is now on the right.
        let half = std::f32::consts::PI;
        assert!(close(
            to_sofa_coords(north, &facing(half)),
            (0.0, -2.0, 0.5)
        ));

        // A rotation: the horizontal distance is preserved at every heading.
        for step in 0..72 {
            let yaw = step as f32 * 5f32.to_radians();
            let (forward, left, _) =
                to_sofa_coords(origin + Vec3::new(3.0, -4.0, 0.0), &facing(yaw));
            assert!(((forward * forward + left * left).sqrt() - 5.0).abs() < 1e-4);
        }
    }

    /// A source hard left arrives at the left ear earlier and louder.
    ///
    /// ITD reference (Woodworth): (a/c)(θ + sin θ) with a = 0.0875 m,
    /// c = 343 m/s, θ = 90° → 0.000255 × 2.571 = 0.656 ms = 31.5 samples at
    /// 48 kHz (28.9 at 44.1 kHz, 63.0 at 96 kHz). KEMAR measures a little wider
    /// than the spherical head: the rendered lag is 35 samples (0.73 ms).
    /// ILD: KEMAR broadband 3.167 / 0.210 energy = 11.8 dB at 90°.
    #[test]
    fn hard_left_source_reaches_left_ear_earlier_and_louder() {
        let (left, right) = impulse_response(0.0, Vec3::new(0.0, 1.0, 0.0));
        let lag = interaural_lag(&left, &right, 60);
        let lag_ms = lag as f32 / SAMPLE_RATE * 1000.0;
        assert!(
            (0.6..0.8).contains(&lag_ms),
            "right ear should lag by ≈ 0.66–0.73 ms, got {lag} samples ({lag_ms:.3} ms)"
        );
        let ild = level_difference_db(&left, &right);
        assert!(
            (ild - 11.8).abs() < 0.5,
            "left ear should be 11.8 dB louder, got {ild:.2} dB"
        );
    }

    /// Straight ahead: both ears identical in level and time.
    #[test]
    fn source_straight_ahead_is_symmetric() {
        let (left, right) = impulse_response(0.0, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(interaural_lag(&left, &right, 60), 0);
        assert!(level_difference_db(&left, &right).abs() < 0.05);
    }

    /// Turning the head 180° puts a left source on the right: the ears swap.
    #[test]
    fn yaw_of_180_degrees_swaps_the_ears() {
        let source = Vec3::new(0.0, 1.0, 0.0);
        let (left_facing_east, right_facing_east) = impulse_response(0.0, source);
        let (left_facing_west, right_facing_west) = impulse_response(std::f32::consts::PI, source);
        let relative = |a: f32, b: f32| (a - b).abs() / b;
        assert!(relative(energy(&left_facing_east), energy(&right_facing_west)) < 1e-3);
        assert!(relative(energy(&right_facing_east), energy(&left_facing_west)) < 1e-3);
        assert_eq!(
            interaural_lag(&left_facing_east, &right_facing_east, 60),
            -interaural_lag(&left_facing_west, &right_facing_west, 60)
        );
    }

    /// Head tracking: facing north (yaw 90°), a source due east is on the
    /// right — the world stays put while the head turns.
    #[test]
    fn listener_facing_north_hears_an_eastern_source_on_the_right() {
        let (left, right) = impulse_response(std::f32::consts::FRAC_PI_2, Vec3::new(1.0, 0.0, 0.0));
        assert!(interaural_lag(&left, &right, 60) < -25, "left ear must lag");
        assert!(level_difference_db(&right, &left) > 10.0);
    }

    /// Fractional ITD delay by linear interpolation: a delay of 3.25 samples
    /// splits a unit impulse 0.75 / 0.25 over output samples 3 and 4.
    #[test]
    fn itd_delay_reads_a_fractional_delay_by_linear_interpolation() {
        let mut itd = ItdDelay::new();
        itd.delay_samples = 3.25;
        itd.target_delay_samples = 3.25;
        let output: Vec<f32> = (0..8)
            .map(|index| itd.process(if index == 0 { 1.0 } else { 0.0 }))
            .collect();
        let expected = [0.0, 0.0, 0.0, 0.75, 0.25, 0.0, 0.0, 0.0];
        for (index, (actual, wanted)) in output.iter().zip(expected).enumerate() {
            assert!(
                (actual - wanted).abs() < 1e-6,
                "sample {index}: expected {wanted}, got {actual}"
            );
        }
    }

    /// ITD delay smoothing is a one-pole of coefficient 0.01 per sample:
    /// after 100 samples a step from 0 to 10 has covered 1 − 0.99¹⁰⁰ = 63.4 %,
    /// i.e. a time constant of ≈ 100 samples (2.1 ms at 48 kHz).
    #[test]
    fn itd_delay_glides_with_a_hundred_sample_time_constant() {
        let mut itd = ItdDelay::new();
        itd.target_delay_samples = 10.0;
        for _ in 0..100 {
            itd.process(0.0);
        }
        let expected = 10.0 * (1.0 - 0.99f32.powi(100));
        assert!((expected - 6.340).abs() < 1e-3);
        assert!((itd.delay_samples - expected).abs() < 1e-3);
    }

    fn reflection_only(direction: Vec3, distance: f32, gain: f32) -> PathSet {
        let mut paths = PathSet::new();
        paths.push(PathContribution {
            kind: PathKind::Reflection,
            direction,
            distance,
            delay_seconds: 0.0,
            gain,
            wall_index: Some(0),
        });
        paths
    }

    /// A reflection from straight ahead, 2 m long, wall gain 0.6:
    /// gain = 0.6 × (1 m / 2 m) = 0.3 (inverse law), centre pan → each ear
    /// 0.3 × cos 45° = 0.21213. From silence the gain ramps linearly over the
    /// first buffer — sample n is 0.21213 · n / 512 — and the next buffer
    /// continues at exactly the target (one ramp step past the last sample).
    #[test]
    fn reflection_gain_ramps_linearly_to_pan_times_distance_gain() {
        let scene = Scene::new();
        let mut renderer = loaded_renderer(&scene);
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let paths = reflection_only(Vec3::new(1.0, 0.0, 0.0), 2.0, 0.6);
        let mut signal = TestSignal {
            sample_index: 0,
            signal_at: |_| 1.0,
        };
        let target = 0.3 * std::f32::consts::FRAC_1_SQRT_2;
        let mut render = || {
            let (block, _) = scene.render(
                &mut renderer,
                &mut signal,
                &listener,
                Vec3::new(2.0, 0.0, 0.0),
                &paths,
                0.0,
                512,
            );
            split_channels(&block)
        };
        let (left, right) = render();
        for frame in [0usize, 1, 256, 511] {
            let expected = target * frame as f32 / 512.0;
            assert!(
                (left[frame] - expected).abs() < 1e-6,
                "frame {frame}: {}",
                left[frame]
            );
            assert!((right[frame] - expected).abs() < 1e-6);
        }
        let (left, right) = render();
        assert!(left
            .iter()
            .chain(&right)
            .all(|sample| (sample - target).abs() < 1e-6));
    }

    /// A reflection from the listener's left (45°) pans by the lateral
    /// component: pan = 0.5 + 0.5·(−sin 45°) = 0.14645, so the left ear gets
    /// cos(π/2 · 0.14645) = 0.97366 and the right sin(…) = 0.22803.
    #[test]
    fn reflection_from_front_left_pans_by_its_lateral_component() {
        let scene = Scene::new();
        let mut renderer = loaded_renderer(&scene);
        renderer.sources[0].paths[0].prev_gain = 1.0;
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let direction = Vec3::new(1.0, 1.0, 0.0).normalize();
        let paths = reflection_only(direction, 1.0, 1.0);
        let mut signal = TestSignal {
            sample_index: 0,
            signal_at: |_| 1.0,
        };
        let (block, _) = scene.render(
            &mut renderer,
            &mut signal,
            &listener,
            direction,
            &paths,
            0.0,
            64,
        );
        let (left, right) = split_channels(&block);
        assert!((left[10] - 0.97366).abs() < 1e-4, "left {}", left[10]);
        assert!((right[10] - 0.22803).abs() < 1e-4, "right {}", right[10]);
    }

    /// A stationary source must render exactly as its HRIR convolved with the
    /// input: when both A/B slots hold the same IR, the crossfade every
    /// FILTER_UPDATE_INTERVAL buffers must be inaudible.
    ///
    /// It is not. Only the active slot is processed outside a crossfade, so
    /// the slot that becomes active again holds the overlap tail it saved
    /// when it last ran — FILTER_UPDATE_INTERVAL buffers ago — and adds that
    /// stale tail (filter_len − 1 = 557 samples, longer than the 128-sample
    /// crossfade) into the new output. Measured with noise input at 1 m,
    /// direction left: buffers 0–3 match to 1e-7, buffer 4 is off by 1.40
    /// (the signal peaks near 3.0), buffer 8 by 0.63, and so on every 4
    /// buffers — a ~23 Hz periodic glitch at 512 frames / 48 kHz, for every
    /// source, moving or not.
    #[test]
    fn stationary_source_equals_plain_hrir_convolution() {
        let scene = Scene::new();
        let mut renderer = loaded_renderer(&scene);
        // The initial IR is the (0, 1, 0) direction, so a source due left at
        // 1 m needs no direction change and, with the ramp settled, unity gain.
        renderer.sources[0].paths[0].prev_gain = 1.0;
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let source_position = Vec3::new(0.0, 1.0, 0.0);
        let paths = direct_path(&listener, source_position);
        let mut signal = TestSignal {
            sample_index: 0,
            signal_at: noise,
        };

        let sofa = renderer.sofa.as_ref().unwrap();
        let mut filter = Filter::new(sofa.filter_len());
        sofa.filter(0.0, 1.0, 0.0, &mut filter);
        assert_eq!((filter.ldelay, filter.rdelay), (0.0, 0.0));
        let mut reference_left = Convolver::new();
        let mut reference_right = Convolver::new();
        reference_left.init(BLOCK_SIZE, &filter.left);
        reference_right.init(BLOCK_SIZE, &filter.right);

        let frames = 512;
        for buffer in 0..12 {
            let (block, _) = scene.render(
                &mut renderer,
                &mut signal,
                &listener,
                source_position,
                &paths,
                0.0,
                frames,
            );
            let (left, right) = split_channels(&block);
            let mut worst = 0.0f32;
            for start in (0..frames).step_by(BLOCK_SIZE) {
                let input: Vec<f32> = (0..BLOCK_SIZE)
                    .map(|index| noise(buffer * frames + start + index))
                    .collect();
                let mut expected_left = vec![0.0; BLOCK_SIZE];
                let mut expected_right = vec![0.0; BLOCK_SIZE];
                reference_left.process(&input, &mut expected_left);
                reference_right.process(&input, &mut expected_right);
                for index in 0..BLOCK_SIZE {
                    worst = worst
                        .max((left[start + index] - expected_left[index]).abs())
                        .max((right[start + index] - expected_right[index]).abs());
                }
            }
            assert!(worst < 1e-4, "buffer {buffer}: max deviation {worst}");
        }
    }

    /// The A/B crossfade lasts BLOCK_SIZE = 128 samples with weights
    /// (n + 1) / 128. With 64-frame device buffers it spans two calls, and the
    /// second call restarts the ramp at (i + 1) / remaining = 1/64 instead of
    /// continuing at 65/128: the new IR's weight drops from 0.5 to 0.016 in
    /// one sample. Only device buffers < 128 frames hit this.
    #[test]
    fn crossfade_continues_across_buffers_shorter_than_a_block() {
        let scene = Scene::new();
        let mut renderer = loaded_renderer(&scene);
        let filter_length = renderer.sofa.as_ref().unwrap().filter_len();
        let silence = vec![0.0f32; filter_length];
        let mut unit = vec![0.0f32; filter_length];
        unit[0] = 1.0;
        {
            let path = &mut renderer.sources[0].paths[0];
            // Retiring slot 0 is silent, new slot 1 passes the input through,
            // so the output is the new slot's crossfade weight itself.
            path.conv[0].left.set_response(&silence);
            path.conv[0].right.set_response(&silence);
            path.conv[1].left.set_response(&unit);
            path.conv[1].right.set_response(&unit);
            path.active = 1;
            path.xfade_remaining = BLOCK_SIZE;
            path.prev_gain = 1.0;
        }
        // Skip filter updates for the next calls (1 and 2 are not multiples of 4).
        renderer.sources[0].update_counter = 1;

        let listener = Listener::new(Vec3::ZERO, 0.0);
        let source_position = Vec3::new(0.0, 1.0, 0.0);
        let paths = direct_path(&listener, source_position);
        let mut signal = TestSignal {
            sample_index: 0,
            signal_at: |_| 1.0,
        };
        let mut weights = Vec::new();
        for _ in 0..2 {
            let (block, _) = scene.render(
                &mut renderer,
                &mut signal,
                &listener,
                source_position,
                &paths,
                0.0,
                64,
            );
            weights.extend(split_channels(&block).0);
        }
        for (index, weight) in weights.iter().enumerate() {
            let expected = (index + 1) as f32 / BLOCK_SIZE as f32;
            assert!(
                (weight - expected).abs() < 1e-5,
                "sample {index}: weight {weight}, expected {expected}"
            );
        }
    }

    /// The late-reverb send contract shared by the VBAP, DBAP and Ambisonics
    /// renderers (and docs/ray-tracing-upgrade-analysis.md): send = source
    /// signal × ctx.reverb_send, with no distance gain — the reverberant field
    /// is diffuse, its level does not fall with listener distance; the
    /// d²/(d² + d_c²) send already encodes the direct-to-reverberant balance.
    ///
    /// HRTF writes the gain-ramped direct signal instead, which includes the
    /// distance and directivity gains: at 10 m (inverse law, 1 m reference)
    /// the send is 0.1 × 0.8 = 0.08 instead of 0.8 — 20 dB less reverb than
    /// the same scene on speakers, and beyond d_c the D/R ratio stops
    /// changing with distance, removing headphones' main distance cue.
    #[test]
    fn reverb_send_is_source_signal_times_send_level() {
        let scene = Scene::new();
        let mut renderer = loaded_renderer(&scene);
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let source_position = Vec3::new(10.0, 0.0, 0.0);
        let paths = direct_path(&listener, source_position);
        let mut signal = TestSignal {
            sample_index: 0,
            signal_at: |_| 1.0,
        };
        let mut reverb = Vec::new();
        for _ in 0..2 {
            reverb = scene
                .render(
                    &mut renderer,
                    &mut signal,
                    &listener,
                    source_position,
                    &paths,
                    0.8,
                    512,
                )
                .1;
        }
        let send = split_channels(&reverb).0;
        assert!(
            send.iter().all(|sample| (sample - 0.8).abs() < 1e-5),
            "send should be 0.8, got {}",
            send[0]
        );
    }

    /// Reflection pan gains are recomputed once per buffer and applied as a
    /// step, unlike every other gain in the renderers, which ramps. A 10°
    /// head turn moves a frontal reflection from pan 0.5 to
    /// 0.5 + 0.5·sin 10° = 0.5868: the left gain steps from cos 45° = 0.7071
    /// to cos(π/2 · 0.5868) = 0.6044 between two samples — a 0.103 step at
    /// full reflection level, where a ramp over 512 frames would be 0.0002.
    #[test]
    fn reflection_pan_change_is_ramped_not_stepped() {
        let scene = Scene::new();
        let mut renderer = loaded_renderer(&scene);
        renderer.sources[0].paths[0].prev_gain = 1.0;
        let paths = reflection_only(Vec3::new(1.0, 0.0, 0.0), 1.0, 1.0);
        let mut signal = TestSignal {
            sample_index: 0,
            signal_at: |_| 1.0,
        };
        let mut render = |yaw: f32| {
            let listener = Listener::new(Vec3::ZERO, yaw);
            let (block, _) = scene.render(
                &mut renderer,
                &mut signal,
                &listener,
                Vec3::new(1.0, 0.0, 0.0),
                &paths,
                0.0,
                512,
            );
            split_channels(&block).0
        };
        let before = render(0.0);
        let after = render(10f32.to_radians());
        let step = (after[0] - before[511]).abs();
        assert!(
            (before[511] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5,
            "settled centre pan should be cos 45°"
        );
        assert!(step < 0.01, "left gain stepped by {step} between buffers");
    }

    /// A reflection arriving from the listener's right must be louder in the
    /// right channel. This guards against the channel-swap bug where
    /// `gain_left` used `(1 - pan)` and `gain_right` used `pan` — mirroring
    /// every reflection to the wrong side.
    #[test]
    fn pan_right_is_louder_in_right_channel() {
        let (l, r) = equal_power_pan(1.0); // fully right
        assert!(
            l.abs() < 1e-6,
            "full-right pan should silence left, got {l}"
        );
        assert!(
            (r - 1.0).abs() < 1e-6,
            "full-right pan should be unity right, got {r}"
        );

        let (l, r) = equal_power_pan(0.75);
        assert!(r > l, "right-of-center pan must favor the right channel");
    }

    #[test]
    fn pan_left_is_louder_in_left_channel() {
        let (l, r) = equal_power_pan(0.0); // fully left
        assert!((l - 1.0).abs() < 1e-6);
        assert!(r.abs() < 1e-6);
    }

    #[test]
    fn pan_center_is_equal_power() {
        let (l, r) = equal_power_pan(0.5);
        assert!((l - r).abs() < 1e-6, "center pan should be symmetric");
        assert!(
            (l - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6,
            "center pan should be -3 dB per channel"
        );
    }

    /// Energy preservation: L² + R² = 1 across the whole pan range.
    #[test]
    fn pan_preserves_power_everywhere() {
        for i in 0..=20 {
            let pan = i as f32 / 20.0;
            let (l, r) = equal_power_pan(pan);
            let power = l * l + r * r;
            assert!(
                (power - 1.0).abs() < 1e-5,
                "pan={pan}: power={power} should be 1.0"
            );
        }
    }

    /// ITD delay targets must be clamped to the ring buffer capacity —
    /// at 96 kHz a 0.7 ms SOFA delay (67 samples) exceeds the 64-sample ring
    /// and would otherwise wrap around and read future samples.
    #[test]
    fn itd_delay_stays_within_buffer() {
        let mut itd = ItdDelay::new();
        // Simulate the clamped assignment path used in render_source.
        let sr: f32 = 96000.0;
        let sofa_delay_seconds: f32 = 0.0007;
        let max_delay = (DELAY_BUF_SIZE - 2) as f32;
        itd.target_delay_samples = (sofa_delay_seconds * sr).min(max_delay);
        assert!(itd.target_delay_samples <= max_delay);

        // Feed an impulse and run long enough for smoothing to settle; the
        // delayed output must appear (buffer indexing stays valid throughout).
        let mut saw_signal = false;
        for n in 0..1024 {
            let input = if n == 0 { 1.0 } else { 0.0 };
            let out = itd.process(input);
            if out.abs() > 1e-9 {
                saw_signal = true;
            }
        }
        assert!(
            saw_signal,
            "delayed impulse should emerge from the ITD line"
        );
    }
    /// HRIRs are resampled to the rate the device runs at, whatever it is.
    /// The bundled set is 558 taps at 48 kHz, so 1116 at 96 kHz and
    /// 558 · 44.1/48 ≈ 513 at 44.1 kHz. They used to be loaded once at a
    /// guessed 48 kHz and played at the device rate, shifting every spectral
    /// cue by the rate ratio (an octave up at 96 kHz).
    #[test]
    fn hrirs_are_resampled_to_the_device_rate() {
        let scene = Scene::new();
        let mut renderer = HrtfRenderer::new(&sofa_file());
        assert!(
            renderer.sofa.is_none(),
            "nothing is loaded before the device rate is known"
        );

        let mut filter_length_at = |sample_rate: f32| {
            renderer.ensure_topology(1, &scene.layout, sample_rate);
            assert_eq!(renderer.sources.len(), 1);
            renderer
                .sofa
                .as_ref()
                .expect("default.sofa must load")
                .filter_len()
        };
        let at_48_khz = filter_length_at(48_000.0);
        let at_96_khz = filter_length_at(96_000.0);
        let at_44_khz = filter_length_at(44_100.0);

        // The same impulse responses last the same time at every rate.
        let duration = |length: usize, sample_rate: f32| length as f32 / sample_rate;
        let reference = duration(at_48_khz, 48_000.0);
        assert!((duration(at_96_khz, 96_000.0) - reference).abs() < 2.0 / 48_000.0);
        assert!((duration(at_44_khz, 44_100.0) - reference).abs() < 2.0 / 44_100.0);
        assert!(at_96_khz > at_48_khz && at_48_khz > at_44_khz);
    }
}
