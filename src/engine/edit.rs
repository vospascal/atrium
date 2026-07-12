//! Live source-pool edits: a non-`Copy` channel for adding/removing sources
//! on the audio thread without gaps and without real-time allocation.
//!
//! The regular [`atrium_core::commands::Command`] channel is `Copy` and can't
//! carry a `Box<dyn SoundSource>`. These edits ride a separate ring buffer.
//! Because deallocating on the audio thread isn't real-time safe, a displaced
//! source is shipped back to the control thread via the [`Retired`] channel to
//! be dropped there.
//!
//! The source pool is a fixed 16 slots (pre-warmed at build), so an add/remove
//! only swaps a `Box` into/out of an existing slot — it never grows the pool,
//! the parallel vectors, or the pipeline topology.

use crate::audio::spectral_profile::BARK_BANDS;
use atrium_core::source::SoundSource;

/// An edit to the audio thread's source pool. Fully built on the control
/// thread; the audio thread only swaps `Box`es (no decode, no allocation).
pub enum SceneEdit {
    /// Install `source` into `slot`, replacing the placeholder (or existing
    /// source) there. The displaced box is retired to the control thread.
    AddSource {
        slot: u16,
        source: Box<dyn SoundSource>,
        bands: [f32; BARK_BANDS],
        amplitude: f32,
    },
    /// Replace `slot` with `filler` (a silent placeholder built on the control
    /// thread) and retire the real source that was there.
    RemoveSource {
        slot: u16,
        filler: Box<dyn SoundSource>,
    },
}

/// A source box shipped back from the audio thread to be dropped on the control
/// thread — deallocation is not real-time safe.
pub struct Retired(pub Box<dyn SoundSource>);
