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
            ref_distance: 1.0,
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

/// Output channel configuration. Determines which output channels exist on
/// the rig: a masked channel has no speaker (or subwoofer) behind it, so
/// nothing may be sent to it. Works by masking channels of the layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChannelMode {
    /// Stereo: channels 0 (L), 1 (R) only.
    Stereo,
    /// Quad 4.0: channels 0 (FL), 1 (FR), 4 (RL), 5 (RR). Center + LFE masked.
    Quad,
    /// Full 5.1 surround: channels 0 (L), 1 (R), 2 (C), 3 (LFE), 4 (Ls), 5 (Rs).
    /// No renderer pans into the LFE; it carries the bass that bass management
    /// redirects from the mains, which is why it must count as present here.
    Surround51,
}

impl ChannelMode {
    /// Output channel indices that exist in this mode, subwoofer included.
    pub fn active_channels(self) -> &'static [usize] {
        match self {
            ChannelMode::Stereo => &[0, 1],
            ChannelMode::Quad => &[0, 1, 4, 5],
            ChannelMode::Surround51 => &[0, 1, 2, 3, 4, 5],
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
    ///
    /// Implemented as masked 5.1 (6 channels) with center and LFE silent.
    /// This guarantees channel semantics match ITU 5.1 on the device —
    /// channels 0/1/4/5 are FL/FR/Ls/Rs, channels 2/3 are zeroed.
    pub fn quad(fl: Vec3, fr: Vec3, rl: Vec3, rr: Vec3) -> Self {
        let mut layout = Self::new(
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
                    channel: 4,
                },
                Speaker {
                    position: rr,
                    channel: 5,
                },
            ],
            Some(3),
            6,
        );
        // Mask center (2) and LFE (3) — quad only uses FL/FR/RL/RR.
        layout.set_active_channels(&[0, 1, 4, 5]);
        layout
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

    /// Middle of the spatial speakers' bounding box; the origin for a layout
    /// without speakers.
    ///
    /// This is the middle of the area the speakers enclose, not their mean: a
    /// 5.1 layout has three speakers in front and two behind, and the mean
    /// would sit forward of the room's middle.
    pub fn bounding_box_centre(&self) -> Vec3 {
        let speakers = self.speakers();
        let Some(first) = speakers.first() else {
            return Vec3::ZERO;
        };
        let mut min = first.position;
        let mut max = first.position;
        for speaker in speakers {
            min = Vec3::new(
                min.x.min(speaker.position.x),
                min.y.min(speaker.position.y),
                min.z.min(speaker.position.z),
            );
            max = Vec3::new(
                max.x.max(speaker.position.x),
                max.y.max(speaker.position.y),
                max.z.max(speaker.position.z),
            );
        }
        (min + max) * 0.5
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

    /// Closest a speaker is ever treated as being to the listener, so that a
    /// listener standing on a speaker does not silence it.
    const MIN_SPEAKER_DISTANCE: f32 = 0.1;

    /// Distance from the listener to the farthest speaker in the layout.
    ///
    /// This is the reference for distance compensation: the farthest speaker
    /// plays at full gain and every closer one is turned down to match it.
    fn farthest_speaker_distance(&self, listener: &Listener) -> f32 {
        self.speakers()
            .iter()
            .map(|speaker| (speaker.position - listener.position).length())
            .fold(Self::MIN_SPEAKER_DISTANCE, f32::max)
    }

    /// Gain that makes one speaker arrive at the listener as loud as the
    /// farthest speaker would: `distance / farthest`.
    ///
    /// The reference is the farthest speaker of the whole layout, not of the
    /// active pair, and the pair is deliberately not renormalised afterwards.
    /// Renormalising per pair cancels the compensation between pairs: with
    /// the centre speaker at 2 m and the others at 3.6 m, a source straight
    /// ahead arrived 5.1 dB louder than the same source at the side.
    fn arrival_compensation(
        &self,
        speaker_index: usize,
        listener: &Listener,
        farthest: f32,
    ) -> f32 {
        let distance = (self.speakers[speaker_index].position - listener.position)
            .length()
            .max(Self::MIN_SPEAKER_DISTANCE);
        distance / farthest
    }

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

        // Per-speaker distance compensation: attenuate closer speakers so
        // every direction arrives at the listener at the same level.
        let farthest = self.farthest_speaker_distance(listener);
        best_ga *= self.arrival_compensation(best_a, listener, farthest);
        if best_b != best_a {
            best_gb *= self.arrival_compensation(best_b, listener, farthest);
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

        let attenuation = dist * dir;
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
        let farthest = self.farthest_speaker_distance(listener);
        best_ga *= self.arrival_compensation(best_a, listener, farthest);
        if best_b != best_a {
            best_gb *= self.arrival_compensation(best_b, listener, farthest);
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
        // Limit the extension by angle, not by gain. The pair's arc is the
        // shorter arc between the speakers (where both VBAP gains are ≥ 0); its
        // centre bisects the two speaker vectors. The source may sit up to
        // `max_ext` of the arc width beyond either speaker: inside that the
        // exact (unclamped) Pulkki solution is used, and beyond it the panning
        // direction is held at the limit so the gains stop changing there.
        let arc_centre = (ay + by).atan2(ax + bx);
        let arc_half_width = 0.5 * (ax * bx + ay * by).clamp(-1.0, 1.0).acos();
        let offset_limit = arc_half_width * (1.0 + 2.0 * max_ext);
        let offset = angle_diff(source_azimuth, arc_centre).clamp(-offset_limit, offset_limit);
        let panned_azimuth = arc_centre + offset;

        let inv_det = 1.0 / det;
        let (sx, sy) = (panned_azimuth.cos(), panned_azimuth.sin());
        let mut ga = (by * sx - bx * sy) * inv_det;
        let mut gb = (-ay * sx + ax * sy) * inv_det;

        // Inside the pair both gains are non-negative; remove rounding noise
        // (this is also where a zero extension pins a source to the speaker).
        // Outside it, one gain is negative (polarity inversion) and already
        // bounded by the angular limit above.
        if ga >= -1e-6 && gb >= -1e-6 {
            ga = ga.max(0.0);
            gb = gb.max(0.0);
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

    // ── Mono mode ─────────────────────────────────────────────────────

    // ── Quad mode ─────────────────────────────────────────────────────

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

/// Pre-computed VBAP gains at 1° azimuth resolution (Shukla 2019).
///
/// Eliminates per-source speaker-pair search + matrix inversion on the audio
/// thread. Build once per buffer (or when listener moves), then index by
/// azimuth angle. With the path architecture (7 VBAP evaluations per source),
/// this is ~7× fewer trig + linear-algebra ops per source.
///
/// 1° resolution matches Shukla's implementation on Bela Mini (ARM Cortex-A8),
/// where VBAP outperformed FOA for both ITD error (150 vs 202 μs) and ILD
/// error (3.18 vs 4.69 dB).
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
        let farthest = layout.farthest_speaker_distance(listener);

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

            // Distance compensation (same as compute_gains_vbap)
            best_ga *= speaker_dists[best_a] / farthest;
            if best_b != best_a {
                best_gb *= speaker_dists[best_b] / farthest;
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
        let layout = SpeakerLayout::surround_5_1(
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
            // Power = (distance_atten * directivity)²
            let expected_attenuation = distance_gain_at_model(
                listener.position,
                source_pos,
                dist.ref_distance,
                dist.max_distance,
                dist.rolloff,
                dist.model,
            );
            let expected_power = expected_attenuation * expected_attenuation;
            assert!(
                (power - expected_power).abs() < 0.05,
                "angle={}° power={} expected={}",
                angle_deg,
                power,
                expected_power
            );
        }
    }

    /// A source is as loud behind the listener as ahead of it. On speakers the
    /// listener's own ears do the directional hearing; the renderer must not
    /// add a second, virtual one. Regression: a "hearing cone" cut everything
    /// more than 45° off the listener's nose to 0.3 (−10.5 dB).
    #[test]
    fn vbap_source_behind_is_as_loud_as_ahead() {
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

        // Compared where it is heard: speakers sit at different distances from
        // this listener, so equal loudness means equal power on arrival.
        let power_ahead = arrival_power(&layout, &listener, &gains_ahead);
        let power_behind = arrival_power(&layout, &listener, &gains_behind);
        assert!(
            (power_ahead / power_behind - 1.0).abs() < 0.01,
            "same distance ahead and behind must arrive equally loud: ahead={power_ahead} behind={power_behind}"
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

    // ── Quad L/R ──

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

    // -- Gotcha: VBAP magnetization effect (Gandemer 2018) --
    // Sources "snap" toward speaker positions during movement. Mitigation:
    // denser speaker arrays or gain interpolation at <= 10ms intervals.
    // This test verifies that gains change smoothly (monotonically) when
    // panning between two speakers — no sudden jumps or plateaus.

    #[test]
    fn vbap_gains_no_large_discontinuities() {
        // Verify VBAP gains don't have sudden jumps when sweeping between speakers.
        // Small bumps are expected (magnetization effect — Gandemer 2018: sources
        // "snap" toward speaker positions), but large discontinuities (> 0.15)
        // would indicate a broken speaker-pair selection.
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
            ],
            None,
            2,
        );
        let listener = Listener::new(Vec3::new(0.0, 0.0, 0.0), 0.0);

        let steps = 90;
        let mut prev_gains = [0.0_f32; 2];
        let mut max_jump = 0.0_f32;

        for step in 0..=steps {
            let angle =
                std::f32::consts::FRAC_PI_2 - (step as f32 / steps as f32) * std::f32::consts::PI;
            let direction = Vec3::new(angle.sin(), angle.cos(), 0.0);
            let gains = layout.compute_vbap_panning(&listener, direction);

            if step > 0 {
                for ch in 0..2 {
                    let jump = (gains.gains[ch] - prev_gains[ch]).abs();
                    max_jump = max_jump.max(jump);
                    assert!(
                        jump < 0.15,
                        "step {step} ch{ch}: gain discontinuity {jump:.4} \
                         ({:.4} -> {:.4}) — possible broken pair selection",
                        prev_gains[ch],
                        gains.gains[ch]
                    );
                }
            }

            prev_gains = [gains.gains[0], gains.gains[1]];
        }

        // Energy should be roughly preserved across the sweep.
        // At endpoints: one speaker at ~1.0. At midpoint: both at ~0.707.
        let midpoint_dir = Vec3::new(0.0, 1.0, 0.0);
        let mid_gains = layout.compute_vbap_panning(&listener, midpoint_dir);
        let energy: f32 = mid_gains.gains[..2].iter().map(|g| g * g).sum();
        assert!(
            (energy - 1.0).abs() < 0.1,
            "midpoint energy {energy:.4} should be ~1.0 (energy preservation)"
        );
    }

    // ── Hand-derived VBAP / layout values ─────────────────────────────

    /// Quad ring of radius 2 around the origin at ±45° / ±135°, listener at the
    /// origin facing +X (yaw 0, left = +Y). Equidistant, so distance
    /// compensation is a no-op and gains are pure Pulkki VBAP.
    fn equidistant_quad() -> SpeakerLayout {
        let offset = 2.0_f32.sqrt();
        SpeakerLayout::quad(
            Vec3::new(offset, offset, 0.0),   // FL ch0 at +45°
            Vec3::new(offset, -offset, 0.0),  // FR ch1 at −45°
            Vec3::new(-offset, offset, 0.0),  // RL ch4 at +135°
            Vec3::new(-offset, -offset, 0.0), // RR ch5 at −135°
        )
    }

    /// Power reaching the listener: each speaker's gain falls off as one over
    /// its distance, so this is Σ(gain / distance)².
    fn arrival_power(layout: &SpeakerLayout, listener: &Listener, gains: &ChannelGains) -> f32 {
        layout
            .speakers()
            .iter()
            .map(|speaker| {
                let distance = (speaker.position - listener.position).length();
                let arrival = gains.gains[speaker.channel] / distance;
                arrival * arrival
            })
            .sum()
    }

    fn total_power(gains: &ChannelGains) -> f32 {
        gains.gains.iter().map(|gain| gain * gain).sum()
    }

    /// World direction for a local azimuth (0 = ahead, + = left) of a listener.
    fn world_direction(listener: &Listener, local_azimuth: f32) -> Vec3 {
        let world_angle = listener.yaw + local_azimuth;
        Vec3::new(world_angle.cos(), world_angle.sin(), 0.0)
    }

    #[test]
    fn vbap_pair_gains_match_pulkki_closed_form() {
        // For a 90° pair at ±45°, solving g = L⁻¹·s gives
        //   g_left = sin(θ + 45°), g_right = cos(θ + 45°),
        // already unit power. θ = 15° → FL = sin 60° = 0.866025, FR = 0.5.
        let layout = equidistant_quad();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let gains =
            layout.compute_vbap_panning(&listener, world_direction(&listener, 15f32.to_radians()));
        assert!(
            (gains.gains[0] - 0.866_025).abs() < 1e-5,
            "FL={}",
            gains.gains[0]
        );
        assert!((gains.gains[1] - 0.5).abs() < 1e-5, "FR={}", gains.gains[1]);
        assert_eq!(gains.gains[4], 0.0);
        assert_eq!(gains.gains[5], 0.0);

        // Sweep the front pair against the closed form.
        for degrees in -45..=45 {
            let theta = (degrees as f32).to_radians();
            let gains = layout.compute_vbap_panning(&listener, world_direction(&listener, theta));
            let quarter = std::f32::consts::FRAC_PI_4;
            assert!((gains.gains[0] - (theta + quarter).sin()).abs() < 1e-4);
            assert!((gains.gains[1] - (theta + quarter).cos()).abs() < 1e-4);
        }

        // Directly behind: midway between RL and RR → 1/√2 each.
        let behind = layout.compute_vbap_panning(&listener, Vec3::new(-1.0, 0.0, 0.0));
        assert!((behind.gains[4] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
        assert!((behind.gains[5] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
    }

    #[test]
    fn vbap_constant_arrival_power_and_continuity_over_full_sweep_in_room() {
        // Room layout (non-equidistant: C 2.0 m, others √13 = 3.606 m). Each
        // gain is scaled by its speaker's distance over the farthest (√13), so
        // the power arriving at the listener is Σ(g/d)² = 1/13 at every
        // azimuth, and adjacent 0.5° steps (including pair boundaries at the
        // speaker azimuths 0°, ±56.31°, ±123.69°) change no gain by > 0.03.
        let layout = room_layout();
        let listener = room_listener();
        let mut previous: Option<ChannelGains> = None;
        for half_degree in 0..=720 {
            let azimuth = (half_degree as f32 * 0.5 - 180.0).to_radians();
            let gains = layout.compute_vbap_panning(&listener, world_direction(&listener, azimuth));
            let power = arrival_power(&layout, &listener, &gains);
            assert!(
                (power - 1.0 / 13.0).abs() < 1e-5,
                "azimuth {azimuth}: arrival power {power}"
            );
            assert_eq!(gains.gains[3], 0.0, "LFE must never receive VBAP gain");
            if let Some(previous_gains) = previous {
                for channel in 0..6 {
                    let step = (gains.gains[channel] - previous_gains.gains[channel]).abs();
                    assert!(
                        step < 0.03,
                        "azimuth {azimuth}, channel {channel}: step {step}"
                    );
                }
            }
            previous = Some(gains);
        }
    }

    #[test]
    fn vbap_source_on_each_speaker_drives_only_that_speaker() {
        // Direction exactly toward a speaker: g = L⁻¹·s = (1, 0), then scaled by
        // that speaker's distance over the farthest (√13 m): the centre at 2 m
        // plays at 2/√13 = 0.554700, the other four at unity.
        let layout = room_layout();
        let listener = room_listener();
        let farthest = 13.0_f32.sqrt();
        for speaker in layout.speakers() {
            let gains =
                layout.compute_vbap_panning(&listener, speaker.position - listener.position);
            let own_gain = (speaker.position - listener.position).length() / farthest;
            for channel in 0..6 {
                let expected = if channel == speaker.channel {
                    own_gain
                } else {
                    0.0
                };
                assert!(
                    (gains.gains[channel] - expected).abs() < 1e-4,
                    "toward ch{}: ch{channel} = {}",
                    speaker.channel,
                    gains.gains[channel]
                );
            }
        }
    }

    #[test]
    fn vbap_is_left_right_mirror_symmetric_in_room() {
        // Room layout is symmetric about the listener's forward axis (+Y, x = 3):
        // local azimuth +θ on (FL, RL) must equal −θ on (FR, RR); C equal.
        let layout = room_layout();
        let listener = room_listener();
        for degrees in [5.0_f32, 30.0, 56.0, 90.0, 140.0, 175.0] {
            let theta = degrees.to_radians();
            let left = layout.compute_vbap_panning(&listener, world_direction(&listener, theta));
            let right = layout.compute_vbap_panning(&listener, world_direction(&listener, -theta));
            for (channel, mirror) in [(0, 1), (2, 2), (4, 5)] {
                assert!(
                    (left.gains[channel] - right.gains[mirror]).abs() < 1e-4,
                    "{degrees}°: ch{channel}={} vs ch{mirror}={}",
                    left.gains[channel],
                    right.gains[mirror]
                );
            }
        }
    }

    #[test]
    fn vbap_distance_compensation_equalises_arrival_within_pair() {
        // Room listener: C at local 0° (2 m), FL at atan2(3, 2) = 56.31° (√13 m).
        // On the bisector (28.155°) raw VBAP gains are equal (1/√2 each), and
        // each is scaled by its distance over the farthest (√13 m):
        //   g_C = (1/√2)·2/√13 = 0.392232, g_FL = 1/√2 = 0.707107,
        // so the arrivals g/d are equal.
        let layout = room_layout();
        let listener = room_listener();
        let bisector = (3.0_f32.atan2(2.0)) * 0.5;
        let gains = layout.compute_vbap_panning(&listener, world_direction(&listener, bisector));
        assert!(
            (gains.gains[2] - 0.392_232).abs() < 1e-4,
            "C={}",
            gains.gains[2]
        );
        assert!(
            (gains.gains[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4,
            "FL={}",
            gains.gains[0]
        );
        let arrival_center = gains.gains[2] / 2.0;
        let arrival_front_left = gains.gains[0] / 13.0_f32.sqrt();
        assert!((arrival_center - arrival_front_left).abs() < 1e-4);
    }

    /// A source at a fixed distance must arrive equally loud from every
    /// direction. In the default 5.1 room (listener at centre, C at 2 m, the
    /// other four at √13 m) renormalising each pair to Σg² = 1 used to cancel
    /// the compensation between pairs: arrival power was 1/4 straight ahead
    /// (C only) but 1/13 at 90° left (FL + RL), 10·log10(13/4) = 5.1 dB apart.
    #[test]
    fn vbap_arrival_level_is_independent_of_azimuth_in_room() {
        let layout = room_layout();
        let listener = room_listener();
        let ahead = layout.compute_vbap_panning(&listener, world_direction(&listener, 0.0));
        let left = layout.compute_vbap_panning(
            &listener,
            world_direction(&listener, std::f32::consts::FRAC_PI_2),
        );
        let level_difference_db = 10.0
            * (arrival_power(&layout, &listener, &ahead)
                / arrival_power(&layout, &listener, &left))
            .log10();
        assert!(
            level_difference_db.abs() < 0.01,
            "ahead vs left arrival level differs by {level_difference_db:.2} dB"
        );
    }

    #[test]
    fn vbap_lookup_matches_live_at_every_degree() {
        // LUT entries are exact VBAP at integer local azimuths; the lookup must
        // reproduce live panning there, and keep the arrival power at 1/13
        // between entries (linear interpolation 1° apart dips < 0.1 %).
        let layout = room_layout();
        let listener = room_listener();
        let mut lookup = VbapLookup::new(layout.total_channels());
        assert!(lookup.update(&layout, &listener));
        assert!(
            !lookup.update(&layout, &listener),
            "unchanged listener must not rebuild"
        );
        for degrees in 0..360 {
            let azimuth = (degrees as f32).to_radians();
            let direction = world_direction(&listener, azimuth);
            let from_lookup = lookup.lookup(direction);
            let live = layout.compute_vbap_panning(&listener, direction);
            for channel in 0..6 {
                assert!(
                    (from_lookup.gains[channel] - live.gains[channel]).abs() < 1e-3,
                    "{degrees}° ch{channel}: lookup {} live {}",
                    from_lookup.gains[channel],
                    live.gains[channel]
                );
            }
            let between = lookup.lookup(world_direction(&listener, azimuth + 0.5_f32.to_radians()));
            let between_power = arrival_power(&layout, &listener, &between);
            assert!((between_power * 13.0 - 1.0).abs() < 1e-2);
        }
    }

    #[test]
    fn vbap_full_gains_apply_inverse_distance() {
        // compute_gains_vbap = unit-power panning × distance gain × directivity.
        // Omni source 4 m ahead, inverse model ref 1 rolloff 1 → amplitude 0.25,
        // split between FL/FR at 1/√2 each: 0.176777, Σg² = 0.0625.
        let layout = equidistant_quad();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let source = omni(Vec3::new(4.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0));
        let gains = layout.compute_gains_vbap(&listener, &source, &default_distance());
        assert!((gains.gains[0] - 0.176_777).abs() < 1e-5);
        assert!((gains.gains[1] - 0.176_777).abs() < 1e-5);
        assert!((total_power(&gains) - 0.0625).abs() < 1e-6);
        // Zero spread is exactly plain VBAP.
        let spread_zero =
            layout.compute_gains_with_spread(&listener, &source, &default_distance(), 0.0);
        assert_eq!(spread_zero.gains, gains.gains);
    }

    #[test]
    fn mdap_spread_is_mirror_symmetric_for_centred_source() {
        // Phantoms at ±fan/2 are placed symmetrically, so a source dead ahead
        // stays centred: FL = FR, RL = RR, and power equals the point source.
        let layout = equidistant_quad();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let source = omni(Vec3::new(2.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0));
        let point = layout.compute_gains_mdap(&listener, &source, &default_distance(), 0.0);
        let spread = layout.compute_gains_mdap(&listener, &source, &default_distance(), 0.8);
        assert!((spread.gains[0] - spread.gains[1]).abs() < 1e-5);
        assert!((spread.gains[4] - spread.gains[5]).abs() < 1e-5);
        assert!(
            spread.gains[4] > 0.0,
            "spread 0.8 (±72°) must reach the rear pair"
        );
        assert!((total_power(&spread) - total_power(&point)).abs() < 1e-5);
    }

    #[test]
    fn angle_diff_wraps_to_shortest_signed_arc() {
        // 170° − (−170°) = 340° ≡ −20°; −170° − 170° ≡ +20°; 30° − 10° = 20°.
        let radians = |degrees: f32| degrees.to_radians();
        assert!((angle_diff(radians(170.0), radians(-170.0)) - radians(-20.0)).abs() < 1e-5);
        assert!((angle_diff(radians(-170.0), radians(170.0)) - radians(20.0)).abs() < 1e-5);
        assert!((angle_diff(radians(30.0), radians(10.0)) - radians(20.0)).abs() < 1e-6);
        assert!(angle_diff(PI, -PI).abs() < 1e-5);
    }

    #[test]
    fn itu_5_1_and_quad_channel_conventions() {
        // ITU/SMPTE 5.1 order: L 0, R 1, C 2, LFE 3, Ls 4, Rs 5.
        let surround = room_layout();
        let channels: Vec<usize> = surround.speakers().iter().map(|s| s.channel).collect();
        assert_eq!(channels, vec![0, 1, 2, 4, 5]);
        assert_eq!(surround.lfe_channel(), Some(3));
        assert_eq!(surround.total_channels(), 6);
        // Quad is masked 5.1: FL/FR/RL/RR on 0/1/4/5, C and LFE silent.
        let quad = equidistant_quad();
        let quad_channels: Vec<usize> = quad.speakers().iter().map(|s| s.channel).collect();
        assert_eq!(quad_channels, vec![0, 1, 4, 5]);
        let mut gains = ChannelGains::silent(6);
        gains.gains[..6].copy_from_slice(&[1.0; 6]);
        quad.apply_mask(&mut gains);
        assert_eq!(&gains.gains[..6], &[1.0, 1.0, 0.0, 0.0, 1.0, 1.0]);
        assert_eq!(ChannelMode::Quad.active_channels(), &[0, 1, 4, 5]);
        for mode in [
            ChannelMode::Stereo,
            ChannelMode::Quad,
            ChannelMode::Surround51,
        ] {
            assert_eq!(ChannelMode::parse(mode.as_str()), Some(mode));
        }
        for (index, mode) in RenderMode::ALL.iter().enumerate() {
            assert_eq!(mode.index(), index);
        }
    }

    /// The doc promises the extension is "clamped to `max_extension` of the
    /// pair's angular span". The old code clamped the *unnormalised* gain at
    /// −e/(1+e) — the tangent-law small-angle approximation of g_a/g_b, not a
    /// bound on the gain — so the limit engaged at 23.9 % extension, not 40 %.
    /// Stereo ±30° (span 60°), source at +48° (30 % beyond the left speaker,
    /// inside max_extension 0.4): exact VBAP gives
    /// g_right = −sin 18°/sin 60° = −0.356822, g_left = sin 78°/sin 60° = 1.129474,
    /// normalised (−0.301245, 0.953547). The old clamp at −0.285714 yielded
    /// (−0.245239, 0.969463).
    /// Experimental path, off by default (`extended_vbap: false`, no setter).
    #[test]
    fn extended_vbap_unclamped_inside_max_extension() {
        let half_span = 30.0_f32.to_radians();
        let layout = SpeakerLayout::stereo(
            Vec3::new(half_span.cos(), half_span.sin(), 0.0),
            Vec3::new(half_span.cos(), -half_span.sin(), 0.0),
        );
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let gains = layout.compute_vbap_panning_extended(
            &listener,
            world_direction(&listener, 48.0_f32.to_radians()),
            0.4,
        );
        assert!(
            (gains.gains[0] - 0.953_547).abs() < 1e-3,
            "L={}",
            gains.gains[0]
        );
        assert!(
            (gains.gains[1] + 0.301_245).abs() < 1e-3,
            "R={}",
            gains.gains[1]
        );
    }

    /// Beyond the allowed extension the gains are held at the limit. Stereo
    /// ±30°, max_extension 0.4 → limit 30° + 0.4·60° = 54°:
    /// g_left = sin 84°/sin 60° = 1.148377, g_right = −sin 24°/sin 60° = −0.469655,
    /// normalised (0.925584, −0.378543). Sources at 54°, 70° and 120° all give
    /// these gains; the mirror side gives them swapped.
    #[test]
    fn extended_vbap_holds_gains_at_extension_limit() {
        let half_span = 30.0_f32.to_radians();
        let layout = SpeakerLayout::stereo(
            Vec3::new(half_span.cos(), half_span.sin(), 0.0),
            Vec3::new(half_span.cos(), -half_span.sin(), 0.0),
        );
        let listener = Listener::new(Vec3::ZERO, 0.0);
        for degrees in [54.0_f32, 70.0, 120.0] {
            let left_side = layout.compute_vbap_panning_extended(
                &listener,
                world_direction(&listener, degrees.to_radians()),
                0.4,
            );
            assert!(
                (left_side.gains[0] - 0.925_584).abs() < 1e-3,
                "{degrees}°: L={}",
                left_side.gains[0]
            );
            assert!(
                (left_side.gains[1] + 0.378_543).abs() < 1e-3,
                "{degrees}°: R={}",
                left_side.gains[1]
            );
            let right_side = layout.compute_vbap_panning_extended(
                &listener,
                world_direction(&listener, (-degrees).to_radians()),
                0.4,
            );
            assert!((right_side.gains[0] - left_side.gains[1]).abs() < 1e-5);
            assert!((right_side.gains[1] - left_side.gains[0]).abs() < 1e-5);
        }
    }
}
