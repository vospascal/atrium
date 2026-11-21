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

mod direction;
mod itd_delay;
mod renderer;
mod source_state;

pub use renderer::HrtfRenderer;
