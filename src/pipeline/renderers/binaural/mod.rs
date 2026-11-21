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
//!
//! The HRIRs were measured at one distance. A **direct path closer than
//! that** gets each ear corrected by the rigid sphere's distance variation
//! function (`near_field`), so a source beside the head is heard beside it
//! and not merely loud. Reflections arrive from image sources far away and
//! never need it.

mod direction;
mod itd_delay;
mod near_field;
mod renderer;
mod source_state;

pub use renderer::HrtfRenderer;
