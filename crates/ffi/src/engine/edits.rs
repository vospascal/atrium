//! The ring that carries built sounds to the audio thread, and the one that
//! brings back those it took out.

use atrium::config::SoundBuilder;
use atrium::engine::edit::{Retired, SceneEdit};

use super::AUDITION_SLOT;
use crate::api::EngineError;

/// How long, and how often, a full edit ring is waited on: some 200 ms, many
/// audio callbacks.
const EDIT_WAIT: std::time::Duration = std::time::Duration::from_millis(5);
const EDIT_WAITS: usize = 40;

/// The way sounds reach the audio thread while it runs, and come back.
pub(super) struct Edits {
    to_audio: rtrb::Producer<SceneEdit>,
    /// Sounds the audio thread took out. Freeing memory is not safe there,
    /// so it sends them back to be dropped on the calling thread.
    retired: rtrb::Consumer<Retired>,
    /// Whether a preset is playing from the audition voice.
    pub(super) auditioned: bool,
}

impl Edits {
    /// The two rings, with nothing auditioned yet.
    pub(super) fn new(
        to_audio: rtrb::Producer<SceneEdit>,
        retired: rtrb::Consumer<Retired>,
    ) -> Self {
        Self {
            to_audio,
            retired,
            auditioned: false,
        }
    }

    /// Hand `edit` to the audio thread. Undo can send several at once, more
    /// than the ring holds, so a full ring is waited on for a few audio
    /// callbacks before the edit is refused.
    pub(super) fn send(&mut self, edit: SceneEdit) -> Result<(), EngineError> {
        let mut edit = edit;
        for _ in 0..EDIT_WAITS {
            while self.retired.pop().is_ok() {}
            match self.to_audio.push(edit) {
                Ok(()) => return Ok(()),
                Err(rtrb::PushError::Full(returned)) => edit = returned,
            }
            std::thread::sleep(EDIT_WAIT);
        }
        Err(EngineError::CommandQueueFull)
    }

    pub(super) fn stop_audition(&mut self) -> Result<(), EngineError> {
        if self.auditioned {
            self.send(SceneEdit::RemoveSource {
                slot: AUDITION_SLOT as u16,
                filler: SoundBuilder::silence(),
            })?;
            self.auditioned = false;
        }
        Ok(())
    }
}
