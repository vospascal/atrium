//! Ambient sound from an arc, at the two ears, as the measured head hears
//! one: a shoreline, a tree line, traffic beyond a hill.
//!
//! Such a sound is many unrelated contributions from the directions of an
//! arc along the horizon, fixed in the world. What the two ears receive of
//! it follows from the measured head restricted to those directions
//! (`horizon`): each ear's power and the two ears' cross-spectrum, whose
//! level difference, coherence and phase say how far to one side the arc
//! lies and how wide it is. Two filters per arc give the ears exactly that
//! (`design`), for every way the head can face it (`table`), designed on a
//! thread of their own (`designer`) and convolved per sound (`voice`).
//!
//! Its two ears join the direct sound of the point sources on the headphone
//! equaliser's bus, so the arc is equalised as everything else is; its
//! filters are designed so that through the equaliser each ear gets exactly
//! its share of the head's diffuse field. An arc of 360° is all round, and
//! takes the all-round path (`DiffuseField`) unchanged.

mod design;
mod designer;
mod horizon;
mod table;
mod voice;

use std::sync::Arc;

use sofar::reader::Sofar;

use super::super::equalised_share::EqualisedShare;
use super::equaliser::HeadphoneEqualiser;
use super::inverse::EqualiserFilters;
use super::statistics::DiffuseStatistics;
use crate::pipeline::renderer::OutputBuffer;
use crate::pipeline::renderers::field::AmbientArc;
pub(in crate::pipeline::renderers::binaural) use design::ArcHead;
use designer::ArcDesigner;
use horizon::Horizon;
use table::ArcTable;
use voice::{ArcVoice, Hearing, Scratch};

use design::FILTER_SECONDS;

/// Every ambient sound's arc on headphones.
pub(in crate::pipeline::renderers::binaural) struct AmbientArcs {
    head: Arc<ArcHead>,
    designer: ArcDesigner,
    voices: Vec<ArcVoice>,
    scratch: Scratch,
    filter_frames: usize,
}

impl ArcHead {
    /// The head along the horizon in `sofa`, with its diffuse field and its
    /// headphone equaliser. Called when the HRIRs are loaded, never on the
    /// audio thread.
    pub(in crate::pipeline::renderers::binaural) fn measure(
        sofa: &Sofar,
        sample_rate: f32,
        grid_size: usize,
        statistics: &DiffuseStatistics,
        equaliser: &EqualiserFilters,
    ) -> Self {
        Self::new(
            Horizon::measure(sofa, sample_rate),
            sample_rate,
            grid_size,
            statistics,
            equaliser,
        )
    }
}

impl AmbientArcs {
    /// Called when the HRIRs are loaded, never on the audio thread.
    pub(in crate::pipeline::renderers::binaural) fn new(
        head: Arc<ArcHead>,
        sample_rate: f32,
    ) -> Self {
        let filter_frames = (FILTER_SECONDS * sample_rate as f64).round() as usize;
        Self {
            designer: ArcDesigner::start(head.clone()),
            head,
            voices: Vec::new(),
            scratch: Scratch::new(filter_frames),
            filter_frames,
        }
    }

    /// A voice for each of `source_count` sounds. Called when the topology
    /// is prepared; once there are enough, it does nothing.
    pub(in crate::pipeline::renderers::binaural) fn ensure_sources(&mut self, source_count: usize) {
        while self.voices.len() < source_count {
            self.voices.push(ArcVoice::new(self.filter_frames));
        }
    }

    /// Design the filters of the sound `source_index`'s arc, `degrees` wide,
    /// here and now, so it is heard from its first callback. Called before
    /// audio runs; while it runs, a new width is designed beside it.
    pub(in crate::pipeline::renderers::binaural) fn prepare(
        &mut self,
        source_index: usize,
        degrees: f32,
    ) {
        self.ensure_sources(source_index + 1);
        let width = whole_degrees(degrees);
        let voice = &self.voices[source_index];
        if voice.table.as_ref().map(|table| table.width_degrees()) == Some(width) {
            return;
        }
        let table = self
            .voices
            .iter()
            .filter_map(|voice| voice.table.clone())
            .find(|table| table.width_degrees() == width)
            .unwrap_or_else(|| Arc::new(ArcTable::design(&self.head, width)));
        let voice = &mut self.voices[source_index];
        voice.table = Some(table);
        voice.wanted = Some(width);
        voice.requested = Some(width);
    }

    /// Hear one callback of the ambient sound `source_index`: from `arc`,
    /// relative to a head facing `listener_yaw`, or all round, into
    /// `ambient_bus`. True when the bus was added to.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::pipeline::renderers::binaural) fn hear(
        &mut self,
        source_index: usize,
        signal: &[f32],
        arc: Option<AmbientArc>,
        listener_yaw: f32,
        equalised: EqualisedShare,
        equaliser: &mut HeadphoneEqualiser,
        ambient_bus: &mut [f32],
        out: &mut OutputBuffer,
    ) -> bool {
        self.take_designed();
        let Some(voice) = self.voices.get_mut(source_index) else {
            for (bus, sample) in ambient_bus.iter_mut().zip(signal) {
                *bus += *sample;
            }
            return true;
        };
        let wanted = arc.map(|arc| whole_degrees(arc.width.to_degrees()));
        voice.wanted = wanted;
        if let Some(width) = wanted {
            let has = voice.table.as_ref().map(|table| table.width_degrees());
            if has != Some(width)
                && voice.requested != Some(width)
                && self.designer.request(source_index, width)
            {
                voice.requested = Some(width);
            }
        }
        voice.hear(
            Hearing {
                signal,
                relative_azimuth: arc.map(|arc| arc.centre - listener_yaw),
                equalised,
                ambient_bus,
            },
            equaliser,
            &mut self.scratch,
            out,
        )
    }

    /// Hand every table the designer finished to the sounds that want its
    /// width, and the tables they no longer need back to be freed.
    fn take_designed(&mut self) {
        while let Some(table) = self.designer.designed() {
            for voice in &mut self.voices {
                if voice.wanted == Some(table.width_degrees())
                    && voice.table.as_ref().map(|held| held.width_degrees())
                        != Some(table.width_degrees())
                {
                    if let Some(old) = voice.table.replace(table.clone()) {
                        self.designer.retire(old);
                    }
                }
            }
            self.designer.retire(table);
        }
    }

    pub(in crate::pipeline::renderers::binaural) fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.reset();
        }
    }
}

/// An arc's width in the whole degrees its filters are designed for, from
/// the narrowest to just short of all round.
fn whole_degrees(degrees: f32) -> u16 {
    degrees.round().clamp(
        atrium_core::source::NARROWEST_ARC_DEGREES,
        atrium_core::source::ALL_ROUND_DEGREES - 1.0,
    ) as u16
}

#[cfg(test)]
mod tests;
