//! One ambient sound heard from an arc: its two ears' filters at work, the
//! glide when the head turns to another bearing, and the glide between the
//! arc and the all-round bus.
//!
//! The filters for the bearing nearest the arc's are convolved with the
//! sound. When the head turns past the middle between two bearings, the
//! other pair of convolvers takes the new bearing's filters and the tail of
//! what the first pair already made, both filter the callback's sound, and
//! the two are crossfaded across it: so nothing clicks and nothing is cut.
//! The same holds when a table of another width arrives.
//!
//! A sound that is all round, or whose arc's filters are not designed yet,
//! goes to the all-round bus, which the ears hear as the diffuse field. When
//! it becomes an arc or stops being one it glides from one to the other over
//! a callback.

use realfft::num_complex::Complex;

use super::super::super::equalised_share::EqualisedShare;
use super::super::equaliser::HeadphoneEqualiser;
use super::table::ArcTable;
use crate::audio::convolver::Convolver;
use crate::pipeline::renderer::OutputBuffer;

/// Frames convolved at a time: one transform per typical callback.
pub(in crate::pipeline::renderers::binaural) const BLOCK_FRAMES: usize = 512;

/// One pair of convolvers and the filters it holds.
struct Slot {
    left: Convolver,
    right: Convolver,
    /// Which table (by address) and which bearing of it the pair holds.
    holds: Option<(usize, usize)>,
}

/// Buffers every voice uses in turn, owned once by the renderer.
pub(in crate::pipeline::renderers::binaural) struct Scratch {
    input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    left: Vec<f32>,
    right: Vec<f32>,
    outgoing_left: Vec<f32>,
    outgoing_right: Vec<f32>,
}

impl Scratch {
    pub(in crate::pipeline::renderers::binaural) fn new(filter_frames: usize) -> Self {
        let frequency_bins = (BLOCK_FRAMES + filter_frames - 1).next_power_of_two() / 2 + 1;
        Self {
            input: vec![0.0; BLOCK_FRAMES],
            spectrum: vec![Complex::new(0.0, 0.0); frequency_bins],
            left: vec![0.0; BLOCK_FRAMES],
            right: vec![0.0; BLOCK_FRAMES],
            outgoing_left: vec![0.0; BLOCK_FRAMES],
            outgoing_right: vec![0.0; BLOCK_FRAMES],
        }
    }
}

/// Where one callback of an ambient sound goes.
pub(in crate::pipeline::renderers::binaural) struct Hearing<'a> {
    /// The sound, at the level the probe lets through.
    pub(in crate::pipeline::renderers::binaural) signal: &'a [f32],
    /// The arc's bearing anticlockwise from where the head faces, radians;
    /// `None` when the sound is all round.
    pub(in crate::pipeline::renderers::binaural) relative_azimuth: Option<f32>,
    pub(in crate::pipeline::renderers::binaural) equalised: EqualisedShare,
    pub(in crate::pipeline::renderers::binaural) ambient_bus: &'a mut [f32],
}

pub(in crate::pipeline::renderers::binaural) struct ArcVoice {
    /// The arc's filters, of the width last delivered; `None` until one is.
    pub(in crate::pipeline::renderers::binaural) table: Option<std::sync::Arc<ArcTable>>,
    /// The width the sound wants, whole degrees; `None` while all round.
    pub(in crate::pipeline::renderers::binaural) wanted: Option<u16>,
    /// The width last asked of the designer.
    pub(in crate::pipeline::renderers::binaural) requested: Option<u16>,
    slots: [Slot; 2],
    active: usize,
    /// How much of the sound went through the arc's filters at the end of
    /// the last callback: 0 all to the all-round bus. `None` after a reset,
    /// when there is nothing to glide from.
    through_arc: Option<f32>,
    filter_frames: usize,
    /// Frames the filters still ring for since they were last fed.
    ringing_frames: usize,
}

impl ArcVoice {
    /// Convolvers for filters `filter_frames` long, built before audio runs.
    pub(in crate::pipeline::renderers::binaural) fn new(filter_frames: usize) -> Self {
        let silent = vec![0.0f32; filter_frames];
        let slot = || {
            let mut left = Convolver::new();
            let mut right = Convolver::new();
            left.init(BLOCK_FRAMES, &silent);
            right.init(BLOCK_FRAMES, &silent);
            Slot {
                left,
                right,
                holds: None,
            }
        };
        Self {
            table: None,
            wanted: None,
            requested: None,
            slots: [slot(), slot()],
            active: 0,
            through_arc: None,
            filter_frames,
            ringing_frames: 0,
        }
    }

    pub(in crate::pipeline::renderers::binaural) fn reset(&mut self) {
        for slot in &mut self.slots {
            slot.left.reset();
            slot.right.reset();
            slot.holds = None;
        }
        self.through_arc = None;
        self.ringing_frames = 0;
    }

    /// Hear one callback: the arc's share through its filters into the
    /// equaliser's bus and, as much as is heard unequalised, the output; the
    /// rest added to the all-round bus. True when the bus was added to.
    pub(in crate::pipeline::renderers::binaural) fn hear(
        &mut self,
        hearing: Hearing,
        equaliser: &mut HeadphoneEqualiser,
        scratch: &mut Scratch,
        out: &mut OutputBuffer,
    ) -> bool {
        let num_frames = out.num_frames;
        let wanted_through = match (hearing.relative_azimuth, &self.table) {
            (Some(_), Some(_)) => 1.0,
            _ => 0.0,
        };
        let from = self.through_arc.unwrap_or(wanted_through);
        self.through_arc = Some(wanted_through);
        if from == 0.0 && wanted_through == 0.0 && self.ringing_frames == 0 {
            // All round: added to the bus exactly as before there were arcs.
            for (bus, sample) in hearing.ambient_bus.iter_mut().zip(hearing.signal) {
                *bus += *sample;
            }
            return true;
        }
        let Some(table) = self.table.clone() else {
            for (bus, sample) in hearing.ambient_bus.iter_mut().zip(hearing.signal) {
                *bus += *sample;
            }
            return true;
        };

        // The bearing to hold: the arc's, or while it glides back to all
        // round, the one it last had.
        let address = std::sync::Arc::as_ptr(&table) as usize;
        let bearing = match hearing.relative_azimuth {
            Some(azimuth) => ArcTable::nearest(azimuth),
            None => self.slots[self.active]
                .holds
                .map(|(_, bearing)| bearing)
                .unwrap_or(0),
        };
        let wanted_hold = Some((address, bearing));
        let mut crossfading = false;
        if self.slots[self.active].holds != wanted_hold {
            let ears = table.ears(bearing);
            if self.slots[self.active].holds.is_none() {
                let slot = &mut self.slots[self.active];
                slot.left.set_response(&ears.left);
                slot.right.set_response(&ears.right);
                slot.holds = wanted_hold;
            } else {
                let incoming = 1 - self.active;
                let (first, second) = self.slots.split_at_mut(1);
                let (incoming_slot, outgoing_slot) = if incoming == 0 {
                    (&mut first[0], &second[0])
                } else {
                    (&mut second[0], &first[0])
                };
                incoming_slot.left.set_response(&ears.left);
                incoming_slot.right.set_response(&ears.right);
                incoming_slot.left.adopt_overlap_from(&outgoing_slot.left);
                incoming_slot.right.adopt_overlap_from(&outgoing_slot.right);
                incoming_slot.holds = wanted_hold;
                self.active = incoming;
                crossfading = true;
            }
        }

        let inverse_frames = 1.0 / num_frames as f32;
        let through_at =
            |frame: usize| from + (wanted_through - from) * frame as f32 * inverse_frames;
        let mut fed_bus = false;
        let mut fed_filters = false;
        let (bus_left, bus_right) = equaliser.bus(num_frames);
        let mut start = 0;
        while start < num_frames {
            let length = (num_frames - start).min(BLOCK_FRAMES);
            for index in 0..length {
                let frame = start + index;
                let through = through_at(frame);
                let sample = hearing.signal[frame];
                scratch.input[index] = sample * through;
                if through < 1.0 {
                    hearing.ambient_bus[frame] += sample * (1.0 - through);
                    fed_bus = true;
                }
                fed_filters |= through > 0.0;
            }
            let active = &mut self.slots[self.active];
            active
                .left
                .forward_fft(&scratch.input[..length], &mut scratch.spectrum);
            active.left.process_with_spectrum(
                &scratch.spectrum,
                length,
                &mut scratch.left[..length],
            );
            active.right.process_with_spectrum(
                &scratch.spectrum,
                length,
                &mut scratch.right[..length],
            );
            if crossfading {
                let outgoing = &mut self.slots[1 - self.active];
                outgoing.left.process_with_spectrum(
                    &scratch.spectrum,
                    length,
                    &mut scratch.outgoing_left[..length],
                );
                outgoing.right.process_with_spectrum(
                    &scratch.spectrum,
                    length,
                    &mut scratch.outgoing_right[..length],
                );
                for index in 0..length {
                    // Ends on the incoming pair alone, as the next callback
                    // starts.
                    let incoming = (start + index + 1) as f32 * inverse_frames;
                    let outgoing = 1.0 - incoming;
                    scratch.left[index] =
                        scratch.left[index] * incoming + scratch.outgoing_left[index] * outgoing;
                    scratch.right[index] =
                        scratch.right[index] * incoming + scratch.outgoing_right[index] * outgoing;
                }
            }
            for index in 0..length {
                let frame = start + index;
                let (left, right) = (scratch.left[index], scratch.right[index]);
                bus_left[frame] += left;
                bus_right[frame] += right;
                let unequalised = match hearing.equalised {
                    EqualisedShare::All => continue,
                    EqualisedShare::None => 1.0,
                    EqualisedShare::Gliding { .. } => {
                        1.0 - hearing.equalised.at(frame as f32 * inverse_frames)
                    }
                };
                let base = frame * out.channels;
                out.buffer[base] += left * unequalised;
                if out.channels > 1 {
                    out.buffer[base + 1] += right * unequalised;
                }
            }
            start += length;
        }
        if fed_filters {
            self.ringing_frames = self.filter_frames;
        } else {
            self.ringing_frames = self.ringing_frames.saturating_sub(num_frames);
        }
        fed_bus
    }
}
