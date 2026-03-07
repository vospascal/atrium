// Multichannel speaker layout and rendering.
//
// Five render modes (RenderMode):
//   1. WorldLocked — each speaker is a virtual microphone at a fixed position in the room.
//      Gain = distance_attenuation(source → speaker) × source_directivity(source → speaker).
//      The listener's position does NOT affect speaker gains (spatial image comes from
//      physical speaker placement). Valid layouts: stereo, quad, 5.1.
//   2. Vbap — 2D Vector Base Amplitude Panning (Pulkki 1997). Speakers form a ring around
//      the listener. Source direction from listener determines which speaker pair activates.
//      Requires 3+ speakers. Valid layouts: quad, 5.1.
//   3. Hrtf — HRTF convolution for headphone rendering. Always stereo output.
//   4. Dbap — Distance-Based Amplitude Panning (Lossius 2009, improved 2021). All speakers
//      active, gains weighted by inverse distance. Listener-independent. Valid layouts:
//      stereo, quad, 5.1.
//   5. Ambisonics — First-Order Ambisonics (FOA). Encodes sources into B-format (W, Y, Z, X),
//      then decodes to any speaker layout via mode-matching pseudo-inverse decoder.
//      Listener-relative. All speakers active. Valid layouts: stereo, quad, 5.1.
//
// Speaker positions are configurable per room. Layouts: stereo, quad 4.0, surround 5.1.

use crate::directivity::{directivity_gain, DirectivityPattern};
use crate::listener::Listener;
use crate::panner::{distance_gain_at_model, DistanceModelType};
use crate::types::Vec3;

/// Maximum output channels supported. Covers stereo, 5.1, and 7.1.
pub const MAX_CHANNELS: usize = 8;

/// A physical speaker with a position in the virtual atrium.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Speaker {
    /// World position in the atrium (meters).
    pub position: Vec3,
    /// Output channel index in the interleaved buffer.
    pub channel: usize,
}

/// Per-channel gain array. Stack-allocated for real-time safety.
#[derive(Clone, Copy, Debug)]
pub struct ChannelGains {
    pub gains: [f32; MAX_CHANNELS],
    pub count: usize,
}

impl ChannelGains {
    pub fn silent(count: usize) -> Self {
        Self {
            gains: [0.0; MAX_CHANNELS],
            count,
        }
    }
}

/// Distance model parameters for attenuation.
#[derive(Clone, Copy, Debug)]
pub struct DistanceParams {
    pub ref_distance: f32,
    pub max_distance: f32,
    pub rolloff: f32,
    pub model: DistanceModelType,
}

impl Default for DistanceParams {
    fn default() -> Self {
        Self {
            ref_distance: 0.3,
            max_distance: 20.0,
            rolloff: 1.0,
            model: DistanceModelType::Inverse,
        }
    }
}

/// Source spatial properties for gain computation.
#[derive(Clone, Copy, Debug)]
pub struct SourceSpatial<'a> {
    pub position: Vec3,
    pub orientation: Vec3,
    pub directivity: &'a DirectivityPattern,
}

/// Output channel configuration. Determines which speaker channels are active.
/// Works by masking channels on the underlying hardware layout (always 5.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChannelMode {
    /// Stereo: channels 0 (L), 1 (R) only.
    Stereo,
    /// Quad 4.0: channels 0 (FL), 1 (FR), 4 (RL), 5 (RR). Center + LFE masked.
    Quad,
    /// Full 5.1 surround: channels 0 (L), 1 (R), 2 (C), 4 (Ls), 5 (Rs). LFE (3) unused.
    Surround51,
}

impl ChannelMode {
    /// Active spatial channel indices for this mode.
    pub fn active_channels(self) -> &'static [usize] {
        match self {
            ChannelMode::Stereo => &[0, 1],
            ChannelMode::Quad => &[0, 1, 4, 5],
            ChannelMode::Surround51 => &[0, 1, 2, 4, 5],
        }
    }

    /// Valid channel modes for a given render mode.
    pub fn valid_for(mode: RenderMode) -> &'static [ChannelMode] {
        match mode {
            RenderMode::WorldLocked | RenderMode::Dbap | RenderMode::Ambisonics => &[
                ChannelMode::Stereo,
                ChannelMode::Quad,
                ChannelMode::Surround51,
            ],
            RenderMode::Vbap => &[ChannelMode::Quad, ChannelMode::Surround51],
            RenderMode::Hrtf => &[ChannelMode::Stereo],
        }
    }

    /// Wire format name for JSON serialization.
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelMode::Stereo => "stereo",
            ChannelMode::Quad => "quad",
            ChannelMode::Surround51 => "5.1",
        }
    }

    /// Parse from wire format string.
    pub fn parse(s: &str) -> Option<ChannelMode> {
        match s {
            "stereo" => Some(ChannelMode::Stereo),
            "quad" => Some(ChannelMode::Quad),
            "5.1" => Some(ChannelMode::Surround51),
            _ => None,
        }
    }
}

/// Rendering algorithm.
///
/// Each mode is a distinct spatialization strategy. The output channel count
/// is determined by the SpeakerLayout, not the render mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RenderMode {
    /// Speakers are virtual microphones at fixed world positions.
    /// Propagation is per-speaker (source→speaker path). Listener irrelevant.
    /// Valid layouts: stereo, quad, 5.1.
    WorldLocked,
    /// VBAP: listener-relative triangulated panning to physical speakers.
    /// Requires 3+ speakers for triangulation.
    /// Valid layouts: quad, 5.1.
    Vbap,
    /// HRTF convolution for headphones.
    /// Always outputs stereo (channels 0, 1).
    Hrtf,
    /// DBAP: distance-based amplitude panning. All speakers active, gains
    /// weighted by inverse distance from source. Listener-independent.
    /// Valid layouts: stereo, quad, 5.1.
    Dbap,
    /// FOA: first-order ambisonics. Encodes to B-format, decodes via
    /// mode-matching pseudo-inverse. Listener-relative, all speakers active.
    /// Valid layouts: stereo, quad, 5.1.
    Ambisonics,
}

impl RenderMode {
    /// Index into the pre-allocated pipeline array.
    pub fn index(self) -> usize {
        match self {
            RenderMode::WorldLocked => 0,
            RenderMode::Vbap => 1,
            RenderMode::Hrtf => 2,
            RenderMode::Dbap => 3,
            RenderMode::Ambisonics => 4,
        }
    }

    pub const ALL: [RenderMode; 5] = [
        RenderMode::WorldLocked,
        RenderMode::Vbap,
        RenderMode::Hrtf,
        RenderMode::Dbap,
        RenderMode::Ambisonics,
    ];

    /// Wire format name for JSON serialization.
    pub fn as_str(self) -> &'static str {
        match self {
            RenderMode::WorldLocked => "world_locked",
            RenderMode::Vbap => "vbap",
            RenderMode::Hrtf => "hrtf",
            RenderMode::Dbap => "dbap",
            RenderMode::Ambisonics => "ambisonics",
        }
    }
}

/// Multichannel speaker configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct SpeakerLayout {
    /// Stereo speakers (excludes LFE).
    speakers: [Speaker; MAX_CHANNELS],
    /// Number of spatial speakers.
    count: usize,
    /// LFE channel index, if present. LFE has no spatial position.
    lfe_channel: Option<usize>,
    /// Total output channels (spatial speakers + LFE if present).
    total_channels: usize,
    /// Speaker mask: which channels are active. Inactive channels produce silence.
    /// Used to run e.g. quad on 5.1 hardware by masking center+LFE.
    /// Default: all channels active.
    pub active: [bool; MAX_CHANNELS],
}

impl SpeakerLayout {
    /// Create a custom speaker layout.
    ///
    /// `speakers` defines the spatial speakers (position + channel index).
    /// `lfe_channel` optionally designates a channel for low-frequency effects.
    /// `total_channels` is the total output channel count (must be ≥ max channel index + 1).
    pub fn new(speakers: &[Speaker], lfe_channel: Option<usize>, total_channels: usize) -> Self {
        let mut arr = [Speaker {
            position: Vec3::ZERO,
            channel: 0,
        }; MAX_CHANNELS];
        let count = speakers.len().min(MAX_CHANNELS);
        arr[..count].copy_from_slice(&speakers[..count]);
        // Validate channel indices are within bounds
        for (i, speaker) in arr.iter().enumerate().take(count) {
            assert!(
                speaker.channel < MAX_CHANNELS,
                "speaker {} channel {} exceeds MAX_CHANNELS ({})",
                i,
                speaker.channel,
                MAX_CHANNELS,
            );
        }
        if let Some(lfe) = lfe_channel {
            assert!(
                lfe < MAX_CHANNELS,
                "LFE channel {} exceeds MAX_CHANNELS ({})",
                lfe,
                MAX_CHANNELS,
            );
        }
        Self {
            speakers: arr,
            count,
            lfe_channel,
            total_channels,
            active: [true; MAX_CHANNELS],
        }
    }

    /// Standard stereo layout.
    pub fn stereo(left_pos: Vec3, right_pos: Vec3) -> Self {
        Self::new(
            &[
                Speaker {
                    position: left_pos,
                    channel: 0,
                },
                Speaker {
                    position: right_pos,
                    channel: 1,
                },
            ],
            None,
            2,
        )
    }

    /// Quad 4.0 layout: front-left, front-right, rear-left, rear-right.
    pub fn quad(fl: Vec3, fr: Vec3, rl: Vec3, rr: Vec3) -> Self {
        Self::new(
            &[
                Speaker {
                    position: fl,
                    channel: 0,
                },
                Speaker {
                    position: fr,
                    channel: 1,
                },
                Speaker {
                    position: rl,
                    channel: 2,
                },
                Speaker {
                    position: rr,
                    channel: 3,
                },
            ],
            None,
            4,
        )
    }

    /// ITU 5.1 surround: L(0), R(1), C(2), LFE(3), Ls(4), Rs(5).
    pub fn surround_5_1(l: Vec3, r: Vec3, c: Vec3, ls: Vec3, rs: Vec3) -> Self {
        Self::new(
            &[
                Speaker {
                    position: l,
                    channel: 0,
                },
                Speaker {
                    position: r,
                    channel: 1,
                },
                Speaker {
                    position: c,
                    channel: 2,
                },
                Speaker {
                    position: ls,
                    channel: 4,
                },
                Speaker {
                    position: rs,
                    channel: 5,
                },
            ],
            Some(3),
            6,
        )
    }

    /// Total output channels (including LFE).
    pub fn total_channels(&self) -> usize {
        self.total_channels
    }

    /// Number of spatial speakers (excludes LFE).
    pub fn speaker_count(&self) -> usize {
        self.count
    }

    /// Spatial speakers as a slice (excludes LFE).
    pub fn speakers(&self) -> &[Speaker] {
        &self.speakers[..self.count]
    }

    /// LFE channel index, if this layout has one.
    pub fn lfe_channel(&self) -> Option<usize> {
        self.lfe_channel
    }

    /// Get a reference to a speaker by its array index (0..speaker_count).
    pub fn speaker_by_index(&self, index: usize) -> Option<&Speaker> {
        if index < self.count {
            Some(&self.speakers[index])
        } else {
            None
        }
    }

    /// Get a mutable reference to a speaker by channel index.
    pub fn speaker_by_channel_mut(&mut self, channel: usize) -> Option<&mut Speaker> {
        self.speakers[..self.count]
            .iter_mut()
            .find(|s| s.channel == channel)
    }

    /// Check if a channel is active (not masked).
    pub fn is_channel_active(&self, channel: usize) -> bool {
        channel < MAX_CHANNELS && self.active[channel]
    }

    /// Set which channels are active. Channels not in the list are masked.
    pub fn set_active_channels(&mut self, channels: &[usize]) {
        self.active = [false; MAX_CHANNELS];
        for &ch in channels {
            if ch < MAX_CHANNELS {
                self.active[ch] = true;
            }
        }
    }

    /// Apply the speaker mask to a gain array: zero out inactive channels.
    pub fn apply_mask(&self, gains: &mut ChannelGains) {
        for ch in 0..gains.count {
            if !self.active[ch] {
                gains.gains[ch] = 0.0;
            }
        }
    }

    // ── VBAP mode ──────────────────────────────────────────────────────

    /// Compute per-channel gains using 2D VBAP.
    ///
    /// Finds the speaker pair straddling the source direction from the listener,
    /// then computes normalized gains. Distance from listener to source is applied
    /// as overall attenuation. Source directivity is applied along listener direction.
    pub fn compute_gains_vbap(
        &self,
        listener: &Listener,
        source: &SourceSpatial,
        distance: &DistanceParams,
    ) -> ChannelGains {
        let source_pos = source.position;
        let source_orientation = source.orientation;
        let source_directivity = source.directivity;
        let mut gains = ChannelGains::silent(self.total_channels);

        if self.count == 0 {
            return gains;
        }

        // Source azimuth in listener's local frame
        let d = source_pos - listener.position;
        let source_azimuth = if d.x * d.x + d.y * d.y < 1e-10 {
            0.0 // source at listener → treat as ahead
        } else {
            let cos_y = listener.yaw.cos();
            let sin_y = listener.yaw.sin();
            let local_x = d.x * cos_y + d.y * sin_y; // forward
            let local_y = -d.x * sin_y + d.y * cos_y; // left
            local_y.atan2(local_x)
        };

        // Compute speaker azimuths relative to listener and sort by angle
        let mut speaker_angles: [(f32, usize); MAX_CHANNELS] = [(0.0, 0); MAX_CHANNELS];
        for (i, speaker) in self.speakers.iter().enumerate().take(self.count) {
            let sp = speaker.position - listener.position;
            let cos_y = listener.yaw.cos();
            let sin_y = listener.yaw.sin();
            let local_x = sp.x * cos_y + sp.y * sin_y;
            let local_y = -sp.x * sin_y + sp.y * cos_y;
            speaker_angles[i] = (local_y.atan2(local_x), i);
        }
        // Sort by angle (insertion sort, N ≤ 8)
        for i in 1..self.count {
            let key = speaker_angles[i];
            let mut j = i;
            while j > 0 && speaker_angles[j - 1].0 > key.0 {
                speaker_angles[j] = speaker_angles[j - 1];
                j -= 1;
            }
            speaker_angles[j] = key;
        }

        // Find the speaker pair that straddles the source azimuth
        let mut best_a = 0usize;
        let mut best_b = 0usize;
        let mut best_ga = 0.0f32;
        let mut best_gb = 0.0f32;
        let mut found = false;

        for pair_idx in 0..self.count {
            let idx_a = pair_idx;
            let idx_b = (pair_idx + 1) % self.count;
            let (angle_a, si_a) = speaker_angles[idx_a];
            let (angle_b, si_b) = speaker_angles[idx_b];

            // Speaker direction unit vectors
            let (ax, ay) = (angle_a.cos(), angle_a.sin());
            let (bx, by) = (angle_b.cos(), angle_b.sin());

            // 2x2 inverse of [a, b] matrix
            let det = ax * by - bx * ay;
            if det.abs() < 1e-8 {
                continue; // collinear speakers
            }
            let inv_det = 1.0 / det;

            // Source direction unit vector
            let (sx, sy) = (source_azimuth.cos(), source_azimuth.sin());

            // g = inv([a,b]) * s
            let ga = (by * sx - bx * sy) * inv_det;
            let gb = (-ay * sx + ax * sy) * inv_det;

            if ga >= -1e-6 && gb >= -1e-6 {
                found = true;
                best_a = si_a;
                best_b = si_b;
                best_ga = ga.max(0.0);
                best_gb = gb.max(0.0);
                break;
            }
        }

        if !found {
            // Fallback: assign to nearest speaker
            let mut min_diff = f32::MAX;
            for &(angle, idx) in speaker_angles.iter().take(self.count) {
                let diff = angle_diff(source_azimuth, angle).abs();
                if diff < min_diff {
                    min_diff = diff;
                    best_a = idx;
                    best_ga = 1.0;
                    best_gb = 0.0;
                }
            }
            best_b = best_a; // single speaker
        }

        // Normalize for constant power: g /= sqrt(ga² + gb²)
        let norm = (best_ga * best_ga + best_gb * best_gb).sqrt();
        if norm > 1e-8 {
            best_ga /= norm;
            best_gb /= norm;
        }

        // Per-speaker distance compensation: attenuate closer speakers so all
        // deliver equal SPL at the listener. Uses farthest speaker as reference
        // (closer speakers get scaled down since they naturally arrive louder).
        let d_a = (self.speakers[best_a].position - listener.position)
            .length()
            .max(0.1);
        let d_b = if best_b != best_a {
            (self.speakers[best_b].position - listener.position)
                .length()
                .max(0.1)
        } else {
            d_a
        };
        let d_ref = d_a.max(d_b);
        best_ga *= d_a / d_ref;
        if best_b != best_a {
            best_gb *= d_b / d_ref;
        }
        // Re-normalize to maintain constant power after compensation
        let norm2 = (best_ga * best_ga + best_gb * best_gb).sqrt();
        if norm2 > 1e-8 {
            best_ga /= norm2;
            best_gb /= norm2;
        }

        // Distance attenuation from listener to source
        let dist = distance_gain_at_model(
            listener.position,
            source_pos,
            distance.ref_distance,
            distance.max_distance,
            distance.rolloff,
            distance.model,
        );

        // Source directivity toward listener
        let dir = directivity_gain(
            source_pos,
            source_orientation,
            listener.position,
            source_directivity,
        );

        let hearing = listener.hearing_gain(source_pos);
        let attenuation = dist * dir * hearing;
        gains.gains[self.speakers[best_a].channel] += best_ga * attenuation;
        if best_b != best_a {
            gains.gains[self.speakers[best_b].channel] += best_gb * attenuation;
        }

        gains
    }

    /// Compute angular VBAP panning gains only (no distance attenuation,
    /// no directivity, no hearing gain). Used by per-path rendering where
    /// each path carries its own gain factor.
    pub fn compute_vbap_panning(&self, listener: &Listener, direction: Vec3) -> ChannelGains {
        let mut gains = ChannelGains::silent(self.total_channels);

        if self.count == 0 {
            return gains;
        }

        // Source azimuth in listener's local frame from direction vector
        let source_azimuth = if direction.x * direction.x + direction.y * direction.y < 1e-10 {
            0.0
        } else {
            let cos_y = listener.yaw.cos();
            let sin_y = listener.yaw.sin();
            let local_x = direction.x * cos_y + direction.y * sin_y;
            let local_y = -direction.x * sin_y + direction.y * cos_y;
            local_y.atan2(local_x)
        };

        // Compute speaker azimuths relative to listener and sort by angle
        let mut speaker_angles: [(f32, usize); MAX_CHANNELS] = [(0.0, 0); MAX_CHANNELS];
        for (i, speaker) in self.speakers.iter().enumerate().take(self.count) {
            let sp = speaker.position - listener.position;
            let cos_y = listener.yaw.cos();
            let sin_y = listener.yaw.sin();
            let local_x = sp.x * cos_y + sp.y * sin_y;
            let local_y = -sp.x * sin_y + sp.y * cos_y;
            speaker_angles[i] = (local_y.atan2(local_x), i);
        }
        for i in 1..self.count {
            let key = speaker_angles[i];
            let mut j = i;
            while j > 0 && speaker_angles[j - 1].0 > key.0 {
                speaker_angles[j] = speaker_angles[j - 1];
                j -= 1;
            }
            speaker_angles[j] = key;
        }

        // Find the speaker pair that straddles the source azimuth
        let mut best_a = 0usize;
        let mut best_b = 0usize;
        let mut best_ga = 0.0f32;
        let mut best_gb = 0.0f32;
        let mut found = false;

        for pair_idx in 0..self.count {
            let idx_a = pair_idx;
            let idx_b = (pair_idx + 1) % self.count;
            let (angle_a, si_a) = speaker_angles[idx_a];
            let (angle_b, si_b) = speaker_angles[idx_b];

            let (ax, ay) = (angle_a.cos(), angle_a.sin());
            let (bx, by) = (angle_b.cos(), angle_b.sin());

            let det = ax * by - bx * ay;
            if det.abs() < 1e-8 {
                continue;
            }
            let inv_det = 1.0 / det;

            let (sx, sy) = (source_azimuth.cos(), source_azimuth.sin());

            let ga = (by * sx - bx * sy) * inv_det;
            let gb = (-ay * sx + ax * sy) * inv_det;

            if ga >= -1e-6 && gb >= -1e-6 {
                found = true;
                best_a = si_a;
                best_b = si_b;
                best_ga = ga.max(0.0);
                best_gb = gb.max(0.0);
                break;
            }
        }

        if !found {
            let mut min_diff = f32::MAX;
            for &(angle, idx) in speaker_angles.iter().take(self.count) {
                let diff = angle_diff(source_azimuth, angle).abs();
                if diff < min_diff {
                    min_diff = diff;
                    best_a = idx;
                    best_ga = 1.0;
                    best_gb = 0.0;
                }
            }
            best_b = best_a;
        }

        // Normalize for constant power
        let norm = (best_ga * best_ga + best_gb * best_gb).sqrt();
        if norm > 1e-8 {
            best_ga /= norm;
            best_gb /= norm;
        }

        // Per-speaker distance compensation (same as compute_gains_vbap)
        let d_a = (self.speakers[best_a].position - listener.position)
            .length()
            .max(0.1);
        let d_b = if best_b != best_a {
            (self.speakers[best_b].position - listener.position)
                .length()
                .max(0.1)
        } else {
            d_a
        };
        let d_ref = d_a.max(d_b);
        best_ga *= d_a / d_ref;
        if best_b != best_a {
            best_gb *= d_b / d_ref;
        }
        let norm2 = (best_ga * best_ga + best_gb * best_gb).sqrt();
        if norm2 > 1e-8 {
            best_ga /= norm2;
            best_gb /= norm2;
        }

        gains.gains[self.speakers[best_a].channel] += best_ga;
        if best_b != best_a {
            gains.gains[self.speakers[best_b].channel] += best_gb;
        }

        gains
    }

    /// Extended VBAP panning with stereo polarity inversion (Gjørup et al.).
    ///
    /// When a source falls outside the active speaker pair, standard VBAP
    /// snaps to the nearest speaker. Extended VBAP instead allows one gain
    /// to go negative (polarity inversion), creating a phantom source beyond
    /// the physical speaker span. Energy normalization `g/sqrt(g1²+g2²)` is
    /// preserved.
    ///
    /// **Stereo only.** With 3+ speakers forming a ring, sources are always
    /// inside some pair's span — extension never triggers. For 2 speakers,
    /// the extension is clamped to `max_extension` of the pair's angular span
    /// (paper validates up to 0.4, i.e., 40% beyond speaker span).
    ///
    /// # Parameters
    /// - `max_extension`: fraction of pair span to allow beyond speakers.
    ///   0.0 = standard VBAP, 0.4 = 40% extension (paper default).
    ///   Clamped to [0.0, 0.6] per paper's maximum evidence.
    pub fn compute_vbap_panning_extended(
        &self,
        listener: &Listener,
        direction: Vec3,
        max_extension: f32,
    ) -> ChannelGains {
        // Only meaningful for exactly 2 speakers (stereo).
        // For 3+ speakers, delegate to standard VBAP — source is always inside some pair.
        if self.count != 2 {
            return self.compute_vbap_panning(listener, direction);
        }

        let max_ext = max_extension.clamp(0.0, 0.6);
        let mut gains = ChannelGains::silent(self.total_channels);

        // Source azimuth in listener's local frame
        let source_azimuth = if direction.x * direction.x + direction.y * direction.y < 1e-10 {
            0.0
        } else {
            let cos_y = listener.yaw.cos();
            let sin_y = listener.yaw.sin();
            let local_x = direction.x * cos_y + direction.y * sin_y;
            let local_y = -direction.x * sin_y + direction.y * cos_y;
            local_y.atan2(local_x)
        };

        // Speaker azimuths
        let mut angles = [(0.0f32, 0usize); 2];
        for (i, speaker) in self.speakers.iter().enumerate().take(2) {
            let sp = speaker.position - listener.position;
            let cos_y = listener.yaw.cos();
            let sin_y = listener.yaw.sin();
            let local_x = sp.x * cos_y + sp.y * sin_y;
            let local_y = -sp.x * sin_y + sp.y * cos_y;
            angles[i] = (local_y.atan2(local_x), i);
        }
        // Sort so angles[0] < angles[1]
        if angles[0].0 > angles[1].0 {
            angles.swap(0, 1);
        }

        let (angle_a, si_a) = angles[0];
        let (angle_b, si_b) = angles[1];

        // Speaker pair span
        let pair_span = angle_b - angle_a;
        if pair_span.abs() < 1e-6 {
            // Degenerate: speakers at same angle
            gains.gains[self.speakers[si_a].channel] = 1.0;
            return gains;
        }

        // Solve g = L^{-1} * p (Gjørup Eq. 1)
        let (ax, ay) = (angle_a.cos(), angle_a.sin());
        let (bx, by) = (angle_b.cos(), angle_b.sin());
        let det = ax * by - bx * ay;
        if det.abs() < 1e-8 {
            gains.gains[self.speakers[si_a].channel] = 1.0;
            return gains;
        }
        let inv_det = 1.0 / det;
        let (sx, sy) = (source_azimuth.cos(), source_azimuth.sin());
        let mut ga = (by * sx - bx * sy) * inv_det;
        let mut gb = (-ay * sx + ax * sy) * inv_det;

        // Standard VBAP: both gains non-negative → source inside pair
        if ga >= -1e-6 && gb >= -1e-6 {
            ga = ga.max(0.0);
            gb = gb.max(0.0);
        } else {
            // Source outside pair → extended VBAP.
            // Clamp the negative gain so extension doesn't exceed max_ext of span.
            // When ga or gb = -max_ext/(1+max_ext), the source is at the extension limit.
            let min_gain = -max_ext / (1.0 + max_ext);
            ga = ga.max(min_gain);
            gb = gb.max(min_gain);
        }

        // Energy normalization (Gjørup Eq. 3): g / sqrt(ga² + gb²)
        let norm = (ga * ga + gb * gb).sqrt();
        if norm > 1e-8 {
            ga /= norm;
            gb /= norm;
        }

        gains.gains[self.speakers[si_a].channel] = ga;
        gains.gains[self.speakers[si_b].channel] = gb;

        gains
    }

    // ── Stereo mode ────────────────────────────────────────────────────

    /// Compute per-channel gains for simple L/R stereo panning.
    ///
    /// Uses equal-power pan law based on source azimuth from the listener.
    /// Only outputs to channels 0 (L) and 1 (R).
    pub fn compute_gains_stereo(
        &self,
        listener: &Listener,
        source: &SourceSpatial,
        distance: &DistanceParams,
    ) -> ChannelGains {
        let source_pos = source.position;
        let source_orientation = source.orientation;
        let source_directivity = source.directivity;
        let mut gains = ChannelGains::silent(self.total_channels);

        // Source azimuth in listener's local frame
        let d = source_pos - listener.position;
        let source_azimuth = if d.x * d.x + d.y * d.y < 1e-10 {
            0.0
        } else {
            let cos_y = listener.yaw.cos();
            let sin_y = listener.yaw.sin();
            let local_x = d.x * cos_y + d.y * sin_y; // forward
            let local_y = -d.x * sin_y + d.y * cos_y; // left
            local_y.atan2(local_x)
        };

        // Map azimuth to pan position [0, 1]:
        //   left (+π/2) → 0, center (0) → 0.5, right (-π/2) → 1
        let pan = (0.5 - source_azimuth / std::f32::consts::PI).clamp(0.0, 1.0);
        let angle = pan * std::f32::consts::FRAC_PI_2;
        let l_gain = angle.cos();
        let r_gain = angle.sin();

        // Distance, directivity, hearing attenuation
        let dist = distance_gain_at_model(
            listener.position,
            source_pos,
            distance.ref_distance,
            distance.max_distance,
            distance.rolloff,
            distance.model,
        );
        let dir = directivity_gain(
            source_pos,
            source_orientation,
            listener.position,
            source_directivity,
        );
        let hearing = listener.hearing_gain(source_pos);
        let atten = dist * dir * hearing;

        gains.gains[0] = l_gain * atten;
        gains.gains[1] = r_gain * atten;
        gains
    }

    // ── Mono mode ─────────────────────────────────────────────────────

    /// Compute per-channel gains for mono output.
    ///
    /// All spatial channels receive equal gain, normalized for constant power.
    pub fn compute_gains_mono(
        &self,
        listener: &Listener,
        source: &SourceSpatial,
        distance: &DistanceParams,
    ) -> ChannelGains {
        let source_pos = source.position;
        let source_orientation = source.orientation;
        let source_directivity = source.directivity;
        let mut gains = ChannelGains::silent(self.total_channels);

        let dist = distance_gain_at_model(
            listener.position,
            source_pos,
            distance.ref_distance,
            distance.max_distance,
            distance.rolloff,
            distance.model,
        );
        let dir = directivity_gain(
            source_pos,
            source_orientation,
            listener.position,
            source_directivity,
        );
        let hearing = listener.hearing_gain(source_pos);
        let atten = dist * dir * hearing;

        // Equal gain to all spatial channels, normalized so total power ≈ atten²
        let per_channel = if self.count > 0 {
            atten / (self.count as f32).sqrt()
        } else {
            0.0
        };

        for i in 0..self.count {
            gains.gains[self.speakers[i].channel] = per_channel;
        }
        gains
    }

    // ── Quad mode ─────────────────────────────────────────────────────

    /// Compute per-channel gains for quad (4.0) output.
    ///
    /// Runs VBAP then zeros out the center channel, redistributing its
    /// energy to the nearest L/R pair for constant power.
    pub fn compute_gains_quad(
        &self,
        listener: &Listener,
        source: &SourceSpatial,
        distance: &DistanceParams,
    ) -> ChannelGains {
        let mut gains = self.compute_gains_vbap(listener, source, distance);

        // Zero out center channel (ch 2 in ITU 5.1) and redistribute to L/R
        if self.total_channels > 2 {
            let center = gains.gains[2];
            if center.abs() > 1e-8 {
                // Distribute center to L and R preserving power
                let half_power = center * std::f32::consts::FRAC_1_SQRT_2;
                gains.gains[0] += half_power;
                gains.gains[1] += half_power;
                gains.gains[2] = 0.0;
            }
        }

        // Also zero LFE if present
        if let Some(lfe) = self.lfe_channel {
            gains.gains[lfe] = 0.0;
        }

        gains
    }

    // ── MDAP (Multiple Direction Amplitude Panning) ────────────────────

    /// Compute per-channel gains using MDAP (Pulkki 1999).
    ///
    /// When `spread > 0`, evaluates VBAP at 7 phantom directions spread
    /// around the true source azimuth. The gain vectors are averaged,
    /// producing a wider image. When `spread == 0`, delegates to plain VBAP.
    pub fn compute_gains_mdap(
        &self,
        listener: &Listener,
        source: &SourceSpatial,
        distance: &DistanceParams,
        spread: f32,
    ) -> ChannelGains {
        if spread <= 0.0 || self.count < 2 {
            return self.compute_gains_vbap(listener, source, distance);
        }

        // Fan angle: spread=1.0 → π radians (180° total arc), spread=0.5 → 90°
        let fan = spread * std::f32::consts::PI;
        const N_PHANTOM: usize = 7;

        let mut acc = ChannelGains::silent(self.total_channels);

        // Source direction from listener
        let d = source.position - listener.position;
        let base_dist = d.length();
        if base_dist < 1e-6 {
            return self.compute_gains_vbap(listener, source, distance);
        }

        for i in 0..N_PHANTOM {
            // Spread phantom directions evenly across [-fan/2, +fan/2]
            let t = (i as f32 / (N_PHANTOM - 1) as f32) - 0.5; // [-0.5, 0.5]
            let angle_offset = t * fan;

            // Rotate source position around listener by angle_offset (2D, Z-up)
            let cos_a = angle_offset.cos();
            let sin_a = angle_offset.sin();
            let phantom_pos = Vec3::new(
                listener.position.x + d.x * cos_a - d.y * sin_a,
                listener.position.y + d.x * sin_a + d.y * cos_a,
                source.position.z,
            );

            let phantom = SourceSpatial {
                position: phantom_pos,
                ..*source
            };
            let phantom_gains = self.compute_gains_vbap(listener, &phantom, distance);

            for ch in 0..self.total_channels {
                acc.gains[ch] += phantom_gains.gains[ch];
            }
        }

        // Average and re-normalize for constant power
        let inv_n = 1.0 / N_PHANTOM as f32;
        let mut power = 0.0f32;
        for ch in 0..self.total_channels {
            acc.gains[ch] *= inv_n;
            power += acc.gains[ch] * acc.gains[ch];
        }

        // Re-normalize so total power matches a single VBAP evaluation
        if power > 1e-12 {
            let ref_gains = self.compute_gains_vbap(listener, source, distance);
            let ref_power: f32 = ref_gains.gains[..self.total_channels]
                .iter()
                .map(|g| g * g)
                .sum();
            let scale = (ref_power / power).sqrt();
            for ch in 0..self.total_channels {
                acc.gains[ch] *= scale;
            }
        }

        acc
    }

    /// Compute per-channel gains for the given render mode.
    pub fn compute_gains(
        &self,
        mode: RenderMode,
        listener: &Listener,
        source: &SourceSpatial,
        distance: &DistanceParams,
    ) -> ChannelGains {
        match mode {
            RenderMode::WorldLocked
            | RenderMode::Hrtf
            | RenderMode::Dbap
            | RenderMode::Ambisonics => self.compute_gains_stereo(listener, source, distance),
            RenderMode::Vbap => self.compute_gains_vbap(listener, source, distance),
        }
    }

    /// Compute VBAP gains with MDAP (Multiple Direction Amplitude Panning) support.
    /// When spread > 0, pans to multiple directions for a wider image;
    /// when spread == 0, falls through to standard VBAP.
    pub fn compute_gains_with_spread(
        &self,
        listener: &Listener,
        source: &SourceSpatial,
        distance: &DistanceParams,
        spread: f32,
    ) -> ChannelGains {
        self.compute_gains_mdap(listener, source, distance, spread)
    }
}

// ── VBAP gain lookup table ────────────────────────────────────────────────

const LUT_SIZE: usize = 360;

/// Pre-computed VBAP gains at 1° azimuth resolution.
///
/// Eliminates per-source speaker-pair search + matrix inversion on the audio
/// thread. Build once per buffer (or when listener moves), then index by
/// azimuth angle. With the path architecture (7 VBAP evaluations per source),
/// this is ~7× fewer trig + linear-algebra ops per source.
pub struct VbapLookup {
    entries: Vec<ChannelGains>,
    total_channels: usize,
    /// Listener state when this LUT was last computed.
    cached_position: Vec3,
    cached_yaw: f32,
}

impl VbapLookup {
    pub fn new(total_channels: usize) -> Self {
        Self {
            entries: vec![ChannelGains::silent(total_channels); LUT_SIZE],
            total_channels,
            cached_position: Vec3::new(f32::NAN, 0.0, 0.0), // force initial build
            cached_yaw: f32::NAN,
        }
    }

    /// Rebuild LUT if listener has moved. Returns true if rebuilt.
    pub fn update(&mut self, layout: &SpeakerLayout, listener: &Listener) -> bool {
        let pos_delta = (listener.position - self.cached_position).length();
        let yaw_delta = (listener.yaw - self.cached_yaw).abs();
        if pos_delta < 0.01 && yaw_delta < 0.001 {
            return false;
        }
        self.build(layout, listener);
        true
    }

    /// Look up pre-computed VBAP gains for a world-space direction vector.
    /// Transforms to listener-local frame, then linearly interpolates
    /// between the two nearest 1° LUT entries.
    pub fn lookup(&self, direction: Vec3) -> ChannelGains {
        // Transform world-space direction to listener-local frame
        let cos_y = self.cached_yaw.cos();
        let sin_y = self.cached_yaw.sin();
        let local_x = direction.x * cos_y + direction.y * sin_y;
        let local_y = -direction.x * sin_y + direction.y * cos_y;
        let azimuth = local_y.atan2(local_x);
        let deg = azimuth.to_degrees().rem_euclid(360.0);
        let idx = deg as usize;
        let frac = deg - idx as f32;

        let idx_a = idx % LUT_SIZE;
        let idx_b = (idx + 1) % LUT_SIZE;

        let a = &self.entries[idx_a];
        let b = &self.entries[idx_b];

        let mut result = ChannelGains::silent(self.total_channels);
        for ch in 0..self.total_channels {
            result.gains[ch] = a.gains[ch] * (1.0 - frac) + b.gains[ch] * frac;
        }
        result
    }

    fn build(&mut self, layout: &SpeakerLayout, listener: &Listener) {
        // Pre-compute speaker angles relative to listener (done once)
        let count = layout.speaker_count();
        let mut speaker_angles: [(f32, usize); MAX_CHANNELS] = [(0.0, 0); MAX_CHANNELS];
        let cos_y = listener.yaw.cos();
        let sin_y = listener.yaw.sin();

        for (i, speaker) in layout.speakers().iter().enumerate().take(count) {
            let sp = speaker.position - listener.position;
            let local_x = sp.x * cos_y + sp.y * sin_y;
            let local_y = -sp.x * sin_y + sp.y * cos_y;
            speaker_angles[i] = (local_y.atan2(local_x), i);
        }
        // Sort by angle
        for i in 1..count {
            let key = speaker_angles[i];
            let mut j = i;
            while j > 0 && speaker_angles[j - 1].0 > key.0 {
                speaker_angles[j] = speaker_angles[j - 1];
                j -= 1;
            }
            speaker_angles[j] = key;
        }

        // Pre-compute per-speaker distances for distance compensation
        let mut speaker_dists = [0.0f32; MAX_CHANNELS];
        for (i, dist) in speaker_dists.iter_mut().enumerate().take(count) {
            *dist = (layout.speakers()[i].position - listener.position)
                .length()
                .max(0.1);
        }

        // Build LUT: for each degree, find straddling pair and compute gains
        for deg in 0..LUT_SIZE {
            let azimuth = (deg as f32).to_radians();
            let sx = azimuth.cos();
            let sy = azimuth.sin();

            let mut best_a = 0usize;
            let mut best_b = 0usize;
            let mut best_ga = 0.0f32;
            let mut best_gb = 0.0f32;
            let mut found = false;

            for pair_idx in 0..count {
                let idx_a = pair_idx;
                let idx_b = (pair_idx + 1) % count;
                let (angle_a, si_a) = speaker_angles[idx_a];
                let (angle_b, si_b) = speaker_angles[idx_b];

                let (ax, ay) = (angle_a.cos(), angle_a.sin());
                let (bx, by) = (angle_b.cos(), angle_b.sin());

                let det = ax * by - bx * ay;
                if det.abs() < 1e-8 {
                    continue;
                }
                let inv_det = 1.0 / det;

                let ga = (by * sx - bx * sy) * inv_det;
                let gb = (-ay * sx + ax * sy) * inv_det;

                if ga >= -1e-6 && gb >= -1e-6 {
                    found = true;
                    best_a = si_a;
                    best_b = si_b;
                    best_ga = ga.max(0.0);
                    best_gb = gb.max(0.0);
                    break;
                }
            }

            if !found {
                // Fallback: nearest speaker
                let mut min_diff = f32::MAX;
                for &(angle, idx) in speaker_angles.iter().take(count) {
                    let diff = angle_diff(azimuth, angle).abs();
                    if diff < min_diff {
                        min_diff = diff;
                        best_a = idx;
                        best_ga = 1.0;
                        best_gb = 0.0;
                    }
                }
                best_b = best_a;
            }

            // Normalize for constant power
            let norm = (best_ga * best_ga + best_gb * best_gb).sqrt();
            if norm > 1e-8 {
                best_ga /= norm;
                best_gb /= norm;
            }

            // Distance compensation
            let d_a = speaker_dists[best_a];
            let d_b = if best_b != best_a {
                speaker_dists[best_b]
            } else {
                d_a
            };
            let d_ref = d_a.max(d_b);
            best_ga *= d_a / d_ref;
            if best_b != best_a {
                best_gb *= d_b / d_ref;
            }
            let norm2 = (best_ga * best_ga + best_gb * best_gb).sqrt();
            if norm2 > 1e-8 {
                best_ga /= norm2;
                best_gb /= norm2;
            }

            let mut entry = ChannelGains::silent(layout.total_channels());
            entry.gains[layout.speakers()[best_a].channel] = best_ga;
            if best_b != best_a {
                entry.gains[layout.speakers()[best_b].channel] = best_gb;
            }
            self.entries[deg] = entry;
        }

        self.cached_position = listener.position;
        self.cached_yaw = listener.yaw;
    }
}

/// Shortest signed angular difference, normalized to [-π, π].
fn angle_diff(a: f32, b: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let d = a - b;
    (d + PI).rem_euclid(TAU) - PI
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn omni(pos: Vec3, dir: Vec3) -> SourceSpatial<'static> {
        static OMNI: DirectivityPattern = DirectivityPattern::Omni;
        SourceSpatial {
            position: pos,
            orientation: dir,
            directivity: &OMNI,
        }
    }

    fn default_distance() -> DistanceParams {
        DistanceParams {
            ref_distance: 1.0,
            max_distance: 20.0,
            rolloff: 1.0,
            model: DistanceModelType::Inverse,
        }
    }

    // ── Stereo pan (equal-power L/R) tests ────────────────────────────────

    #[test]
    fn mic_source_to_listeners_left_is_louder_in_ch0() {
        let layout = SpeakerLayout::stereo(Vec3::new(0.0, 2.0, 0.0), Vec3::new(6.0, 2.0, 0.0));
        let dist = default_distance();
        // Listener at (3, 2) facing +X (yaw=0). Left ear = +Y direction.
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);

        // Source to the listener's left (+Y)
        let gains = layout.compute_gains_stereo(
            &listener,
            &omni(Vec3::new(3.0, 4.0, 0.0), Vec3::new(1.0, 0.0, 0.0)),
            &dist,
        );

        assert!(
            gains.gains[0] > gains.gains[1],
            "left ear={} should be louder than right ear={}",
            gains.gains[0],
            gains.gains[1]
        );
    }

    #[test]
    fn mic_source_ahead_is_centered() {
        let layout = SpeakerLayout::stereo(Vec3::new(0.0, 2.0, 0.0), Vec3::new(6.0, 2.0, 0.0));
        let dist = default_distance();
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);

        // Source directly ahead of listener (+X)
        let gains = layout.compute_gains_stereo(
            &listener,
            &omni(Vec3::new(5.0, 2.0, 0.0), Vec3::new(1.0, 0.0, 0.0)),
            &dist,
        );

        assert!(
            (gains.gains[0] - gains.gains[1]).abs() < 0.01,
            "left={} and right={} should be roughly equal for centered source",
            gains.gains[0],
            gains.gains[1]
        );
    }

    #[test]
    fn mic_panning_follows_listener_yaw() {
        let layout = SpeakerLayout::stereo(Vec3::new(0.0, 2.0, 0.0), Vec3::new(6.0, 2.0, 0.0));
        let dist = default_distance();
        // Source at (5, 2). Listener facing +X → source is ahead (centered).
        let listener_fwd = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);
        let src = omni(Vec3::new(5.0, 2.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let gains_fwd = layout.compute_gains_stereo(&listener_fwd, &src, &dist);

        // Same source, but listener rotated to face +Y → source is now to the RIGHT
        let listener_rotated = Listener::new(Vec3::new(3.0, 2.0, 0.0), PI / 2.0);
        let gains_rot = layout.compute_gains_stereo(&listener_rotated, &src, &dist);

        // When facing forward, should be centered
        assert!(
            (gains_fwd.gains[0] - gains_fwd.gains[1]).abs() < 0.01,
            "facing source: left={} right={} should be equal",
            gains_fwd.gains[0],
            gains_fwd.gains[1]
        );
        // When rotated, source is to the right → ch1 louder
        assert!(
            gains_rot.gains[1] > gains_rot.gains[0],
            "source to right: right={} should be louder than left={}",
            gains_rot.gains[1],
            gains_rot.gains[0]
        );
    }

    #[test]
    fn mic_5_1_has_six_channels() {
        let layout = SpeakerLayout::surround_5_1(
            Vec3::new(1.0, 3.0, 0.0),
            Vec3::new(5.0, 3.0, 0.0),
            Vec3::new(3.0, 4.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(6.0, 0.0, 0.0),
        );
        assert_eq!(layout.total_channels(), 6);
        assert_eq!(layout.speaker_count(), 5);
        assert_eq!(layout.lfe_channel(), Some(3));
    }

    // ── VBAP tests ─────────────────────────────────────────────────────

    fn vbap_5_1_layout() -> SpeakerLayout {
        // Room 6×4m, speakers on walls at standard ITU angles
        let mut layout = SpeakerLayout::surround_5_1(
            Vec3::new(1.0, 3.5, 0.0), // L: front-left
            Vec3::new(5.0, 3.5, 0.0), // R: front-right
            Vec3::new(3.0, 4.0, 0.0), // C: front-center
            Vec3::new(0.5, 0.5, 0.0), // Ls: rear-left
            Vec3::new(5.5, 0.5, 0.0), // Rs: rear-right
        );
        layout
    }

    #[test]
    fn vbap_source_ahead_activates_front_speakers() {
        let layout = vbap_5_1_layout();
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), PI / 2.0); // facing +Y
        let dist = default_distance();

        // Source directly ahead of listener
        let gains = layout.compute_gains_vbap(
            &listener,
            &omni(Vec3::new(3.0, 5.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
            &dist,
        );

        // Front speakers (L=0, R=1, C=2) should have nonzero gains
        let front_sum = gains.gains[0] + gains.gains[1] + gains.gains[2];
        let rear_sum = gains.gains[4] + gains.gains[5];
        assert!(
            front_sum > rear_sum,
            "front={front_sum} should dominate rear={rear_sum}"
        );
    }

    #[test]
    fn vbap_constant_power() {
        let layout = SpeakerLayout::stereo(Vec3::new(1.0, 3.0, 0.0), Vec3::new(5.0, 3.0, 0.0));
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);

        // Fixed distance (2m), vary angle — power should stay roughly constant
        let r = 2.0;
        let dist = DistanceParams {
            ref_distance: 1.0,
            max_distance: 20.0,
            rolloff: 1.0,
            model: DistanceModelType::Inverse,
        };

        for angle_deg in (0..360).step_by(30) {
            let angle = (angle_deg as f32).to_radians();
            let source_pos = Vec3::new(
                listener.position.x + r * angle.cos(),
                listener.position.y + r * angle.sin(),
                0.0,
            );
            let gains = layout.compute_gains_vbap(
                &listener,
                &omni(source_pos, Vec3::new(1.0, 0.0, 0.0)),
                &dist,
            );

            let power: f32 = gains.gains[..2].iter().map(|g| g * g).sum();
            // Power = (distance_atten * directivity * hearing_cone)²
            let expected_attenuation = distance_gain_at_model(
                listener.position,
                source_pos,
                dist.ref_distance,
                dist.max_distance,
                dist.rolloff,
                dist.model,
            );
            let hearing = listener.hearing_gain(source_pos);
            let expected_power =
                (expected_attenuation * hearing) * (expected_attenuation * hearing);
            assert!(
                (power - expected_power).abs() < 0.05,
                "angle={}° power={} expected={}",
                angle_deg,
                power,
                expected_power
            );
        }
    }

    #[test]
    fn vbap_hearing_cone_attenuates_rear_source() {
        let layout = vbap_5_1_layout();
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), PI / 2.0); // facing +Y
        let dist = default_distance();

        let gains_ahead = layout.compute_gains_vbap(
            &listener,
            &omni(Vec3::new(3.0, 4.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
            &dist,
        );
        let gains_behind = layout.compute_gains_vbap(
            &listener,
            &omni(Vec3::new(3.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
            &dist,
        );

        let power_ahead: f32 = gains_ahead.gains[..6].iter().map(|g| g * g).sum();
        let power_behind: f32 = gains_behind.gains[..6].iter().map(|g| g * g).sum();
        assert!(
            power_ahead > power_behind * 2.0,
            "ahead power={power_ahead} should be much greater than behind power={power_behind}"
        );
    }

    #[test]
    fn vbap_lfe_channel_stays_zero() {
        let layout = vbap_5_1_layout();
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);
        let dist = default_distance();

        let gains = layout.compute_gains_vbap(
            &listener,
            &omni(Vec3::new(4.0, 3.0, 0.0), Vec3::new(1.0, 0.0, 0.0)),
            &dist,
        );

        assert_eq!(gains.gains[3], 0.0, "LFE should be zero from VBAP");
    }

    // ── MDAP tests ───────────────────────────────────────────────────

    #[test]
    fn mdap_spread_activates_more_speakers() {
        let layout = vbap_5_1_layout();
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), PI / 2.0); // facing +Y
        let dist = default_distance();
        let src = omni(Vec3::new(3.0, 4.0, 0.0), Vec3::new(0.0, 1.0, 0.0));

        let point_gains = layout.compute_gains_mdap(&listener, &src, &dist, 0.0);
        let spread_gains = layout.compute_gains_mdap(&listener, &src, &dist, 0.5);

        // With spread, more speakers should have nonzero gain
        let point_active = point_gains.gains[..6]
            .iter()
            .filter(|g| g.abs() > 0.01)
            .count();
        let spread_active = spread_gains.gains[..6]
            .iter()
            .filter(|g| g.abs() > 0.01)
            .count();
        assert!(
            spread_active >= point_active,
            "spread should activate >= as many speakers: point={point_active} spread={spread_active}"
        );
    }

    #[test]
    fn mdap_preserves_power() {
        let layout = vbap_5_1_layout();
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), PI / 2.0);
        let dist = default_distance();
        let src = omni(Vec3::new(3.0, 4.0, 0.0), Vec3::new(0.0, 1.0, 0.0));

        let point_gains = layout.compute_gains_mdap(&listener, &src, &dist, 0.0);
        let spread_gains = layout.compute_gains_mdap(&listener, &src, &dist, 0.5);

        let point_power: f32 = point_gains.gains[..6].iter().map(|g| g * g).sum();
        let spread_power: f32 = spread_gains.gains[..6].iter().map(|g| g * g).sum();
        assert!(
            (point_power - spread_power).abs() < 0.05,
            "MDAP should preserve power: point={point_power} spread={spread_power}"
        );
    }

    // ── Real room layout L/R verification ──────────────────────────────
    //
    // Tests use the exact speaker positions from main.rs to verify
    // that audio L/R mapping is correct for a listener at room center
    // facing the front wall (+Y).
    //
    // Room layout (6×4m), audience perspective (facing front wall):
    //   FL(ch0) ── C(ch2) ── FR(ch1)   (front wall, y=4)
    //   │                     │
    //   │    listener (3,2)   │         facing +Y → left = -X, right = +X
    //   │                     │
    //   RL(ch4) ────────── RR(ch5)      (rear wall, y=0)
    //
    // FL should be at low-x (listener's left), FR at high-x (listener's right).

    fn room_layout() -> SpeakerLayout {
        // 6×4m room. Audience faces front wall (+Y). Left = -X, Right = +X.
        SpeakerLayout::surround_5_1(
            Vec3::new(0.0, 4.0, 0.0), // FL (ch 0): front-left  (low x = left)
            Vec3::new(6.0, 4.0, 0.0), // FR (ch 1): front-right (high x = right)
            Vec3::new(3.0, 4.0, 0.0), // C  (ch 2): front-center
            Vec3::new(0.0, 0.0, 0.0), // RL (ch 4): rear-left
            Vec3::new(6.0, 0.0, 0.0), // RR (ch 5): rear-right
        )
    }

    /// Listener at room center facing front wall (+Y). yaw=π/2.
    fn room_listener() -> Listener {
        Listener::new(Vec3::new(3.0, 2.0, 0.0), PI / 2.0)
    }

    // ── Stereo L/R ──

    #[test]
    fn stereo_source_left_has_more_ch0() {
        let layout = room_layout();
        let listener = room_listener();
        let dist = default_distance();

        // Source to listener's LEFT: facing +Y, left is -X → (1, 3, 0)
        let gains = layout.compute_gains_stereo(
            &listener,
            &omni(Vec3::new(1.0, 3.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
            &dist,
        );

        assert!(
            gains.gains[0] > gains.gains[1],
            "source LEFT of listener → ch0 (L) should be louder: ch0={}, ch1={}",
            gains.gains[0],
            gains.gains[1]
        );
    }

    #[test]
    fn stereo_source_right_has_more_ch1() {
        let layout = room_layout();
        let listener = room_listener();
        let dist = default_distance();

        // Source to listener's RIGHT: facing +Y, right is +X → (5, 3, 0)
        let gains = layout.compute_gains_stereo(
            &listener,
            &omni(Vec3::new(5.0, 3.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
            &dist,
        );

        assert!(
            gains.gains[1] > gains.gains[0],
            "source RIGHT of listener → ch1 (R) should be louder: ch0={}, ch1={}",
            gains.gains[0],
            gains.gains[1]
        );
    }

    // ── VBAP L/R ──

    #[test]
    fn vbap_source_left_has_more_fl_than_fr() {
        let layout = room_layout();
        let listener = room_listener();
        let dist = default_distance();

        // Source to listener's LEFT (forward-left): (1, 3, 0)
        let gains = layout.compute_gains_vbap(
            &listener,
            &omni(Vec3::new(1.0, 3.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
            &dist,
        );

        assert!(
            gains.gains[0] > gains.gains[1],
            "source LEFT → FL (ch0) should be louder than FR (ch1): ch0={}, ch1={}",
            gains.gains[0],
            gains.gains[1]
        );
    }

    #[test]
    fn vbap_source_right_has_more_fr_than_fl() {
        let layout = room_layout();
        let listener = room_listener();
        let dist = default_distance();

        // Source to listener's RIGHT (forward-right): (5, 3, 0)
        let gains = layout.compute_gains_vbap(
            &listener,
            &omni(Vec3::new(5.0, 3.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
            &dist,
        );

        assert!(
            gains.gains[1] > gains.gains[0],
            "source RIGHT → FR (ch1) should be louder than FL (ch0): ch0={}, ch1={}",
            gains.gains[0],
            gains.gains[1]
        );
    }

    #[test]
    fn vbap_source_rear_left_has_more_rl_than_rr() {
        let layout = room_layout();
        let listener = room_listener();
        let dist = default_distance();

        // Source rear-left: behind and to the left → (1, 1, 0)
        let gains = layout.compute_gains_vbap(
            &listener,
            &omni(Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
            &dist,
        );

        assert!(
            gains.gains[4] > gains.gains[5],
            "source REAR-LEFT → RL (ch4) should be louder than RR (ch5): ch4={}, ch5={}",
            gains.gains[4],
            gains.gains[5]
        );
    }

    #[test]
    fn vbap_source_rear_right_has_more_rr_than_rl() {
        let layout = room_layout();
        let listener = room_listener();
        let dist = default_distance();

        // Source rear-right: behind and to the right → (5, 1, 0)
        let gains = layout.compute_gains_vbap(
            &listener,
            &omni(Vec3::new(5.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
            &dist,
        );

        assert!(
            gains.gains[5] > gains.gains[4],
            "source REAR-RIGHT → RR (ch5) should be louder than RL (ch4): ch4={}, ch5={}",
            gains.gains[4],
            gains.gains[5]
        );
    }

    // ── Stereo pan L/R ──

    #[test]
    fn mic_source_left_of_listener_has_more_ch0() {
        let layout = room_layout();
        let listener = room_listener(); // at (3,2) facing +Y (yaw=π/2), left = -X
        let dist = default_distance();

        // Source to listener's LEFT (-X direction): (1, 3, 0)
        let gains = layout.compute_gains_stereo(
            &listener,
            &omni(Vec3::new(1.0, 3.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
            &dist,
        );

        assert!(
            gains.gains[0] > gains.gains[1],
            "source to listener's left → ch0 should be loudest: ch0={}, ch1={}",
            gains.gains[0],
            gains.gains[1]
        );
    }

    #[test]
    fn mic_source_right_of_listener_has_more_ch1() {
        let layout = room_layout();
        let listener = room_listener(); // at (3,2) facing +Y, right = +X
        let dist = default_distance();

        // Source to listener's RIGHT (+X direction): (5, 3, 0)
        let gains = layout.compute_gains_stereo(
            &listener,
            &omni(Vec3::new(5.0, 3.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
            &dist,
        );

        assert!(
            gains.gains[1] > gains.gains[0],
            "source to listener's right → ch1 should be loudest: ch0={}, ch1={}",
            gains.gains[0],
            gains.gains[1]
        );
    }

    // ── Quad L/R ──

    #[test]
    fn quad_source_left_has_more_left_channels() {
        let layout = room_layout();
        let listener = room_listener();
        let dist = default_distance();

        // Source to listener's LEFT: (1, 3, 0)
        let gains = layout.compute_gains_quad(
            &listener,
            &omni(Vec3::new(1.0, 3.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
            &dist,
        );

        let left_sum = gains.gains[0] + gains.gains[4]; // FL + RL
        let right_sum = gains.gains[1] + gains.gains[5]; // FR + RR
        assert!(
            left_sum > right_sum,
            "source LEFT → left channels (FL+RL={}) should be louder than right (FR+RR={})",
            left_sum,
            right_sum
        );
    }

    // ── Mode dispatch test ─────────────────────────────────────────────

    #[test]
    fn compute_gains_dispatches_on_mode() {
        // Use 5.1 layout so WorldLocked (stereo, channels 0+1 only) and
        // VBAP (distributes across all speaker pairs) clearly diverge.
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);
        let dist = default_distance();
        let src = omni(Vec3::new(1.0, 3.0, 0.0), Vec3::new(1.0, 0.0, 0.0));

        let layout = SpeakerLayout::surround_5_1(
            Vec3::new(0.0, 4.0, 0.0),
            Vec3::new(6.0, 4.0, 0.0),
            Vec3::new(3.0, 4.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(6.0, 0.0, 0.0),
        );

        let mic_gains = layout.compute_gains(RenderMode::WorldLocked, &listener, &src, &dist);
        let vbap_gains = layout.compute_gains(RenderMode::Vbap, &listener, &src, &dist);

        // WorldLocked only fills channels 0-1 (stereo), VBAP uses 5.1 speaker pairs.
        // Compare across all channels — they must differ.
        let diff: f32 = (0..layout.total_channels())
            .map(|i| (mic_gains.gains[i] - vbap_gains.gains[i]).abs())
            .sum();
        assert!(
            diff > 0.01,
            "modes should produce different gains: mic={:?} vbap={:?}",
            &mic_gains.gains[..6],
            &vbap_gains.gains[..6]
        );
    }

    // ── VbapLookup accuracy vs live ─────────────────────────────────────

    #[test]
    fn vbap_lut_matches_live_panning() {
        let layout = room_layout();
        let listener = room_listener(); // yaw=π/2, so local != world frame

        let mut lut = VbapLookup::new(layout.total_channels());
        lut.update(&layout, &listener);

        // Test every 15° around the listener using world-space direction vectors
        let channels = layout.total_channels();
        for deg in (0..360).step_by(15) {
            let rad = (deg as f32).to_radians();
            let direction = Vec3::new(rad.cos(), rad.sin(), 0.0);

            let lut_gains = lut.lookup(direction);
            let live_gains = layout.compute_vbap_panning(&listener, direction);

            for ch in 0..channels {
                let diff = (lut_gains.gains[ch] - live_gains.gains[ch]).abs();
                assert!(
                    diff < 0.05,
                    "LUT vs live mismatch at {}°, ch{}: lut={:.4} live={:.4} (diff={:.4})",
                    deg,
                    ch,
                    lut_gains.gains[ch],
                    live_gains.gains[ch],
                    diff,
                );
            }
        }
    }

    // ── Extended VBAP (Gjørup et al.) tests ──────────────────────────────

    #[test]
    fn extended_vbap_inside_pair_matches_standard() {
        // Stereo speakers ahead-left and ahead-right. Source between them.
        let layout = SpeakerLayout::stereo(
            Vec3::new(-0.5, 1.0, 0.0), // left-forward
            Vec3::new(0.5, 1.0, 0.0),  // right-forward
        );
        let listener = Listener::new(Vec3::new(0.0, 0.0, 0.0), 0.0);

        // Source directly between speakers (slightly left of center).
        let dir = Vec3::new(-0.1, 1.0, 0.0);
        let standard = layout.compute_vbap_panning(&listener, dir);
        let extended = layout.compute_vbap_panning_extended(&listener, dir, 0.4);

        for ch in 0..2 {
            assert!(
                (standard.gains[ch] - extended.gains[ch]).abs() < 0.1,
                "ch{}: standard={} extended={} should be close",
                ch,
                standard.gains[ch],
                extended.gains[ch]
            );
        }
    }

    #[test]
    fn extended_vbap_at_speaker_gives_unity() {
        let layout = SpeakerLayout::stereo(Vec3::new(-1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0));
        let listener = Listener::new(Vec3::new(0.0, 0.0, 0.0), 0.0);

        // Direction exactly toward left speaker.
        let dir = Vec3::new(-1.0, 1.0, 0.0);
        let gains = layout.compute_vbap_panning_extended(&listener, dir, 0.4);

        // One gain should be ~1.0, other ~0.0.
        let max_g = gains.gains[0].abs().max(gains.gains[1].abs());
        assert!(
            max_g > 0.9,
            "at speaker, dominant gain should be near 1.0, got {}",
            max_g
        );
    }

    #[test]
    fn extended_vbap_beyond_speaker_has_negative_gain() {
        // Stereo: speakers at ±30° from forward (typical stereo).
        let layout = SpeakerLayout::stereo(
            Vec3::new(-0.5, 1.0, 0.0), // left-forward
            Vec3::new(0.5, 1.0, 0.0),  // right-forward
        );
        let listener = Listener::new(Vec3::new(0.0, 0.0, 0.0), 0.0);

        // Source well to the left (beyond left speaker span).
        let dir = Vec3::new(-1.0, 0.3, 0.0);
        let gains = layout.compute_vbap_panning_extended(&listener, dir, 0.4);

        // One gain should be negative (polarity inversion).
        let has_negative = gains.gains[0] < -0.01 || gains.gains[1] < -0.01;
        assert!(
            has_negative,
            "source beyond speaker should have a negative gain: L={} R={}",
            gains.gains[0], gains.gains[1]
        );
    }

    #[test]
    fn extended_vbap_energy_normalized() {
        let layout = SpeakerLayout::stereo(Vec3::new(-0.5, 1.0, 0.0), Vec3::new(0.5, 1.0, 0.0));
        let listener = Listener::new(Vec3::new(0.0, 0.0, 0.0), 0.0);

        // Test several directions, including ones beyond speaker span.
        let directions = [
            Vec3::new(1.0, 0.0, 0.0),  // ahead
            Vec3::new(-1.0, 0.3, 0.0), // far left
            Vec3::new(1.0, -0.5, 0.0), // far right
            Vec3::new(0.0, -1.0, 0.0), // behind
        ];

        for dir in &directions {
            let gains = layout.compute_vbap_panning_extended(&listener, *dir, 0.4);
            let energy = gains.gains[0] * gains.gains[0] + gains.gains[1] * gains.gains[1];
            assert!(
                (energy - 1.0).abs() < 0.15,
                "energy should be ≈ 1.0, got {} for dir {:?}",
                energy,
                dir
            );
        }
    }

    #[test]
    fn extended_vbap_zero_extension_matches_standard() {
        let layout = SpeakerLayout::stereo(Vec3::new(-0.5, 1.0, 0.0), Vec3::new(0.5, 1.0, 0.0));
        let listener = Listener::new(Vec3::new(0.0, 0.0, 0.0), 0.0);

        // Source beyond speaker span, but extension = 0 → should behave like standard.
        let dir = Vec3::new(-1.0, 0.3, 0.0);
        let gains = layout.compute_vbap_panning_extended(&listener, dir, 0.0);

        // With zero extension, no negative gains allowed.
        assert!(
            gains.gains[0] >= -0.01 && gains.gains[1] >= -0.01,
            "zero extension should not produce negative gains: L={} R={}",
            gains.gains[0],
            gains.gains[1]
        );
    }

    #[test]
    fn extended_vbap_3plus_speakers_delegates_to_standard() {
        // Quad layout (4 speakers) — extended should delegate to standard VBAP.
        let layout = SpeakerLayout::new(
            &[
                Speaker {
                    position: Vec3::new(-1.0, 1.0, 0.0),
                    channel: 0,
                },
                Speaker {
                    position: Vec3::new(1.0, 1.0, 0.0),
                    channel: 1,
                },
                Speaker {
                    position: Vec3::new(-1.0, -1.0, 0.0),
                    channel: 2,
                },
                Speaker {
                    position: Vec3::new(1.0, -1.0, 0.0),
                    channel: 3,
                },
            ],
            None,
            4,
        );
        let listener = Listener::new(Vec3::new(0.0, 0.0, 0.0), 0.0);
        let dir = Vec3::new(1.0, 0.0, 0.0);

        let standard = layout.compute_vbap_panning(&listener, dir);
        let extended = layout.compute_vbap_panning_extended(&listener, dir, 0.4);

        for ch in 0..4 {
            assert!(
                (standard.gains[ch] - extended.gains[ch]).abs() < 1e-6,
                "ch{}: 3+ speakers should delegate to standard VBAP",
                ch
            );
        }
    }
}
