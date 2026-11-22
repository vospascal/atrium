//! Renderer trait — the mode-specific core of the pipeline.
//!
//! Handles how a mono source signal becomes multichannel output.
//! Five implementations:
//!
//! - **MultichannelRenderer**: gain ramp × sample per channel (VBAP)
//! - **WorldLockedRenderer**: per-speaker propagation + gain ramp (WorldLocked)
//! - **HrtfRenderer**: per-path HRTF convolution to stereo headphones, and
//!   ambient sound through the head's diffuse field (Hrtf)
//! - **DbapRenderer**: per-path DBAP gain ramp × sample per channel (DBAP)
//! - **AmbisonicsRenderer**: per-path FOA encode to a B-format bus, or inline
//!   bilateral decode (Ambisonics)
//!
//! The renderer manages per-source state (gain ramps, HRTF convolvers,
//! per-speaker propagation, etc.).

use atrium_core::directivity::DirectivityPattern;
use atrium_core::listener::Listener;
use atrium_core::speaker::SpeakerLayout;
use atrium_core::types::Vec3;

use super::environment::Environment;
use super::path::{PathEffectChain, PathKind, PathSet};
use super::renderers::field::{AmbientArc, FieldEncoding};
use crate::audio::distance::DistanceModel;
use atrium_core::probe::Probe;

/// Per-source context for rendering. Borrowed from AudioScene per render call.
pub struct SourceContext<'a> {
    pub listener: &'a Listener,
    pub source_pos: Vec3,
    pub source_orientation: Vec3,
    pub source_directivity: &'a DirectivityPattern,
    pub source_spread: f32,
    pub source_ref_distance: f32,
    /// The point propagation paths were resolved towards: the listener for
    /// listener-relative modes, a fixed point of the room for the others.
    pub path_target: Vec3,
    pub environment: &'a Environment,
    pub sample_rate: f32,
    pub distance_model: &'a DistanceModel,
    pub layout: &'a SpeakerLayout,
    /// Reverb send level (0.0–1.0), computed from critical distance.
    pub reverb_send: f32,
    /// Which parts of the render reach the output.
    pub probe: Probe,
}

impl SourceContext<'_> {
    /// How much of a path of `kind` reaches the output: all of it, or none
    /// while a probe hides that part. Renderers scale a path's output gains
    /// by this and leave its reverb send alone, so the reverb is still fed
    /// while the direct sound is hidden.
    pub fn heard(&self, kind: PathKind) -> f32 {
        let let_through = match kind {
            PathKind::Direct => self.probe.direct,
            _ => self.probe.reflections,
        };
        if let_through {
            1.0
        } else {
            0.0
        }
    }
}

/// Interleaved output buffer with format metadata.
pub struct OutputBuffer<'a> {
    pub buffer: &'a mut [f32],
    /// Optional reverb send buffer (same layout as buffer).
    /// When present, the renderer writes distance-weighted FOA signal here
    /// for the FDN to use as its input, preserving per-source D/R balance.
    pub reverb_send: Option<&'a mut [f32]>,
    pub channels: usize,
    pub num_frames: usize,
    pub sample_rate: f32,
}

/// Renders one source's samples into the output buffer.
///
/// Called for each active source per buffer.
/// Receives resolved propagation paths and per-path effect chains.
pub trait Renderer: Send {
    /// Render one source into the output buffer.
    ///
    /// - `source_idx`: index into per-source state owned by this renderer
    /// - `source`: the SoundSource to pull samples from
    /// - `ctx`: full source geometry (position, orientation, directivity, reverb send)
    /// - `paths`: resolved propagation paths (direct + reflections)
    /// - `path_effects`: per-path effect chains (air absorption, ground effect, etc.)
    /// - `out`: interleaved output buffer to accumulate into
    #[allow(clippy::too_many_arguments)]
    fn render_source(
        &mut self,
        source_idx: usize,
        source: &mut dyn atrium_core::source::SoundSource,
        ctx: &SourceContext,
        paths: &PathSet,
        path_effects: &mut [PathEffectChain],
        out: &mut OutputBuffer,
    );

    /// Human-readable name for profiling/debugging.
    fn name(&self) -> &str;

    /// How environmental fields must be written so the post-mix stages read
    /// them as this renderer's bus does. Speaker channels unless the renderer
    /// leaves an encoded bus for a later decode stage.
    fn field_encoding(&self, _layout: &SpeakerLayout, _device_channels: usize) -> FieldEncoding {
        FieldEncoding::Speaker
    }

    /// Where an ambient sound's arc is centred from: the receiver whose
    /// bearing to the sound's place is the arc's middle. The point paths are
    /// resolved towards, unless the renderer feeds speakers from a point of
    /// its own.
    fn ambient_receiver(&self, listener: &Listener, layout: &SpeakerLayout) -> Vec3 {
        self.path_target(listener, layout)
    }

    /// Hear one ambient sound through the two ears, for a renderer whose
    /// `field_encoding` is `FieldEncoding::Ears`: `signal` is one callback of
    /// it, `arc` where it arrives from (`None` all round). What is all round
    /// is added to `ambient_bus`, which `finish_ears` then hears; what the
    /// renderer places itself goes to `out`. Returns whether the bus was
    /// added to. By default all of it is all round.
    #[allow(clippy::too_many_arguments)]
    fn hear_ambient(
        &mut self,
        _source_index: usize,
        signal: &[f32],
        _arc: Option<AmbientArc>,
        _listener: &Listener,
        _probe: &Probe,
        ambient_bus: &mut [f32],
        _out: &mut OutputBuffer,
    ) -> bool {
        for (bus, sample) in ambient_bus.iter_mut().zip(signal) {
            *bus += *sample;
        }
        true
    }

    /// Make ready what `sources` need before audio runs: on headphones, the
    /// filters of every ambient sound's arc, so each is heard from its
    /// first callback. Called when the pipelines are initialised, never on
    /// the audio thread.
    fn prepare_sources(&mut self, _sources: &[Box<dyn atrium_core::source::SoundSource>]) {}

    /// Add what reaches the two ears once every source has rendered: the
    /// mono ambient bus as the listener hears it, and whatever the renderer
    /// does to the sum of its sources' direct sound (on headphones, the
    /// equalisation). Called once per callback, only of a renderer whose
    /// `field_encoding` is `FieldEncoding::Ears`; `ambient_fed` says whether
    /// any ambient sound wrote to the bus this callback, so a renderer can
    /// stop working once its filters have rung out.
    fn finish_ears(
        &mut self,
        _ambient_bus: &[f32],
        _ambient_fed: bool,
        _probe: &Probe,
        _out: &mut OutputBuffer,
    ) {
    }

    /// The receiver position this renderer's propagation paths are resolved
    /// towards. Listener-relative renderers hear the room from the listener;
    /// a renderer that feeds speakers from the scene alone overrides this with
    /// a point that does not move with the listener.
    fn path_target(&self, listener: &Listener, _layout: &SpeakerLayout) -> Vec3 {
        listener.position
    }

    /// Resize internal state when sources, layout, or sample rate change.
    ///
    /// Called at init and whenever the topology changes (source added/removed,
    /// speaker mask changed, layout reconfigured).
    fn ensure_topology(&mut self, source_count: usize, layout: &SpeakerLayout, sample_rate: f32);

    /// Reset all per-source state (gain ramps, filters, convolvers).
    /// Called on mode switch to avoid artifacts from stale state.
    fn reset(&mut self);
}
