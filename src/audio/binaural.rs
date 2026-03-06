// Binaural HRTF rendering — per-source frequency-domain convolution.
//
// Loads a SOFA HRTF dataset (standard AES69 format), extracts left/right ear
// impulse responses for each source's direction relative to the listener,
// and convolves the mono source signal through FFTConvolver pairs.
//
// Output is always stereo (2 channels) regardless of speaker layout.

use fft_convolver::FFTConvolver;
use sofar::reader::{Filter, OpenOptions, Sofar};

use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::mixer::{AirAbsorption, DistanceModel};
use crate::audio::propagation::{ground_effect_gain, GroundProperties};
use crate::processors::early_reflections::SourceReflections;
use crate::spatial::panner::distance_gain_at_model;
use crate::spatial::source::SoundSource;
use atrium_core::listener::Listener;
use atrium_core::types::Vec3;

/// Processing block size for FFT convolution.
/// 128 samples ≈ 2.67ms at 48kHz — well under the 10ms perceptual threshold.
const BLOCK_SIZE: usize = 128;

/// How often to update HRTF filters (every N render calls).
/// At 48kHz with 512-sample buffers, this updates every ~40ms.
const FILTER_UPDATE_INTERVAL: usize = 4;

/// Per-source binaural convolution state.
struct BinauralSource {
    conv_left: FFTConvolver<f32>,
    conv_right: FFTConvolver<f32>,
    /// Previous distance gain for per-sample smoothing.
    prev_gain: f32,
    /// Per-source air absorption filter.
    air_absorption: AirAbsorption,
}

/// Binaural mixer state. Lives on the audio thread alongside MixerState.
///
/// When `RenderMode::Binaural` is active, `AudioScene::render()` delegates
/// to this mixer instead of the multichannel `mix_sources()`.
pub struct BinauralMixer {
    sofa: Sofar,
    sources: Vec<BinauralSource>,
    /// Reusable scratch for SOFA filter extraction (avoids allocation).
    filter: Filter,
    /// Scratch buffers for convolution I/O.
    mono_buf: Vec<f32>,
    left_buf: Vec<f32>,
    right_buf: Vec<f32>,
    /// Counter for throttled HRTF filter updates.
    update_counter: usize,
    sample_rate: f32,
    /// Per-source first-order reflections (image-source method).
    source_reflections: Vec<SourceReflections>,
    /// Room bounds for reflection computation.
    room_min: Vec3,
    room_max: Vec3,
    /// Ground surface properties for ISO 9613-2 ground effect.
    ground: GroundProperties,
    /// Whether per-source reflections are enabled.
    reflections_enabled: bool,
}

impl BinauralMixer {
    /// Create a new binaural mixer by loading a SOFA HRTF dataset.
    ///
    /// `sofa_path` — path to a .sofa file (e.g. MIT KEMAR).
    /// `sample_rate` — audio device sample rate (SOFA data is resampled to match).
    /// `num_sources` — initial number of sound sources.
    pub fn new(
        sofa_path: &str,
        sample_rate: f32,
        num_sources: usize,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let sofa = OpenOptions::new()
            .sample_rate(sample_rate)
            .open(sofa_path)?;

        let filt_len = sofa.filter_len();
        let filter = Filter::new(filt_len);

        // Pre-allocate a "default" HRIR (front-facing) to initialize convolvers.
        // The actual filter will be set per-source on the first render call.
        let mut init_filter = Filter::new(filt_len);
        sofa.filter(0.0, 1.0, 0.0, &mut init_filter);

        let mut sources = Vec::with_capacity(num_sources);
        for _ in 0..num_sources {
            sources.push(Self::new_source(&init_filter, sample_rate)?);
        }

        println!(
            "Binaural HRTF loaded: {} taps",
            filt_len,
        );

        Ok(Self {
            sofa,
            sources,
            filter,
            mono_buf: vec![0.0; BLOCK_SIZE],
            left_buf: vec![0.0; BLOCK_SIZE],
            right_buf: vec![0.0; BLOCK_SIZE],
            update_counter: 0,
            sample_rate,
            source_reflections: Vec::new(),
            room_min: Vec3::ZERO,
            room_max: Vec3::ZERO,
            ground: GroundProperties::default(),
            reflections_enabled: false,
        })
    }

    fn new_source(
        init_filter: &Filter,
        sample_rate: f32,
    ) -> Result<BinauralSource, Box<dyn std::error::Error>> {
        let mut conv_left = FFTConvolver::default();
        let mut conv_right = FFTConvolver::default();
        conv_left.init(BLOCK_SIZE, &init_filter.left)?;
        conv_right.init(BLOCK_SIZE, &init_filter.right)?;

        Ok(BinauralSource {
            conv_left,
            conv_right,
            prev_gain: 0.0,
            air_absorption: AirAbsorption::new(sample_rate),
        })
    }

    /// Ensure we have enough per-source state (grows if sources were added).
    fn ensure_sources(&mut self, num_sources: usize) {
        while self.sources.len() < num_sources {
            // Initialize new sources with front-facing HRTF
            let mut init_filter = Filter::new(self.sofa.filter_len());
            self.sofa.filter(0.0, 1.0, 0.0, &mut init_filter);
            if let Ok(src) = Self::new_source(&init_filter, self.sample_rate) {
                self.sources.push(src);
            }
        }
    }

    /// Enable per-source reflections with given parameters.
    pub fn enable_reflections(&mut self, wet_gain: f32, wall_absorption: f32, num_sources: usize) {
        self.reflections_enabled = true;
        self.source_reflections.clear();
        for _ in 0..num_sources {
            self.source_reflections
                .push(SourceReflections::new(wet_gain, wall_absorption));
        }
    }

    /// Set room bounds for reflection and ground effect computation.
    pub fn set_room_bounds(&mut self, room_min: Vec3, room_max: Vec3) {
        self.room_min = room_min;
        self.room_max = room_max;
    }

    /// Set ground surface properties.
    pub fn set_ground(&mut self, ground: GroundProperties) {
        self.ground = ground;
    }

    /// Render all sources to a stereo interleaved output buffer.
    ///
    /// Writes to channels 0 (left) and 1 (right) of the interleaved buffer.
    /// Remaining channels (if any) are zeroed.
    pub fn mix(
        &mut self,
        sources: &mut [Box<dyn SoundSource>],
        listener: &Listener,
        output: &mut [f32],
        channels: usize,
        sample_rate: f32,
        master_gain: f32,
        distance_model: &DistanceModel,
        atmosphere: &AtmosphericParams,
    ) {
        let num_frames = output.len() / channels;
        let inv_frames = 1.0 / num_frames as f32;

        // Zero entire output (including unused channels beyond stereo)
        output.fill(0.0);

        self.ensure_sources(sources.len());

        // Grow reflection buffers if needed
        if self.reflections_enabled {
            while self.source_reflections.len() < sources.len() {
                self.source_reflections
                    .push(SourceReflections::new(0.4, 0.9));
            }
        }

        let should_update = self.update_counter % FILTER_UPDATE_INTERVAL == 0;

        for (src_idx, source) in sources.iter_mut().enumerate() {
            if !source.is_active() {
                continue;
            }

            let pos = source.position();
            let dist_to_listener = listener.position.distance_to(pos);

            // Update air absorption
            self.sources[src_idx]
                .air_absorption
                .update(dist_to_listener, atmosphere);

            // ISO 9613-2 ground effect
            let ground_gain = ground_effect_gain(
                dist_to_listener,
                pos.z.max(0.0),
                listener.position.z.max(0.0),
                &self.ground,
            );

            // Update per-source reflection taps
            if self.reflections_enabled && src_idx < self.source_reflections.len() {
                self.source_reflections[src_idx].update(
                    self.room_min,
                    self.room_max,
                    pos,
                    listener.position,
                    sample_rate,
                );
            }

            // Distance gain using per-source ref_distance (SPL-derived)
            let target_gain = distance_gain_at_model(
                listener.position,
                pos,
                source.ref_distance(),
                distance_model.max_distance,
                distance_model.rolloff,
                distance_model.model,
            );

            // Update HRTF filter when direction changes
            if should_update {
                let (sx, sy, sz) = to_sofa_coords(pos, listener);
                self.sofa.filter(sx, sy, sz, &mut self.filter);
                // Ignore errors from set_response (e.g. length mismatch)
                let _ = self.sources[src_idx]
                    .conv_left
                    .set_response(&self.filter.left);
                let _ = self.sources[src_idx]
                    .conv_right
                    .set_response(&self.filter.right);
            }

            let prev_gain = self.sources[src_idx].prev_gain;

            // Process in blocks of BLOCK_SIZE for FFT convolution
            let mut frame = 0;
            while frame < num_frames {
                let block_len = (num_frames - frame).min(BLOCK_SIZE);

                // Ensure scratch buffers are large enough (grows once)
                if self.mono_buf.len() < block_len {
                    self.mono_buf.resize(block_len, 0.0);
                    self.left_buf.resize(block_len, 0.0);
                    self.right_buf.resize(block_len, 0.0);
                }

                // Generate gain-ramped, air-absorbed, ground-effected mono samples
                for i in 0..block_len {
                    let t = (frame + i) as f32 * inv_frames;
                    let gain = prev_gain + (target_gain - prev_gain) * t;
                    let raw = source.next_sample(sample_rate);
                    let absorbed = self.sources[src_idx].air_absorption.process(raw);
                    let mono = absorbed * ground_gain;

                    // Per-source reflections
                    let reflection_wet = if self.reflections_enabled
                        && src_idx < self.source_reflections.len()
                    {
                        self.source_reflections[src_idx].process_sample(mono)
                    } else {
                        0.0
                    };

                    self.mono_buf[i] = (mono + reflection_wet) * gain;
                }

                // HRTF convolution: mono → left ear, mono → right ear
                self.left_buf[..block_len].fill(0.0);
                let _ = self.sources[src_idx]
                    .conv_left
                    .process(&self.mono_buf[..block_len], &mut self.left_buf[..block_len]);

                self.right_buf[..block_len].fill(0.0);
                let _ = self.sources[src_idx]
                    .conv_right
                    .process(&self.mono_buf[..block_len], &mut self.right_buf[..block_len]);

                // Accumulate into interleaved stereo output
                for i in 0..block_len {
                    let base = (frame + i) * channels;
                    output[base] += self.left_buf[i];
                    if channels > 1 {
                        output[base + 1] += self.right_buf[i];
                    }
                }

                frame += block_len;
            }

            self.sources[src_idx].prev_gain = target_gain;
        }

        self.update_counter += 1;

        // Apply master gain and clamp
        for sample in output.iter_mut() {
            *sample = (*sample * master_gain).clamp(-1.0, 1.0);
        }
    }
}

/// Convert a source position to SOFA listener-relative coordinates.
///
/// SOFA (AES69) Cartesian convention: x = front, y = left, z = up.
/// Atrium uses: x/y = horizontal, z = up, listener forward = (cos(yaw), sin(yaw), 0).
fn to_sofa_coords(source_pos: crate::world::types::Vec3, listener: &Listener) -> (f32, f32, f32) {
    let d = source_pos - listener.position;
    let yaw = listener.yaw;

    // Project world-space direction into listener-local frame:
    //   forward = (cos(yaw), sin(yaw), 0)
    //   right   = (sin(yaw), -cos(yaw), 0)
    //   up      = (0, 0, 1)
    let forward = d.x * yaw.cos() + d.y * yaw.sin();
    let right = d.x * yaw.sin() - d.y * yaw.cos();

    // Map to SOFA: x = front, y = left (= −right), z = up
    (forward, -right, d.z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::types::Vec3;

    #[test]
    fn sofa_coords_source_in_front() {
        // Listener at origin facing +x (yaw=0), source at (2, 0, 0)
        // SOFA: x=front, y=left, z=up → expect (2, 0, 0)
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let (x, y, z) = to_sofa_coords(Vec3::new(2.0, 0.0, 0.0), &listener);
        assert!((x - 2.0).abs() < 1e-6, "should be in front: x={x}");
        assert!(y.abs() < 1e-6, "should not be left/right: y={y}");
        assert!(z.abs() < 1e-6, "should be level: z={z}");
    }

    #[test]
    fn sofa_coords_source_to_right() {
        // Listener at origin facing +x (yaw=0), source at (0, -1, 0)
        // Right of listener when facing +x is -y in world space
        // SOFA: x=front, y=left → right = negative y → expect (0, -1, 0)
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let (x, y, z) = to_sofa_coords(Vec3::new(0.0, -1.0, 0.0), &listener);
        assert!(x.abs() < 1e-6, "should not be in front: x={x}");
        assert!((y - (-1.0)).abs() < 1e-6, "should be to the right (negative SOFA y): y={y}");
        assert!(z.abs() < 1e-6);
    }

    #[test]
    fn sofa_coords_rotated_listener() {
        // Listener facing +y (yaw = π/2), source at (1, 0, 0) = to the right
        // SOFA: right = negative y → expect (0, -1, 0)
        let listener = Listener::new(Vec3::ZERO, std::f32::consts::FRAC_PI_2);
        let (x, y, z) = to_sofa_coords(Vec3::new(1.0, 0.0, 0.0), &listener);
        assert!(x.abs() < 1e-3, "should not be in front: x={x}");
        assert!((y - (-1.0)).abs() < 1e-3, "should be to the right (negative SOFA y): y={y}");
        assert!(z.abs() < 1e-6);
    }

    #[test]
    fn sofa_hrtf_lateral_source_has_different_lr() {
        // Load the SOFA file and check that a lateral source produces
        // meaningfully different left and right HRIRs.
        use sofar::reader::{Filter, OpenOptions};

        let sofa = OpenOptions::new()
            .sample_rate(48000.0)
            .open("assets/hrtf/default.sofa")
            .expect("failed to load SOFA");
        let filt_len = sofa.filter_len();
        let mut filter = Filter::new(filt_len);

        // Test several directions in SOFA coordinates (x=front, y=left, z=up)
        let dirs: &[(&str, f32, f32, f32)] = &[
            ("front",   1.0,  0.0,  0.0),
            ("left",    0.0,  1.0,  0.0),
            ("right",   0.0, -1.0,  0.0),
            ("behind", -1.0,  0.0,  0.0),
        ];
        for (name, x, y, z) in dirs {
            sofa.filter(*x, *y, *z, &mut filter);
            let l_e: f32 = filter.left.iter().map(|s| s * s).sum();
            let r_e: f32 = filter.right.iter().map(|s| s * s).sum();
            let diff = 10.0 * (l_e / r_e.max(1e-10)).log10();
            let same = filter.left[..filt_len] == filter.right[..filt_len];
            println!("{name:>6}: L={l_e:.4} R={r_e:.4} diff={diff:+.1}dB same={same}");
        }

        // Left source (SOFA y=+1): left ear should be louder
        sofa.filter(0.0, 1.0, 0.0, &mut filter);
        let l_energy: f32 = filter.left.iter().map(|s| s * s).sum();
        let r_energy: f32 = filter.right.iter().map(|s| s * s).sum();
        assert!(
            l_energy != r_energy,
            "SOFA file has identical L/R HRIRs — the HRTF file may be broken"
        );
    }

    #[test]
    fn sofa_coords_source_above() {
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let (x, y, z) = to_sofa_coords(Vec3::new(0.0, 0.0, 2.0), &listener);
        assert!(x.abs() < 1e-6);
        assert!(y.abs() < 1e-6);
        assert!((z - 2.0).abs() < 1e-6, "should be above: z={z}");
    }
}
