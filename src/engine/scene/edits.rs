//! Live edits of the source pool: a sound swapped into or out of a slot.

use atrium_core::source::SoundSource;

use super::AudioScene;
use crate::engine::edit::{Retired, SceneEdit};

impl AudioScene {
    /// Drain live source-pool edits (add/remove) from the control thread.
    /// Called at the top of each audio callback. Swaps `Box`es in and out of the
    /// fixed slot pool — never allocates or deallocates on the audio thread: the
    /// incoming box is pre-built on the control thread, and the displaced box is
    /// shipped to the retire channel to be dropped there.
    pub fn process_scene_edits(&mut self) {
        // Take the consumer out so the slot accesses below don't alias its
        // borrow of `self`; restore it afterwards.
        let Some(mut consumer) = self.scene_edits.take() else {
            return;
        };
        while let Ok(edit) = consumer.pop() {
            match edit {
                SceneEdit::AddSource {
                    slot,
                    source,
                    bands,
                    amplitude,
                } => {
                    let slot = slot as usize;
                    if slot < self.sources.len() {
                        let displaced = std::mem::replace(&mut self.sources[slot], source);
                        if let Some(profile) = self.spectral_profiles.get_mut(slot) {
                            *profile = bands;
                        }
                        if let Some(amp) = self.source_amplitudes.get_mut(slot) {
                            *amp = amplitude;
                        }
                        self.retire(displaced);
                    } else {
                        self.retire(source);
                    }
                }
                SceneEdit::RemoveSource { slot, filler } => {
                    let slot = slot as usize;
                    if slot < self.sources.len() {
                        let displaced = std::mem::replace(&mut self.sources[slot], filler);
                        if let Some(amp) = self.source_amplitudes.get_mut(slot) {
                            *amp = 0.0;
                        }
                        self.retire(displaced);
                    } else {
                        self.retire(filler);
                    }
                }
            }
        }
        self.scene_edits = Some(consumer);
    }

    /// Ship a displaced source box back to the control thread to be dropped
    /// there. If the retire channel is full or absent, the box is dropped here
    /// as a last resort (rare; the control thread drains every frame).
    fn retire(&mut self, source: Box<dyn SoundSource>) {
        if let Some(out) = self.retired_out.as_mut() {
            let _ = out.push(Retired(source));
        }
    }
}
